use crossterm::event::{
    Event as CrosstermEvent, KeyCode as CrosstermKeyCode, KeyEvent as CrosstermKeyEvent,
    KeyEventKind, KeyModifiers as CrosstermModifiers,
};
use portable_pty::{Child, CommandBuilder, NativePtySystem, PtyPair, PtySize, PtySystem};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    symbols::border,
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Widget},
};
use ratatui_hypertile::HypertileEvent;
use std::{
    io::{self},
    mem,
    pin::Pin,
    sync::Arc,
    task::{Poll, ready},
};

use termwiz::color::ColorAttribute;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, unix::AsyncFd},
    sync::{
        mpsc::{self, error::TrySendError},
        oneshot,
    },
};
use wezterm_cell::{Cell, Intensity};
use wezterm_term::{
    KeyCode, KeyModifiers, Terminal, TerminalConfiguration, TerminalSize, TerminalState,
};

use crate::{HypertilePlugin, runtime::tokio_spawn};

#[derive(Default)]
pub struct PtyPlugin {
    mounted: Option<MountedPty>,
}

impl PtyPlugin {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn close(&mut self) {
        // TODO: terminate gracefully
        if let Some(mut pty) = self.mounted.take() {
            if let Err(e) = pty.child.kill() {
                log::error!("kill child: {e}")
            }
            // match pty.child.wait() {
            //     Ok(s) => log::info!("child exited with {s}"),
            //     Err(e) => log::error!("child exit: {e}"),
            // }
        }
    }
}

pub struct MountedPty {
    root: PtyPair,
    child: Box<dyn Child + Send + Sync>,
    area: Rect,
    render_tx: mpsc::Sender<RenderMsg>,
}

#[derive(Clone, Debug)]
struct PtyFd(Arc<AsyncFd<i32>>);

impl AsyncRead for PtyFd {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        loop {
            let mut guard = ready!(self.0.poll_read_ready(cx))?;
            let b = unsafe { buf.unfilled_mut() };
            let n = unsafe { libc::read(*guard.get_inner(), b.as_mut_ptr() as _, b.len()) };
            if n == -1 {
                let err = std::io::Error::last_os_error();
                if err.kind() == io::ErrorKind::WouldBlock {
                    guard.clear_ready();
                } else {
                    return Poll::Ready(Err(err));
                }
            } else {
                let n = n as usize;
                // Safety: We trust `read` to have filled up `n` bytes in the
                // buffer.
                unsafe { buf.assume_init(n) };
                buf.advance(n);
                return Poll::Ready(Ok(()));
            }
        }
    }
}

