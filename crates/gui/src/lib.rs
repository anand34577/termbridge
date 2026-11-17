//! egui desktop GUI for Termbridge.

mod forms;
mod pages;
mod session;
mod terminal;
mod theme;
mod widgets;

use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, CornerRadius, Margin, RichText, Stroke, Vec2};
use termbridge_core::config::{recent_key, ConfigStore};
use termbridge_core::net;
use termbridge_core::session::SessionConfig;
use termbridge_core::types::{SessionEvent, SessionKind};
use termbridge_core::validate;
use termbridge_core::SessionManager;

use forms::{kind_short, ConnectForms, FormResult};
use session::SessionView;
use theme::Palette;
use widgets::strong_font;

/// Launch the desktop application.
pub fn run(config: Arc<ConfigStore>) -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1320.0, 840.0])
            .with_min_inner_size([1040.0, 700.0])
            .with_title("Termbridge")
            .with_app_id("termbridge")
            .with_icon(Arc::new(app_icon())),
        ..Default::default()
    };
    eframe::run_native(
        "Termbridge",
        options,
        Box::new(move |cc| Ok(Box::new(App::new(cc, config)?))),
    )
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Sessions,
    Discovery,
    Tools,
    Mcp,
    Settings,
}

struct Toast {
    message: String,
    error: bool,
    at: Instant,
}

