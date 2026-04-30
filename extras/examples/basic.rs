use ratatui::crossterm::{
    self,
    event::{
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event, KeyCode, KeyModifiers, MouseEventKind,
    },
    terminal::EnterAlternateScreen,
};
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    symbols::border,
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Widget, Wrap},
};
use ratatui_hypertile::{EventOutcome, HypertileAction, HypertileEvent};
use ratatui_hypertile_extras::{
    AnimationConfig, HypertilePlugin, HypertileRuntime, ModeIndicator, SplitBehavior,
    WorkspaceRuntime,
    pty::{CURSOR_POS, PICKER, PtyPlugin},
};
use std::{
    io::{self, stdout},
    sync::LazyLock,
    time::{Duration, Instant},
};
use tui_logger::TuiWidgetState;

fn build_runtime() -> HypertileRuntime {
    let mut rt = HypertileRuntime::builder()
        .with_split_behavior(SplitBehavior::Placeholder)
        .with_animation_config(AnimationConfig {
            enabled: true,
            ..AnimationConfig::default()
        })
        .build();
    rt.register_plugin_type("monitor", || MonitorPlugin {
        cpu: [15, 42, 8, 63],
        mem: 34,
        tick: 0,
    });

    rt.register_plugin_type("logs", || LogsPlugin {
        log_state: TuiWidgetState::new(),
    });
    rt.register_plugin_type("editor", || EditorPlugin {
        text: String::new(),
    });
    rt.register_plugin_type("network", || NetworkPlugin { tick: 0 });
    rt.register_plugin_type("fish", || PtyPlugin::new("fish".to_string()));
    rt.register_plugin_type("zsh", || PtyPlugin::new("zsh".to_string()));
    rt.register_plugin_type("bash", || PtyPlugin::new("bash".to_string()));
    rt
}

fn main() -> io::Result<()> {
    // Set max_log_level to Trace
    // it spawns a thread
    tui_logger::init_logger(log::LevelFilter::Trace).unwrap();

    // Set default level for unknown targets to Trace
    tui_logger::set_default_level(log::LevelFilter::Trace);
    tui_logger::set_env_filter_from_string(
        "basic=trace, ratatui_hypertile_extras=trace, ratatui_hypertile=trace",
    );
    crossterm::terminal::enable_raw_mode()?;
    crossterm::execute!(stdout(), EnterAlternateScreen)?;
    LazyLock::force(&PICKER);

    let mut terminal = ratatui::init();
    let mut stdout = stdout();
    // enable bracketed paste
    crossterm::execute!(stdout, EnableBracketedPaste)?;
    crossterm::execute!(stdout, EnableMouseCapture)?;

    let mut workspace = WorkspaceRuntime::new(build_runtime);
    let rt = workspace.active_runtime_mut();
    // Create the initial root pane
    let _ = rt.apply_core_action(HypertileAction::SplitFocused {
        direction: Direction::Horizontal,
    });
    let _ = rt.replace_focused_plugin("monitor");
    let _ = rt.split_focused(Some(Direction::Vertical), "logs");
    let _ = rt.split_focused(Some(Direction::Horizontal), "network");

    let result = run(&mut terminal, &mut workspace);
    crossterm::execute!(stdout, DisableBracketedPaste)?;
    crossterm::execute!(stdout, DisableMouseCapture)?;
    ratatui::restore();
    result
}

fn run(
    terminal: &mut ratatui::DefaultTerminal,
    workspace: &mut WorkspaceRuntime,
) -> io::Result<()> {
    let tick_rate = Duration::from_millis(300);
    let mut last_tick = Instant::now();
    loop {
        terminal.draw(|frame| {
            let [tabs, gap_top, body, gap_bot, footer] = Layout::vertical([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(0),
                Constraint::Length(1),
                Constraint::Length(1),
            ])
            .areas(frame.area());

            render_tabs(workspace, tabs, frame.buffer_mut());
            let _ = gap_top;
            workspace.render(body, frame.buffer_mut());
            let _ = gap_bot;

            let rt = workspace.active_runtime();
            let [mode_area, hint_area] =
                Layout::horizontal([Constraint::Length(10), Constraint::Min(0)]).areas(footer);
            ModeIndicator::new(rt.mode()).render(mode_area, frame.buffer_mut());
            Paragraph::new(
                "Ctrl+Alt+c: quit | Alt+←/→: workspace | Alt+t: split | Alt+q: close | Alt+p: palette",
            )
            .style(Style::default().fg(Color::DarkGray))
            .render(hint_area, frame.buffer_mut());
            let cursor_pos = CURSOR_POS.take();
            if let Some(pos) = cursor_pos{
                frame.set_cursor_position(pos);
            }
        })?;

        // let timeout = workspace.next_frame_in().map_or_else(
        //     || tick_rate.saturating_sub(last_tick.elapsed()),
        //     |frame| frame.min(tick_rate.saturating_sub(last_tick.elapsed())),
        // );
        let timeout = Duration::from_millis(16);
        if event::poll(timeout)? {
            let event = event::read()?;
            // match event {
            //     Event::Mouse(_) => (),
            //     Event::Key(key) if key.code == KeyCode::Up || key.code == KeyCode::Down => (),
            //     _ => match event {
            //         Event::Paste(_) => log::info!("paste event"),
            //         _ => log::info!("{:?}", event),
            //     },
            // }
            if let Event::Key(key) = event
                && key.code == KeyCode::Char('c')
                && key.modifiers == KeyModifiers::CONTROL | KeyModifiers::ALT
            {
                return Ok(());
            }
            workspace.handle_event(HypertileEvent::Term(event));
        }

        if last_tick.elapsed() >= tick_rate {
            workspace.handle_event(HypertileEvent::Tick);
            last_tick = Instant::now();
        }
    }
}

