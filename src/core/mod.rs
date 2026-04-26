mod helpers;
#[cfg(feature = "serde")]
mod serde_impl;
mod state;
mod types;

use std::sync::LazyLock;

pub use state::{HypertileState, collect_pane_ids};
pub use types::{Node, PaneId, StateError};

static CELL_RATIO: LazyLock<CellInfo> = LazyLock::new(get_cell_aspect_ratio);

fn get_cell_aspect_ratio() -> CellInfo {
    if let Ok(ws) = ratatui::crossterm::terminal::window_size()
        && ws.width > 0
        && ws.height > 0
    {
        let cell_width = ws.width as f64 / ws.columns as f64;
        let cell_height = ws.height as f64 / ws.rows as f64;
        return CellInfo {
            width: cell_width,
            height: cell_height,
            ratio: cell_height / cell_width,
        };
    }
    CellInfo {
        width: 16.0,
        height: 36.0,
        ratio: 2.25,
    }
}

pub struct CellInfo {
    width: f64,
    height: f64,
    ratio: f64,
}

impl CellInfo {
    pub fn width() -> f64 {
        CELL_RATIO.width
    }

    pub fn height() -> f64 {
        CELL_RATIO.height
    }

    pub fn ratio() -> f64 {
        CELL_RATIO.ratio
    }
    pub fn pixel_width(cols: impl Into<f64>) -> f64 {
        CELL_RATIO.width * cols.into()
    }
    pub fn pixel_height(rows: impl Into<f64>) -> f64 {
        CELL_RATIO.height * rows.into()
    }
}