struct App {
    rt: tokio::runtime::Runtime,
    manager: SessionManager,
    config: Arc<ConfigStore>,
    palette: Palette,
    dark: bool,
    tab: Tab,
    sessions: Vec<SessionView>,
    selected: Option<String>,
    forms: ConnectForms,
    toasts: Vec<Toast>,
    events: tokio::sync::broadcast::Receiver<SessionEvent>,
    discovery: pages::DiscoveryState,
    mcp: pages::McpState,
    settings: pages::SettingsState,
    tools: pages::ToolsState,
    ping_rx: Option<std::sync::mpsc::Receiver<(String, Result<String, String>)>>,
    pending_close: Option<String>,
    last_sync: Instant,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>, config: Arc<ConfigStore>) -> Result<Self, String> {
        theme::install_fonts(&cc.egui_ctx);
        let cfg = config.get();
        let dark = matches!(cfg.theme, termbridge_core::config::Theme::Dark);
        let palette = theme::apply(&cc.egui_ctx, dark);
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .map_err(|err| format!("could not start the async runtime: {err}"))?;
        let manager = SessionManager::with_buffer(cfg.buffer_bytes);
        let events = manager.events();
        let mut app = Self {
            rt,
            manager,
            config: config.clone(),
            palette,
            dark,
            tab: Tab::Sessions,
            sessions: Vec::new(),
            selected: None,
            forms: ConnectForms::new(),
            toasts: Vec::new(),
            events,
            discovery: pages::DiscoveryState::default(),
            mcp: pages::McpState::new(&cfg),
            settings: pages::SettingsState::from_config(&cfg),
            tools: pages::ToolsState::default(),
            ping_rx: None,
            pending_close: None,
            last_sync: Instant::now(),
        };
        app.refresh_ports(false);
        if cfg.mcp.autostart {
            app.start_mcp();
        }
        Ok(app)
    }

    fn toast_err(&mut self, message: impl Into<String>) {
        self.push_toast(message.into(), true);
    }

    fn toast_ok(&mut self, message: impl Into<String>) {
        self.push_toast(message.into(), false);
    }

    fn push_toast(&mut self, message: String, error: bool) {
        self.toasts.retain(|t| t.message != message);
        self.toasts.push(Toast {
            message,
            error,
            at: Instant::now(),
        });
        if self.toasts.len() > 4 {
            self.toasts.remove(0);
        }
    }

    fn refresh_ports(&mut self, announce: bool) {
        match net::list_ports() {
            Ok(ports) => {
                let n = ports.len();
                self.forms.set_ports(ports);
                if announce {
                    self.toast_ok(match n {
                        0 => "No serial ports found".to_string(),
                        1 => "Found 1 serial port".to_string(),
                        n => format!("Found {n} serial ports"),
                    });
                }
            }
            Err(err) => self.toast_err(format!("Could not list serial ports: {err}")),
        }
    }

    fn select(&mut self, id: String) {
        self.selected = Some(id);
        self.tab = Tab::Sessions;
    }

    /// Keep the sidebar in sync with sessions created or closed by MCP clients.
    fn sync_sessions(&mut self) {
        let live = self.manager.list();
        let cfg = self.config.get();
        for info in &live {
            if !self.sessions.iter().any(|view| view.id == info.id) {
                let id = info.id.clone();
                self.sessions.push(SessionView::new(info.clone(), &cfg));
                if self.selected.is_none() {
                    self.selected = Some(id);
                }
            }
        }
        self.sessions
            .retain(|view| live.iter().any(|info| info.id == view.id));
        if let Some(sel) = &self.selected {
            if !self.sessions.iter().any(|v| &v.id == sel) {
                self.selected = self.sessions.last().map(|v| v.id.clone());
            }
        }
    }

    fn open_config(&mut self, config: SessionConfig) {
        let result = match config.clone() {
            SessionConfig::Serial(cfg) => self.rt.block_on(self.manager.open_serial(cfg)),
            SessionConfig::TcpClient(cfg) => self.rt.block_on(self.manager.open_tcp_client(cfg)),
            SessionConfig::TcpServer(cfg) => self.rt.block_on(self.manager.open_tcp_server(cfg)),
            SessionConfig::Udp(cfg) => self.rt.block_on(self.manager.open_udp(cfg)),
        };
        match result {
            Ok(info) => {
                self.toast_ok(format!("Opened {}", info.label));
                let cfg = self.config.get();
                let id = info.id.clone();
                self.sessions.push(SessionView::new(info, &cfg));
                self.select(id);
                self.config.update(|c| c.remember(config));
                if let Err(e) = self.config.save() {
                    tracing::warn!("could not save recent connections: {e}");
                }
            }
            Err(err) => {
                self.toast_err(validate::sentence(&err.to_string()));
                // Reopen the dialog so the user can correct the settings.
                self.forms.open_from(&config);
            }
        }
    }

    fn close_session(&mut self, id: &str) {
        if let Err(err) = self.manager.close(id) {
            self.toast_err(validate::sentence(&err.to_string()));
        }
        let index = self.sessions.iter().position(|v| v.id == id);
        self.sessions.retain(|view| view.id != id);
        if self.selected.as_deref() == Some(id) {
            self.selected = index
                .and_then(|i| self.sessions.get(i.saturating_sub(1)))
                .or_else(|| self.sessions.first())
                .map(|v| v.id.clone());
        }
    }

    fn start_ping(&mut self, host: String, port: u16, session: String) {
        let (tx, rx) = std::sync::mpsc::channel();
        self.rt.spawn(async move {
            let result = net::tcp_probe(&host, port, Duration::from_millis(2000), false).await;
            let message = match result {
                Ok(probe) if probe.connected => Ok(format!("{} ms round trip", probe.rtt_ms)),
                Ok(_) => Err("no answer".to_string()),
                Err(err) => Err(err.to_string()),
            };
            let _ = tx.send((session, message));
        });
        self.ping_rx = Some(rx);
    }

    fn poll(&mut self, ctx: &egui::Context) {
        if self.last_sync.elapsed() >= Duration::from_millis(300) {
            self.last_sync = Instant::now();
            self.sync_sessions();
        }
        loop {
            match self.events.try_recv() {
                Ok(SessionEvent::Error { id, message }) => {
                    let label = self
                        .sessions
                        .iter()
                        .find(|v| v.id == id)
                        .map(|v| v.label.clone())
                        .unwrap_or(id);
                    self.toast_err(format!("{label}: {}", validate::sentence(&message)));
                }
                Ok(SessionEvent::Opened { .. }) | Ok(SessionEvent::Closed { .. }) => {
                    self.sync_sessions();
                }
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }

        if let Some(rx) = &self.ping_rx {
            match rx.try_recv() {
                Ok((id, result)) => {
                    if let Some(view) = self.sessions.iter_mut().find(|v| v.id == id) {
                        view.ping = Some(match result {
                            Ok(m) => m,
                            Err(m) => format!("failed: {m}"),
                        });
                    }
                    self.ping_rx = None;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.ping_rx = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
        }

        self.poll_discovery();

        let selected = self.selected.clone();
        let mut fast = false;
        for view in &mut self.sessions {
            let is_selected =
                selected.as_deref() == Some(view.id.as_str()) && self.tab == Tab::Sessions;
            view.poll(&self.manager, is_selected);
            fast |= view.wants_fast_repaint();
        }

        self.toasts
            .retain(|t| t.at.elapsed() < Duration::from_secs(if t.error { 8 } else { 4 }));

        let delay = if fast || !self.sessions.is_empty() || self.discovery.scanning {
            50
        } else {
            300
        };
        ctx.request_repaint_after(Duration::from_millis(delay));
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        if self.forms.open || self.pending_close.is_some() {
            return;
        }
        let (new, close, clear, next, prev) = ctx.input(|i| {
            let cmd = i.modifiers.command;
            (
                cmd && i.key_pressed(egui::Key::N),
                cmd && i.key_pressed(egui::Key::W),
                cmd && i.key_pressed(egui::Key::L),
                i.modifiers.ctrl && !i.modifiers.shift && i.key_pressed(egui::Key::Tab),
                i.modifiers.ctrl && i.modifiers.shift && i.key_pressed(egui::Key::Tab),
            )
        });
        if new {
            self.forms.open_with(self.forms.kind);
            self.refresh_ports(false);
        }
        if close {
            if let Some(id) = self.selected.clone() {
                self.pending_close = Some(id);
            }
        }
        if clear {
            let manager = self.manager.clone();
            if let Some(view) = self.selected_view_mut() {
                view.clear(&manager);
            }
        }
        if (next || prev) && !self.sessions.is_empty() {
            let n = self.sessions.len();
            let current = self
                .selected
                .as_ref()
                .and_then(|id| self.sessions.iter().position(|v| &v.id == id))
                .unwrap_or(0);
            let index = if next {
                (current + 1) % n
            } else {
                (current + n - 1) % n
            };
            let id = self.sessions[index].id.clone();
            self.select(id);
        }

        // Function keys run macros on the selected session.
        const KEYS: [egui::Key; 12] = [
            egui::Key::F1,
            egui::Key::F2,
            egui::Key::F3,
            egui::Key::F4,
            egui::Key::F5,
            egui::Key::F6,
            egui::Key::F7,
            egui::Key::F8,
            egui::Key::F9,
            egui::Key::F10,
            egui::Key::F11,
            egui::Key::F12,
        ];
        let macros = self.config.get().macros;
        let pressed: Vec<usize> = ctx.input(|i| {
            KEYS.iter()
                .enumerate()
                .filter(|(_, k)| i.key_pressed(**k))
                .map(|(n, _)| n)
                .collect()
        });
        for index in pressed {
            let Some(mac) = macros.get(index) else {
                continue;
            };
            let Some(id) = self.selected.clone() else {
                continue;
            };
            if let Some(view) = self.sessions.iter().find(|v| v.id == id) {
                if let Err(e) = view.send_macro(&self.manager, mac) {
                    self.toast_err(e);
                }
            }
        }

        // Dropped files become the "Send file" source of the current session.
        let dropped: Vec<String> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().display().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        });
        if let Some(path) = dropped.into_iter().next() {
            if let Some(view) = self.selected_view_mut() {
                view.set_file_path(path.clone());
                self.toast_ok(format!("Ready to send {path}. Open Send file to confirm."));
            } else {
                self.toast_err("Open a session before dropping a file.");
            }
        }
    }

    fn selected_view_mut(&mut self) -> Option<&mut SessionView> {
        let id = self.selected.clone()?;
        self.sessions.iter_mut().find(|v| v.id == id)
    }

    fn set_dark(&mut self, ctx: &egui::Context, dark: bool) {
        self.dark = dark;
        self.palette = theme::apply(ctx, dark);
        let theme = if dark {
            termbridge_core::config::Theme::Dark
        } else {
            termbridge_core::config::Theme::Light
        };
        self.settings.theme = theme;
        self.config.update(|c| c.theme = theme);
        let _ = self.config.save();
    }

    // ----------------------------------------------------------------- layout

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        let p = self.palette;
        egui::Panel::top("top_bar")
            .exact_size(56.0)
            .frame(
                egui::Frame::new()
                    .fill(p.surface)
                    .inner_margin(Margin::symmetric(16, 0))
                    .stroke(Stroke::new(1.0, p.border)),
            )
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    widgets::logo(ui, &p, 26.0);
                    ui.add_space(2.0);
                    ui.label(
                        RichText::new("Termbridge")
                            .font(strong_font(16.0))
                            .color(p.text_strong),
                    );
                    ui.add_space(18.0);
                    let tabs = [
                        (Tab::Sessions, "Sessions"),
                        (Tab::Discovery, "Discovery"),
                        (Tab::Tools, "Tools"),
                        (Tab::Mcp, "MCP server"),
                        (Tab::Settings, "Settings"),
                    ];
                    for (tab, label) in tabs {
                        if nav_tab(ui, &p, label, self.tab == tab).clicked() {
                            self.tab = tab;
                        }
                    }
                    let narrow = ui.available_width() < 470.0;
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if widgets::primary(ui, &p, "New connection")
                            .on_hover_text("Ctrl+N")
                            .clicked()
                        {
                            self.forms.open_with(self.forms.kind);
                            self.refresh_ports(false);
                        }
                        let label = match (self.dark, narrow) {
                            (true, false) => "Light theme",
                            (false, false) => "Dark theme",
                            (true, true) => "Light",
                            (false, true) => "Dark",
                        };
                        if widgets::ghost(ui, &p, label).clicked() {
                            let dark = !self.dark;
                            self.set_dark(ui.ctx(), dark);
                        }
                        widgets::vdivider(ui, &p);
                        let (text, color) = match (self.mcp.running, narrow) {
                            (true, false) => ("MCP online", p.ok),
                            (true, true) => ("MCP", p.ok),
                            (false, false) => ("MCP offline", p.faint),
                            (false, true) => ("MCP", p.faint),
                        };
                        let pill = widgets::status_pill(ui, &p, text, color)
                            .interact(egui::Sense::click())
                            .on_hover_text(if self.mcp.running {
                                format!("http://{}/mcp", self.mcp.bind)
                            } else {
                                "Open the MCP server page to start it".to_string()
                            });
                        if pill.clicked() {
                            self.tab = Tab::Mcp;
                        }
                    });
                });
            });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        let p = self.palette;
        egui::Panel::bottom("status_bar")
            .exact_size(28.0)
            .frame(
                egui::Frame::new()
                    .fill(p.surface)
                    .inner_margin(Margin::symmetric(16, 0))
                    .stroke(Stroke::new(1.0, p.border)),
            )
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = 14.0;
                    let total = self.sessions.len();
                    let live = self.sessions.iter().filter(|s| s.connected()).count();
                    let (rx, tx) = self.sessions.iter().fold((0u64, 0u64), |(r, t), s| {
                        s.info
                            .as_ref()
                            .map(|i| (r + i.rx_bytes, t + i.tx_bytes))
                            .unwrap_or((r, t))
                    });
                    let small = |t: String, c: Color32| RichText::new(t).size(11.5).color(c);
                    ui.label(small(format!("{total} sessions, {live} active"), p.muted));
                    ui.label(small(format!("RX {}", widgets::human_count(rx)), p.rx));
                    ui.label(small(format!("TX {}", widgets::human_count(tx)), p.tx));
                    ui.label(small(
                        format!("{} serial ports", self.forms.ports.len()),
                        p.muted,
                    ));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(small(
                            format!(
                                "v{}  ·  {} {}",
                                env!("CARGO_PKG_VERSION"),
                                std::env::consts::OS,
                                std::env::consts::ARCH
                            ),
                            p.faint,
                        ));
                        if self.mcp.running {
                            ui.label(small(format!("http://{}/mcp", self.mcp.bind), p.ok));
                        }
                    });
                });
            });
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        let p = self.palette;
        egui::Panel::left("sidebar")
            .resizable(true)
            .default_size(268.0)
            .size_range(230.0..=380.0)
            .frame(
                egui::Frame::new()
                    .fill(p.surface)
                    .inner_margin(Margin::symmetric(12, 14))
                    .stroke(Stroke::new(1.0, p.border)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Sessions")
                            .font(strong_font(13.5))
                            .color(p.text_strong),
                    );
                    widgets::badge(ui, &p, &self.sessions.len().to_string(), p.muted);
                });
                ui.add_space(6.0);

                let mut select = None;
                let mut close = None;
                egui::ScrollArea::vertical()
                    .id_salt("sessions_scroll")
                    .max_height((ui.available_height() * 0.55).max(140.0))
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        if self.sessions.is_empty() {
                            ui.add_space(4.0);
                            ui.label(
                                RichText::new("No open sessions. Use New connection to start.")
                                    .size(12.5)
                                    .color(p.faint),
                            );
                        }
                        for view in &self.sessions {
                            let selected = self.selected.as_deref() == Some(view.id.as_str())
                                && self.tab == Tab::Sessions;
                            let resp = session_row(ui, &p, view, selected);
                            if resp.clicked() {
                                select = Some(view.id.clone());
                            }
                            resp.context_menu(|ui| {
                                if ui.button("Close session").clicked() {
                                    close = Some(view.id.clone());
                                    ui.close();
                                }
                            });
                        }
                    });
                if let Some(id) = select {
                    self.select(id);
                }
                if let Some(id) = close {
                    self.pending_close = Some(id);
                }

                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Recent")
                            .font(strong_font(13.5))
                            .color(p.text_strong),
                    );
                });
                ui.add_space(4.0);
                let recent = self.config.get().recent;
                if recent.is_empty() {
                    ui.label(
                        RichText::new("Connections you open are remembered here.")
                            .size(12.5)
                            .color(p.faint),
                    );
                }
                let mut reopen = None;
                let mut forget = None;
                egui::ScrollArea::vertical()
                    .id_salt("recent_scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for config in &recent {
                            let resp = recent_row(ui, &p, config);
                            if resp.clicked() {
                                reopen = Some(config.clone());
                            }
                            resp.on_hover_text("Click to review and open")
                                .context_menu(|ui| {
                                    if ui.button("Forget").clicked() {
                                        forget = Some(recent_key(config));
                                        ui.close();
                                    }
                                });
                        }
                    });
                if let Some(config) = reopen {
                    self.refresh_ports(false);
                    self.forms.open_from(&config);
                }
                if let Some(key) = forget {
                    self.config
                        .update(|c| c.recent.retain(|r| recent_key(r) != key));
                    let _ = self.config.save();
                }
            });
    }

    fn sessions_page(&mut self, ui: &mut egui::Ui) {
        let Some(id) = self.selected.clone() else {
            self.welcome(ui);
            return;
        };
        let Some(index) = self.sessions.iter().position(|view| view.id == id) else {
            self.welcome(ui);
            return;
        };
        let macros = self.config.get().macros;
        let action = self.sessions[index].ui(ui, &self.manager, &self.palette, &macros);
        if action.close {
            self.pending_close = Some(id.clone());
        }
        if let Some(msg) = action.toast_ok {
            self.toast_ok(msg);
        }
        if let Some(msg) = action.toast_err {
            self.toast_err(msg);
        }
        if let Some((host, port)) = action.ping {
            self.start_ping(host, port, id);
        }
    }

    fn welcome(&mut self, ui: &mut egui::Ui) {
        let p = self.palette;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let width = ui.available_width().min(920.0);
                let left = ((ui.available_width() - width) / 2.0).max(0.0);
                ui.horizontal(|ui| {
                    ui.add_space(left);
                    ui.vertical(|ui| {
                        ui.set_width(width);
                        ui.add_space(36.0);
                        ui.label(
                            RichText::new("Start a session")
                                .font(strong_font(26.0))
                                .color(p.text_strong),
                        );
                        ui.add_space(2.0);
                        widgets::hint(
                            ui,
                            &p,
                            "Talk to serial devices, TCP endpoints and UDP peers. Every session is also available to AI agents over MCP.",
                        );
                        ui.add_space(22.0);
                        let gap = 12.0;
                        let card_w = (width - gap) / 2.0;
                        let cards = [
                            (SessionKind::Serial, "Serial port", "Open a COM / tty device with full control over baud rate, framing, flow control and modem lines."),
                            (SessionKind::TcpClient, "TCP client", "Connect to a device server, PLC or Telnet console. Optional NVT negotiation and auto-reconnect."),
                            (SessionKind::TcpServer, "TCP server", "Listen on a port and accept multiple clients. Broadcast writes and optional echo."),
                            (SessionKind::Udp, "UDP", "Bind a socket, send datagrams to a target or broadcast address, and reply to the last sender."),
                        ];
                        for pair in cards.chunks(2) {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = gap;
                                for (kind, title, desc) in pair {
                                    if transport_card(ui, &p, card_w, *kind, title, desc).clicked() {
                                        self.refresh_ports(false);
                                        self.forms.open_with(*kind);
                                    }
                                }
                            });
                            ui.add_space(gap - 8.0);
                        }
                        ui.add_space(18.0);
                        widgets::card(ui, &p, |ui| {
                            ui.set_width(width - 30.0);
                            ui.horizontal(|ui| {
                                ui.vertical(|ui| {
                                    widgets::section_title(ui, &p, "Shortcuts");
                                    ui.add_space(2.0);
                                    for (k, d) in [
                                        ("Ctrl+N", "New connection"),
                                        ("Enter / Up / Down", "Send, browse history"),
                                        ("F1 to F12", "Run macros"),
                                        ("Ctrl+F / Ctrl+L", "Filter, clear terminal"),
                                        ("Ctrl+Tab / Ctrl+W", "Next session, close session"),
                                    ] {
                                        ui.horizontal(|ui| {
                                            ui.add_sized(
                                                [150.0, 18.0],
                                                egui::Label::new(RichText::new(k).monospace().size(12.0).color(p.text)),
                                            );
                                            ui.label(RichText::new(d).size(12.5).color(p.muted));
                                        });
                                    }
                                });
                                ui.add_space(24.0);
                                ui.vertical(|ui| {
                                    widgets::section_title(ui, &p, "Headless and MCP");
                                    ui.add_space(2.0);
                                    for line in [
                                        "termbridge tui",
                                        "termbridge mcp --transport stdio",
                                        "termbridge serial COM3 --baud 9600 --interactive",
                                        "termbridge tcp-server 9000 --echo",
                                    ] {
                                        ui.label(RichText::new(line).monospace().size(12.0).color(p.text));
                                    }
                                });
                            });
                        });
                    });
                });
            });
    }

    fn confirm_close(&mut self, ctx: &egui::Context) {
        let Some(id) = self.pending_close.clone() else {
            return;
        };
        let p = self.palette;
        let label = self
            .sessions
            .iter()
            .find(|v| v.id == id)
            .map(|v| v.label.clone())
            .unwrap_or_else(|| id.clone());
        let mut decision = None;
        let modal = egui::Modal::new(egui::Id::unique("confirm_close"))
            .backdrop_color(Color32::from_black_alpha(if p.dark { 150 } else { 90 }))
            .show(ctx, |ui| {
                ui.set_width(380.0);
                ui.label(
                    RichText::new("Close this session?")
                        .font(strong_font(16.0))
                        .color(p.text_strong),
                );
                ui.add_space(4.0);
                widgets::hint(
                    ui,
                    &p,
                    &format!("{label} will be disconnected and its buffered traffic discarded. Any active file log is flushed first."),
                );
                ui.add_space(12.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if widgets::danger(ui, &p, "Close session").clicked() || enter {
                        decision = Some(true);
                    }
                    if widgets::secondary(ui, &p, "Cancel").clicked() {
                        decision = Some(false);
                    }
                });
            });
        if modal.should_close() {
            decision = decision.or(Some(false));
        }
        match decision {
            Some(true) => {
                self.pending_close = None;
                self.close_session(&id);
                self.toast_ok(format!("Closed {label}"));
            }
            Some(false) => self.pending_close = None,
            None => {}
        }
    }

    fn toasts(&mut self, ctx: &egui::Context) {
        if self.toasts.is_empty() {
            return;
        }
        let p = self.palette;
        let mut dismiss = None;
        egui::Area::new(egui::Id::unique("toasts"))
            .anchor(egui::Align2::RIGHT_BOTTOM, Vec2::new(-18.0, -40.0))
            .order(egui::Order::Foreground)
            .interactable(true)
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing.y = 8.0;
                for (index, toast) in self.toasts.iter().enumerate() {
                    let color = if toast.error { p.error } else { p.ok };
                    // Fade in over 150 ms and out over the last 400 ms.
                    let age = toast.at.elapsed().as_secs_f32();
                    let life = if toast.error { 8.0 } else { 4.0 };
                    let alpha = (age / 0.15).min(1.0).min(((life - age) / 0.4).max(0.0));
                    ui.set_opacity(alpha.clamp(0.0, 1.0));
                    let resp = egui::Frame::new()
                        .fill(p.card)
                        .stroke(Stroke::new(1.0, p.border_strong))
                        .corner_radius(CornerRadius::same(10))
                        .shadow(ctx.global_style().visuals.popup_shadow)
                        .inner_margin(Margin::symmetric(14, 10))
                        .show(ui, |ui| {
                            ui.set_max_width(380.0);
                            ui.horizontal(|ui| {
                                widgets::dot(ui, color, true);
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(&toast.message)
                                            .size(13.0)
                                            .color(p.text_strong),
                                    )
                                    .wrap(),
                                );
                            });
                        })
                        .response
                        .interact(egui::Sense::click())
                        .on_hover_text("Click to dismiss");
                    if resp.clicked() {
                        dismiss = Some(index);
                    }
                }
            });
        if let Some(index) = dismiss {
            self.toasts.remove(index);
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll(&ctx);
        self.handle_shortcuts(&ctx);
        self.top_bar(ui);
        self.status_bar(ui);
        self.sidebar(ui);
        let p = self.palette;
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(p.bg)
                    .inner_margin(Margin::symmetric(20, 16)),
            )
            .show(ui, |ui| match self.tab {
                Tab::Sessions => self.sessions_page(ui),
                Tab::Discovery => self.discovery_page(ui),
                Tab::Tools => self.tools_page(ui),
                Tab::Mcp => self.mcp_page(ui),
                Tab::Settings => self.settings_page(ui),
            });

        let open: Vec<_> = self
            .sessions
            .iter()
            .filter_map(|v| v.info.clone())
            .collect();
        match self.forms.show(&ctx, &self.palette, &open) {
            FormResult::Submit(config) => self.open_config(config),
            FormResult::RefreshPorts => self.refresh_ports(true),
            FormResult::None => {}
        }
        self.confirm_close(&ctx);
        self.toasts(&ctx);
    }
}

