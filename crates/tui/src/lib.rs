//! Terminal user interface for Termbridge (ratatui + crossterm).
//!
//! Runs everywhere a terminal does, including SSH sessions and headless ARM
//! boards. Shares the session engine, configuration, macros and recent
//! connections with the desktop GUI, and can expose its sessions over MCP.

mod form;
mod theme;

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Alignment, Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols;
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Clear, List, ListItem, ListState, Padding, Paragraph, Sparkline, Wrap,
};
use ratatui::{DefaultTerminal, Frame};

use termbridge_core::checksum::{self, ChecksumKind};
use termbridge_core::codec::{self, ViewFormat};
use termbridge_core::config::ConfigStore;
use termbridge_core::logging::LogFormat;
use termbridge_core::net::{self, PortInfo};
use termbridge_core::session::SessionConfig;
use termbridge_core::types::{Direction, SessionKind};
use termbridge_core::util::human_bytes;
use termbridge_core::validate;
use termbridge_core::view::{dir_tag, TerminalBuffer};
use termbridge_core::{ControlOp, SessionInfo, SessionManager};
use termbridge_mcp::McpServer;

use form::{ConnectForm, Value};
use theme::{glyphs, th};

/// Largest file accepted by "Send file", matching the GUI and MCP.
const MAX_SEND_FILE: u64 = 16 * 1024 * 1024;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Eol {
    None,
    Cr,
    Lf,
    CrLf,
}

impl Eol {
    fn next(self) -> Self {
        match self {
            Eol::None => Eol::Cr,
            Eol::Cr => Eol::Lf,
            Eol::Lf => Eol::CrLf,
            Eol::CrLf => Eol::None,
        }
    }
    fn label(self) -> &'static str {
        match self {
            Eol::None => "none",
            Eol::Cr => "CR",
            Eol::Lf => "LF",
            Eol::CrLf => "CRLF",
        }
    }
    fn bytes(self) -> &'static [u8] {
        match self {
            Eol::None => b"",
            Eol::Cr => b"\r",
            Eol::Lf => b"\n",
            Eol::CrLf => b"\r\n",
        }
    }
}

struct View {
    id: String,
    label: String,
    kind: SessionKind,
    term: TerminalBuffer,
    info: Option<SessionInfo>,
    /// Lines scrolled up from the bottom; 0 follows new data.
    scroll: usize,
    input: String,
    cursor: usize,
    hex: bool,
    eol: Eol,
    checksum: ChecksumKind,
    history: Vec<String>,
    history_pos: Option<usize>,
    /// Bytes per sample (RX + TX), newest last, for the activity sparkline.
    activity: VecDeque<u64>,
    sample: Option<(Instant, u64, u64)>,
    rx_rate: f64,
    tx_rate: f64,
    rx_flash: Option<Instant>,
    tx_flash: Option<Instant>,
}

impl View {
    fn new(info: SessionInfo, show_ts: bool) -> Self {
        Self {
            id: info.id.clone(),
            label: info.label.clone(),
            kind: info.kind,
            term: TerminalBuffer::new(ViewFormat::Ascii, show_ts),
            info: Some(info),
            scroll: 0,
            input: String::new(),
            cursor: 0,
            hex: false,
            eol: Eol::None,
            checksum: ChecksumKind::None,
            history: Vec::new(),
            history_pos: None,
            activity: VecDeque::from(vec![0; 120]),
            sample: None,
            rx_rate: 0.0,
            tx_rate: 0.0,
            rx_flash: None,
            tx_flash: None,
        }
    }

    fn byte_index(&self) -> usize {
        self.input
            .char_indices()
            .nth(self.cursor)
            .map(|(i, _)| i)
            .unwrap_or(self.input.len())
    }

    fn encoded(&self) -> Result<Vec<u8>, String> {
        let data = validate::payload(&self.input, self.hex)?;
        let mut bytes = checksum::append(self.checksum, &data);
        bytes.extend_from_slice(self.eol.bytes());
        Ok(bytes)
    }
}

/// Something the user can click or tap. Rebuilt on every frame.
#[derive(Clone)]
enum Hit {
    Session(usize),
    Recent(usize),
    Key(KeyEvent),
    FormKind(usize),
    FormField(usize),
    RecordFormat(LogFormat),
}