impl AsyncWrite for PtyFd {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        loop {
            log::trace!("wait for writing...");
            let mut guard = ready!(self.0.poll_write_ready(cx))?;
            log::trace!("write once");
            let n = unsafe { libc::write(*guard.get_inner(), buf.as_ptr() as _, buf.len()) };
            log::trace!("write call finished");
            if n == -1 {
                let err = std::io::Error::last_os_error();
                if err.kind() == io::ErrorKind::WouldBlock {
                    log::trace!("would block");
                    guard.clear_ready();
                } else {
                    return Poll::Ready(Err(err));
                }
            } else {
                log::trace!("write finished");
                return Poll::Ready(Ok(n as usize));
            }
        }
    }
    fn poll_flush(self: Pin<&mut Self>, _cx: &mut std::task::Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(
        self: Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

enum RenderMsg {
    RenderScreen(Rect, Buffer, oneshot::Sender<Buffer>, bool),
    Event(HypertileEvent),
    SetSize(Rect),
}

impl RenderMsg {
    fn render_screen(
        area: Rect,
        buf: Buffer,
        is_focused: bool,
    ) -> (Self, oneshot::Receiver<Buffer>) {
        let (tx, rx) = oneshot::channel();
        (Self::RenderScreen(area, buf, tx, is_focused), rx)
    }
}

struct AsyncWriteAdapter {
    input_tx: mpsc::UnboundedSender<InputMsg>,
}

impl AsyncWriteAdapter {
    fn new(input_tx: mpsc::UnboundedSender<InputMsg>) -> Self {
        Self { input_tx }
    }
}

impl std::io::Write for AsyncWriteAdapter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.input_tx
            .send(InputMsg {
                event: buf.to_vec(),
            })
            .map_err(io::Error::other)?;
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
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
        let rows = area.height.max(MIN_ROW);
        let cols = area.width.max(MIN_COL);
        let pty = NativePtySystem::default();
        let root = pty.openpty(PtySize {
            rows,
            cols,
            ..Default::default()
        })?;
        let child = root.slave.spawn_command(CommandBuilder::new("fish"))?;
        let fd = root.master.as_raw_fd().expect("valid on macOS");
        unsafe {
            // set nonblocking
            let res = libc::ioctl(fd, libc::FIONBIO, &mut (true as libc::c_int));
            if res == -1 {
                return Err(io::Error::last_os_error().into());
            }
        }
        let (render_tx, mut render_rx) = mpsc::channel::<RenderMsg>(100);
        let (input_tx, mut input_rx) = mpsc::unbounded_channel::<InputMsg>();
        let mut size = TerminalSize {
            rows: rows as usize,
            cols: cols as usize,
            pixel_width: 0,
            pixel_height: 0,
            dpi: 96,
        };
        let writer = AsyncWriteAdapter::new(input_tx);
        tokio_spawn(async move {
            let res: Result<(), anyhow::Error> = async {
                let async_fd = AsyncFd::new(fd)?;
                let mut pty_fd = PtyFd(Arc::new(async_fd));
                let mut pty_fd_write = pty_fd.clone();
                tokio_spawn(async move {
                    let res: Result<(), anyhow::Error> = async {
                        loop {
                            let msg = input_rx
                                .recv()
                                .await
                                .ok_or(anyhow::anyhow!("recv input msg"))?;
                            pty_fd_write.write_all(&msg.event).await?;
                        }
                    }
                    .await;
                    if let Err(e) = res {
                        log::error!("pty input: {e}")
                    }
                });
                let config = Arc::new(SimpleTermConfig);
                let mut terminal = Terminal::new(size, config, "", "", Box::new(writer));
                let mut buf = [0; 8192];
                loop {
                    tokio::select! {
                        biased;
                        // read all first
                        res = pty_fd.read(&mut buf) => {
                            log::info!("read");
                            let n = res?;
                            if n != 0{
                                // update
                                terminal.advance_bytes(&buf[..n]);
                            } else {
                                // exit now!
                                break;
                            }
                        }
                        // no more to read, render it
                        Some(msg) = render_rx.recv() => {
                            handle_msg(msg, &mut terminal, &mut size);
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

        Ok(Self {
            root,
            child,
            area,
            render_tx,
        })
    }
}

fn handle_msg(msg: RenderMsg, terminal: &mut TerminalState, size: &mut TerminalSize) {
    match msg {
        RenderMsg::RenderScreen(area, mut buf, tx, is_focused) => {
            let title = terminal.get_title();
            let title = if !title.is_empty() {
                format!(" fish {title} ")
            } else {
                format!(" fish ")
            };
            let block = pane_block(title, is_focused, Color::Blue);
            let term = TerminalWidget::new(&terminal);
            // let ins = std::time::Instant::now();
            term.render(block.inner(area), &mut buf);
            // about 50 - 500 micro seconds
            // log::info!("render terminal cost: {:?}", ins.elapsed());
            block.render(area, &mut buf);
            if tx.send(buf).is_err() {
                log::error!("can not send pty buffer back")
            }
        }
        RenderMsg::SetSize(rect) => {
            size.cols = rect.width as usize;
            size.rows = rect.height as usize;
            terminal.resize(*size);
        }
        RenderMsg::Event(event) => {
            if let HypertileEvent::Term(term_event) = event {
                match term_event {
                    CrosstermEvent::Key(key_event)
                        if let Some((key, mods)) = crossterm_to_wezterm(key_event) =>
                    {
                        match key_event.kind {
                            KeyEventKind::Press => {
                                let _ = terminal.key_down(key, mods);
                            }

                            KeyEventKind::Release => {
                                let _ = terminal.key_up(key, mods);
                            }
                            KeyEventKind::Repeat => {
                                let _ = terminal.key_down(key, mods);
                                let _ = terminal.key_up(key, mods);
                            }
                        }
                    }
                    CrosstermEvent::Paste(s) => {
                        if let Err(e) = terminal.send_paste(&s) {
                            log::error!("paste: {e}");
                        }
                    }
                    _ => (),
                }
            }
        }
    }
}

#[derive(Debug)]
struct SimpleTermConfig;

impl TerminalConfiguration for SimpleTermConfig {
    fn scrollback_size(&self) -> usize {
        100
    }

    fn color_palette(&self) -> wezterm_term::color::ColorPalette {
        wezterm_term::color::ColorPalette::default()
    }
}

impl HypertilePlugin for PtyPlugin {
    fn is_closed(&mut self) -> bool {
        self.mounted.is_none()
    }
    fn on_event(
        &mut self,
        event: &mut ratatui_hypertile::HypertileEvent,
    ) -> ratatui_hypertile::EventOutcome {
        let Some(pty) = &mut self.mounted else {
            return ratatui_hypertile::EventOutcome::Ignored;
        };
        let mut send_event = HypertileEvent::Tick;
        mem::swap(&mut send_event, event);
        // send event
        let _res = pty.render_tx.try_send(RenderMsg::Event(send_event));
        // log::info!("send key: {:?}, result: {:?}", key, res);
        ratatui_hypertile::EventOutcome::Consumed
    }
    fn render(
        &mut self,
        area: Rect,
        buf: &mut ratatui::prelude::Buffer,
        is_focused: bool,
        target_rect: Option<Rect>,
    ) {
        let block = pane_block("Terminal", is_focused, Color::Blue);
        let term_area = block.inner(area);
        let pty = match &mut self.mounted {
            Some(pty) => {
                if let Some(target) = target_rect {
                    pty.resize(block.inner(target));
                } else {
                    pty.resize(term_area);
                }
                pty
            }
            None => match MountedPty::create(term_area) {
                Ok(mounted) => self.mounted.insert(mounted),
                Err(e) => {
                    log::error!("{e}");
                    return;
                }
            },
        };
        let buffer = mem::take(buf);
        let (msg, rx) = RenderMsg::render_screen(area, buffer, is_focused);
        if let Err(e) = pty.render_tx.try_send(msg) {
            let msg = match e {
                TrySendError::Closed(msg) | TrySendError::Full(msg) => msg,
            };
            if let RenderMsg::RenderScreen(_, mut buffer, _, _) = msg {
                // restore buf
                mem::swap(&mut buffer, buf);
                self.close();
                return;
            } else {
                // SAFETY: we just send `RenderScreen` msg
                unsafe { std::hint::unreachable_unchecked() }
            }
        }
        // let ins = std::time::Instant::now();
        let Ok(mut buffer) = rx.blocking_recv() else {
            return;
        };
        // log::info!("recv msg cost: {:?}", ins.elapsed());
        mem::swap(&mut buffer, buf);
        // // log::info!("render term");
    }
    fn on_unmount(&mut self, _ctx: crate::PluginContext) {
        self.close();
    }
}

/// 将 WezTerm 的屏幕转换为 Ratatui 可渲染的内容
pub struct TerminalRenderer<'a> {
    terminal: &'a TerminalState,
}

impl<'a> TerminalRenderer<'a> {
    pub fn new(terminal: &'a TerminalState) -> Self {
        Self { terminal }
    }

    /// 将终端屏幕转换为 Ratatui 的 Line
    pub fn render_screen(&self) -> Vec<Line<'static>> {
        let screen = self.terminal.screen();
        let mut lines = Vec::new();
        let rows = self.terminal.get_size().rows as i64;
        // 遍历所有可见行
        for line in screen.lines_in_phys_range(screen.phys_range(&(0..rows))) {
            let mut spans = Vec::new();

            // 遍历该行中的所有 cell
            for cell in line.visible_cells() {
                let span = self.cell_to_span(&cell.as_cell());
                spans.push(span);
            }

            // 如果行为空，添加至少一个空 span
            if spans.is_empty() {
                spans.push(Span::raw(""));
            }

            lines.push(Line::from(spans));
        }

        lines
    }

    /// 将单个 cell 转换为 Ratatui 的 Span
    fn cell_to_span(&self, cell: &Cell) -> Span<'static> {
        let text = cell.str().to_string();
        let attrs = cell.attrs();

        // 解析前景色
        let fg = self.parse_color(attrs.foreground());

        // 解析背景色
        let bg = self.parse_color(attrs.background());

        // 构建修饰符
        let mut modifier = Modifier::empty();
        match attrs.intensity() {
            Intensity::Bold => modifier |= Modifier::BOLD,
            Intensity::Half => modifier |= Modifier::DIM,
            Intensity::Normal => (),
        }

        if attrs.italic() {
            modifier |= Modifier::ITALIC;
        }
        if attrs.underline() != wezterm_cell::Underline::None {
            modifier |= Modifier::UNDERLINED;
        }
        if attrs.reverse() {
            modifier |= Modifier::REVERSED;
        }
        if attrs.strikethrough() {
            modifier |= Modifier::CROSSED_OUT;
        }

        let style = Style::new().fg(fg).bg(bg).add_modifier(modifier);

        Span::styled(text, style)
    }

    /// 将 WezTerm 的颜色转换为 Ratatui 的颜色
    fn parse_color(&self, color_attr: ColorAttribute) -> Color {
        match color_attr {
            ColorAttribute::Default => Color::Reset,

            ColorAttribute::PaletteIndex(idx) => self.palette_index_to_color(idx),

            ColorAttribute::TrueColorWithPaletteFallback(rgb, _)
            | ColorAttribute::TrueColorWithDefaultFallback(rgb) => {
                let (r, g, b, _) = rgb.as_rgba_u8();
                Color::Rgb(r, g, b)
            }
        }
    }

    /// 将 ANSI 调色板索引转换为 Ratatui 颜色
    fn palette_index_to_color(&self, idx: u8) -> Color {
        match idx {
            // 标准 ANSI 16 色
            0 => Color::Black,
            1 => Color::Red,
            2 => Color::Green,
            3 => Color::Yellow,
            4 => Color::Blue,
            5 => Color::Magenta,
            6 => Color::Cyan,
            7 => Color::Gray,
            8 => Color::DarkGray,
            9 => Color::LightRed,
            10 => Color::LightGreen,
            11 => Color::LightYellow,
            12 => Color::LightBlue,
            13 => Color::LightMagenta,
            14 => Color::LightCyan,
            15 => Color::White,
            // 256 色调色板（索引 16-255）
            idx => Color::Indexed(idx),
        }
    }

    /// 获取光标位置和形状
    pub fn get_cursor_info(&self) -> CursorInfo {
        let state = &self.terminal;
        let cursor = state.cursor_pos();

        CursorInfo {
            x: cursor.x,
            y: cursor.y as usize,
            shape: cursor.shape,
            visibility: cursor.visibility,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct CursorInfo {
    pub x: usize,
    pub y: usize,
    pub shape: wezterm_surface::CursorShape,
    pub visibility: wezterm_surface::CursorVisibility,
}

pub struct TerminalWidget<'a> {
    renderer: TerminalRenderer<'a>,
}

impl<'a> TerminalWidget<'a> {
    pub fn new(terminal: &'a TerminalState) -> Self {
        Self {
            renderer: TerminalRenderer::new(terminal),
        }
    }
}

impl Widget for TerminalWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 {
            return;
        }

        let lines = self.renderer.render_screen();

        let paragraph = Paragraph::new(lines).scroll((0, 0)); // 可以根据需要调整滚动位置

        paragraph.render(area, buf);

        let cursor_info = self.renderer.get_cursor_info();
        if cursor_info.visibility == wezterm_surface::CursorVisibility::Visible {
            Self::draw_cursor(area, buf, cursor_info);
        }
    }
}

impl TerminalWidget<'_> {
    fn draw_cursor(area: Rect, buf: &mut Buffer, cursor: CursorInfo) {
        let cursor_x = area.left().saturating_add(cursor.x as u16);
        let cursor_y = area.top().saturating_add(cursor.y as u16);

        if cursor_x >= area.right() || cursor_y >= area.bottom() {
            return;
        }

        if let Some(cell) = &mut buf.cell_mut((cursor_x, cursor_y)) {
            match cursor.shape {
                wezterm_surface::CursorShape::Default
                | wezterm_surface::CursorShape::SteadyBlock
                | wezterm_surface::CursorShape::BlinkingBlock => {
                    cell.modifier ^= Modifier::REVERSED;
                }
                wezterm_surface::CursorShape::BlinkingBar
                | wezterm_surface::CursorShape::SteadyBar => {
                    cell.modifier |= Modifier::UNDERLINED;
                }
                wezterm_surface::CursorShape::BlinkingUnderline
                | wezterm_surface::CursorShape::SteadyUnderline => {
                    cell.modifier |= Modifier::UNDERLINED;
                }
            }
        }
    }
}

pub fn crossterm_to_wezterm(event: CrosstermKeyEvent) -> Option<(KeyCode, KeyModifiers)> {
    let key_code = match event.code {
        // 普通字符
        CrosstermKeyCode::Char(c) => KeyCode::Char(c),

        // 功能键
        CrosstermKeyCode::F(n) => KeyCode::Function(n),

        // 箭头键
        CrosstermKeyCode::Up => KeyCode::UpArrow,
        CrosstermKeyCode::Down => KeyCode::DownArrow,
        CrosstermKeyCode::Left => KeyCode::LeftArrow,
        CrosstermKeyCode::Right => KeyCode::RightArrow,

        // 编辑键
        CrosstermKeyCode::Home => KeyCode::Home,
        CrosstermKeyCode::End => KeyCode::End,
        CrosstermKeyCode::PageUp => KeyCode::PageUp,
        CrosstermKeyCode::PageDown => KeyCode::PageDown,
        CrosstermKeyCode::Tab => KeyCode::Tab,
        CrosstermKeyCode::BackTab => KeyCode::Tab, // BackTab 用 Tab + Shift
        CrosstermKeyCode::Backspace => KeyCode::Backspace,
        CrosstermKeyCode::Delete => KeyCode::Delete,
        CrosstermKeyCode::Insert => KeyCode::Insert,
        CrosstermKeyCode::Enter => KeyCode::Enter,
        CrosstermKeyCode::Esc => KeyCode::Escape,

        // 其他
        CrosstermKeyCode::CapsLock => KeyCode::CapsLock,
        CrosstermKeyCode::ScrollLock => KeyCode::ScrollLock,
        CrosstermKeyCode::NumLock => KeyCode::NumLock,
        CrosstermKeyCode::PrintScreen => KeyCode::PrintScreen,
        CrosstermKeyCode::Pause => KeyCode::Pause,

        // Media 键等 - 暂不支持
        _ => return None,
    };

    // 转换修饰符
    let modifiers = crossterm_modifiers_to_wezterm(event.modifiers);

    Some((key_code, modifiers))
}

/// 转换 Crossterm 的修饰符到 WezTerm
fn crossterm_modifiers_to_wezterm(mods: CrosstermModifiers) -> KeyModifiers {
    let mut result = KeyModifiers::NONE;

    if mods.contains(CrosstermModifiers::CONTROL) {
        result |= KeyModifiers::CTRL;
    }
    if mods.contains(CrosstermModifiers::ALT) {
        result |= KeyModifiers::ALT;
    }
    if mods.contains(CrosstermModifiers::SHIFT) {
        result |= KeyModifiers::SHIFT;
    }

    result
}

fn pane_block<'a>(title: impl Into<Line<'a>>, is_focused: bool, color: Color) -> Block<'a> {
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
#[allow(unused_variables)]
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
    let async_fd1 = AsyncFd::new(fd)?;
    Ok(())
}
