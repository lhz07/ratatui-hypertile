use ratatui_hypertile::HypertileEvent;

pub fn event_from_crossterm(key: crossterm::event::KeyEvent) -> HypertileEvent {
    HypertileEvent::Key(key)
}