impl Drop for App {
    fn drop(&mut self) {
        self.manager.close_all();
        if let Some(tx) = self.mcp.shutdown.take() {
            let _ = tx.send(());
        }
    }
}

// --------------------------------------------------------------- components

fn nav_tab(ui: &mut egui::Ui, p: &Palette, label: &str, selected: bool) -> egui::Response {
    let text =
        RichText::new(label)
            .size(13.5)
            .color(if selected { p.text_strong } else { p.muted });
    let text = if selected {
        text.font(strong_font(13.5))
    } else {
        text
    };
    let resp = ui.add(
        egui::Button::new(text)
            .fill(if selected {
                p.raised
            } else {
                Color32::TRANSPARENT
            })
            .stroke(Stroke::NONE)
            .corner_radius(CornerRadius::same(7))
            .min_size(Vec2::new(0.0, 32.0)),
    );
    if selected {
        let r = resp.rect;
        ui.painter().rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(r.left() + 10.0, r.bottom() + 10.0),
                egui::pos2(r.right() - 10.0, r.bottom() + 12.0),
            ),
            CornerRadius::same(1),
            p.accent,
        );
    }
    resp
}

fn session_row(
    ui: &mut egui::Ui,
    p: &Palette,
    view: &SessionView,
    selected: bool,
) -> egui::Response {
    let color = if view.connected() {
        p.ok
    } else if view.has_error() {
        p.error
    } else {
        p.warn
    };
    let sub = view
        .info
        .as_ref()
        .map(|i| {
            format!(
                "RX {}  ·  TX {}",
                widgets::human_count(i.rx_bytes),
                widgets::human_count(i.tx_bytes)
            )
        })
        .unwrap_or_default();
    let resp = egui::Frame::new()
        .fill(if selected {
            p.card
        } else {
            Color32::TRANSPARENT
        })
        .stroke(Stroke::new(
            1.0,
            if selected {
                p.border_strong
            } else {
                Color32::TRANSPARENT
            },
        ))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(9, 7))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                widgets::badge(ui, p, kind_short(view.kind), p.kind_color(view.kind));
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    ui.add(
                        egui::Label::new(
                            RichText::new(&view.label)
                                .font(strong_font(13.0))
                                .color(if selected { p.text_strong } else { p.text }),
                        )
                        .truncate(),
                    );
                    ui.label(RichText::new(sub).size(11.0).color(p.faint));
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    widgets::dot(ui, color, view.connected());
                });
            });
        })
        .response
        .interact(egui::Sense::click());
    if resp.hovered() && !selected {
        ui.painter().rect_filled(
            resp.rect,
            CornerRadius::same(8),
            Color32::from_rgba_unmultiplied(p.text.r(), p.text.g(), p.text.b(), 8),
        );
    }
    resp
}

