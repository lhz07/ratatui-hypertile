use crate::{HypertilePlugin, runtime::tokio_spawn};
use crossterm::event::{
    self, Event as CrosstermEvent, KeyCode as CrosstermKeyCode, KeyEvent as CrosstermKeyEvent,
    KeyEventKind, KeyModifiers as CrosstermModifiers,
};
use image::{DynamicImage, RgbaImage};
use portable_pty::{Child, CommandBuilder, NativePtySystem, PtyPair, PtySize, PtySystem};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    symbols::border,
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, StatefulWidget, Widget},
};
use ratatui_hypertile::{CellInfo, EventOutcome, HypertileEvent};
use ratatui_image::{
    StatefulImage,
    picker::{Picker, ProtocolType},
    protocol::StatefulProtocol,
};
use std::{
    borrow::Cow,
    collections::HashMap,
    env,
    io::{self, Cursor},
    mem,
    pin::Pin,
    sync::{Arc, LazyLock},
    task::{Poll, ready},
};
use termwiz::{color::ColorAttribute, image::ImageDataType};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, unix::AsyncFd},
    sync::{
        mpsc::{self, error::TrySendError},
        oneshot,
    },
};
use wezterm_cell::{Cell, Intensity};
use wezterm_term::{KeyCode, KeyModifiers, Terminal, TerminalConfiguration, TerminalSize};

#[derive(Default)]
pub struct PtyPlugin {
    mounted: Option<MountedPty>,
    is_closed: bool,
    program: String,
}

impl PtyPlugin {
    pub fn new(program: String) -> Self {
        Self {
            mounted: None,
            is_closed: false,
            program,
        }
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
        self.is_closed = true;
    }
}

pub struct MountedPty {
    root: PtyPair,
    child: Box<dyn Child + Send + Sync>,
    area: Rect,
    animation_active: bool,
    space_ani_active: bool,
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
            // log::trace!("wait for writing...");
            let mut guard = ready!(self.0.poll_write_ready(cx))?;
            // log::trace!("write once");
            let n = unsafe { libc::write(*guard.get_inner(), buf.as_ptr() as _, buf.len()) };
            // log::trace!("write call finished");
            if n == -1 {
                let err = std::io::Error::last_os_error();
                if err.kind() == io::ErrorKind::WouldBlock {
                    // log::trace!("would block");
                    guard.clear_ready();
                } else {
                    return Poll::Ready(Err(err));
                }
            } else {
                // log::trace!("write finished");
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
    SetDirty,
    AniStart,
    AniStop,
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

    pub fn create(area: Rect, program: &str) -> anyhow::Result<Self> {
        let rows = area.height.max(MIN_ROW);
        let cols = area.width.max(MIN_COL);
        let pty = NativePtySystem::default();
        let root = pty.openpty(PtySize {
            rows,
            cols,
            ..Default::default()
        })?;
        let child = root.slave.spawn_command(CommandBuilder::new(program))?;
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
        let size = TerminalSize::new(cols, rows);
        let writer = AsyncWriteAdapter::new(input_tx);
        let program = program.to_string();
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
                let terminal = Terminal::new(size, config, "", "", Box::new(writer));
                let mut state = TerminalState::new(terminal, program, size);
                let mut buf = [0; 8192];
                loop {
                    tokio::select! {
                        biased;
                        // read all first
                        res = pty_fd.read(&mut buf) => {
                            // log::info!("read");
                            let n = res?;
                            if n != 0{
                                // update
                                state.terminal.advance_bytes(&buf[..n]);
                            } else {
                                // exit now!
                                break;
                            }
                        }
                        // no more to read, render it
                        Some(msg) = render_rx.recv() => {
                            handle_msg(msg, &mut state);
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
            } else {
                log::debug!("pty output finished");
            }
        });

        Ok(Self {
            root,
            child,
            area,
            render_tx,
            animation_active: false,
            space_ani_active: false,
        })
    }
}

fn handle_msg(msg: RenderMsg, state: &mut TerminalState) {
    match msg {
        RenderMsg::RenderScreen(area, mut buf, tx, is_focused) => {
            let title = state.terminal.get_title();
            let title = if !title.is_empty() {
                format!(" {} {title} ", state.program)
            } else {
                format!(" {} ", state.program)
            };
            let block = pane_block(title, is_focused, Color::Blue);
            let term = TerminalWidget::new(state);
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
            state.resize(rect.width, rect.height);
        }
        RenderMsg::SetDirty => state.dirty = true,
        RenderMsg::AniStart => state.ani_active += 1,
        RenderMsg::AniStop => {
            state.ani_active = state.ani_active.saturating_sub(1);
            if state.ani_active == 0 {
                state.dirty = true;
            }
        }
        RenderMsg::Event(event) => match event {
            HypertileEvent::Term(term_event) => match term_event {
                CrosstermEvent::Key(key_event)
                    if let Some((key, mods)) = crossterm_to_wezterm(key_event) =>
                {
                    match key_event.kind {
                        KeyEventKind::Press => {
                            let _ = state.terminal.key_down(key, mods);
                        }

                        KeyEventKind::Release => {
                            let _ = state.terminal.key_up(key, mods);
                        }
                        KeyEventKind::Repeat => {
                            let _ = state.terminal.key_down(key, mods);
                            let _ = state.terminal.key_up(key, mods);
                        }
                    }
                }
                CrosstermEvent::Paste(s) => {
                    if let Err(e) = state.terminal.send_paste(&s) {
                        log::error!("paste: {e}");
                    }
                }
                CrosstermEvent::Mouse(mouse) => {
                    let offset = match mouse.kind {
                        event::MouseEventKind::ScrollDown => 1,
                        event::MouseEventKind::ScrollUp => -1,
                        _ => return,
                    };
                    let new = state.view_row + offset;
                    let invisible = state.terminal.screen().phys_row(0);
                    if new <= 0 && new >= -(invisible as i64) {
                        state.view_row = new;
                    }
                }
                _ => (),
            },
            _ => (),
        },
    }
}

trait ResizeTrait {
    fn new(cols: u16, rows: u16) -> Self;
    fn resize(&mut self, cols: u16, rows: u16);
}

impl ResizeTrait for TerminalSize {
    fn new(cols: u16, rows: u16) -> Self {
        Self {
            rows: rows as usize,
            cols: cols as usize,
            pixel_width: CellInfo::pixel_width(cols) as usize,
            pixel_height: CellInfo::pixel_height(rows) as usize,
            dpi: 96,
        }
    }
    fn resize(&mut self, cols: u16, rows: u16) {
        self.cols = cols as usize;
        self.rows = rows as usize;
        self.pixel_width = CellInfo::pixel_width(cols) as usize;
        self.pixel_height = CellInfo::pixel_height(rows) as usize;
    }
}

#[derive(Debug)]
struct SimpleTermConfig;

impl TerminalConfiguration for SimpleTermConfig {
    fn scrollback_size(&self) -> usize {
        1000
    }

