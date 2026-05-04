use crate::{
    HypertilePlugin,
    runtime::{termwiz::IntoRatatui, tokio_spawn},
};
use image::{DynamicImage, EncodableLayout, RgbaImage};
use portable_pty::{
    CommandBuilder, MasterPty, NativePtySystem, PtySize, PtySystem, SlavePty, unix::UnixMasterPty,
};
use ratatui::{
    buffer::Buffer,
    crossterm::cursor::SetCursorStyle,
    layout::Rect,
    style::{Color, Modifier, Style},
    symbols::border,
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Widget},
};
use ratatui::{
    crossterm::event::{
        self, Event as CrosstermEvent, KeyCode as CrosstermKeyCode, KeyEvent as CrosstermKeyEvent,
        KeyEventKind, KeyModifiers as CrosstermModifiers, MouseEvent as CrosstermMouseEvent,
        MouseEventKind as CrosstermMouseEventKind,
    },
    layout::Position,
};
use ratatui_hypertile::{CellInfo, EventOutcome, HypertileEvent};
use ratatui_image::picker::{Picker, ProtocolType};
use std::{
    borrow::Cow,
    cell::Cell as StdCell,
    collections::HashMap,
    env,
    io::{self, Cursor},
    mem,
    pin::Pin,
    sync::{Arc, LazyLock},
    task::{Poll, ready},
};
use std::{fmt::Write, process::Child};
use termwiz::image::ImageDataType;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, unix::AsyncFd},
    sync::{
        mpsc::{self, Sender, error::TrySendError},
        oneshot,
    },
};
use wezterm_surface::CursorShape;
use wezterm_term::{
    CellRef, KeyCode, KeyModifiers, MouseEventKind, Terminal, TerminalConfiguration, TerminalSize,
};
use wezterm_term::{MouseButton, input::MouseEvent};

pub struct PtyPlugin {
    mounted: Option<MountedPty>,
    render: Sender<()>,
    is_closed: bool,
    program: String,
}

impl PtyPlugin {
    pub fn new(program: String, render_req_tx: Sender<()>) -> Self {
        Self {
            mounted: None,
            render: render_req_tx,
            is_closed: false,
            program,
        }
    }
    pub fn close(&mut self) {
        if let Some(mut pty) = self.mounted.take() {
            if let Err(e) = pty.child.kill() {
                log::error!("kill child: {e}")
            }
        }
        self.is_closed = true;
    }
}