fn recent_row(ui: &mut egui::Ui, p: &Palette, config: &SessionConfig) -> egui::Response {
    let kind = config.kind();
    let text = match config {
        SessionConfig::Serial(c) => format!("{}  {}", c.port, c.baud),
        SessionConfig::TcpClient(c) => format!("{}:{}", c.host, c.port),
        SessionConfig::TcpServer(c) => format!("listen :{}", c.port),
        SessionConfig::Udp(c) => match (&c.remote_host, c.remote_port) {
            (Some(h), Some(port)) => format!("{h}:{port}"),
            _ => format!("udp :{}", c.local_port),
        },
    };
    let resp = egui::Frame::new()
        .inner_margin(Margin::symmetric(9, 5))
        .corner_radius(CornerRadius::same(7))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                widgets::badge(ui, p, kind_short(kind), p.kind_color(kind));
                ui.add(
                    egui::Label::new(RichText::new(text).monospace().size(12.0).color(p.text))
                        .truncate(),
                );
            });
        })
        .response
        .interact(egui::Sense::click());
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        ui.painter().rect_filled(
            resp.rect,
            CornerRadius::same(7),
            Color32::from_rgba_unmultiplied(p.text.r(), p.text.g(), p.text.b(), 8),
        );
    }
    resp
}

