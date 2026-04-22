use std::{
    io::{self, Write},
    time::Duration,
};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use portable_pty::{Child, CommandBuilder, NativePtySystem, PtyPair, PtySize, PtySystem};
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    symbols::border,
    widgets::{Block, Borders, Paragraph, Widget},
};
use ratatui_hypertile::HypertileEvent;
use tokio::{
    io::{AsyncRead, unix::AsyncFd},
    sync::{mpsc, oneshot},
};
use tui_term::widget::PseudoTerminal;
use vt100::Screen;

use crate::{HypertilePlugin, runtime::tokio_spawn};

#[derive(Default)]
pub struct PtyPlugin {
    mounted: Option<MountedPty>,
}

impl PtyPlugin {
    pub fn new() -> Self {
        Self::default()
    }
}

pub struct MountedPty {
    root: PtyPair,
    child: Box<dyn Child + Send + Sync>,
    area: Rect,
    render_tx: mpsc::Sender<RenderMsg>,
    input_tx: mpsc::Sender<InputMsg>,
}

struct PtyFd(AsyncFd<i32>);

impl PtyFd {
    async fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            let mut guard = self.0.readable().await?;
            let n = unsafe { libc::read(*guard.get_inner(), buf.as_mut_ptr() as _, buf.len()) };
            if n == -1 {
                let err = std::io::Error::last_os_error();
                if err.kind() == io::ErrorKind::WouldBlock {
                    guard.clear_ready();
                    continue;
                } else {
                    return Err(err);
                }
            } else {
                return Ok(n as usize);
            }
        }
    }
    async fn write(&self, buf: &[u8]) -> io::Result<usize> {
        loop {
            let mut guard = self.0.writable().await?;
            let n = unsafe { libc::write(*guard.get_inner(), buf.as_ptr() as _, buf.len()) };
            if n == -1 {
                let err = std::io::Error::last_os_error();
                if err.kind() == io::ErrorKind::WouldBlock {
                    guard.clear_ready();
                    continue;
                } else {
                    return Err(err);
                }
            } else {
                return Ok(n as usize);
            }
        }
    }
}

enum RenderMsg {
    GetScreen(oneshot::Sender<Screen>),
    SetSize(Rect),
}

impl RenderMsg {
    fn new() -> (Self, oneshot::Receiver<Screen>) {
        let (tx, rx) = oneshot::channel();
        (Self::GetScreen(tx), rx)
    }
}

struct InputMsg {
    event: Vec<u8>,
}

const MIN_COL: u16 = 20;
const MIN_ROW: u16 = 10;

impl MountedPty {
    fn resize(&mut self, mut area: Rect) {
        area.height = area.height.max(MIN_ROW);
        area.width = area.width.max(MIN_COL);
        if area.height != self.area.height || area.width != self.area.width {
            let rows = area.height;
            let cols = area.width;
            if self
                .root
                .master
                .resize(PtySize {
                    rows,
                    cols,
                    ..Default::default()
                })
                .is_err()
            {
                return;
            }
            if self.render_tx.try_send(RenderMsg::SetSize(area)).is_err() {
                return;
            }
            self.area = area;
            // log::info!("resize success: {:?}", area);
        }
    }
    pub fn create(area: Rect) -> anyhow::Result<Self> {
        let rows = 24;
        let cols = 80;
        let pty = NativePtySystem::default();
        let root = pty.openpty(PtySize {
            rows,
            cols,
            ..Default::default()
        })?;
        let child = root.slave.spawn_command(CommandBuilder::new("zsh"))?;
        let fd = root.master.as_raw_fd().expect("valid on macOS");
        unsafe {
            // set nonblocking
            let res = libc::ioctl(fd, libc::FIONBIO, &mut (true as libc::c_int));
            if res == -1 {
                return Err(io::Error::last_os_error().into());
            }
        }
        let (render_tx, mut render_rx) = mpsc::channel::<RenderMsg>(10);
        let (input_tx, mut input_rx) = mpsc::channel::<InputMsg>(100);
        std::thread::sleep(Duration::from_secs(3));
        tokio_spawn(async move {
            let res: Result<(), anyhow::Error> = async {
                let async_fd = AsyncFd::new(fd)?;
                let pty_fd = PtyFd(async_fd);
                let mut parser = vt100::Parser::new(rows, cols, 0);
                let mut buf = [0; 8192];
                loop {
                    tokio::select! {
                        res = pty_fd.read(&mut buf) => {
                            log::info!("read");
                            let n = res?;
                            if n != 0{
                                // update
                                parser.process(&buf[..n]);
                            }
                        }
                        Some(msg) = render_rx.recv() => {
                            match msg{
                                RenderMsg::GetScreen(tx) => {
                                    if tx.send(parser.screen().clone()).is_err(){
                                        log::error!("can not send screen")
                                    }
                                }
                                RenderMsg::SetSize(rect) => {
                                    parser.screen_mut().set_size(rect.height, rect.width);
                                }
                            }
                        }
                        else => {
                            break;
                        }
                    }
                }
                Ok(())
            }
            .await;
            // log error
            if let Err(e) = res {
                log::error!("pty output: {e}")
            }
        });
        tokio_spawn(async move {
            let res: Result<(), anyhow::Error> = async {
                let async_fd = AsyncFd::new(fd)?;
                let pty_fd = PtyFd(async_fd);
                loop {
                    let msg = input_rx
                        .recv()
                        .await
                        .ok_or(anyhow::anyhow!("recv input msg"))?;
                    log::info!("write key");
                    pty_fd.write(&msg.event).await?;
                }
            }
            .await;
            if let Err(e) = res {
                log::error!("pty input: {e}")
            }
        });

        Ok(Self {
            root,
            child,
            area,
            render_tx,
            input_tx,
        })
    }
}