enum Mode {
    Normal,
    Help,
    Connect(Box<ConnectForm>, Vec<(&'static str, String)>),
    Confirm(String),
    Record {
        format: LogFormat,
        path: String,
        error: Option<String>,
    },
    SendFile {
        path: String,
        error: Option<String>,
    },
}

struct App {
    rt: tokio::runtime::Runtime,
    manager: SessionManager,
    config: Arc<ConfigStore>,
    views: Vec<View>,
    selected: usize,
    mode: Mode,
    toast: Option<(String, bool, Instant)>,
    ports: Vec<PortInfo>,
    mcp_url: Option<String>,
    mcp_shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    quit: bool,
    hits: Vec<(Rect, Hit)>,
    term_rect: Rect,
    list_rect: Rect,
    /// Frame counter driving animations (spinner, blinking REC).
    tick: u64,
}

/// Display options for the TUI. Unset values are detected from the terminal.
#[derive(Default, Clone)]
pub struct Options {
    /// Serve MCP over HTTP on this address.
    pub mcp: Option<String>,
    /// Theme name: Midnight, Arctic, Ember, Light or "High contrast".
    pub theme: Option<String>,
    /// Colour depth: truecolor, 256, 16 or none.
    pub colors: Option<String>,
    /// Force plain ASCII glyphs and borders.
    pub ascii: bool,
}

/// Run the TUI until the user quits. `mcp_bind` optionally starts the MCP
/// HTTP endpoint so agents can use the same sessions.
pub fn run(config: Arc<ConfigStore>, mcp_bind: Option<String>) -> anyhow::Result<()> {
    run_with(
        config,
        Options {
            mcp: mcp_bind,
            ..Options::default()
        },
    )
}

/// Run the TUI with explicit display options.
pub fn run_with(config: Arc<ConfigStore>, options: Options) -> anyhow::Result<()> {
    let depth = match options.colors.as_deref() {
        Some(value) => theme::Depth::parse(value).ok_or_else(|| {
            anyhow::anyhow!("unknown colour depth '{value}' (use truecolor, 256, 16 or none)")
        })?,
        None => theme::detect_depth(),
    };
    let glyph_level = if options.ascii {
        theme::GlyphLevel::Ascii
    } else {
        theme::detect_glyphs()
    };
    let theme_name = options
        .theme
        .clone()
        .unwrap_or_else(|| config.get().tui_theme);
    if let Some(name) = &options.theme {
        if !theme::THEMES
            .iter()
            .any(|t| t.name.eq_ignore_ascii_case(name.trim()))
        {
            let names: Vec<&str> = theme::THEMES.iter().map(|t| t.name).collect();
            anyhow::bail!("unknown theme '{name}' (available: {})", names.join(", "));
        }
    }
    theme::set(theme::theme_index(&theme_name), depth, glyph_level);
    let mcp_bind = options.mcp;
    let mut app = App::new(config)?;
    if let Some(bind) = mcp_bind {
        app.start_mcp(&bind)?;
    }
    let mut terminal = ratatui::try_init()?;
    // Mouse reporting makes clicks, taps (touch terminals translate them to
    // clicks) and wheel/swipe scrolling work, like htop or Midnight Commander.
    let _ = ratatui::crossterm::execute!(std::io::stdout(), EnableMouseCapture);
    let previous = std::panic::take_hook();
    let main_thread = std::thread::current().id();
    std::panic::set_hook(Box::new(move |info| {
        // Only the UI thread owns the terminal. A panic in a worker thread
        // must not tear down the screen under a running app.
        if std::thread::current().id() == main_thread {
            let _ = ratatui::crossterm::execute!(std::io::stdout(), DisableMouseCapture);
            ratatui::restore();
        }
        previous(info);
    }));
    let result = app.event_loop(&mut terminal);
    let _ = ratatui::crossterm::execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();
    app.manager.close_all();
    if let Some(tx) = app.mcp_shutdown.take() {
        let _ = tx.send(());
    }
    result
}

impl App {
    fn new(config: Arc<ConfigStore>) -> anyhow::Result<Self> {
        let cfg = config.get();
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()?;
        let manager = SessionManager::with_buffer(cfg.buffer_bytes);
        Ok(App {
            rt,
            manager,
            config,
            views: Vec::new(),
            selected: 0,
            mode: Mode::Normal,
            toast: None,
            ports: net::list_ports().unwrap_or_default(),
            mcp_url: None,
            mcp_shutdown: None,
            quit: false,
            hits: Vec::new(),
            term_rect: Rect::default(),
            list_rect: Rect::default(),
            tick: 0,
        })
    }

    fn start_mcp(&mut self, bind: &str) -> anyhow::Result<()> {
        let addr = validate::socket_addr(bind).map_err(anyhow::Error::msg)?;
        let listener = self.rt.block_on(tokio::net::TcpListener::bind(addr))?;
        let server = Arc::new(McpServer::new(self.manager.clone(), self.config.clone()));
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.rt.spawn(async move {
            let shutdown = async {
                let _ = rx.await;
            };
            if let Err(err) = termbridge_mcp::serve_http_listener(server, listener, shutdown).await
            {
                tracing::error!("MCP HTTP server stopped: {err}");
            }
        });
        self.mcp_shutdown = Some(tx);
        self.mcp_url = Some(format!("http://{addr}/mcp"));
        Ok(())
    }

    fn toast(&mut self, message: impl Into<String>, error: bool) {
        self.toast = Some((message.into(), error, Instant::now()));
    }

    fn event_loop(&mut self, terminal: &mut DefaultTerminal) -> anyhow::Result<()> {
        while !self.quit {
            self.tick = self.tick.wrapping_add(1);
            self.update();
            terminal.draw(|frame| self.draw(frame))?;
            if event::poll(Duration::from_millis(50))? {
                match event::read()? {
                    Event::Key(key) if key.kind != KeyEventKind::Release => self.on_key(key),
                    Event::Mouse(mouse) => self.on_mouse(mouse),
                    Event::Paste(text) => {
                        for c in text.chars().filter(|c| !c.is_control()) {
                            self.insert_char(c);
                        }
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }

    /// Pull new traffic, refresh counters and pick up sessions opened over MCP.
    fn update(&mut self) {
        let live = self.manager.list();
        let show_ts = self.config.get().show_timestamps;
        for info in &live {
            if !self.views.iter().any(|v| v.id == info.id) {
                self.views.push(View::new(info.clone(), show_ts));
            }
        }
        self.views.retain(|v| live.iter().any(|i| i.id == v.id));
        if self.selected >= self.views.len() {
            self.selected = self.views.len().saturating_sub(1);
        }
        let now = Instant::now();
        for view in &mut self.views {
            let previous = view.info.as_ref().map(|i| (i.rx_bytes, i.tx_bytes));
            view.info = live.iter().find(|i| i.id == view.id).cloned();
            let Some(info) = view.info.as_ref() else {
                continue;
            };
            if let Some((rx, tx)) = previous {
                if info.rx_bytes > rx {
                    view.rx_flash = Some(now);
                }
                if info.tx_bytes > tx {
                    view.tx_flash = Some(now);
                }
            }
            match view.sample {
                Some((at, rx, tx)) if now.duration_since(at) >= Duration::from_millis(250) => {
                    let dt = now.duration_since(at).as_secs_f64();
                    let drx = info.rx_bytes.saturating_sub(rx);
                    let dtx = info.tx_bytes.saturating_sub(tx);
                    view.rx_rate = view.rx_rate * 0.4 + (drx as f64 / dt) * 0.6;
                    view.tx_rate = view.tx_rate * 0.4 + (dtx as f64 / dt) * 0.6;
                    view.activity.push_back(drx + dtx);
                    while view.activity.len() > 120 {
                        view.activity.pop_front();
                    }
                    view.sample = Some((now, info.rx_bytes, info.tx_bytes));
                }
                None => view.sample = Some((now, info.rx_bytes, info.tx_bytes)),
                _ => {}
            }
        }
        if let Some(view) = self.views.get_mut(self.selected) {
            if let Ok(batch) = self
                .manager
                .read_since(&view.id, view.term.last_seq, 512 * 1024)
            {
                if batch.is_empty() {
                    view.term.last_seq = batch.next_seq;
                } else {
                    let before = view.term.lines.len();
                    view.term.ingest(&batch);
                    if view.scroll > 0 {
                        view.scroll += view.term.lines.len().saturating_sub(before);
                    }
                }
            }
            view.term.flush_idle();
        }
        if let Some((_, error, at)) = &self.toast {
            if at.elapsed() > Duration::from_secs(if *error { 8 } else { 4 }) {
                self.toast = None;
            }
        }
    }

    // ------------------------------------------------------------------ mouse

    fn on_mouse(&mut self, mouse: MouseEvent) {
        let pos = Position::new(mouse.column, mouse.row);
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let hit = self
                    .hits
                    .iter()
                    .rev()
                    .find(|(rect, _)| rect.contains(pos))
                    .map(|(_, hit)| hit.clone());
                if let Some(hit) = hit {
                    self.on_hit(hit);
                }
            }
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let up = mouse.kind == MouseEventKind::ScrollUp;
                match &mut self.mode {
                    Mode::Connect(form, _) => {
                        if up {
                            form.prev()
                        } else {
                            form.next()
                        }
                    }
                    Mode::Normal if self.list_rect.contains(pos) && !self.views.is_empty() => {
                        let n = self.views.len();
                        self.selected = if up {
                            self.selected.saturating_sub(1)
                        } else {
                            (self.selected + 1).min(n - 1)
                        };
                    }
                    Mode::Normal => {
                        if let Some(view) = self.views.get_mut(self.selected) {
                            view.scroll = if up {
                                (view.scroll + 3).min(view.term.lines.len())
                            } else {
                                view.scroll.saturating_sub(3)
                            };
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn on_hit(&mut self, hit: Hit) {
        let recent = self.config.get().recent;
        match hit {
            Hit::Session(i) if i < self.views.len() => self.selected = i,
            Hit::Session(_) => {}
            Hit::Recent(i) => {
                if let Some(config) = recent.get(i).cloned() {
                    self.ports = net::list_ports().unwrap_or_default();
                    let form = ConnectForm::new(config.kind(), self.ports.clone(), &[config]);
                    self.mode = Mode::Connect(Box::new(form), Vec::new());
                }
            }
            Hit::Key(key) => self.on_key(key),
            Hit::FormKind(k) => {
                if let Mode::Connect(form, errors) = &mut self.mode {
                    form.kind_index = k;
                    form.rebuild(&recent);
                    form.focus = 0;
                    errors.clear();
                }
            }
            Hit::FormField(i) => {
                if let Mode::Connect(form, _) = &mut self.mode {
                    let cyclable = form
                        .fields
                        .get(i)
                        .is_some_and(|f| !matches!(f.value, Value::Text(_)));
                    if form.focus == i + 1 && cyclable {
                        form.cycle(true, &recent);
                    } else {
                        form.focus = i + 1;
                    }
                }
            }
            Hit::RecordFormat(target) => {
                for _ in 0..3 {
                    match &self.mode {
                        Mode::Record { format, .. } if *format != target => {
                            self.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE))
                        }
                        _ => break,
                    }
                }
            }
        }
    }

    // ------------------------------------------------------------------ input

    fn on_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && matches!(key.code, KeyCode::Char('q') | KeyCode::Char('c')) {
            if matches!(self.mode, Mode::Normal) {
                self.quit = true;
            } else {
                self.mode = Mode::Normal;
            }
            return;
        }
        match std::mem::replace(&mut self.mode, Mode::Normal) {
            Mode::Normal => self.on_key_normal(key, ctrl),
            Mode::Help => {
                if !matches!(key.code, KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q')) && !ctrl
                {
                    self.mode = Mode::Help;
                }
            }
            Mode::Confirm(id) => match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                    let label = self
                        .views
                        .iter()
                        .find(|v| v.id == id)
                        .map(|v| v.label.clone())
                        .unwrap_or_default();
                    match self.manager.close(&id) {
                        Ok(()) => self.toast(format!("Closed {label}"), false),
                        Err(e) => self.toast(validate::sentence(&e.to_string()), true),
                    }
                }
                KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => {}
                _ => self.mode = Mode::Confirm(id),
            },
            Mode::Connect(form, errors) => self.on_key_connect(form, errors, key),
            Mode::Record {
                mut format,
                mut path,
                error,
            } => match key.code {
                KeyCode::Esc => {}
                KeyCode::Tab | KeyCode::Left | KeyCode::Right | KeyCode::BackTab => {
                    let old = format;
                    format = match (format, key.code) {
                        (LogFormat::Text, KeyCode::Left | KeyCode::BackTab) => LogFormat::Raw,
                        (LogFormat::Text, _) => LogFormat::Csv,
                        (LogFormat::Csv, KeyCode::Left | KeyCode::BackTab) => LogFormat::Text,
                        (LogFormat::Csv, _) => LogFormat::Raw,
                        (LogFormat::Raw, KeyCode::Left | KeyCode::BackTab) => LogFormat::Csv,
                        (LogFormat::Raw, _) => LogFormat::Text,
                    };
                    let p = std::path::Path::new(&path);
                    if p.extension().is_some_and(|e| e == old.extension()) {
                        path = p.with_extension(format.extension()).display().to_string();
                    }
                    self.mode = Mode::Record {
                        format,
                        path,
                        error,
                    };
                }
                KeyCode::Backspace => {
                    path.pop();
                    self.mode = Mode::Record {
                        format,
                        path,
                        error,
                    };
                }
                KeyCode::Char(c) => {
                    if !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                    {
                        path.push(c);
                    }
                    self.mode = Mode::Record {
                        format,
                        path,
                        error,
                    };
                }
                KeyCode::Enter => {
                    let Some(id) = self.views.get(self.selected).map(|v| v.id.clone()) else {
                        return;
                    };
                    match validate::log_path(&path) {
                        Ok(p) => {
                            match self
                                .manager
                                .set_logging_with(&id, Some(p.clone()), true, format)
                            {
                                Ok(()) => self.toast(
                                    format!("Recording {} to {}", format.as_str(), p.display()),
                                    false,
                                ),
                                Err(e) => {
                                    self.mode = Mode::Record {
                                        format,
                                        path,
                                        error: Some(validate::sentence(&e.to_string())),
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            self.mode = Mode::Record {
                                format,
                                path,
                                error: Some(e),
                            }
                        }
                    }
                }
                _ => {
                    self.mode = Mode::Record {
                        format,
                        path,
                        error,
                    }
                }
            },
            Mode::SendFile { mut path, error } => match key.code {
                KeyCode::Esc => {}
                KeyCode::Backspace => {
                    path.pop();
                    self.mode = Mode::SendFile { path, error };
                }
                KeyCode::Char(c) => {
                    if !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                    {
                        path.push(c);
                    }
                    self.mode = Mode::SendFile { path, error };
                }
                KeyCode::Enter => {
                    let Some(id) = self.views.get(self.selected).map(|v| v.id.clone()) else {
                        return;
                    };
                    match self.send_file(&id, &path) {
                        Ok(message) => self.toast(message, false),
                        Err(e) => {
                            self.mode = Mode::SendFile {
                                path,
                                error: Some(e),
                            }
                        }
                    }
                }
                _ => {
                    self.mode = Mode::SendFile { path, error };
                }
            },
        }
    }

    fn on_key_connect(
        &mut self,
        mut form: Box<ConnectForm>,
        errors: Vec<(&'static str, String)>,
        key: KeyEvent,
    ) {
        let recent = self.config.get().recent;
        match key.code {
            KeyCode::Esc => return,
            KeyCode::Tab | KeyCode::Down => form.next(),
            KeyCode::BackTab | KeyCode::Up => form.prev(),
            KeyCode::Left => form.cycle(false, &recent),
            KeyCode::Right => form.cycle(true, &recent),
            KeyCode::Backspace => form.backspace(),
            KeyCode::Char(c) => {
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                {
                    form.type_char(c);
                }
            }
            KeyCode::Enter => {
                let open: Vec<SessionInfo> =
                    self.views.iter().filter_map(|v| v.info.clone()).collect();
                let (config, errs) = form.build(&open);
                match config {
                    Some(config) => {
                        self.open(config.clone());
                        if !matches!(self.mode, Mode::Normal) {
                            // open() switched to an error state; keep the form.
                        }
                        return;
                    }
                    None => {
                        self.mode = Mode::Connect(form, errs);
                        return;
                    }
                }
            }
            _ => {}
        }
        // Re-validate live once the user has seen errors.
        let errors = if errors.is_empty() {
            errors
        } else {
            let open: Vec<SessionInfo> = self.views.iter().filter_map(|v| v.info.clone()).collect();
            form.build(&open).1
        };
        self.mode = Mode::Connect(form, errors);
    }

    fn open(&mut self, config: SessionConfig) {
        let result = match config.clone() {
            SessionConfig::Serial(c) => self.rt.block_on(self.manager.open_serial(c)),
            SessionConfig::TcpClient(c) => self.rt.block_on(self.manager.open_tcp_client(c)),
            SessionConfig::TcpServer(c) => self.rt.block_on(self.manager.open_tcp_server(c)),
            SessionConfig::Udp(c) => self.rt.block_on(self.manager.open_udp(c)),
        };
        match result {
            Ok(info) => {
                self.toast(format!("Opened {}", info.label), false);
                let show_ts = self.config.get().show_timestamps;
                let id = info.id.clone();
                self.views.push(View::new(info, show_ts));
                self.selected = self.views.iter().position(|v| v.id == id).unwrap_or(0);
                self.config.update(|c| c.remember(config));
                let _ = self.config.save();
            }
            Err(e) => {
                let recent = self.config.get().recent;
                let mut form = ConnectForm::new(config.kind(), self.ports.clone(), &recent);
                // Keep what the user typed by remembering it temporarily.
                form.rebuild(&[config]);
                self.mode = Mode::Connect(
                    Box::new(form),
                    vec![("", validate::sentence(&e.to_string()))],
                );
            }
        }
    }

    fn insert_char(&mut self, c: char) {
        if let Some(view) = self.views.get_mut(self.selected) {
            if view.input.chars().count() >= 4096 {
                return;
            }
            let at = view.byte_index();
            view.input.insert(at, c);
            view.cursor += 1;
            view.history_pos = None;
        }
    }

    fn on_key_normal(&mut self, key: KeyEvent, ctrl: bool) {
        // Function keys run macros.
        if let KeyCode::F(n) = key.code {
            let Some(index) = (n as usize).checked_sub(1) else {
                return;
            };
            let macros = self.config.get().macros;
            if let (Some(mac), Some(view)) = (macros.get(index), self.views.get(self.selected)) {
                let result = codec::parse_input(&mac.data, mac.hex)
                    .map_err(|e| e.to_string())
                    .and_then(|b| self.manager.write(&view.id, &b).map_err(|e| e.to_string()));
                match result {
                    Ok(_) => self.toast(format!("Sent macro {}", mac.name), false),
                    Err(e) => self.toast(validate::sentence(&e), true),
                }
            }
            return;
        }
        if ctrl {
            match key.code {
                KeyCode::Char('n') => {
                    self.ports = net::list_ports().unwrap_or_default();
                    let recent = self.config.get().recent;
                    let kind = recent
                        .first()
                        .map(|c| c.kind())
                        .unwrap_or(SessionKind::Serial);
                    self.mode = Mode::Connect(
                        Box::new(ConnectForm::new(kind, self.ports.clone(), &recent)),
                        Vec::new(),
                    );
                }
                KeyCode::Char('g') => self.mode = Mode::Help,
                KeyCode::Char('w') => {
                    if let Some(view) = self.views.get(self.selected) {
                        self.mode = Mode::Confirm(view.id.clone());
                    }
                }
                KeyCode::Char('o') => self.toggle_record(),
                KeyCode::Char('s') => {
                    if self.views.get(self.selected).is_some() {
                        self.mode = Mode::SendFile {
                            path: String::new(),
                            error: None,
                        };
                    } else {
                        self.toast("Open a session first (Ctrl+N).", true);
                    }
                }
                KeyCode::Char('p') => {
                    let next = (theme::index() + 1) % theme::THEMES.len();
                    theme::set(next, theme::depth(), glyphs().level);
                    let name = theme::THEMES[next].name;
                    self.config.update(|c| c.tui_theme = name.to_string());
                    let _ = self.config.save();
                    self.toast(format!("Theme: {name} ({})", theme::depth().label()), false);
                }
                _ => self.on_key_session_ctrl(key),
            }
            return;
        }
        match key.code {
            KeyCode::Tab if !self.views.is_empty() => {
                self.selected = (self.selected + 1) % self.views.len()
            }
            KeyCode::BackTab if !self.views.is_empty() => {
                self.selected = (self.selected + self.views.len() - 1) % self.views.len()
            }
            _ => self.on_key_input(key),
        }
    }

    fn toggle_record(&mut self) {
        let Some(view) = self.views.get(self.selected) else {
            self.toast("Open a session first (Ctrl+N).", true);
            return;
        };
        if let Some(rec) = view.info.as_ref().and_then(|i| i.recording.clone()) {
            let _ = self.manager.set_logging(&view.id, None, false);
            self.toast(format!("Saved recording to {}", rec.path), false);
            return;
        }
        let dir = self
            .config
            .get()
            .log_dir
            .filter(|d| d.is_dir())
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| ".".into()));
        let name: String = view
            .label
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let path = dir
            .join(format!("{name}-{stamp}.log"))
            .display()
            .to_string();
        self.mode = Mode::Record {
            format: LogFormat::Text,
            path,
            error: None,
        };
    }

    /// Queue a local file to the selected session in 1 KiB chunks.
    fn send_file(&self, id: &str, path: &str) -> Result<String, String> {
        let trimmed = path.trim();
        if trimmed.is_empty() {
            return Err("Enter a file path.".into());
        }
        let path = std::path::PathBuf::from(trimmed);
        let meta = std::fs::metadata(&path)
            .map_err(|e| format!("Cannot read {}: {e}.", path.display()))?;
        if !meta.is_file() {
            return Err("That path is not a file.".into());
        }
        if meta.len() == 0 {
            return Err("The file is empty.".into());
        }
        if meta.len() > MAX_SEND_FILE {
            return Err(format!(
                "File is {}; the limit is {}.",
                human_bytes(meta.len() as usize),
                human_bytes(MAX_SEND_FILE as usize)
            ));
        }
        let data = std::fs::read(&path).map_err(|e| format!("Cannot read file: {e}."))?;
        for chunk in data.chunks(1024) {
            self.manager
                .write(id, chunk)
                .map_err(|e| validate::sentence(&e.to_string()))?;
        }
        Ok(format!(
            "Queued {} from {}",
            human_bytes(data.len()),
            path.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default()
        ))
    }

    fn on_key_session_ctrl(&mut self, key: KeyEvent) {
        let Some(view) = self.views.get_mut(self.selected) else {
            return;
        };
        let id = view.id.clone();
        let mut message: Option<(String, bool)> = None;
        match key.code {
            KeyCode::Char('x') => {
                view.hex = !view.hex;
                message = Some((
                    format!("Input mode: {}", if view.hex { "hex" } else { "text" }),
                    false,
                ));
            }
            KeyCode::Char('e') => view.eol = view.eol.next(),
            KeyCode::Char('k') => {
                let all = ChecksumKind::ALL;
                let i = all.iter().position(|k| *k == view.checksum).unwrap_or(0);
                view.checksum = all[(i + 1) % all.len()];
            }
            KeyCode::Char('t') => {
                view.term.show_ts = !view.term.show_ts;
            }
            KeyCode::Char('f') => {
                let next = match view.term.format {
                    ViewFormat::Ascii => ViewFormat::Hex,
                    ViewFormat::Hex => ViewFormat::Dec,
                    ViewFormat::Dec => ViewFormat::Ascii,
                };
                view.term.set_format(next);
                view.scroll = 0;
            }
            KeyCode::Char('l') => {
                let _ = self.manager.clear_log(&id);
                view.term.clear_view();
                view.scroll = 0;
            }
            KeyCode::Char('r') | KeyCode::Char('d') if view.kind == SessionKind::Serial => {
                let modem = view.info.as_ref().map(|i| i.modem).unwrap_or_default();
                let op = if key.code == KeyCode::Char('r') {
                    ControlOp::Rts(!modem.rts)
                } else {
                    ControlOp::Dtr(!modem.dtr)
                };
                if let Err(e) = self.manager.set_control(&id, op) {
                    message = Some((validate::sentence(&e.to_string()), true));
                }
            }
            KeyCode::Char('u') => {
                view.input.clear();
                view.cursor = 0;
            }
            _ => {}
        }
        if let Some((m, e)) = message {
            self.toast(m, e);
        }
    }

    fn on_key_input(&mut self, key: KeyEvent) {
        let Some(view) = self.views.get_mut(self.selected) else {
            if key.code == KeyCode::Enter {
                self.toast("Press Ctrl+N to open a connection.", false);
            }
            return;
        };
        match key.code {
            KeyCode::Char(c) => {
                let at = view.byte_index();
                if view.input.chars().count() < 4096 {
                    view.input.insert(at, c);
                    view.cursor += 1;
                }
            }
            KeyCode::Backspace if view.cursor > 0 => {
                view.cursor -= 1;
                let at = view.byte_index();
                view.input.remove(at);
            }
            KeyCode::Delete if view.cursor < view.input.chars().count() => {
                let at = view.byte_index();
                view.input.remove(at);
            }
            KeyCode::Left => view.cursor = view.cursor.saturating_sub(1),
            KeyCode::Right => view.cursor = (view.cursor + 1).min(view.input.chars().count()),
            KeyCode::Home => view.cursor = 0,
            KeyCode::End => view.cursor = view.input.chars().count(),
            KeyCode::Up if !view.history.is_empty() => {
                let pos = match view.history_pos {
                    Some(p) => p.saturating_sub(1),
                    None => view.history.len() - 1,
                };
                view.history_pos = Some(pos);
                view.input = view.history[pos].clone();
                view.cursor = view.input.chars().count();
            }
            KeyCode::Down => {
                if let Some(p) = view.history_pos {
                    if p + 1 < view.history.len() {
                        view.history_pos = Some(p + 1);
                        view.input = view.history[p + 1].clone();
                    } else {
                        view.history_pos = None;
                        view.input.clear();
                    }
                    view.cursor = view.input.chars().count();
                }
            }
            KeyCode::PageUp => view.scroll = (view.scroll + 10).min(view.term.lines.len()),
            KeyCode::PageDown => view.scroll = view.scroll.saturating_sub(10),
            KeyCode::Esc => view.scroll = 0,
            KeyCode::Enter => {
                if view.input.is_empty() {
                    return;
                }
                match view.encoded() {
                    Ok(bytes) => {
                        let id = view.id.clone();
                        match self.manager.write(&id, &bytes) {
                            Ok(_) => {
                                let entry = std::mem::take(&mut view.input);
                                view.history.retain(|h| h != &entry);
                                view.history.push(entry);
                                if view.history.len() > 100 {
                                    view.history.remove(0);
                                }
                                view.history_pos = None;
                                view.cursor = 0;
                                view.scroll = 0;
                            }
                            Err(e) => self.toast(validate::sentence(&e.to_string()), true),
                        }
                    }
                    Err(e) => self.toast(e, true),
                }
            }
            _ => {}
        }
    }

    // ----------------------------------------------------------------- render

    fn draw(&mut self, frame: &mut Frame) {
        self.hits.clear();
        let area = frame.area();
        if area.width < 60 || area.height < 16 {
            frame.render_widget(
                Paragraph::new("Terminal too small. Resize to at least 60x16.")
                    .style(Style::new().fg(th().warn))
                    .alignment(Alignment::Center),
                area,
            );
            return;
        }
        let [top, body, bottom] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(8),
            Constraint::Length(1),
        ])
        .areas(area);
        self.draw_top(frame, top);
        let sidebar_w = if area.width >= 100 { 30 } else { 24 };
        let [side, main] =
            Layout::horizontal([Constraint::Length(sidebar_w), Constraint::Min(30)]).areas(body);
        self.draw_sidebar(frame, side);
        self.draw_main(frame, main);
        self.draw_bottom(frame, bottom);

        if !matches!(self.mode, Mode::Normal) {
            // Dialogs are modal: only their own targets respond.
            self.hits.clear();
        }
        let hits = &mut self.hits;
        match &self.mode {
            Mode::Help => draw_help(frame, area, hits),
            Mode::Connect(form, errors) => draw_connect(frame, area, form, errors, hits),
            Mode::Confirm(id) => {
                let label = self
                    .views
                    .iter()
                    .find(|v| &v.id == id)
                    .map(|v| v.label.clone())
                    .unwrap_or_default();
                draw_confirm(frame, area, &label, hits);
            }
            Mode::Record {
                format,
                path,
                error,
            } => draw_record(frame, area, *format, path, error.as_deref(), hits),
            Mode::SendFile { path, error } => {
                draw_send_file(frame, area, path, error.as_deref(), hits)
            }
            Mode::Normal => {}
        }
    }

    fn draw_top(&self, frame: &mut Frame, area: Rect) {
        let live = self
            .views
            .iter()
            .filter(|v| v.info.as_ref().is_some_and(|i| i.connected))
            .count();
        let mut spans = vec![
            Span::styled(
                " Termbridge ",
                Style::new().fg(th().strong).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" {} sessions, {live} active ", self.views.len()),
                Style::new().fg(th().muted),
            ),
        ];
        let g = glyphs();
        let narrow = area.width < 80;
        match &self.mcp_url {
            Some(url) if !narrow => spans.push(Span::styled(
                format!("  {} MCP {url}", g.on),
                Style::new().fg(th().ok),
            )),
            Some(_) => spans.push(Span::styled(
                format!("  {} MCP on", g.on),
                Style::new().fg(th().ok),
            )),
            None => spans.push(Span::styled(
                format!("  {} MCP off", g.off),
                Style::new().fg(th().faint),
            )),
        }
        frame.render_widget(Paragraph::new(Line::from(spans)), area);
        // Toasts appear on the right; otherwise version and theme.
        let right = match &self.toast {
            Some((msg, error, _)) => Line::from(vec![
                Span::styled(
                    format!(" {} ", if *error { g.fail } else { g.ok }),
                    Style::new()
                        .fg(th().on_accent)
                        .bg(if *error { th().err } else { th().ok })
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!(" {msg} "),
                    Style::new().fg(if *error { th().err } else { th().strong }),
                ),
            ]),
            None => {
                let text = if area.width < 80 {
                    format!(
                        "{}  v{} ",
                        theme::THEMES[theme::index()].name,
                        env!("CARGO_PKG_VERSION")
                    )
                } else {
                    format!(
                        "{}  {}  v{}  {} {} ",
                        theme::THEMES[theme::index()].name,
                        theme::depth().label(),
                        env!("CARGO_PKG_VERSION"),
                        std::env::consts::OS,
                        std::env::consts::ARCH
                    )
                };
                Line::from(Span::styled(text, Style::new().fg(th().faint)))
            }
        };
        frame.render_widget(Paragraph::new(right).alignment(Alignment::Right), area);
    }

    fn draw_sidebar(&mut self, frame: &mut Frame, area: Rect) {
        let recent = self.config.get().recent;
        let recent_h = (recent.len().min(6) as u16 + 2).max(3);
        let [sessions_area, recent_area] =
            Layout::vertical([Constraint::Min(5), Constraint::Length(recent_h)]).areas(area);
        let items: Vec<ListItem> = self
            .views
            .iter()
            .map(|v| {
                let (dot, color) = match &v.info {
                    Some(i) if i.connected => (glyphs().on, th().ok),
                    Some(i) if i.error.is_some() => (glyphs().on, th().err),
                    _ => (glyphs().off, th().warn),
                };
                let info = v.info.as_ref();
                ListItem::new(vec![
                    Line::from(vec![
                        Span::styled(format!("{dot} "), Style::new().fg(color)),
                        Span::styled(
                            format!("{} ", kind_tag(v.kind)),
                            Style::new()
                                .fg(kind_color(v.kind))
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(v.label.clone(), Style::new().fg(th().text)),
                    ]),
                    Line::from(Span::styled(
                        format!(
                            "    RX {}  TX {}",
                            human_bytes(info.map(|i| i.rx_bytes as usize).unwrap_or(0)),
                            human_bytes(info.map(|i| i.tx_bytes as usize).unwrap_or(0))
                        ),
                        Style::new().fg(th().faint),
                    )),
                ])
            })
            .collect();
        let block = panel(" Sessions ");
        if items.is_empty() {
            frame.render_widget(
                Paragraph::new(vec![
                    Line::from(Span::styled("No sessions.", Style::new().fg(th().muted))),
                    Line::from(""),
                    Line::from(vec![
                        Span::styled("Ctrl+N", Style::new().fg(th().accent)),
                        Span::styled(" new connection", Style::new().fg(th().muted)),
                    ]),
                ])
                .block(block)
                .wrap(Wrap { trim: true }),
                sessions_area,
            );
        } else {
            let mut state = ListState::default().with_selected(Some(self.selected));
            frame.render_stateful_widget(
                List::new(items)
                    .block(block)
                    .highlight_style(selected_style())
                    .highlight_symbol(glyphs().pointer),
                sessions_area,
                &mut state,
            );
            // Each item is two rows tall.
            let inner = sessions_area.inner(ratatui::layout::Margin::new(1, 1));
            for (row, index) in (state.offset()..self.views.len()).enumerate() {
                let y = inner.y + row as u16 * 2;
                if y + 1 > inner.y + inner.height {
                    break;
                }
                self.hits
                    .push((Rect::new(inner.x, y, inner.width, 2), Hit::Session(index)));
            }
        }
        let lines: Vec<Line> = if recent.is_empty() {
            vec![Line::from(Span::styled(
                "None yet.",
                Style::new().fg(th().faint),
            ))]
        } else {
            recent
                .iter()
                .take(6)
                .map(|c| {
                    Line::from(vec![
                        Span::styled(
                            format!("{} ", kind_tag(c.kind())),
                            Style::new().fg(kind_color(c.kind())),
                        ),
                        Span::styled(recent_text(c), Style::new().fg(th().muted)),
                    ])
                })
                .collect()
        };
        frame.render_widget(Paragraph::new(lines).block(panel(" Recent ")), recent_area);
        self.list_rect = sessions_area;
        let inner = recent_area.inner(ratatui::layout::Margin::new(1, 1));
        for i in 0..recent.len().min(6).min(inner.height as usize) {
            self.hits.push((
                Rect::new(inner.x, inner.y + i as u16, inner.width, 1),
                Hit::Recent(i),
            ));
        }
    }

    fn draw_main(&mut self, frame: &mut Frame, area: Rect) {
        let Some(view) = self.views.get(self.selected) else {
            let text = vec![
                Line::from(""),
                Line::from(Span::styled(
                    "Start a session",
                    Style::new().fg(th().strong).add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
                Line::from(Span::styled(
                    "Ctrl+N  open a serial port, TCP client, TCP server or UDP socket",
                    Style::new().fg(th().muted),
                )),
                Line::from(Span::styled(
                    "Ctrl+G  all keyboard shortcuts",
                    Style::new().fg(th().muted),
                )),
                Line::from(Span::styled("Ctrl+Q  quit", Style::new().fg(th().muted))),
            ];
            frame.render_widget(
                Paragraph::new(text)
                    .alignment(Alignment::Center)
                    .block(panel("")),
                area,
            );
            return;
        };
        let [header, term, input] = Layout::vertical([
            Constraint::Length(4),
            Constraint::Min(3),
            Constraint::Length(4),
        ])
        .areas(area);

        // Header
        let info = view.info.as_ref();
        let (status, scolor) = match info {
            Some(i) if i.connected => (
                match view.kind {
                    SessionKind::TcpServer => "listening",
                    SessionKind::Udp => "bound",
                    _ => "connected",
                },
                th().ok,
            ),
            Some(i) if i.error.is_some() => ("error", th().err),
            Some(SessionInfo {
                config: SessionConfig::TcpClient(_),
                ..
            }) => ("connecting", th().warn),
            _ => ("disconnected", th().warn),
        };
        let g = glyphs();
        let status_icon = if status == "connecting" {
            g.spinner[(self.tick as usize / 2) % g.spinner.len()]
        } else {
            g.on
        };
        let mut line1 = vec![
            Span::styled(
                format!(" {} ", kind_tag(view.kind)),
                Style::new()
                    .fg(kind_color(view.kind))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                view.label.clone(),
                Style::new().fg(th().strong).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("  {status_icon} {status}"), Style::new().fg(scolor)),
        ];
        if let Some(rec) = info.and_then(|i| i.recording.as_ref()) {
            // Blink the recording dot once a second.
            let dot = if (self.tick / 10).is_multiple_of(2) {
                g.on
            } else {
                " "
            };
            line1.push(Span::styled(
                format!(
                    "  {dot} REC {} {}",
                    rec.format.as_str().to_uppercase(),
                    human_bytes(rec.bytes as usize)
                ),
                Style::new().fg(th().err).add_modifier(Modifier::BOLD),
            ));
        }
        if let Some(n) = info.map(|i| i.auto_responses).filter(|n| *n > 0) {
            line1.push(Span::styled(
                format!("  auto-reply {n}"),
                Style::new().fg(th().accent),
            ));
        }
        let mut line2 = vec![];
        if let Some(i) = info {
            // Activity lights flash briefly whenever data moves, like modem LEDs.
            let lit =
                |t: Option<Instant>| t.is_some_and(|t| t.elapsed() < Duration::from_millis(160));
            let led = |on: bool, color: Color| {
                Span::styled(
                    if on { g.on } else { g.off },
                    Style::new()
                        .fg(if on { color } else { th().faint })
                        .add_modifier(if on {
                            Modifier::BOLD
                        } else {
                            Modifier::empty()
                        }),
                )
            };
            line2.push(Span::raw(" "));
            line2.push(led(lit(view.rx_flash), th().rx));
            line2.push(Span::styled(" RX ", Style::new().fg(th().muted)));
            line2.push(Span::styled(
                human_bytes(i.rx_bytes as usize),
                Style::new().fg(th().rx),
            ));
            line2.push(Span::styled(
                format!(" {}/s", human_bytes(view.rx_rate as usize)),
                Style::new().fg(th().faint),
            ));
            line2.push(Span::raw("   "));
            line2.push(led(lit(view.tx_flash), th().tx));
            line2.push(Span::styled(" TX ", Style::new().fg(th().muted)));
            line2.push(Span::styled(
                human_bytes(i.tx_bytes as usize),
                Style::new().fg(th().tx),
            ));
            line2.push(Span::styled(
                format!(" {}/s", human_bytes(view.tx_rate as usize)),
                Style::new().fg(th().faint),
            ));
            line2.push(Span::styled(
                format!("   packets {}/{}", i.rx_packets, i.tx_packets),
                Style::new().fg(th().muted),
            ));
            match view.kind {
                SessionKind::Serial => {
                    let m = i.modem;
                    line2.push(Span::raw("   "));
                    for (name, on) in [
                        ("RTS", m.rts),
                        ("DTR", m.dtr),
                        ("CTS", m.cts),
                        ("DSR", m.dsr),
                        ("RI", m.ri),
                        ("CD", m.cd),
                    ] {
                        line2.push(Span::styled(
                            format!("{name} "),
                            Style::new()
                                .fg(if on { th().ok } else { th().faint })
                                .add_modifier(if on {
                                    Modifier::BOLD
                                } else {
                                    Modifier::empty()
                                }),
                        ));
                    }
                }
                SessionKind::TcpServer => line2.push(Span::styled(
                    format!("   clients {}", i.clients.len()),
                    Style::new().fg(th().muted),
                )),
                _ => {
                    if let Some(peer) = &i.peer {
                        line2.push(Span::styled(
                            format!("   peer {peer}"),
                            Style::new().fg(th().muted),
                        ));
                    }
                }
            }
            if let Some(err) = &i.error {
                line2.push(Span::styled(
                    format!("   {}", validate::sentence(err)),
                    Style::new().fg(th().err),
                ));
            }
        }
        let block = panel("");
        let inner = block.inner(header);
        frame.render_widget(block, header);
        // Live throughput sparkline on the right when there is room.
        let spark_w = if inner.width >= 110 && g.level != theme::GlyphLevel::Ascii {
            36
        } else {
            0
        };
        let [text_area, spark_area] =
            Layout::horizontal([Constraint::Min(20), Constraint::Length(spark_w)]).areas(inner);
        frame.render_widget(
            Paragraph::new(vec![Line::from(line1), Line::from(line2)]),
            text_area,
        );
        if spark_w > 0 {
            let data: Vec<u64> = view
                .activity
                .iter()
                .rev()
                .take(spark_w as usize - 2)
                .rev()
                .copied()
                .collect();
            let max = data.iter().copied().max().unwrap_or(0).max(1);
            let bars = if g.level == theme::GlyphLevel::Full {
                symbols::bar::NINE_LEVELS
            } else {
                symbols::bar::THREE_LEVELS
            };
            frame.render_widget(
                Sparkline::default()
                    .data(&data)
                    .max(max)
                    .bar_set(bars)
                    .style(Style::new().fg(th().accent)),
                Rect::new(
                    spark_area.x + 2,
                    spark_area.y,
                    spark_area.width.saturating_sub(2),
                    2,
                ),
            );
        }

        // Terminal
        let inner_h = term.height.saturating_sub(2) as usize;
        let n = view.term.lines.len();
        let end = n.saturating_sub(view.scroll);
        let start = end.saturating_sub(inner_h);
        let lines: Vec<Line> = view
            .term
            .lines
            .range(start..end)
            .map(|l| {
                let color = dir_color(l.dir);
                let mut spans = Vec::with_capacity(4);
                if view.term.show_ts {
                    spans.push(Span::styled(
                        format!("{} ", l.ts),
                        Style::new().fg(th().faint),
                    ));
                }
                spans.push(Span::styled(
                    format!("{:<3} ", dir_tag(l.dir)),
                    Style::new().fg(color).add_modifier(Modifier::BOLD),
                ));
                let text_color = if matches!(l.dir, Direction::Rx | Direction::Tx) {
                    th().text
                } else {
                    color
                };
                spans.push(Span::styled(l.text.clone(), Style::new().fg(text_color)));
                Line::from(spans)
            })
            .collect();
        let title = format!(
            " {} {sep} {} lines{} ",
            view.term.format.as_str(),
            n,
            if view.scroll > 0 {
                format!(" {} scrolled {} (Esc to follow)", glyphs().sep, view.scroll)
            } else {
                String::new()
            },
            sep = glyphs().sep
        );
        let body = if lines.is_empty() {
            Paragraph::new(Line::from(Span::styled(
                format!("Waiting for data{}", glyphs().ellipsis),
                Style::new().fg(th().faint),
            )))
        } else {
            Paragraph::new(lines)
        };
        frame.render_widget(body.block(panel(&title)), term);
        self.term_rect = term;

        // Composer
        let preview = view.encoded();
        let mode = format!(
            " {} {sep} EOL {} {sep} {} ",
            if view.hex { "HEX" } else { "TEXT" },
            view.eol.label(),
            if view.checksum == ChecksumKind::None {
                "no checksum".to_string()
            } else {
                view.checksum.label().to_string()
            },
            sep = glyphs().sep
        );
        let status_line = match (&preview, view.input.is_empty()) {
            (_, true) => Line::from(Span::styled(
                if view.hex {
                    "Hex bytes, e.g. 01 03 00 00 00 0A"
                } else {
                    "Type and press Enter.  \\r \\n \\xNN $0D \\#013 escapes work."
                },
                Style::new().fg(th().faint),
            )),
            (Ok(bytes), false) => {
                let shown: Vec<u8> = bytes.iter().take(24).copied().collect();
                Line::from(Span::styled(
                    format!(
                        "{} B  {}{}",
                        bytes.len(),
                        codec::to_hex(&shown),
                        if bytes.len() > 24 {
                            glyphs().ellipsis
                        } else {
                            ""
                        }
                    ),
                    Style::new().fg(th().faint),
                ))
            }
            (Err(e), false) => Line::from(Span::styled(e.clone(), Style::new().fg(th().err))),
        };
        let input_block = Block::new()
            .borders(Borders::ALL)
            .border_set(border_set())
            .border_style(Style::new().fg(th().accent))
            .title(Span::styled(mode, Style::new().fg(th().muted)))
            .padding(Padding::horizontal(1));
        let width = input.width.saturating_sub(4) as usize;
        let chars: Vec<char> = view.input.chars().collect();
        let offset = view.cursor.saturating_sub(width.saturating_sub(1));
        let visible: String = chars.iter().skip(offset).take(width).collect();
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled(visible, Style::new().fg(th().strong))),
                status_line,
            ])
            .block(input_block),
            input,
        );
        if matches!(self.mode, Mode::Normal) {
            frame.set_cursor_position((input.x + 2 + (view.cursor - offset) as u16, input.y + 1));
        }
    }

    fn draw_bottom(&mut self, frame: &mut Frame, area: Rect) {
        let ctrl = |c: char| Some(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL));
        let keys: [(&str, &str, Option<KeyEvent>); 13] = [
            ("^N", "new", ctrl('n')),
            (
                "Tab",
                "switch",
                Some(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            ),
            ("^X", "hex", ctrl('x')),
            ("^E", "EOL", ctrl('e')),
            ("^K", "checksum", ctrl('k')),
            ("^F", "view", ctrl('f')),
            ("^O", "record", ctrl('o')),
            ("F1-12", "macros", None),
            ("^P", "theme", ctrl('p')),
            ("^W", "close", ctrl('w')),
            ("^G", "help", ctrl('g')),
            ("^Q", "quit", ctrl('q')),
            ("^S", "file", ctrl('s')),
        ];
        let mut spans = vec![Span::raw(" ")];
        let mut x = area.x;
        for (k, d, event) in keys {
            let label = format!(" {d}  ");
            let width = (k.chars().count() + label.chars().count()) as u16;
            if let Some(event) = event {
                if x + width <= area.x + area.width {
                    self.hits
                        .push((Rect::new(x, area.y, width, 1), Hit::Key(event)));
                }
            }
            x += width;
            spans.push(Span::styled(
                k,
                Style::new().fg(th().accent).add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::styled(label, Style::new().fg(th().muted)));
        }
        frame.render_widget(Paragraph::new(Line::from(spans)), area);
    }
}

// ------------------------------------------------------------------- helpers

fn panel(title: &str) -> Block<'_> {
    Block::new()
        .borders(Borders::ALL)
        .border_set(border_set())
        .border_style(Style::new().fg(th().border))
        .title(Span::styled(title, Style::new().fg(th().muted)))
}

fn border_set() -> symbols::border::Set<'static> {
    if glyphs().level == theme::GlyphLevel::Ascii {
        symbols::border::Set {
            top_left: "+",
            top_right: "+",
            bottom_left: "+",
            bottom_right: "+",
            vertical_left: "|",
            vertical_right: "|",
            horizontal_top: "-",
            horizontal_bottom: "-",
        }
    } else {
        symbols::border::ROUNDED
    }
}

/// Filled button / chip. Without colour support it falls back to reverse video.
fn button_style(color: Color) -> Style {
    if th().mono {
        Style::new().add_modifier(Modifier::REVERSED | Modifier::BOLD)
    } else {
        Style::new()
            .fg(th().on_accent)
            .bg(color)
            .add_modifier(Modifier::BOLD)
    }
}

fn secondary_style() -> Style {
    if th().mono {
        Style::new().add_modifier(Modifier::UNDERLINED)
    } else {
        Style::new().fg(th().strong).bg(th().sel_bg)
    }
}

fn selected_style() -> Style {
    if th().mono {
        Style::new().add_modifier(Modifier::REVERSED)
    } else {
        Style::new().fg(th().strong).bg(th().sel_bg)
    }
}

fn kind_tag(kind: SessionKind) -> &'static str {
    match kind {
        SessionKind::Serial => "SER",
        SessionKind::TcpClient => "TCP",
        SessionKind::TcpServer => "SRV",
        SessionKind::Udp => "UDP",
    }
}

fn kind_color(kind: SessionKind) -> Color {
    match kind {
        SessionKind::Serial => th().accent,
        SessionKind::TcpClient => th().ok,
        SessionKind::TcpServer => th().warn,
        SessionKind::Udp => th().nvt,
    }
}

fn dir_color(dir: Direction) -> Color {
    match dir {
        Direction::Rx => th().rx,
        Direction::Tx => th().tx,
        Direction::Info => th().muted,
        Direction::Error => th().err,
        Direction::Nvt => th().nvt,
    }
}

fn recent_text(c: &SessionConfig) -> String {
    match c {
        SessionConfig::Serial(c) => format!("{} {}", c.port, c.baud),
        SessionConfig::TcpClient(c) => format!("{}:{}", c.host, c.port),
        SessionConfig::TcpServer(c) => format!("listen :{}", c.port),
        SessionConfig::Udp(c) => match (&c.remote_host, c.remote_port) {
            (Some(h), Some(p)) => format!("{h}:{p}"),
            _ => format!("udp :{}", c.local_port),
        },
    }
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(area.width.saturating_sub(2));
    let h = height.min(area.height.saturating_sub(2));
    Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    )
}

