use crate::runtime::workspace::Tab;

use super::types::AnimationConfig;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui_hypertile::PaneId;
use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

/// Drives the per-pane slide animation for `MoveFocused` actions.
///
/// # Lifecycle
///
/// 1. **`capture_before`** snapshots each pane's current displayed rect
///    (interpolated if an animation is already in flight).
/// 2. The core engine applies the move action, rearranging pane ids.
/// 3. **`start`** diffs before/after rects and creates transitions for panes
///    that moved. Reuses the previous `ActiveAnimation`'s HashMap to avoid
///    re-allocating.
/// 4. **`display_rects`** is called during render to produce interpolated rects.
///    Static panes come first, moving panes are appended last (painted on top).
/// 5. **`next_frame_in`** tells the event loop when to wake for the next frame.
///
/// If a new move arrives mid-animation, `capture_before` reads the in-progress
/// interpolated positions so the new animation starts seamlessly from the
/// visually displayed state.
#[derive(Debug, Default)]
pub(super) struct AnimationState {
    active: Option<ActiveAnimation>,
    last_area: Option<Rect>,
    before_panes: HashMap<PaneId, Rect>,
    display_panes: Vec<(PaneId, Rect)>,
    moving_panes: Vec<(PaneId, Rect)>,
}

impl AnimationState {
    pub(super) fn clear(&mut self) {
        self.active = None;
        self.before_panes.clear();
        self.display_panes.clear();
        self.moving_panes.clear();
    }

    pub(super) fn last_area(&self) -> Option<Rect> {
        self.last_area
    }

    /// Cancels active animation if the area changed (e.g. terminal resize).
    pub(super) fn remember_area(&mut self, area: Rect) {
        if self.last_area != Some(area) {
            self.active = None;
        }
        self.last_area = Some(area);
    }

    /// If an animation is running, reads interpolated positions so a chained
    /// move starts from where panes visually are.
    pub(super) fn capture_before<I>(&mut self, area: Rect, panes: I, now: Instant)
    where
        I: IntoIterator<Item = (PaneId, Rect)>,
    {
        self.before_panes.clear();

        let Some(active) = self.active.as_ref() else {
            self.before_panes.extend(panes);
            return;
        };

        if active.area != area || active.is_finished(now) {
            self.active = None;
            self.before_panes.extend(panes);
            return;
        }

        let progress = active.progress(now);
        for (pane_id, rect) in panes {
            let rect = active
                .transition_for(pane_id)
                .map_or(rect, |transition| transition.interpolate(progress));
            self.before_panes.insert(pane_id, rect);
        }
    }

    /// Diffs before/after and starts transitions for panes that moved.
    pub(super) fn start<I>(&mut self, area: Rect, panes: I, now: Instant, config: AnimationConfig)
    where
        I: IntoIterator<Item = (PaneId, Rect)>,
    {
        if !config.enabled {
            self.active = None;
            return;
        }

        let mut transitions = self.active.take().map_or_else(HashMap::new, |mut active| {
            active.transitions.clear();
            active.transitions
        });

        for (pane_id, to) in panes {
            let from = match self.before_panes.get(&pane_id).copied() {
                Some(from) => from,
                None => Rect {
                    x: to.x + to.width / 2,
                    y: to.y + to.height / 2,
                    width: 0,
                    height: 0,
                },
            };
            if from != to {
                transitions.insert(pane_id, RectTransition { from, to });
            }
        }

        if transitions.is_empty() {
            self.active = None;
            return;
        }

        self.active = Some(ActiveAnimation {
            area,
            started_at: now,
            duration: normalize_duration(config.duration),
            transitions,
        });
    }

    pub(super) fn next_frame_in(&self, now: Instant, config: AnimationConfig) -> Option<Duration> {
        let active = self.active.as_ref()?;
        if !config.enabled || active.is_finished(now) {
            return None;
        }
        active.next_frame_in(now, normalize_frame_interval(config.frame_interval))
    }

    /// Moving panes are appended last so they paint on top.
    pub(super) fn display_rects<I>(
        &mut self,
        area: Rect,
        panes: I,
        now: Instant,
    ) -> &[(PaneId, Rect)]
    where
        I: IntoIterator<Item = (PaneId, Rect)>,
    {
        self.display_panes.clear();
        self.moving_panes.clear();

        let Some(active) = self.active.as_ref() else {
            self.display_panes.extend(panes);
            return self.display_panes.as_slice();
        };

        if active.area != area || active.is_finished(now) {
            self.active = None;
            self.display_panes.extend(panes);
            return self.display_panes.as_slice();
        }

        let progress = active.progress(now);
        for (pane_id, rect) in panes {
            if let Some(transition) = active.transition_for(pane_id) {
                self.moving_panes
                    .push((pane_id, transition.interpolate(progress)));
            } else {
                self.display_panes.push((pane_id, rect));
            }
        }

        self.display_panes.extend_from_slice(&self.moving_panes);
        self.display_panes.as_slice()
    }
}