fn transport_card(
    ui: &mut egui::Ui,
    p: &Palette,
    width: f32,
    kind: SessionKind,
    title: &str,
    desc: &str,
) -> egui::Response {
    let color = p.kind_color(kind);
    let resp = egui::Frame::new()
        .fill(p.card)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::same(18))
        .show(ui, |ui| {
            ui.set_width(width - 38.0);
            ui.set_height(92.0);
            ui.horizontal(|ui| {
                widgets::badge(ui, p, kind_short(kind), color);
                ui.label(
                    RichText::new(title)
                        .font(strong_font(15.5))
                        .color(p.text_strong),
                );
            });
            ui.add_space(4.0);
            ui.add(egui::Label::new(RichText::new(desc).size(12.5).color(p.muted)).wrap());
            ui.add_space(2.0);
            ui.label(
                RichText::new("Configure  →")
                    .font(strong_font(12.5))
                    .color(color),
            );
        })
        .response
        .interact(egui::Sense::click());
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        widgets::outline(ui, resp.rect, color, 12);
    }
    resp
}

/// 64x64 window icon: accent rounded square with a white step waveform.
fn app_icon() -> egui::IconData {
    const N: usize = 64;
    let mut rgba = vec![0u8; N * N * 4];
    let accent = [0x4Cu8, 0x9A, 0xFF];
    let radius = 14.0f32;
    let inside = |x: f32, y: f32| {
        let cx = x.clamp(radius, N as f32 - radius);
        let cy = y.clamp(radius, N as f32 - radius);
        (x - cx).powi(2) + (y - cy).powi(2) <= radius * radius
    };
    // Waveform segments in 0..1 space.
    let pts = [
        (0.2, 0.5),
        (0.33, 0.5),
        (0.33, 0.3),
        (0.5, 0.3),
        (0.5, 0.7),
        (0.67, 0.7),
        (0.67, 0.5),
        (0.8, 0.5),
    ];
    let on_line = |x: f32, y: f32| {
        pts.windows(2).any(|w| {
            let (x1, y1) = (w[0].0 * N as f32, w[0].1 * N as f32);
            let (x2, y2) = (w[1].0 * N as f32, w[1].1 * N as f32);
            let (minx, maxx) = (x1.min(x2) - 2.6, x1.max(x2) + 2.6);
            let (miny, maxy) = (y1.min(y2) - 2.6, y1.max(y2) + 2.6);
            x >= minx && x <= maxx && y >= miny && y <= maxy
        })
    };
    for y in 0..N {
        for x in 0..N {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let i = (y * N + x) * 4;
            if inside(fx, fy) {
                let c = if on_line(fx, fy) {
                    [255, 255, 255]
                } else {
                    accent
                };
                rgba[i..i + 3].copy_from_slice(&c);
                rgba[i + 3] = 255;
            }
        }
    }
    egui::IconData {
        rgba,
        width: N as u32,
        height: N as u32,
    }
}
