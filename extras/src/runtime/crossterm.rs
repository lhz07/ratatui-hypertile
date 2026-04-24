use ratatui_hypertile::HypertileEvent;

pub fn event_from_crossterm(event: crossterm::event::Event) -> HypertileEvent {
    HypertileEvent::Term(event)
}
