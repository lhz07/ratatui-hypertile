use ratatui::crossterm::event::{Event, KeyCode, KeyModifiers};
use ratatui::prelude::*;
use ratatui_hypertile::{EventOutcome, HypertileEvent};
use std::collections::HashMap;
use std::sync::LazyLock;
use std::{
    borrow::Cow,
    time::{Duration, Instant},
};

use crate::{AnimationConfig, InputMode, runtime::animation::SpaceAnimation};

use super::HypertileRuntime;

pub struct Tab {
    label: Option<String>,
    pub runtime: HypertileRuntime,
}

/// Small tab manager around [`HypertileRuntime`].
///
/// Use this when one runtime is not enough and you want a lightweight
/// workspace model without building it yourself. It intercepts a few `Ctrl+...`
/// keys for tab management and forwards everything else to the active tab.
pub struct WorkspaceRuntime {
    /// offset, old_active, new_active
    animation: Option<SpaceAnimation>,
    area: Option<Rect>,
    ani_config: AnimationConfig,
    tabs: Vec<Tab>,
    active: usize,
    factory: Box<dyn Fn() -> HypertileRuntime>,
}

/// Command understood by [`WorkspaceRuntime`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceAction {
    /// Add a tab and switch to it.
    NewTab,
    /// Remove one tab by index.
    CloseTab(usize),
    /// Move to the next tab, wrapping at the end.
    NextTab,
    /// Move to the previous tab, wrapping at the start.
    PrevTab,
    /// Focus a specific tab by index.
    GoToTab(usize),
    /// Replace one tab label.
    RenameTab(usize, String),
}

impl WorkspaceRuntime {
    /// Creates a workspace from a runtime factory.
    ///
    /// The factory is reused for every new tab, so it should return a fully
    /// configured runtime with your plugin registrations already in place.
    pub fn new(factory: impl Fn() -> HypertileRuntime + 'static) -> Self {
        let first = factory();
        Self {
            animation: None,
            ani_config: AnimationConfig::default(),
            tabs: vec![Tab {
                label: None,
                runtime: first,
            }],
            area: None,
            active: 0,
            factory: Box::new(factory),
        }
    }

    pub fn active_runtime(&self) -> &HypertileRuntime {
        &self.tabs[self.active].runtime
    }

    pub fn active_runtime_mut(&mut self) -> &mut HypertileRuntime {
        &mut self.tabs[self.active].runtime
    }

    /// Mirrors [`HypertileRuntime::next_frame_in`] for the active tab.
    pub fn next_frame_in(&self) -> Option<Duration> {
        if let Some(ani) = &self.animation
            && let Some(dur) = ani.next_frame_in(Instant::now(), self.ani_config.frame_interval)
        {
            Some(dur)
        } else {
            self.tabs[self.active].runtime.next_frame_in()
        }
    }

    pub fn tab_count(&self) -> usize {
        self.tabs.len()
    }

    pub fn active_tab_index(&self) -> usize {
        self.active
    }

    pub fn tab_labels(&self) -> impl Iterator<Item = (Cow<'_, str>, bool)> {
        self.tabs.iter().enumerate().map(move |(i, tab)| {
            (
                if let Some(name) = &tab.label {
                    name.as_str().into()
                } else {
                    (i + 1).to_string().into()
                },
                i == self.active,
            )
        })
    }

    /// Adds a new tab and switches to it.
    pub fn new_tab(&mut self) {
        let runtime = (self.factory)();
        self.tabs.push(Tab {
            label: None,
            runtime,
        });
        self.go_to_tab(self.tabs.len() - 1);
    }

    pub fn new_tab_silently(&mut self) {
        let runtime = (self.factory)();
        self.tabs.push(Tab {
            label: None,
            runtime,
        });
    }

    pub fn set_active(&mut self, active: usize) {
        self.tabs[self.active].runtime.set_active(false);
        self.active = active;
        self.tabs[self.active].runtime.set_active(true);
    }

    /// Does nothing if this is the last tab or the index is out of range.
    pub fn close_tab(&mut self, index: usize) {
        if self.tabs.len() <= 1 || index >= self.tabs.len() {
            return;
        }
        self.tabs.remove(index);
        if self.active >= self.tabs.len() {
            self.set_active(self.tabs.len() - 1);
        } else if self.active > index {
            self.set_active(self.active - 1);
        }
    }