    fn enable_kitty_graphics(&self) -> bool {
        true
    }

    fn color_palette(&self) -> wezterm_term::color::ColorPalette {
        wezterm_term::color::ColorPalette::default()
    }
}

impl HypertilePlugin for PtyPlugin {
    fn is_closed(&mut self) -> bool {
        self.is_closed
    }
    fn on_event(
        &mut self,
        event: &mut ratatui_hypertile::HypertileEvent,
    ) -> ratatui_hypertile::EventOutcome {
        let pty = if let Some(pty) = &mut self.mounted
            && !self.is_closed
        {
            pty
        } else {
            return ratatui_hypertile::EventOutcome::Ignored;
        };
        match event {
            HypertileEvent::AniStart if !pty.space_ani_active => {
                pty.space_ani_active = true;
                log::info!("space animation: true");

                if pty.render_tx.try_send(RenderMsg::AniStart).is_err() {
                    log::error!("can not set animation start");
                }
                EventOutcome::Consumed
            }
            HypertileEvent::AniStop if pty.space_ani_active => {
                pty.space_ani_active = false;
                log::info!("space animation: false");

                if pty.render_tx.try_send(RenderMsg::AniStop).is_err() {
                    log::error!("can not set animation stop");
                }
                EventOutcome::Consumed
            }
            _ => {
                let mut send_event = HypertileEvent::Empty;
                mem::swap(&mut send_event, event);
                // send event
                let _res = pty.render_tx.try_send(RenderMsg::Event(send_event));
                // log::info!("send key: {:?}, result: {:?}", key, res);
                ratatui_hypertile::EventOutcome::Consumed
            }
        }
    }
    fn render(
        &mut self,
        area: Rect,
        buf: &mut ratatui::prelude::Buffer,
        is_focused: bool,
        target_rect: Option<Rect>,
    ) {
        if self.is_closed {
            return;
        }
        let block = pane_block("Terminal", is_focused, Color::Blue);
        let term_area = block.inner(area);
        let pty = match &mut self.mounted {
            Some(pty) => {
                if let Some(target) = target_rect {
                    if !pty.animation_active {
                        pty.animation_active = true;
                        log::info!("animation: true");
                        if pty.render_tx.try_send(RenderMsg::AniStart).is_err() {
                            log::error!("can not set animation start");
                        }
                    }
                    pty.resize(block.inner(target));
                } else {
                    pty.resize(term_area);
                    if pty.animation_active {
                        pty.animation_active = false;
                        log::info!("animation false");
                        if pty.render_tx.try_send(RenderMsg::AniStop).is_err() {
                            log::error!("can not set animation stop");
                        }
                    }
                }
                pty
            }
            None => match MountedPty::create(term_area, &self.program) {
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

#[derive(Debug, Clone, Copy)]
pub struct CursorInfo {
    pub x: usize,
    pub y: usize,
    pub shape: wezterm_surface::CursorShape,
    pub visibility: wezterm_surface::CursorVisibility,
}

pub struct TerminalWidget<'a> {
    state: &'a mut TerminalState,
}

impl<'a> TerminalWidget<'a> {
    fn new(state: &'a mut TerminalState) -> Self {
        Self { state }
    }
}

struct Image {
    row: u16,
    col: u16,
    width: u32,
    height: u32,
    img: DynamicImage,
    area: Vec<(u16, u16)>,
}

impl Image {
    // fn relative(&self, reference: Rect) -> Rect {
    //     let mut rect = self.area;
    //     rect.x += reference.x;
    //     rect.y += reference.y;
    //     rect
    // }
}

struct TerminalState {
    view_row: i64,
    size: TerminalSize,
    terminal: Terminal,
    program: String,
    dirty: bool,
    ani_active: u8,
    images: HashMap<u64, Image>,
}

impl TerminalState {
    pub fn new(terminal: Terminal, program: String, size: TerminalSize) -> Self {
        Self {
            view_row: 0,
            terminal,
            program,
            dirty: true,
            ani_active: 0,
            size,
            images: HashMap::new(),
        }
    }
    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.dirty = true;
        self.size.resize(cols, rows);
        self.terminal.resize(self.size);
    }
    /// 将终端屏幕转换为 Ratatui 的 Line
    pub fn render_screen(&mut self) -> (Vec<Line<'static>>, HashMap<u64, Vec<(u16, u16)>>) {
        let screen = self.terminal.screen();
        let mut lines = Vec::new();
        let phys_row = screen.phys_row(0);
        let start = phys_row.saturating_sub(self.view_row.abs() as usize);
        let end = start + self.terminal.get_size().rows;
        let mut pos_map: HashMap<u64, Vec<(u16, u16)>> = HashMap::new();
        // 遍历所有可见行
        for (rows, line) in screen.lines_in_phys_range(start..end).iter().enumerate() {
            let mut spans = Vec::new();

            let mut logical_col = 0usize;
            // 遍历该行中的所有 cell
            for cell in line.visible_cells() {
                if let Some(images) = cell.attrs().images() {
                    for img_cell in images {
                        let data = &*img_cell.image_data().data();
                        let hash = img_cell.unique_hash();
                        match self.images.get_mut(&hash) {
                            Some(_) => {
                                // TODO: implement crop logic
                                match pos_map.get_mut(&hash) {
                                    Some(positions) => {
                                        positions.push((logical_col as u16, rows as u16));
                                    }
                                    None => {
                                        pos_map
                                            .insert(hash, vec![(logical_col as u16, rows as u16)]);
                                    }
                                }
                            }
                            None => match to_dynamic_image(data) {
                                Ok(img) => {
                                    let width = img.width();
                                    let col = width as f64 / CellInfo::width();
                                    let height = img.height();
                                    let row = height as f64 / CellInfo::height();
                                    let img = Image {
                                        row: row.round() as u16,
                                        col: col.round() as u16,
                                        height,
                                        width,
                                        img,
                                        area: Vec::new(),
                                    };
                                    self.images.insert(hash, img);
                                    pos_map.insert(hash, vec![(logical_col as u16, rows as u16)]);
                                }
                                Err(e) => {
                                    log::error!("{e}");
                                }
                            },
                        }
                    }
                } else {
                    let span = self.cell_to_span(&cell.as_cell());
                    spans.push(span);
                }
                logical_col += cell.width();
            }

            // 如果行为空，添加至少一个空 span
            if spans.is_empty() {
                spans.push(Span::raw(""));
            }

            lines.push(Line::from(spans));
        }
        (lines, pos_map)
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

pub fn to_dynamic_image(data: &ImageDataType) -> Result<DynamicImage, Cow<'static, str>> {
    match data {
        ImageDataType::Rgba8 {
            data,
            width,
            height,
            hash: _,
        } => Ok(DynamicImage::ImageRgba8(
            RgbaImage::from_raw(*width, *height, data.clone())
                .ok_or("error loading img from raw")?,
        )),
        ImageDataType::EncodedFile(data) => match image::load_from_memory(&data) {
            Ok(img) => Ok(img),
            Err(e) => Err(format!("can not load img from memory: {e}").into()),
        },
        _ => Err("unsupported format".into()),
    }
}

impl Widget for TerminalWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 {
            return;
        }

        let (lines, pos_map) = self.state.render_screen();
        let paragraph = Paragraph::new(lines).scroll((0, 0));

        paragraph.render(area, buf);

        let cursor_info = self.state.get_cursor_info();
        if cursor_info.visibility == wezterm_surface::CursorVisibility::Visible {
            Self::draw_cursor(area, buf, cursor_info);
        }
        if self.state.ani_active > 0 {
            return;
        }
        for (id, image) in &mut self.state.images {
            // if the image is not shown on screen, clear its last area
            if !pos_map.contains_key(id) {
                image.area.clear();
            }
        }
        for (id, positions) in pos_map {
            if let Some(image) = self.state.images.get_mut(&id) {
                // area change, redraw
                let mut pos = positions
                    .iter()
                    .copied()
                    .map(|(x, y)| (x + area.x, y + area.y));
                let first = pos.next().expect("positions is never empty");
                // skip other cells
                for (x, y) in pos {
                    if let Some(cell) = buf.cell_mut((x, y)) {
                        cell.set_skip(true);
                    } else {
                        log::error!("image cell position is wrong!");
                    }
                }
                if positions != image.area || self.state.dirty {
                    log::info!("{:?}", positions);
                    if let Some(cell) = buf.cell_mut(first) {
                        let symbol = encode(&image.img, image.col, image.row);
                        log::info!("set {:?} symbol", first);
                        cell.set_symbol(&symbol);
                    } else {
                        log::error!("image cell position is wrong!");
                    }
                    image.area = positions;
                } else {
                    if let Some(cell) = buf.cell_mut(first) {
                        cell.set_skip(true);
                    } else {
                        log::error!("image cell position is wrong!");
                    }
                }
            }
        }

        self.state.dirty = false;
    }
}

fn encode(img: &DynamicImage, width: u16, height: u16) -> String {
    use std::fmt::Write;
    let mut png: Vec<u8> = vec![];
    if let Err(e) = img.write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png) {
        log::error!("image write error: {e}");
    }

