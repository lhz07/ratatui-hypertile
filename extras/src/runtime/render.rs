use crate::{InputMode, runtime::HypertileRuntime};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, StatefulWidget, Widget},
};
use ratatui_hypertile::PaneId;
use std::{collections::HashMap, time::Instant};

impl HypertileRuntime {
    /// Renders panes and the palette overlay if it is open.
    ///
    /// Call [`next_frame_in`](super::HypertileRuntime::next_frame_in) after drawing if
    /// you want move animations to keep updating.
    pub fn render(&mut self, area: Rect, buf: &mut Buffer) {
        let now = Instant::now();
        self.animation_state.remember_area(area);
        self.core.compute_layout(area);
        let focused = self.core.focused_pane();
        let full = self.core.state().full_pane();
        let mut full_rect = None;
        let highlight = self.core.state().focus_highlight();
        let registry = &mut self.registry;
        let border_config = &self.border_config;
        let mut close_pane = None;
        let (panes, ani_active) =
            self.animation_state
                .display_rects(area, self.core.state().panes(), now);
        let target_rects = self.core.state().panes().collect::<HashMap<_, _>>();
        // skip other panes if there is no animation and showing a full pane
        if let Some(full_id) = full
            && !ani_active
        {
            // layout_cache is sorted here
            debug_assert!(panes.is_sorted_by(|(a, _), (b, _)| a <= b));
            full_rect = panes
                .binary_search_by(|i| i.0.cmp(&full_id))
                .map(|i| panes[i].1)
                .ok();
        } else {
            for &(pane_id, rect) in panes {
                if let Some(full_id) = full
                    && full_id == pane_id
                {
                    full_rect = Some(rect);
                    continue;
                }
                Clear.render(rect, buf);
                let is_focused = highlight && Some(pane_id) == focused;
                if let Some(plugin_instance) = registry.plugin_instance_mut(pane_id) {
                    // if plugin_instance.plugin_type() != "logs" {
                    //     log::info!("render {}", plugin_instance.plugin_type());
                    // }
                    let target_rect = if ani_active {
                        target_rects.get(&pane_id).map(|rect| *rect)
                    } else {
                        None
                    };
                    plugin_instance
                        .plugin()
                        .render(rect, buf, is_focused, target_rect);
                    if plugin_instance.plugin().is_closed() {
                        close_pane = Some(pane_id);
                    }
                } else {
                    render_fallback_pane(border_config, pane_id, rect, buf, is_focused);
                }
            }
        }
        // render full pane
        if let Some(rect) = full_rect
            && !rect.is_empty()
            && let Some(pane_id) = full
        {
            Clear.render(rect, buf);
            let is_focused = highlight && Some(pane_id) == focused;
            if let Some(plugin) = registry.plugin_mut(pane_id) {
                let target_rect = if ani_active {
                    target_rects.get(&pane_id).map(|rect| *rect)
                } else {
                    None
                };
                plugin.render(rect, buf, is_focused, target_rect);
                if plugin.is_closed() {
                    close_pane = Some(pane_id);
                }
            } else {
                render_fallback_pane(border_config, pane_id, rect, buf, is_focused);
            }
        }
        if let Some(id) = close_pane {
            let now = Instant::now();
            registry.remove_plugin_if_exists(id);
            self.capture_displayed_rects(now);
            if Some(id) == focused {
                let _ = self.core.state_mut().remove_focused();
                if self.mode == InputMode::PluginInput {
                    self.mode = InputMode::Layout;
                }
            } else {
                let _ = self.core.state_mut().remove(id);
            }
            self.start_action_animation(now);
        }

        if self.palette.show {
            self.render_palette(area, buf);
        }
    }

    pub(super) fn render_palette(&mut self, area: Rect, buf: &mut Buffer) {
        let filtered = self.filtered_palette_items();
        if filtered.is_empty() {
            return;
        }

        let popup = centered_rect(
            self.palette.width_percent,
            self.palette.height_percent,
            area,
        );
        Clear.render(popup, buf);

        let max_visible = self.palette.max_items.max(1).min(filtered.len());
        let start = self
            .palette
            .selected
            .saturating_sub(max_visible.saturating_sub(1));
        let end = (start + max_visible).min(filtered.len());
        let visible = &filtered[start..end];
        let selected = self.palette.selected.saturating_sub(start);

        let title = if self.palette.query.is_empty() {
            " Plugins ".to_string()
        } else {
            format!(" {} ", self.palette.query)
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Rgb(137, 180, 250)))
            .title(title);
        let inner = block.inner(popup);
        block.render(popup, buf);

        let items = visible
            .iter()
            .map(|name| ListItem::new(format!("  {name}  ")))
            .collect::<Vec<_>>();
        let list = List::new(items).highlight_style(
            Style::default()
                .fg(Color::Rgb(30, 30, 46))
                .bg(Color::Rgb(137, 180, 250))
                .bold(),
        );
        let mut state = ListState::default();
        state.select(Some(selected));
        StatefulWidget::render(list, inner, buf, &mut state);
    }
}

fn render_fallback_pane(
    cfg: &crate::runtime::BorderConfig,
    pane_id: PaneId,
    area: Rect,
    buf: &mut Buffer,
    is_focused: bool,
) {
    let mut block = Block::default()
        .borders(cfg.borders)
        .border_set(cfg.border_set)
        .border_style(cfg.border_style)
        .title(format!("Pane {}", pane_id.get()));
    if is_focused {
        block = block
            .border_set(cfg.focused_border_set)
            .border_style(cfg.focused_border_style);
    }
    block.render(area, buf);
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let w = area.width * percent_x / 100;
    let h = area.height * percent_y / 100;
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    Rect::new(x, y, w, h)
}