    pub fn next_tab(&mut self) -> bool {
        if self.active + 1 >= self.tabs.len() {
            return false;
        }
        let new = self.active + 1;
        self.start_animation(new);
        self.set_active(new);
        true
    }

    pub fn prev_tab(&mut self) {
        let new = self.active.saturating_sub(1);
        if new == self.active {
            return;
        }
        self.start_animation(new);
        self.set_active(new);
    }

    fn start_animation(&mut self, new: usize) {
        match &mut self.animation {
            Some(ani) => {
                ani.push_switch(new);
            }
            None => {
                let Some(area) = self.area else {
                    return;
                };
                let ani = SpaceAnimation::new(
                    self.ani_config.duration,
                    area,
                    self.active as u16,
                    new as u16,
                );
                self.animation = Some(ani);
            }
        }
    }

    /// Does nothing if the index is out of range.
    pub fn go_to_tab(&mut self, index: usize) {
        if index < self.tabs.len() && index != self.active {
            self.start_animation(index);
            self.set_active(index);
        }
    }

    /// Does nothing if the index is out of range.
    pub fn rename_tab(&mut self, index: usize, label: String) {
        if let Some(tab) = self.tabs.get_mut(index) {
            tab.label = Some(label);
        }
    }

    pub fn apply_workspace_action(&mut self, action: WorkspaceAction) {
        match action {
            WorkspaceAction::NewTab => self.new_tab(),
            WorkspaceAction::CloseTab(i) => self.close_tab(i),
            WorkspaceAction::NextTab => {
                self.next_tab();
            }
            WorkspaceAction::PrevTab => self.prev_tab(),
            WorkspaceAction::GoToTab(i) => self.go_to_tab(i),
            WorkspaceAction::RenameTab(i, label) => self.rename_tab(i, label),
        }
    }