    let escape = "\x1b";

    let mut seq = String::new();
    for _ in 0..height {
        write!(seq, "{escape}[{width}X{escape}[1B").unwrap();
    }
    write!(seq, "{escape}[{height}A").unwrap();

    write!(
        seq,
        "{escape}]1337;File=inline=1;size={};width={}px;height={}px;doNotMoveCursor=1:",
        png.len(),
        img.width(),
        img.height(),
    )
    .unwrap();

    base64_simd::STANDARD.encode_append(&png, &mut seq);

    write!(seq, "\x07").unwrap();
    seq
}

pub static PICKER: LazyLock<Picker> = LazyLock::new(|| {
    let mut picker = Picker::from_query_stdio().expect("can not detect img protocol");
    // it recognizes iTerm as kitty...
    log::info!("type: {:?}", picker.protocol_type());
    if picker.protocol_type() == ProtocolType::Kitty
        && let Some(proto) = iterm2_from_env()
    {
        picker.set_protocol_type(proto);
        log::info!("force set type: {:?}", picker.protocol_type());
    }
    picker
});

fn iterm2_from_env() -> Option<ProtocolType> {
    if env::var("TERM_PROGRAM").is_ok_and(|term_program| term_program.contains("iTerm")) {
        return Some(ProtocolType::Iterm2);
    }
    if env::var("LC_TERMINAL").is_ok_and(|lc_term| lc_term.contains("iTerm")) {
        return Some(ProtocolType::Iterm2);
    }
    None
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