impl HypertilePlugin for PtyPlugin {
    fn on_event(
        &mut self,
        event: &ratatui_hypertile::HypertileEvent,
    ) -> ratatui_hypertile::EventOutcome {
        let Some(pty) = &mut self.mounted else {
            return ratatui_hypertile::EventOutcome::Ignored;
        };
        match event {
            HypertileEvent::Key(key) => {
                if let Some(event) = convert_key_event(key) {
                    // send event
                    let res = pty.input_tx.try_send(InputMsg { event });
                    // log::info!("send key: {:?}, result: {:?}", key, res);
                    return ratatui_hypertile::EventOutcome::Consumed;
                }
            }
            _ => (),
        }
        ratatui_hypertile::EventOutcome::Ignored
    }
    fn render(&mut self, area: Rect, buf: &mut ratatui::prelude::Buffer, is_focused: bool) {
        let pty = match &mut self.mounted {
            Some(p) => p,
            None => match MountedPty::create(area) {
                Ok(mounted) => self.mounted.insert(mounted),
                // TODO: Don't panic here, add a log instead.
                Err(e) => panic!("{e}"),
            },
        };
        // pty.resize(area);
        let (msg, rx) = RenderMsg::new();
        if let Err(_) = pty.render_tx.try_send(msg) {
            return;
        }
        let Ok(screen) = rx.blocking_recv() else {
            return;
        };
        let pseudo_term = PseudoTerminal::new(&screen)
            .block(pane_block("Terminal", is_focused, Color::Blue))
            .style(
                Style::default()
                    .fg(Color::White)
                    .bg(Color::Black)
                    .add_modifier(Modifier::BOLD),
            );
        // log::info!("render term");
        pseudo_term.render(area, buf);
        // render_screen_to_buffer(&screen, area, buf);
    }
    fn on_unmount(&mut self, _ctx: crate::PluginContext) {
        if let Some(mut pty) = self.mounted.take() {
            let _ = pty.child.kill();
        }
    }
}

fn convert_key_event(key: &KeyEvent) -> Option<Vec<u8>> {
    let input_bytes = match key.code {
        KeyCode::Char(ch) => {
            let mut send = vec![ch as u8];
            let upper = ch.to_ascii_uppercase();
            if key.modifiers == KeyModifiers::CONTROL {
                match upper {
                    'N' => {
                        // Ignore Ctrl+n within a pane
                        return None;
                    }
                    'X' => {
                        // Close the pane
                        return None;
                    }
                    // https://github.com/fyne-io/terminal/blob/master/input.go
                    // https://gist.github.com/ConnerWill/d4b6c776b509add763e17f9f113fd25b
                    '2' | '@' | ' ' => send = vec![0],
                    '3' | '[' => send = vec![27],
                    '4' | '\\' => send = vec![28],
                    '5' | ']' => send = vec![29],
                    '6' | '^' => send = vec![30],
                    '7' | '-' | '_' => send = vec![31],
                    char if ('A'..='_').contains(&char) => {
                        // Since A == 65,
                        // we can safely subtract 64 to get
                        // the corresponding control character
                        let ascii_val = char as u8;
                        let ascii_to_send = ascii_val - 64;
                        send = vec![ascii_to_send];
                    }
                    _ => {}
                }
            }
            send
        }
        #[cfg(unix)]
        KeyCode::Enter => vec![b'\n'],
        #[cfg(windows)]
        KeyCode::Enter => vec![b'\r', b'\n'],
        KeyCode::Backspace => vec![8],
        KeyCode::Left => vec![27, 91, 68],
        KeyCode::Right => vec![27, 91, 67],
        KeyCode::Up => vec![27, 91, 65],
        KeyCode::Down => vec![27, 91, 66],
        KeyCode::Tab => vec![9],
        KeyCode::Home => vec![27, 91, 72],
        KeyCode::End => vec![27, 91, 70],
        KeyCode::PageUp => vec![27, 91, 53, 126],
        KeyCode::PageDown => vec![27, 91, 54, 126],
        KeyCode::BackTab => vec![27, 91, 90],
        KeyCode::Delete => vec![27, 91, 51, 126],
        KeyCode::Insert => vec![27, 91, 50, 126],
        KeyCode::Esc => vec![27],
        _ => return None,
    };

    Some(input_bytes)
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

#[tokio::test]
async fn test_fd() -> anyhow::Result<()> {
    let pty = NativePtySystem::default();
    let root = pty.openpty(PtySize::default())?;
    let child = root.slave.spawn_command(CommandBuilder::new("fish"))?;
    let fd = root.master.as_raw_fd().expect("valid on macOS");
    unsafe {
        // set nonblocking
        let res = libc::ioctl(fd, libc::FIONBIO, &mut (true as libc::c_int));
        if res == -1 {
            return Err(io::Error::last_os_error().into());
        }
    }
    let async_fd = AsyncFd::new(fd)?;
    Ok(())
}