fn dialog(title: &str) -> Block<'_> {
    Block::new()
        .borders(Borders::ALL)
        .border_set(border_set())
        .border_style(Style::new().fg(th().accent))
        .title(Span::styled(
            format!(" {title} "),
            Style::new().fg(th().strong).add_modifier(Modifier::BOLD),
        ))
        .padding(Padding::new(2, 2, 1, 0))
}

fn draw_help(frame: &mut Frame, area: Rect, hits: &mut Vec<(Rect, Hit)>) {
    let rows = [
        (
            "Ctrl+N",
            "New connection (serial, TCP client, TCP server, UDP)",
        ),
        ("Tab / Shift+Tab", "Next / previous session"),
        ("Enter", "Send the input line"),
        ("Up / Down", "Browse send history"),
        ("Ctrl+X", "Toggle text / hex input"),
        ("Ctrl+E", "Cycle line ending: none, CR, LF, CRLF"),
        (
            "Ctrl+K",
            "Cycle checksum: CRC-16 Modbus/CCITT/XMODEM, CRC-32, LRC, XOR, SUM-8",
        ),
        ("Ctrl+F", "Cycle view: text, hex dump, decimal"),
        ("Ctrl+T", "Toggle timestamps"),
        ("Ctrl+L", "Clear the terminal"),
        ("Ctrl+O", "Start or stop recording (text, CSV, raw)"),
        ("Ctrl+S", "Send a local file (up to 16 MiB)"),
        ("Ctrl+P", "Next colour theme (saved)"),
        (
            "Mouse / touch",
            "Tap sessions, buttons and hints; scroll or swipe to scroll",
        ),
        ("Ctrl+R / Ctrl+D", "Toggle RTS / DTR (serial)"),
        ("Ctrl+U", "Clear the input line"),
        (
            "F1 to F12",
            "Send macros (edit them in the GUI or config file)",
        ),
        ("PgUp / PgDn / Esc", "Scroll back / forward / follow"),
        ("Ctrl+W", "Close the session"),
        ("Ctrl+Q", "Quit"),
    ];
    let lines: Vec<Line> = rows
        .iter()
        .map(|(k, d)| {
            Line::from(vec![
                Span::styled(
                    format!("{k:<20}"),
                    Style::new().fg(th().accent).add_modifier(Modifier::BOLD),
                ),
                Span::styled(*d, Style::new().fg(th().text)),
            ])
        })
        .chain([
            Line::from(""),
            Line::from(Span::styled(
                "Esc or tap anywhere to close",
                Style::new().fg(th().faint),
            )),
        ])
        .collect();
    let rect = centered(area, 96, rows.len() as u16 + 5);
    frame.render_widget(Clear, rect);
    frame.render_widget(
        Paragraph::new(lines).block(dialog("Keyboard shortcuts")),
        rect,
    );
    hits.push((
        area,
        Hit::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
    ));
}