fn render_tabs(workspace: &WorkspaceRuntime, area: Rect, buf: &mut Buffer) {
    let spans: Vec<Span> = workspace
        .tab_labels()
        .enumerate()
        .flat_map(|(i, (label, active))| {
            let sep = if i > 0 { vec![Span::raw(" ")] } else { vec![] };
            let tab = if active {
                Span::styled(
                    format!(" {label} "),
                    Style::default()
                        .fg(Color::Rgb(30, 30, 46))
                        .bg(Color::Rgb(137, 180, 250))
                        .bold(),
                )
            } else {
                Span::styled(
                    format!(" {label} "),
                    Style::default()
                        .fg(Color::Rgb(205, 214, 244))
                        .bg(Color::Rgb(69, 71, 90)),
                )
            };
            sep.into_iter().chain(std::iter::once(tab))
        })
        .collect();
    Line::from(spans).render(area, buf);
}

struct MonitorPlugin {
    cpu: [u8; 4],
    mem: u8,
    tick: u64,
}

impl HypertilePlugin for MonitorPlugin {
    fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        is_focused: bool,
        _target_rect: Option<Rect>,
    ) {
        let mut lines = vec![Line::from("")];
        for (i, &usage) in self.cpu.iter().enumerate() {
            let filled = usage as usize * 20 / 100;
            let bar = "\u{2588}".repeat(filled) + &"\u{2591}".repeat(20 - filled);
            let color = match usage {
                0..50 => Color::Green,
                50..80 => Color::Yellow,
                _ => Color::Red,
            };
            lines.push(Line::from(vec![
                Span::raw(format!("  cpu{i} ")),
                Span::styled(bar, Style::default().fg(color)),
                Span::raw(format!(" {:>3}%", usage)),
            ]));
        }
        lines.push(Line::from(""));
        let filled = self.mem as usize * 20 / 100;
        let bar = "\u{2588}".repeat(filled) + &"\u{2591}".repeat(20 - filled);
        lines.push(Line::from(vec![
            Span::raw("  mem  "),
            Span::styled(bar, Style::default().fg(Color::Cyan)),
            Span::raw(format!(" {:.1}G/16G", self.mem as f64 * 16.0 / 100.0)),
        ]));

        Paragraph::new(lines)
            .block(pane_block("Monitor", is_focused, Color::Green))
            .render(area, buf);
    }

    fn on_event(&mut self, event: &mut HypertileEvent) -> EventOutcome {
        if !matches!(event, HypertileEvent::Tick) {
            return EventOutcome::Ignored;
        }
        self.tick += 1;
        let t = self.tick;
        self.cpu[0] = ((t * 7 + 15) % 85 + 10) as u8;
        self.cpu[1] = ((t * 13 + 42) % 75 + 5) as u8;
        self.cpu[2] = ((t * 3 + 28) % 90 + 8) as u8;
        self.cpu[3] = ((t * 11 + 55) % 70 + 15) as u8;
        self.mem = ((t * 2 + 34) % 30 + 40) as u8;
        EventOutcome::Consumed
    }
}

struct LogsPlugin {
    log_state: TuiWidgetState,
}