/// Dropped on area mismatch (e.g. terminal resize).
#[derive(Debug)]
struct ActiveAnimation {
    area: Rect,
    started_at: Instant,
    duration: Duration,
    transitions: HashMap<PaneId, RectTransition>,
}

impl ActiveAnimation {
    fn is_finished(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.started_at) >= self.duration
    }

    fn progress(&self, now: Instant) -> f32 {
        let elapsed = now.saturating_duration_since(self.started_at);
        ease_out_cubic(elapsed.as_secs_f32() / self.duration.as_secs_f32()).clamp(0.0, 1.0)
    }

    fn next_frame_in(&self, now: Instant, frame_interval: Duration) -> Option<Duration> {
        let elapsed = now.saturating_duration_since(self.started_at);
        if elapsed >= self.duration {
            return None;
        }

        let until_end = self.duration - elapsed;
        let until_next_frame = remaining_until_next_frame(elapsed, frame_interval);
        Some(until_end.min(until_next_frame))
    }

    fn transition_for(&self, pane_id: PaneId) -> Option<RectTransition> {
        self.transitions.get(&pane_id).copied()
    }
}

fn copy_with_offset(src: &Buffer, dst: &mut Buffer, screen_area: Rect, offset_x: i32) {
    for y in screen_area.top()..screen_area.bottom() {
        for x in screen_area.left()..screen_area.right() {
            // 计算当前屏幕坐标 (x, y) 对应源 Buffer 中的哪一个点
            let src_x = x as i32 - offset_x;

            // 检查计算出的源坐标是否在离屏 Buffer 的合法范围内
            if src_x >= src.area.left() as i32 && src_x < src.area.right() as i32 {
                let src_cell = &src[(src_x as u16, y)];
                // 将 Cell 内容（字符、样式等）完整拷贝
                dst[(x, y)] = src_cell.clone();
            }
        }
    }
}

fn draw_workspace_transition(
    area: Rect,
    progress: f32,
    offset: f32,
    direction: AniDirection,
    old: &Buffer,
    new: &Buffer,
    main_buffer: &mut Buffer,
) {
    let width = area.width as f32;
    let offset_a;
    let offset_b;
    match direction {
        AniDirection::Left => {
            offset_a = (-width * progress + offset) as i32;
            offset_b = (width * (1.0 - progress) - offset) as i32;
        }
        AniDirection::Right => {
            offset_a = (width * progress + offset) as i32;
            offset_b = (-width * (1.0 - progress) + offset) as i32;
        }
    }

    copy_with_offset(&old, main_buffer, area, offset_a);
    copy_with_offset(&new, main_buffer, area, offset_b);
}

#[derive(Debug, Default)]
pub struct SpaceTransitions {
    transitions: VecDeque<SpaceAnimation>,
}

