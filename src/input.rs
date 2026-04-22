use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::Direction;
use std::ops::{BitOr, BitOrAssign};

/// Event delivered to the layout engine or runtime.
///
/// The core engine only acts on `Action`. Key and tick are for higher-level code.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HypertileEvent {
    Key(KeyEvent),
    Action(HypertileAction),
    Tick,
}

/// `Start` means left or up. `End` means right or down.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Towards {
    Start,
    End,
}

/// How pane moves are resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MoveScope {
    /// Swap with the nearest pane in that direction (requires layout).
    Window,
    /// Swap inside the nearest ancestor split on that axis.
    Split,
}

/// Layout command for [`Hypertile::apply_action`](crate::Hypertile::apply_action).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HypertileAction {
    FocusNext,
    FocusPrev,
    FocusFull,
    FocusDirection {
        direction: Direction,
        towards: Towards,
    },
    SplitFocused {
        direction: Direction,
    },
    CloseFocused,
    ResizeFocused {
        delta: f32,
    },
    SetFocusedRatio {
        ratio: f32,
    },
    MoveFocused {
        direction: Direction,
        towards: Towards,
        scope: MoveScope,
    },
}

/// Whether an event handler consumed an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventOutcome {
    Ignored,
    Consumed,
}

impl EventOutcome {
    pub fn is_consumed(self) -> bool {
        matches!(self, Self::Consumed)
    }
}