impl HypertilePlugin for LogsPlugin {
    fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        is_focused: bool,
        _target_rect: Option<Rect>,
    ) {
        let widget = tui_logger::TuiLoggerWidget::default()
            .style_error(Style::new().red().bold())
            .style_warn(Style::new().yellow())
            .style_info(Style::new().blue())
            .style_debug(Style::new().magenta())
            .style_trace(Style::new().gray())
            .output_timestamp(Some("%H:%M:%S.%f".to_string()));
        let logs = widget
            .block(pane_block("Logs", is_focused, Color::Blue))
            .state(&self.log_state);
        logs.render(area, buf);
    }

    fn on_event(&mut self, event: &mut HypertileEvent) -> EventOutcome {
        if let HypertileEvent::Term(term) = event {
            match term {
                Event::Mouse(mouse) => match mouse.kind {
                    MouseEventKind::ScrollDown => self
                        .log_state
                        .transition(tui_logger::TuiWidgetEvent::NextPageKey),
                    MouseEventKind::ScrollUp => {
                        self.log_state
                            .transition(tui_logger::TuiWidgetEvent::PrevPageKey);
                    }
                    _ => (),
                },
                Event::Key(key) => match key.code {
                    KeyCode::Down => self
                        .log_state
                        .transition(tui_logger::TuiWidgetEvent::NextPageKey),
                    KeyCode::Up => self
                        .log_state
                        .transition(tui_logger::TuiWidgetEvent::PrevPageKey),
                    _ => (),
                },
                _ => (),
            }
        }

        // self.tick += 1;
        // let (msg, color) = LOG_ENTRIES[self.tick as usize % LOG_ENTRIES.len()];
        // let h = (self.tick / 3600) % 24;
        // let m = (self.tick / 60) % 60;
        // let s = self.tick % 60;
        // if self.lines.len() >= 100 {
        //     self.lines.pop_front();
        // }
        // self.lines
        //     .push_back((format!("{h:02}:{m:02}:{s:02} {msg}"), color));
        EventOutcome::Consumed
    }
}

struct EditorPlugin {
    text: String,
}

impl HypertilePlugin for EditorPlugin {
    fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        is_focused: bool,
        _target_rect: Option<Rect>,
    ) {
        Paragraph::new(format!("{}\u{2588}", self.text))
            .block(pane_block("Editor", is_focused, Color::Magenta))
            .wrap(Wrap { trim: false })
            .render(area, buf);
    }

    fn on_event(&mut self, event: &mut HypertileEvent) -> EventOutcome {
        let HypertileEvent::Term(term) = event else {
            return EventOutcome::Ignored;
        };
        if let Event::Key(key) = term {
            match key.code {
                KeyCode::Char(ch) => {
                    self.text.push(ch);
                    EventOutcome::Consumed
                }
                KeyCode::Enter => {
                    self.text.push('\n');
                    EventOutcome::Consumed
                }
                KeyCode::Backspace => {
                    self.text.pop();
                    EventOutcome::Consumed
                }
                _ => EventOutcome::Ignored,
            }
        } else {
            EventOutcome::Ignored
        }
    }
}

struct NetworkPlugin {
    tick: u64,
}

impl HypertilePlugin for NetworkPlugin {
    fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        is_focused: bool,
        _target_rect: Option<Rect>,
    ) {
        let t = self.tick;
        let conns = 800 + (t * 17 % 120) as u32;
        let rps = 1100 + (t * 31 % 400) as u32;
        let p50 = 8 + (t * 3 % 15) as u32;
        let p99 = 60 + (t * 7 % 80) as u32;
        let errs = (t * 11 % 12) as u32;
        let up_h = t / 12;
        let up_m = (t * 5) % 60;

        let stat = |label: &str, value: String, color: Color| {
            Line::from(vec![
                Span::styled(
                    format!("  {label:<16}"),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(value, Style::default().fg(color)),
            ])
        };

        let text = vec![
            Line::from(""),
            stat("connections", format!("{conns}"), Color::Green),
            stat("requests/s", format!("{rps}"), Color::Green),
            Line::from(""),
            stat("latency p50", format!("{p50}ms"), Color::Cyan),
            stat(
                "latency p99",
                format!("{p99}ms"),
                if p99 > 100 {
                    Color::Yellow
                } else {
                    Color::Cyan
                },
            ),
            Line::from(""),
            stat(
                "errors/min",
                format!("{errs}"),
                if errs > 8 { Color::Red } else { Color::Green },
            ),
            stat("uptime", format!("{up_h}h {up_m}m"), Color::DarkGray),
        ];
        Paragraph::new(text)
            .block(pane_block("Network", is_focused, Color::Blue))
            .render(area, buf);
    }

    fn on_event(&mut self, event: &mut HypertileEvent) -> EventOutcome {
        if matches!(event, HypertileEvent::Tick) {
            self.tick += 1;
            EventOutcome::Consumed
        } else {
            EventOutcome::Ignored
        }
    }
}

fn pane_block<'a>(title: &'a str, is_focused: bool, color: Color) -> Block<'a> {
    if is_focused {
        Block::default()
            .borders(Borders::ALL)
            .border_set(border::THICK)
            .border_style(Style::default().fg(color).bold())
            .title(title)
    } else {
        Block::default().borders(Borders::ALL).title(title)
    }
}
