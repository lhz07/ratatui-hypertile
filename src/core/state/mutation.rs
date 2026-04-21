use crate::core::helpers::{node_at_path, node_mut_at_path, normalize_ratio};
use crate::core::{CELL_RATIO, Node, PaneId, StateError};
use ratatui::layout::Direction;

use super::HypertileState;

impl HypertileState {
    /// [`split_with_ratio`](Self::split_with_ratio) with a 50/50 split.
    pub fn split(&mut self, direction: Direction, new_id: PaneId) -> Result<(), StateError> {
        self.split_ratio_with_direction(direction, new_id, 0.5)
    }

    /// Splits the focused pane and inserts `new_id` as the new sibling.
    ///
    /// Uses `ratio` for the new split and returns an error if `new_id` is already in the tree.
    pub fn split_ratio_with_direction(
        &mut self,
        direction: Direction,
        new_id: PaneId,
        ratio: f32,
    ) -> Result<(), StateError> {
        if self.pane_path_cached(new_id).is_some() {
            return Err(StateError::DuplicatePaneId(new_id));
        }
        let focused = node_mut_at_path(&mut self.root, &self.focused_path)?;
        let ratio = normalize_ratio(ratio);

        let old = match std::mem::replace(focused, Node::Pane(PaneId::ROOT)) {
            Node::Pane(id) => Node::Pane(id),
            other => {
                *focused = other;
                return Err(StateError::FocusedNodeNotPane);
            }
        };

        *focused = Node::Split {
            direction,
            ratio,
            first: Box::new(old),
            second: Box::new(Node::Pane(new_id)),
        };

        self.focused_path.push(1);
        self.rebuild_pane_index();
        self.invalidate_layout_cache();
        Ok(())
    }

    /// Removes the focused pane and promotes its sibling.
    ///
    /// Returns the removed pane id.
    pub fn remove_focused(&mut self) -> Result<PaneId, StateError> {
        if self.focused_path.is_empty() {
            return Err(StateError::CannotRemoveRootPane);
        }

        let removed_id = self.focused_pane().ok_or(StateError::FocusedNodeNotPane)?;
        if let Some(fid) = self.full_pane
            && fid == removed_id
        {
            self.full_pane.take();
        }
        let parent_len = self.focused_path.len() - 1;
        let child_idx = self.focused_path[parent_len];
        let sibling_idx = 1 - child_idx;

        let parent = node_mut_at_path(&mut self.root, &self.focused_path[..parent_len])?;

        let Node::Split { first, second, .. } = parent else {
            return Err(StateError::ParentNodeNotSplit);
        };

        let sibling = if sibling_idx == 0 {
            std::mem::replace(first.as_mut(), Node::Pane(PaneId::ROOT))
        } else {
            std::mem::replace(second.as_mut(), Node::Pane(PaneId::ROOT))
        };

        *parent = sibling;

        self.focused_path.truncate(parent_len);
        while matches!(
            node_at_path(&self.root, &self.focused_path),
            Some(Node::Split { .. })
        ) {
            self.focused_path.push(0);
        }

        self.rebuild_pane_index();
        self.invalidate_layout_cache();
        Ok(removed_id)
    }

    pub fn split_ratio(&mut self, new_id: PaneId, ratio: f32) -> Result<(), StateError> {
        if self.focused_path.is_empty() {
            self.split_ratio_with_direction(Direction::Horizontal, new_id, ratio)?;
            return Ok(());
        }
        let node = self.focused_pane().ok_or(StateError::FocusedNodeNotPane)?;
        let (_, rect) = self
            .layout_cache
            .iter()
            .find(|(id, _)| *id == node)
            .ok_or(StateError::UnknownPaneId(node))?;
        let direction = if rect.width as f64 >= rect.height as f64 * *CELL_RATIO {
            Direction::Horizontal
        } else {
            Direction::Vertical
        };
        // let parent_path = &self.focused_path[..self.focused_path.len() - 1];
        // let parent = node_mut_at_path(&mut self.root, parent_path)?;

        // let Node::Split { direction, .. } = parent else {
        //     return Err(StateError::ParentNodeNotSplit);
        // };
        // let direction = direction.perpendicular();
        self.split_ratio_with_direction(direction, new_id, ratio)
    }

    /// Adjusts the parent split ratio by `delta`.
    ///
    /// Returns `Ok(true)` if the ratio changed, or `Ok(false)` if there was nothing to change.
    pub fn resize_focused(&mut self, delta: f32) -> Result<bool, StateError> {
        let Some(&child_idx) = self.focused_path.last() else {
            return Ok(false);
        };

        let parent_path = &self.focused_path[..self.focused_path.len() - 1];
        let parent = node_mut_at_path(&mut self.root, parent_path)?;

        let Node::Split { ratio, .. } = parent else {
            return Err(StateError::ParentNodeNotSplit);
        };

        let next = if child_idx == 0 {
            *ratio + delta
        } else {
            *ratio - delta
        };
        let next = normalize_ratio(next);
        if (*ratio - next).abs() < f32::EPSILON {
            return Ok(false);
        }

        *ratio = next;
        self.invalidate_layout_cache();
        Ok(true)
    }
    pub fn toggle_focused_full(&mut self) -> Result<bool, StateError> {
        if self.sorted_panes.len() == 1 {
            return Ok(false);
        }
        if self.full_pane.is_some() {
            self.full_pane.take();
            self.invalidate_layout_cache();
            return Ok(true);
        }

        let pane = self.focused_pane().ok_or(StateError::FocusedNodeNotPane)?;
        self.full_pane = Some(pane);
        self.invalidate_layout_cache();
        Ok(true)
    }

    /// Sets the parent split ratio directly.
    ///
    /// Returns `Ok(true)` if the ratio changed, or `Ok(false)` if it was already at that value.
    pub fn set_focused_ratio(&mut self, ratio: f32) -> Result<bool, StateError> {
        if self.focused_path.is_empty() {
            return Ok(false);
        }

        let parent_path = &self.focused_path[..self.focused_path.len() - 1];
        let parent = node_mut_at_path(&mut self.root, parent_path)?;

        let Node::Split { ratio: current, .. } = parent else {
            return Err(StateError::ParentNodeNotSplit);
        };

        let next = normalize_ratio(ratio);
        if (*current - next).abs() < f32::EPSILON {
            return Ok(false);
        }

        *current = next;
        self.invalidate_layout_cache();
        Ok(true)
    }
}