pub struct MountedPty {
    master: UnixMasterPty,
    child: Child,
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
    RenderScreen(
        Rect,
        Buffer,
        oneshot::Sender<(Buffer, Option<Position>)>,
        bool,
    ),
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
    ) -> (Self, oneshot::Receiver<(Buffer, Option<Position>)>) {
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

    pub fn create(area: Rect, program: &str, render_req_tx: Sender<()>) -> anyhow::Result<Self> {
        let rows = area.height.max(MIN_ROW);
        let cols = area.width.max(MIN_COL);
        let pty = NativePtySystem::default();
        let (master, slave) = pty.openpty(PtySize {
            rows,
            cols,
            ..Default::default()
        })?;
        let child = slave.spawn_command(CommandBuilder::new(program))?;
        let fd = master.as_raw_fd().expect("valid on macOS");
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
                        while let Some(msg) = input_rx.recv().await {
                            pty_fd_write.write_all(&msg.event).await?;
                        }
                        Ok(())
                    }
                    .await;
                    log::debug!("pty input finished: {:?}", res);
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
                                render_req_tx.send(()).await?;
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
            log::debug!("pty output finished: {:?}", res);
        });

        Ok(Self {
            master,
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
            let mut cursor_pos = None;
            if is_focused {
                let cursor_info = state.get_cursor_info();
                if cursor_info.visibility == wezterm_surface::CursorVisibility::Visible {
                    cursor_pos = draw_cursor(area, cursor_info, &mut buf);
                }
            }
            // about 50 - 500 micro seconds
            // log::info!("render terminal cost: {:?}", ins.elapsed());
            block.render(area, &mut buf);
            if tx.send((buf, cursor_pos)).is_err() {
                log::error!("can not send pty buffer back")
            }
        }
        RenderMsg::SetSize(rect) => {
            state.resize(rect);
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
                    if state.terminal.is_alt_screen_active() {
                        // pass mouse event to the terminal
                        let event = mouse_convert(mouse, state);
                        if let Err(e) = state.terminal.mouse_event(event) {
                            log::error!("mouse event: {e}");
                        }
                    }
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
            None => match MountedPty::create(term_area, &self.program, self.render.clone()) {
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
        let Ok((mut buffer, cursor_pos)) = rx.blocking_recv() else {
            return;
        };
        // log::info!("recv msg cost: {:?}", ins.elapsed());
        mem::swap(&mut buffer, buf);
        // log::info!("render term");
        if cursor_pos.is_some() {
            CURSOR_POS.set(cursor_pos);
        }
    }
    fn on_unmount(&mut self, _ctx: crate::PluginContext) {
        self.close();
    }
}

#[derive(Debug, Clone, Copy)]
pub struct CursorInfo {
    pub x: usize,
    pub y: i64,
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

struct TerminalState {
    view_row: i64,
    size: TerminalSize,
    area: Rect,
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
            area: Rect::default(),
            images: HashMap::new(),
        }
    }
    pub fn resize(&mut self, area: Rect) {
        self.dirty = true;
        self.area = area;
        self.size.resize(area.width, area.height);
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
                    let span = self.cell_to_span(cell);
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
    fn cell_to_span(&self, cell: CellRef<'_>) -> Span<'static> {
        let text = cell.str().to_string();
        let style: Style = cell.attrs().into_ratatui();

        Span::styled(text, style)
    }

    /// 获取光标位置和形状
    pub fn get_cursor_info(&self) -> CursorInfo {
        let state = &self.terminal;
        let cursor = state.cursor_pos();

        CursorInfo {
            x: cursor.x + 1,
            y: cursor.y - self.view_row + 1,
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
        if self.state.ani_active > 0 || matches!(PICKER.protocol_type(), ProtocolType::Halfblocks) {
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
                        match PICKER.protocol_type() {
                            ProtocolType::Iterm2 => {
                                let symbol = encode_iterm2(&image.img, image.col, image.row);
                                cell.set_symbol(&symbol);
                            }
                            ProtocolType::Sixel => {
                                let symbol = encode_sixel(&image.img, image.col, image.row);
                                cell.set_symbol(&symbol);
                            }
                            _ => (),
                        }
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

fn encode_iterm2(img: &DynamicImage, width: u16, height: u16) -> String {
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

fn encode_sixel(img: &DynamicImage, width: u16, height: u16) -> String {
    use icy_sixel::{EncodeOptions, sixel_encode};

    let (w, h) = (img.width(), img.height());
    let bytes = match img {
        DynamicImage::ImageRgba8(img) => Cow::Borrowed(img.as_bytes()),
        _ => img.to_rgb8().into_vec().into(),
    };
    let escape = "\x1b";

    let sixel_data = match sixel_encode(&bytes, w as usize, h as usize, &EncodeOptions::default()) {
        Ok(s) => s,
        Err(e) => {
            log::error!("sixel img encode error: {e}");
            return String::new();
        }
    };

    let mut data = String::new();

    for _ in 0..height {
        write!(data, "{escape}[{width}X{escape}[1B").unwrap();
    }
    write!(data, "{escape}[{height}A").unwrap();
    data.push_str(&sixel_data);

    data
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

thread_local! {
    pub static CURSOR_POS: StdCell<Option<Position>> = StdCell::new(None);
}

fn draw_cursor(area: Rect, cursor: CursorInfo, buf: &mut Buffer) -> Option<Position> {
    let cursor_x = area.left().saturating_add(cursor.x as u16);
    let cursor_y = area.top().saturating_add(cursor.y as u16);

    if cursor_x >= area.right() || cursor_y + 1 >= area.bottom() {
        return None;
    }
    if let ProtocolType::Iterm2 = PICKER.protocol_type() {
        // iTerm2's cursor support is poor, so we draw it.
        if let Some(cell) = &mut buf.cell_mut((cursor_x, cursor_y)) {
            match cursor.shape {
                wezterm_surface::CursorShape::Default
                | wezterm_surface::CursorShape::SteadyBlock => {
                    cell.bg = Color::Reset;
                    cell.fg = Color::Reset;
                    cell.modifier |= Modifier::REVERSED;
                }
                wezterm_surface::CursorShape::BlinkingBlock => {
                    cell.bg = Color::Reset;
                    cell.fg = Color::Reset;
                    cell.modifier |= Modifier::REVERSED | Modifier::SLOW_BLINK;
                }
                wezterm_surface::CursorShape::BlinkingBar => {
                    cell.modifier |= Modifier::UNDERLINED | Modifier::SLOW_BLINK;
                }
                wezterm_surface::CursorShape::SteadyBar => {
                    cell.modifier |= Modifier::UNDERLINED;
                }
                wezterm_surface::CursorShape::BlinkingUnderline => {
                    cell.modifier |= Modifier::UNDERLINED | Modifier::SLOW_BLINK;
                }
                wezterm_surface::CursorShape::SteadyUnderline => {
                    cell.modifier |= Modifier::UNDERLINED;
                }
            }
        }
        None
    } else {
        let _ = ratatui::crossterm::execute!(std::io::stdout(), wez_to_cross(cursor.shape));
        Some((cursor_x, cursor_y).into())
    }
}

fn wez_to_cross(shape: CursorShape) -> SetCursorStyle {
    match shape {
        CursorShape::BlinkingBar => SetCursorStyle::BlinkingBar,
        CursorShape::BlinkingBlock => SetCursorStyle::BlinkingBlock,
        CursorShape::BlinkingUnderline => SetCursorStyle::BlinkingUnderScore,
        CursorShape::Default => SetCursorStyle::DefaultUserShape,
        CursorShape::SteadyBar => SetCursorStyle::SteadyBar,
        CursorShape::SteadyBlock => SetCursorStyle::SteadyBlock,
        CursorShape::SteadyUnderline => SetCursorStyle::SteadyUnderScore,
    }
}

fn convert_mouse_buttons(button: event::MouseButton) -> MouseButton {
    match button {
        event::MouseButton::Left => MouseButton::Left,
        event::MouseButton::Right => MouseButton::Right,
        event::MouseButton::Middle => MouseButton::Middle,
    }
}

fn mouse_convert(event: CrosstermMouseEvent, state: &mut TerminalState) -> MouseEvent {
    let mut button = MouseButton::None;
    let kind = match event.kind {
        CrosstermMouseEventKind::Down(btn) => {
            button = convert_mouse_buttons(btn);
            MouseEventKind::Press
        }
        CrosstermMouseEventKind::Up(btn) => {
            button = convert_mouse_buttons(btn);
            MouseEventKind::Release
        }
        CrosstermMouseEventKind::Drag(btn) => {
            button = convert_mouse_buttons(btn);
            MouseEventKind::Move
        }
        CrosstermMouseEventKind::Moved => MouseEventKind::Move,
        CrosstermMouseEventKind::ScrollDown => {
            button = MouseButton::WheelDown(1);
            MouseEventKind::Press
        }
        CrosstermMouseEventKind::ScrollUp => {
            button = MouseButton::WheelUp(1);
            MouseEventKind::Press
        }
        CrosstermMouseEventKind::ScrollLeft => {
            button = MouseButton::WheelLeft(1);
            MouseEventKind::Press
        }
        CrosstermMouseEventKind::ScrollRight => {
            button = MouseButton::WheelRight(1);
            MouseEventKind::Press
        }
    };

    let modifiers = crossterm_modifiers_to_wezterm(event.modifiers);
    let x = event.column.saturating_sub(state.area.x);
    let y = event.row.saturating_sub(state.area.y);
    MouseEvent {
        kind,
        x: x as usize,
        y: y as i64,
        x_pixel_offset: CellInfo::pixel_width(x) as isize,
        y_pixel_offset: CellInfo::pixel_height(y) as isize,
        button,
        modifiers,
    }
}

fn crossterm_to_wezterm(event: CrosstermKeyEvent) -> Option<(KeyCode, KeyModifiers)> {
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
    let (master, slave) = pty.openpty(PtySize::default())?;
    let child = slave.spawn_command(CommandBuilder::new("fish"))?;
    let fd = master.as_raw_fd().expect("valid on macOS");
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
