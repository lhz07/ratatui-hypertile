use crate::runtime::HypertileRuntime;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Direction;
use ratatui_hypertile::{HypertileAction, Towards};

#[derive(Debug, Clone, PartialEq)]
pub(super) enum RuntimeAction {
    Core(HypertileAction),
    SplitDirection(Direction),
    SplitDefault,
    OpenPalette,
}

/// Movement key preset for layout mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoveBindings {
    /// `HJKL` to move panes.
    Vim,
    /// `Shift+Arrow` to move panes.
    ShiftArrows,
    /// Both. Default.
    VimAndShiftArrows,
}

impl MoveBindings {
    fn includes_vim(self) -> bool {
        matches!(self, Self::Vim | Self::VimAndShiftArrows)
    }

    // fn includes_shift_arrows(self) -> bool {
    //     matches!(self, Self::ShiftArrows | Self::VimAndShiftArrows)
    // }
}

impl HypertileRuntime {
    pub(super) fn default_layout_action(&self, chord: KeyEvent) -> Option<RuntimeAction> {
        // const SHIFT_ARROW_MOVES: [(KeyCode, Direction, Towards); 4] = [
        //     (KeyCode::Left, Direction::Horizontal, Towards::Start),
        //     (KeyCode::Right, Direction::Horizontal, Towards::End),
        //     (KeyCode::Down, Direction::Vertical, Towards::End),
        //     (KeyCode::Up, Direction::Vertical, Towards::Start),
        // ];

        // if self.move_bindings.includes_shift_arrows()
        //     && chord.modifiers == KeyModifiers::SHIFT | KeyModifiers::ALT
        // {
        //     for &(code, direction, towards) in &SHIFT_ARROW_MOVES {
        //         if chord.code == code {
        //             return Some(RuntimeAction::Core(HypertileAction::MoveFocused {
        //                 direction,
        //                 towards,
        //                 scope: self.default_move_scope,
        //             }));
        //         }
        //     }
        // }

        const VIM_MOVES: [(char, Direction, Towards); 4] = [
            ('H', Direction::Horizontal, Towards::Start),
            ('L', Direction::Horizontal, Towards::End),
            ('J', Direction::Vertical, Towards::End),
            ('K', Direction::Vertical, Towards::Start),
        ];
        if self.move_bindings.includes_vim()
            && chord.modifiers == KeyModifiers::SHIFT | KeyModifiers::ALT
        {
            for &(ch, direction, towards) in &VIM_MOVES {
                if chord.code == KeyCode::Char(ch) {
                    return Some(RuntimeAction::Core(HypertileAction::MoveFocused {
                        direction,
                        towards,
                        scope: self.default_move_scope,
                    }));
                }
            }
        }

        match (chord.modifiers, chord.code) {
            (KeyModifiers::ALT, KeyCode::Char('p')) => Some(RuntimeAction::OpenPalette),
            (KeyModifiers::ALT, KeyCode::Char('d')) => {
                Some(RuntimeAction::Core(HypertileAction::FocusMax))
            }
            (KeyModifiers::ALT, KeyCode::Char('q')) => {
                Some(RuntimeAction::Core(HypertileAction::CloseFocused))
            }
            (KeyModifiers::ALT, KeyCode::Char('t')) => Some(RuntimeAction::SplitDefault),
            (KeyModifiers::ALT, KeyCode::Char('s')) => {
                Some(RuntimeAction::SplitDirection(Direction::Horizontal))
            }
            (KeyModifiers::ALT, KeyCode::Char('v')) => {
                Some(RuntimeAction::SplitDirection(Direction::Vertical))
            }
            (KeyModifiers::ALT, KeyCode::Char('-')) => {
                Some(RuntimeAction::Core(HypertileAction::ResizeFocused {
                    delta: -self.core.resize_step(),
                }))
            }
            (KeyModifiers::ALT, KeyCode::Char('=')) => {
                Some(RuntimeAction::Core(HypertileAction::ResizeFocused {
                    delta: self.core.resize_step(),
                }))
            }
            (KeyModifiers::ALT, KeyCode::Char('h')) => {
                Some(RuntimeAction::Core(HypertileAction::FocusDirection {
                    direction: Direction::Horizontal,
                    towards: Towards::Start,
                }))
            }
            (KeyModifiers::ALT, KeyCode::Char('l')) => {
                Some(RuntimeAction::Core(HypertileAction::FocusDirection {
                    direction: Direction::Horizontal,
                    towards: Towards::End,
                }))
            }
            (KeyModifiers::ALT, KeyCode::Char('j')) => {
                Some(RuntimeAction::Core(HypertileAction::FocusDirection {
                    direction: Direction::Vertical,
                    towards: Towards::End,
                }))
            }
            (KeyModifiers::ALT, KeyCode::Char('k')) => {
                Some(RuntimeAction::Core(HypertileAction::FocusDirection {
                    direction: Direction::Vertical,
                    towards: Towards::Start,
                }))
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::HypertileRuntime;
    use ratatui_hypertile::MoveScope;

    #[test]
    fn default_move_bindings_include_vim_and_shift_arrows() {
        let runtime = HypertileRuntime::new();

        let shift_arrow =
            runtime.default_layout_action(KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT));
        assert!(matches!(
            shift_arrow,
            Some(RuntimeAction::Core(HypertileAction::MoveFocused {
                direction: Direction::Horizontal,
                towards: Towards::Start,
                scope: MoveScope::Window,
            }))
        ));

        let vim =
            runtime.default_layout_action(KeyEvent::new(KeyCode::Char('H'), KeyModifiers::SHIFT));
        assert!(matches!(
            vim,
            Some(RuntimeAction::Core(HypertileAction::MoveFocused {
                direction: Direction::Horizontal,
                towards: Towards::Start,
                scope: MoveScope::Window,
            }))
        ));
    }
}