    /// Handles one event for the active tab.
    ///
    /// `Ctrl+t`, `Ctrl+w`, `Ctrl+n`, `Ctrl+p`, `Ctrl+Left`, and `Ctrl+Right`
    /// are reserved for tab management. Everything else goes to the active
    /// runtime.
    pub fn handle_event(&mut self, mut event: HypertileEvent) -> EventOutcome {
        if self.tabs[self.active].runtime.mode() == InputMode::PluginInput {
            self.tabs[self.active].runtime.handle_event(&mut event);
            return EventOutcome::Consumed;
        }
        static SHIFT_MAP: LazyLock<HashMap<char, usize>> = LazyLock::new(|| {
            [
                ('!', 1),
                ('@', 2),
                ('#', 3),
                ('$', 4),
                ('%', 5),
                ('^', 6),
                ('&', 7),
                ('*', 8),
                ('(', 9),
                (')', 0),
            ]
            .into_iter()
            .collect()
        });
        if let HypertileEvent::Term(term) = &event
            && let Event::Key(chord) = term
        {
            if chord.modifiers == KeyModifiers::CONTROL | KeyModifiers::ALT {
                match chord.code {
                    KeyCode::Char('t') => {
                        self.new_tab();
                        return EventOutcome::Consumed;
                    }
                    KeyCode::Char('w') => {
                        self.close_tab(self.active);
                        return EventOutcome::Consumed;
                    }

                    _ => (),
                }
            } else if chord.modifiers == KeyModifiers::ALT {
                match chord.code {
                    KeyCode::Right => {
                        if !self.next_tab() {
                            self.new_tab();
                        }
                        return EventOutcome::Consumed;
                    }
                    KeyCode::Left => {
                        self.prev_tab();
                        return EventOutcome::Consumed;
                    }
                    KeyCode::Char(ch) if ch.is_ascii_digit() => {
                        let mut i = ch as usize - '0' as usize;
                        if i == 0 {
                            i = 9;
                        } else {
                            i -= 1;
                        }
                        let base = self.active / 10 * 10;
                        self.go_to_tab(base + i);
                        return EventOutcome::Consumed;
                    }
                    KeyCode::Char(ch) if SHIFT_MAP.contains_key(&ch) => {
                        let mut i = SHIFT_MAP[&ch];
                        if i == 0 {
                            i = 9;
                        } else {
                            i -= 1;
                        }
                        let base = self.active / 10 * 10;
                        let target = base + i;
                        if self.tabs.get(target).is_some()
                            && let Ok(plugin) = self.tabs[self.active].runtime.pop_focused()
                        {
                            let target = &mut self.tabs[target].runtime;
                            if let Err(e) = target.split_focused_with_plugin(None, plugin) {
                                log::error!("Can not insert plugin into other workspace: {e}");
                            }
                            if let Some(area) = self.area {
                                target.core.compute_layout(area);
                            }
                        }
                        return EventOutcome::Consumed;
                    }
                    _ => (),
                }
            } else if chord.modifiers == KeyModifiers::ALT | KeyModifiers::SHIFT {
                match chord.code {
                    KeyCode::Right => {
                        let target = self.active + 1;
                        if target >= self.tabs.len() {
                            self.new_tab_silently();
                        }
                        if let Ok(plugin) = self.tabs[self.active].runtime.pop_focused() {
                            let target = &mut self.tabs[target].runtime;
                            if let Err(e) = target.split_focused_with_plugin(None, plugin) {
                                log::error!("Can not insert plugin into other workspace: {e}");
                            }
                            if let Some(area) = self.area {
                                target.core.compute_layout(area);
                            }
                        }

                        return EventOutcome::Consumed;
                    }
                    KeyCode::Left => {
                        if self.active == 0 {
                            return EventOutcome::Consumed;
                        }
                        let target = self.active - 1;
                        if let Ok(plugin) = self.tabs[self.active].runtime.pop_focused() {
                            let target = &mut self.tabs[target].runtime;
                            if let Err(e) = target.split_focused_with_plugin(None, plugin) {
                                log::error!("Can not insert plugin into other workspace: {e}");
                            }
                            if let Some(area) = self.area {
                                target.core.compute_layout(area);
                            }
                        }

                        return EventOutcome::Consumed;
                    }
                    _ => (),
                }
            }
        }
        if let Some(ani) = &self.animation
            && !ani.is_finished(Instant::now())
            && let HypertileEvent::Tick = event
        {
            let (left, right) = ani.get_workspaces();
            if left != self.active
                && let Some(tab) = self.tabs.get_mut(left)
            {
                tab.runtime.handle_event(&mut event);
            }
            if right != self.active
                && let Some(tab) = self.tabs.get_mut(right)
            {
                tab.runtime.handle_event(&mut event);
            }
        }
        self.tabs[self.active].runtime.handle_event(&mut event)
    }

    pub fn render(&mut self, area: Rect, buf: &mut Buffer) {
        // update area width if changed
        match &mut self.area {
            Some(a) => {
                if a.width != area.width {
                    *a = area;
                    self.animation.take();
                }
            }
            None => self.area = Some(area),
        }
        if let Some(ani) = &mut self.animation {
            if !ani.is_finished(Instant::now()) {
                ani.display_spaces(&mut self.tabs, buf);
                return;
            } else {
                self.animation.take();
                for tab in self.tabs.iter_mut() {
                    tab.runtime
                        .registry
                        .broadcast_event(&mut ratatui_hypertile::HypertileEvent::AniStop);
                }
            }
        }

        self.tabs[self.active].runtime.render(area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_workspace() -> WorkspaceRuntime {
        WorkspaceRuntime::new(HypertileRuntime::new)
    }

    #[test]
    fn tab_lifecycle_keeps_active_index_valid() {
        let mut ws = test_workspace();
        ws.new_tab();
        ws.new_tab();
        ws.go_to_tab(0);
        ws.close_tab(0);
        assert_eq!(ws.tab_count(), 2);
        assert_eq!(ws.active_tab_index(), 0);
        ws.next_tab();
        assert_eq!(ws.active_tab_index(), 1);
        ws.prev_tab();
        assert_eq!(ws.active_tab_index(), 0);
        ws.close_tab(0);
        ws.close_tab(0);
        assert_eq!(ws.tab_count(), 1);
        assert_eq!(ws.active_tab_index(), 0);
    }
}