fn draw_confirm(frame: &mut Frame, area: Rect, label: &str, hits: &mut Vec<(Rect, Hit)>) {
    let rect = centered(area, 60, 7);
    frame.render_widget(Clear, rect);
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                format!("Close {label}?"),
                Style::new().fg(th().strong),
            )),
            Line::from(Span::styled(
                "Buffered traffic is discarded; recordings are flushed.",
                Style::new().fg(th().muted),
            )),
            Line::from(""),
            Line::from(vec![
                Span::styled(" Close (y) ", button_style(th().err)),
                Span::raw("   "),
                Span::styled(" Cancel (Esc) ", secondary_style()),
            ]),
        ])
        .block(dialog("Close session")),
        rect,
    );
    let (x, y) = (rect.x + 3, rect.y + 2 + 3);
    hits.push((
        Rect::new(x, y, 11, 1),
        Hit::Key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE)),
    ));
    hits.push((
        Rect::new(x + 14, y, 14, 1),
        Hit::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
    ));
}

fn draw_send_file(
    frame: &mut Frame,
    area: Rect,
    path: &str,
    error: Option<&str>,
    hits: &mut Vec<(Rect, Hit)>,
) {
    let rect = centered(area, 84, 9);
    frame.render_widget(Clear, rect);
    let mut lines = vec![
        Line::from(vec![
            Span::styled("File     ", Style::new().fg(th().muted)),
            Span::styled(
                if path.is_empty() {
                    "<type a path>".to_string()
                } else {
                    path.to_string()
                },
                Style::new().fg(if path.is_empty() {
                    th().faint
                } else {
                    th().strong
                }),
            ),
        ]),
        Line::from(Span::styled(
            "         Raw bytes are sent in 1 KiB chunks. Limit 16 MiB.",
            Style::new().fg(th().faint),
        )),
    ];
    if let Some(e) = error {
        lines.push(Line::from(Span::styled(
            format!("         {e}"),
            Style::new().fg(th().err),
        )));
    }
    lines.push(Line::from(""));
    let button_row = rect.y + 2 + lines.len() as u16;
    lines.push(Line::from(vec![
        Span::styled(" Send ", button_style(th().accent)),
        Span::raw("  "),
        Span::styled(" Cancel ", secondary_style()),
        Span::styled("   type to edit the path", Style::new().fg(th().faint)),
    ]));
    let rect = Rect::new(
        rect.x,
        rect.y,
        rect.width,
        rect.height.max(button_row - rect.y + 2),
    );
    frame.render_widget(Clear, rect);
    frame.render_widget(Paragraph::new(lines).block(dialog("Send file")), rect);
    hits.push((
        Rect::new(rect.x + 3, button_row, 6, 1),
        Hit::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    ));
    hits.push((
        Rect::new(rect.x + 11, button_row, 8, 1),
        Hit::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
    ));
}