impl SpaceTransitions {
    pub fn next_frame_in(&self, now: Instant, frame_interval: Duration) -> Option<Duration> {
        let first = self.transitions.front()?;
        first.next_frame_in(now, frame_interval)
    }
    pub fn is_finished(&self) -> bool {
        self.transitions.is_empty()
    }
    pub fn old_space(&self) -> Option<usize> {
        Some(self.transitions.front()?.from)
    }
    pub fn push(&mut self, mut ani: SpaceAnimation) {
        if self.transitions.is_empty() {
            self.transitions.push_back(ani);
        } else if self.transitions.len() == 1 {
            let first = &mut self.transitions[0];
            if first.direction != ani.direction {
                ani.record_progress = first.progress(Instant::now());
                *first = ani;
            } else {
                self.transitions.push_back(ani);
            }
        }
    }
    pub fn display_buf(&mut self, area: Rect, tabs: &mut [Tab], main_buffer: &mut Buffer) {
        if let Some(first) = self.transitions.front_mut() {
            let progress = first.progress(Instant::now());
            let mut buf_old = Buffer::empty(area);
            tabs[first.from].runtime.render(area, &mut buf_old);
            let mut buf_new = Buffer::empty(area);
            tabs[first.to].runtime.render(area, &mut buf_new);
            draw_workspace_transition(
                area,
                progress,
                area.width as f32 * first.record_progress,
                first.direction,
                &buf_old,
                &buf_new,
                main_buffer,
            );
            if first.is_finished(Instant::now()) {
                self.transitions.pop_front();
            }
        }
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum AniDirection {
    Left,
    Right,
}

#[derive(Debug)]
pub struct SpaceAnimation {
    started_at: Instant,
    duration: Duration,
    from: usize,
    to: usize,
    direction: AniDirection,
    record_progress: f32,
}

impl SpaceAnimation {
    pub fn new(duration: Duration, from: usize, to: usize) -> Self {
        Self {
            started_at: Instant::now(),
            duration,
            from,
            to,
            direction: if from < to {
                AniDirection::Left
            } else {
                AniDirection::Right
            },
            record_progress: 0.0,
        }
    }
    fn is_finished(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.started_at) >= self.duration
    }

    fn progress(&self, now: Instant) -> f32 {
        let elapsed = now.saturating_duration_since(self.started_at);
        ease_out_cubic(elapsed.as_secs_f32() / self.duration.as_secs_f32()).clamp(0.0, 1.0)
    }

    fn next_frame_in(&self, now: Instant, frame_interval: Duration) -> Option<Duration> {
        let elapsed = now.saturating_duration_since(self.started_at);
        if elapsed >= self.duration {
            return None;
        }

        let until_end = self.duration - elapsed;
        let until_next_frame = remaining_until_next_frame(elapsed, frame_interval);
        Some(until_end.min(until_next_frame))
    }
}

#[derive(Debug, Clone, Copy)]
struct RectTransition {
    from: Rect,
    to: Rect,
}

impl RectTransition {
    fn interpolate(self, progress: f32) -> Rect {
        Rect::new(
            lerp_u16(self.from.x, self.to.x, progress),
            lerp_u16(self.from.y, self.to.y, progress),
            lerp_u16(self.from.width, self.to.width, progress),
            lerp_u16(self.from.height, self.to.height, progress),
        )
    }
}

fn normalize_duration(duration: Duration) -> Duration {
    if duration.is_zero() {
        Duration::from_millis(1)
    } else {
        duration
    }
}

fn normalize_frame_interval(frame_interval: Duration) -> Duration {
    if frame_interval.is_zero() {
        Duration::from_millis(16)
    } else {
        frame_interval
    }
}

fn remaining_until_next_frame(elapsed: Duration, frame_interval: Duration) -> Duration {
    let elapsed_nanos = elapsed.as_nanos();
    let frame_nanos = frame_interval.as_nanos();
    let next_frame_nanos = elapsed_nanos
        .saturating_div(frame_nanos)
        .saturating_add(1)
        .saturating_mul(frame_nanos);
    saturating_duration_from_nanos(next_frame_nanos.saturating_sub(elapsed_nanos))
}

fn saturating_duration_from_nanos(nanos: u128) -> Duration {
    Duration::from_nanos(nanos.min(u128::from(u64::MAX)) as u64)
}

fn ease_out_cubic(progress: f32) -> f32 {
    let t = progress.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

fn lerp_u16(from: u16, to: u16, progress: f32) -> u16 {
    let from = f32::from(from);
    let to = f32::from(to);
    (from + (to - from) * progress)
        .round()
        .clamp(0.0, f32::from(u16::MAX)) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config(duration_ms: u64, frame_interval_ms: u64) -> AnimationConfig {
        AnimationConfig {
            enabled: true,
            duration: Duration::from_millis(duration_ms),
            frame_interval: Duration::from_millis(frame_interval_ms),
        }
    }

    #[test]
    fn transition_reaches_end_rect() {
        let transition = RectTransition {
            from: Rect::new(0, 0, 10, 5),
            to: Rect::new(20, 6, 8, 4),
        };

        assert_eq!(transition.interpolate(0.0), Rect::new(0, 0, 10, 5));
        assert_eq!(transition.interpolate(1.0), Rect::new(20, 6, 8, 4));
    }

    #[test]
    fn next_frame_in_tracks_remaining_time_to_frame_boundary_and_end() {
        let now = Instant::now();
        let mut state = AnimationState::default();
        state.capture_before(
            Rect::new(0, 0, 20, 5),
            [(PaneId::ROOT, Rect::new(0, 0, 10, 5))],
            now,
        );
        state.start(
            Rect::new(0, 0, 20, 5),
            [(PaneId::ROOT, Rect::new(10, 0, 10, 5))],
            now,
            test_config(40, 16),
        );

        assert_eq!(
            state.next_frame_in(now + Duration::from_millis(10), test_config(40, 16)),
            Some(Duration::from_millis(6))
        );
        assert_eq!(
            state.next_frame_in(now + Duration::from_millis(35), test_config(40, 16)),
            Some(Duration::from_millis(5))
        );
        assert_eq!(
            state.next_frame_in(now + Duration::from_millis(50), test_config(40, 16)),
            None
        );
    }

    #[test]
    fn interrupted_animation_restarts_from_current_displayed_rect() {
        let now = Instant::now();
        let area = Rect::new(0, 0, 40, 5);
        let pane_id = PaneId::ROOT;
        let mut state = AnimationState::default();
        state.capture_before(area, [(pane_id, Rect::new(0, 0, 10, 5))], now);
        state.start(
            area,
            [(pane_id, Rect::new(20, 0, 10, 5))],
            now,
            test_config(100, 16),
        );

        let restart_at = now + Duration::from_millis(50);
        state.capture_before(area, [(pane_id, Rect::new(20, 0, 10, 5))], restart_at);
        state.start(
            area,
            [(pane_id, Rect::new(30, 0, 10, 5))],
            restart_at,
            test_config(100, 16),
        );

        assert_eq!(
            state.display_rects(area, [(pane_id, Rect::new(30, 0, 10, 5))], restart_at),
            &[(pane_id, Rect::new(18, 0, 10, 5))]
        );
    }
}
