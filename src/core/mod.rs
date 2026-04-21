mod helpers;
#[cfg(feature = "serde")]
mod serde_impl;
mod state;
mod types;

use std::sync::LazyLock;

pub use state::{HypertileState, collect_pane_ids};
pub use types::{Node, PaneId, StateError};

static CELL_RATIO: LazyLock<f64> = LazyLock::new(get_cell_aspect_ratio);

fn get_cell_aspect_ratio() -> f64 {
    if let Ok(ws) = ratatui::crossterm::terminal::window_size()
        && ws.width > 0
        && ws.height > 0
    {
        let cell_width = ws.width as f64 / ws.columns as f64;
        let cell_height = ws.height as f64 / ws.rows as f64;
        return cell_height / cell_width;
    }

    2.1
}