fn draw_record(
    frame: &mut Frame,
    area: Rect,
    format: LogFormat,
    path: &str,
    error: Option<&str>,
    hits: &mut Vec<(Rect, Hit)>,
) {
    let rect = centered(area, 84, 11);
    frame.render_widget(Clear, rect);
    let mut fmt = vec![Span::styled("Format   ", Style::new().fg(th().muted))];
    let mut fx = rect.x + 3 + 9;
    for f in [LogFormat::Text, LogFormat::Csv, LogFormat::Raw] {
        let w = f.as_str().len() as u16 + 2;
        hits.push((Rect::new(fx, rect.y + 2, w, 1), Hit::RecordFormat(f)));
        fx += w + 1;
        let style = if f == format {
            button_style(th().accent)
        } else {
            Style::new().fg(th().muted)
        };
        fmt.push(Span::styled(format!(" {} ", f.as_str()), style));
        fmt.push(Span::raw(" "));
    }
    let help = match format {
        LogFormat::Text => "Readable log: timestamp, direction, escaped data.",
        LogFormat::Csv => "timestamp,direction,length,hex,text rows.",
        LogFormat::Raw => "Received bytes only, exactly as they arrived.",
    };
    let mut lines = vec![
        Line::from(fmt),
        Line::from(""),
        Line::from(vec![
            Span::styled("File     ", Style::new().fg(th().muted)),
            Span::styled(path.to_string(), Style::new().fg(th().strong)),
        ]),
        Line::from(Span::styled(
            format!("         {help}"),
            Style::new().fg(th().faint),
        )),
    ];
    if let Some(e) = error {
        lines.push(Line::from(Span::styled(
            format!("         {e}"),
            Style::new().fg(th().err),
        )));
    }
    lines.push(Line::from(""));
    let button_row = rect.y + 2 + lines.len() as u16;
    lines.push(Line::from(vec![
        Span::styled(" Start recording ", button_style(th().accent)),
        Span::raw("  "),
        Span::styled(" Cancel ", secondary_style()),
        Span::styled(
            "   Tab format, type to edit the path",
            Style::new().fg(th().faint),
        ),
    ]));
    let rect = Rect::new(
        rect.x,
        rect.y,
        rect.width,
        rect.height.max(button_row - rect.y + 2),
    );
    frame.render_widget(Clear, rect);
    frame.render_widget(Paragraph::new(lines).block(dialog("Record session")), rect);
    hits.push((
        Rect::new(rect.x + 3, button_row, 17, 1),
        Hit::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    ));
    hits.push((
        Rect::new(rect.x + 22, button_row, 8, 1),
        Hit::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
    ));
}

fn draw_connect(
    frame: &mut Frame,
    area: Rect,
    form: &ConnectForm,
    errors: &[(&'static str, String)],
    hits: &mut Vec<(Rect, Hit)>,
) {
    let height = form.fields.len() as u16
        + 9
        + errors.len().min(4) as u16
        + if form.kind() == SessionKind::Serial {
            1
        } else {
            0
        };
    let rect = centered(area, 78, height);
    frame.render_widget(Clear, rect);
    let mut lines = Vec::new();
    // Transport row
    let mut kinds = vec![Span::styled(
        format!("{:<16}", "Transport"),
        Style::new().fg(if form.focus == 0 {
            th().strong
        } else {
            th().muted
        }),
    )];
    let (cx, cy) = (rect.x + 3, rect.y + 2);
    let lead = if form.focus == 0 { 1 } else { 2 };
    let mut kx = cx + lead + 16;
    for (i, kind) in form::KINDS.iter().enumerate() {
        let label = match kind {
            SessionKind::Serial => "Serial",
            SessionKind::TcpClient => "TCP client",
            SessionKind::TcpServer => "TCP server",
            SessionKind::Udp => "UDP",
        };
        let style = if i == form.kind_index {
            button_style(kind_color(*kind))
        } else {
            Style::new().fg(th().muted)
        };
        kinds.push(Span::styled(format!(" {label} "), style));
        kinds.push(Span::raw(" "));
        let w = label.chars().count() as u16 + 2;
        hits.push((Rect::new(kx, cy, w, 1), Hit::FormKind(i)));
        kx += w + 1;
    }
    if form.focus == 0 {
        kinds.insert(
            0,
            Span::styled(glyphs().pointer, Style::new().fg(th().accent)),
        );
    } else {
        kinds.insert(0, Span::raw("  "));
    }
    lines.push(Line::from(kinds));
    lines.push(Line::from(""));
    for (i, field) in form.fields.iter().enumerate() {
        hits.push((
            Rect::new(cx, cy + 2 + i as u16, rect.width.saturating_sub(6), 1),
            Hit::FormField(i),
        ));
        let focused = form.focus == i + 1;
        let has_error = errors.iter().any(|(k, _)| *k == field.check);
        let value_style = match (&field.value, focused) {
            (_, true) => secondary_style(),
            (Value::Toggle(true), _) => Style::new().fg(th().ok),
            _ => Style::new().fg(th().text),
        };
        let mut value = field.display();
        if focused && matches!(field.value, Value::Text(_)) {
            value.push_str(glyphs().caret);
        }
        if let Value::Choice(..) = field.value {
            value = format!("{} {value} {}", glyphs().choice_l, glyphs().choice_r);
        }
        lines.push(Line::from(vec![
            Span::styled(
                if focused { glyphs().pointer } else { "  " },
                Style::new().fg(th().accent),
            ),
            Span::styled(
                format!("{:<16}", field.label),
                Style::new().fg(if has_error {
                    th().err
                } else if focused {
                    th().strong
                } else {
                    th().muted
                }),
            ),
            Span::styled(format!(" {value} "), value_style),
        ]));
    }
    if form.kind() == SessionKind::Serial {
        let names: Vec<&str> = form.ports.iter().map(|p| p.port_name.as_str()).collect();
        let text = if names.is_empty() {
            "No serial ports detected. Type a port name.".to_string()
        } else {
            format!(
                "Detected: {}  ({} on Port to pick)",
                names.join(", "),
                glyphs().horiz
            )
        };
        lines.push(Line::from(Span::styled(
            format!("  {text}"),
            Style::new().fg(th().faint),
        )));
    }
    lines.push(Line::from(""));
    for (field, msg) in errors.iter().take(4) {
        let label = form
            .fields
            .iter()
            .find(|f| f.check == *field)
            .map(|f| f.label)
            .unwrap_or("");
        let text = if label.is_empty() {
            msg.clone()
        } else {
            format!("{label}: {msg}")
        };
        lines.push(Line::from(Span::styled(
            format!("  {text}"),
            Style::new().fg(th().err),
        )));
    }
    let button_row = cy + lines.len() as u16;
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(" Open ", button_style(th().accent)),
        Span::raw("  "),
        Span::styled(" Cancel ", secondary_style()),
        Span::styled(
            format!(
                "   {} field  {} change  tap a choice to cycle",
                glyphs().vert,
                glyphs().horiz
            ),
            Style::new().fg(th().faint),
        ),
    ]));
    let rect = Rect::new(
        rect.x,
        rect.y,
        rect.width,
        rect.height.max(button_row - rect.y + 2).min(area.height),
    );
    frame.render_widget(Clear, rect);
    frame.render_widget(Paragraph::new(lines).block(dialog("New connection")), rect);
    hits.push((
        Rect::new(cx + 2, button_row, 6, 1),
        Hit::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    ));
    hits.push((
        Rect::new(cx + 10, button_row, 8, 1),
        Hit::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn screen(app: &mut App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(120, 36)).unwrap();
        terminal.draw(|f| app.draw(f)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let mut out = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                out.push_str(buffer[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    fn type_text(app: &mut App, text: &str) {
        for c in text.chars() {
            app.on_key(key(KeyCode::Char(c)));
        }
    }

    #[test]
    fn full_keyboard_flow() {
        let dir = std::env::temp_dir().join(format!("termbridge-tui-{}", std::process::id()));
        let config = Arc::new(ConfigStore::load(Some(&dir.join("config.toml"))));
        let mut app = App::new(config).unwrap();
        assert!(screen(&mut app).contains("Start a session"));

        // A TCP server for the TUI client to connect to.
        let server = app
            .rt
            .block_on(
                app.manager
                    .open_tcp_server(termbridge_core::TcpServerConfig {
                        bind: "127.0.0.1".into(),
                        port: 0,
                        echo: true,
                        ..Default::default()
                    }),
            )
            .unwrap();
        let port = server.peer.unwrap().rsplit(':').next().unwrap().to_string();
        app.update();

        // Ctrl+N opens the form; pick "TCP client" on the transport row.
        app.on_key(ctrl('n'));
        assert!(screen(&mut app).contains("New connection"));
        app.on_key(key(KeyCode::Up)); // focus transport row
        while !matches!(&app.mode, Mode::Connect(f, _) if f.kind() == SessionKind::TcpClient) {
            app.on_key(key(KeyCode::Right));
        }
        // Host: clear and type, then port.
        app.on_key(key(KeyCode::Down));
        for _ in 0..20 {
            app.on_key(key(KeyCode::Backspace));
        }
        type_text(&mut app, "127.0.0.1");
        app.on_key(key(KeyCode::Down));
        for _ in 0..6 {
            app.on_key(key(KeyCode::Backspace));
        }
        // Invalid port first: the form must refuse and show an error.
        type_text(&mut app, "99999");
        app.on_key(key(KeyCode::Enter));
        assert!(matches!(app.mode, Mode::Connect(_, ref e) if !e.is_empty()));
        assert!(screen(&mut app).contains("Port:"));
        for _ in 0..6 {
            app.on_key(key(KeyCode::Backspace));
        }
        type_text(&mut app, &port);
        app.on_key(key(KeyCode::Enter));
        assert!(
            matches!(app.mode, Mode::Normal),
            "form should close after a valid submit"
        );
        assert_eq!(app.views.len(), 2);
        let client_id = app.views[app.selected].id.clone();
        app.rt
            .block_on(
                app.manager
                    .wait_connected(&client_id, Duration::from_secs(3)),
            )
            .unwrap();

        // Send a line with CRLF and read the echo back.
        app.on_key(ctrl('e'));
        app.on_key(ctrl('e'));
        app.on_key(ctrl('e'));
        type_text(&mut app, "hello tui");
        app.on_key(key(KeyCode::Enter));
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut text = String::new();
        while Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(30));
            app.update();
            text = screen(&mut app);
            if text.matches("hello tui").count() >= 2 {
                break;
            }
        }
        assert!(
            text.matches("hello tui").count() >= 2,
            "expected TX and echoed RX:\n{text}"
        );
        assert!(text.contains("EOL CRLF"));

        // History recall, hex mode validation, help overlay.
        app.on_key(key(KeyCode::Up));
        assert_eq!(app.views[app.selected].input, "hello tui");
        app.on_key(ctrl('u'));
        app.on_key(ctrl('x'));
        type_text(&mut app, "zz");
        assert!(screen(&mut app).contains("Invalid") || screen(&mut app).contains("hex"));
        app.on_key(ctrl('g'));
        assert!(screen(&mut app).contains("Keyboard shortcuts"));
        app.on_key(key(KeyCode::Esc));

        // Mouse / touch: tap the first session in the sidebar, swipe the terminal.
        screen(&mut app);
        let (rect, _) = app
            .hits
            .iter()
            .find(|(_, h)| matches!(h, Hit::Session(0)))
            .cloned()
            .expect("session rows are clickable");
        app.on_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: rect.x + 1,
            row: rect.y,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.selected, 0);
        app.on_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: rect.x + 1,
            row: rect.y + 2,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.selected, 1);
        // Tap "help" in the footer, then tap anywhere to dismiss.
        screen(&mut app);
        let (help, _) = app
            .hits
            .iter()
            .find(|(_, h)| matches!(h, Hit::Key(k) if k.code == KeyCode::Char('g')))
            .cloned()
            .unwrap();
        app.on_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: help.x,
            row: help.y,
            modifiers: KeyModifiers::NONE,
        });
        assert!(matches!(app.mode, Mode::Help));
        screen(&mut app);
        app.on_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 1,
            row: 1,
            modifiers: KeyModifiers::NONE,
        });
        assert!(matches!(app.mode, Mode::Normal));

        // Close with confirmation.
        app.on_key(ctrl('w'));
        assert!(screen(&mut app).contains("Close session"));
        app.on_key(key(KeyCode::Char('y')));
        app.update();
        assert_eq!(app.views.len(), 1);

        app.manager.close_all();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn send_file_dialog_validates_and_queues() {
        let dir = std::env::temp_dir().join(format!("termbridge-tui-file-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let config = Arc::new(ConfigStore::load(Some(&dir.join("config.toml"))));
        let mut app = App::new(config).unwrap();
        app.rt
            .block_on(
                app.manager
                    .open_tcp_server(termbridge_core::TcpServerConfig {
                        bind: "127.0.0.1".into(),
                        port: 0,
                        ..Default::default()
                    }),
            )
            .unwrap();
        app.update();

        app.on_key(ctrl('s'));
        assert!(matches!(app.mode, Mode::SendFile { .. }));
        // Empty path: the dialog stays open with an error.
        app.on_key(key(KeyCode::Enter));
        assert!(matches!(app.mode, Mode::SendFile { error: Some(_), .. }));

        let file = dir.join("payload.bin");
        std::fs::write(&file, b"hello").unwrap();
        for c in file.display().to_string().chars() {
            app.on_key(key(KeyCode::Char(c)));
        }
        app.on_key(key(KeyCode::Enter));
        assert!(matches!(app.mode, Mode::Normal));
        assert!(app
            .toast
            .as_ref()
            .is_some_and(|(message, error, _)| !*error && message.contains("Queued")));

        app.manager.close_all();
        let _ = std::fs::remove_dir_all(dir);
    }
}
