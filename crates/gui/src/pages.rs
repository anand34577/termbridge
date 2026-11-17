//! Discovery, MCP server and Settings pages.

use std::sync::Arc;
use std::time::Instant;

use eframe::egui::{self, RichText};
use termbridge_core::codec;
use termbridge_core::config::{AppConfig, MacroDef, Theme, MAX_BUFFER, MIN_BUFFER};
use termbridge_core::discovery::{ScanRequest, ScanResponse};
use termbridge_core::session::SessionConfig;
use termbridge_core::UdpConfig;
use termbridge_mcp::McpServer;

use crate::widgets::{self, strong_font};
use crate::App;
use termbridge_core::validate;

// ---------------------------------------------------------------- discovery

pub struct DiscoveryState {
    target: String,
    port: u16,
    payload_hex: String,
    timeout_ms: u64,
    local_port: u16,
    pub scanning: bool,
    results: Vec<ScanResponse>,
    error: Option<String>,
    last_run: Option<(Instant, usize)>,
    rx: Option<std::sync::mpsc::Receiver<Result<Vec<ScanResponse>, String>>>,
}

impl Default for DiscoveryState {
    fn default() -> Self {
        Self {
            target: "255.255.255.255".into(),
            port: 4000,
            payload_hex: String::new(),
            timeout_ms: 1000,
            local_port: 0,
            scanning: false,
            results: Vec::new(),
            error: None,
            last_run: None,
            rx: None,
        }
    }
}

// ---------------------------------------------------------------------- mcp

pub struct McpState {
    pub bind: String,
    pub running: bool,
    pub autostart: bool,
    error: Option<String>,
    pub shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<()>>,
    started: Option<Instant>,
}

impl McpState {
    pub fn new(cfg: &AppConfig) -> Self {
        Self {
            bind: cfg.mcp.bind.clone(),
            running: false,
            autostart: cfg.mcp.autostart,
            error: None,
            shutdown: None,
            task: None,
            started: None,
        }
    }
}

// ----------------------------------------------------------------- settings

#[derive(Clone, PartialEq)]
struct MacroDraft {
    name: String,
    data: String,
    hex: bool,
}

pub struct SettingsState {
    pub theme: Theme,
    show_timestamps: bool,
    buffer_kib: u64,
    log_dir: String,
    macros: Vec<MacroDraft>,
    saved: Option<(Theme, bool, u64, String, Vec<MacroDraft>)>,
}

impl SettingsState {
    pub fn from_config(cfg: &AppConfig) -> Self {
        let mut s = Self {
            theme: cfg.theme,
            show_timestamps: cfg.show_timestamps,
            buffer_kib: (cfg.buffer_bytes / 1024) as u64,
            log_dir: cfg
                .log_dir
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            macros: cfg
                .macros
                .iter()
                .map(|m| MacroDraft {
                    name: m.name.clone(),
                    data: m.data.clone(),
                    hex: m.hex,
                })
                .collect(),
            saved: None,
        };
        s.saved = Some(s.snapshot());
        s
    }

    fn snapshot(&self) -> (Theme, bool, u64, String, Vec<MacroDraft>) {
        (
            self.theme,
            self.show_timestamps,
            self.buffer_kib,
            self.log_dir.clone(),
            self.macros.clone(),
        )
    }

    fn dirty(&self) -> bool {
        self.saved.as_ref() != Some(&self.snapshot())
    }
}

/// Centered, width-limited page column.
fn page<R>(ui: &mut egui::Ui, max: f32, add: impl FnOnce(&mut egui::Ui, f32) -> R) {
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let width = ui.available_width().min(max);
            let left = ((ui.available_width() - width) / 2.0).max(0.0);
            ui.horizontal(|ui| {
                ui.add_space(left);
                ui.vertical(|ui| {
                    ui.set_width(width);
                    add(ui, width);
                    ui.add_space(24.0);
                });
            });
        });
}

fn page_header(ui: &mut egui::Ui, p: &crate::theme::Palette, title: &str, sub: &str) {
    ui.add_space(6.0);
    ui.label(
        RichText::new(title)
            .font(strong_font(22.0))
            .color(p.text_strong),
    );
    widgets::hint(ui, p, sub);
    ui.add_space(14.0);
}

fn form_row(
    ui: &mut egui::Ui,
    p: &crate::theme::Palette,
    label: &str,
    add: impl FnOnce(&mut egui::Ui),
) {
    ui.horizontal(|ui| {
        widgets::field_label(ui, p, label, 130.0);
        add(ui);
    });
}

fn issue_line(ui: &mut egui::Ui, p: &crate::theme::Palette, msg: &str, warn: bool) {
    ui.horizontal(|ui| {
        ui.add_space(138.0);
        ui.label(
            RichText::new(msg)
                .size(12.0)
                .color(if warn { p.warn } else { p.error }),
        );
    });
}

impl App {
    pub(crate) fn poll_discovery(&mut self) {
        if let Some(rx) = &self.discovery.rx {
            match rx.try_recv() {
                Ok(result) => {
                    self.discovery.scanning = false;
                    self.discovery.rx = None;
                    match result {
                        Ok(results) => {
                            self.discovery.last_run = Some((Instant::now(), results.len()));
                            self.discovery.results = results;
                            self.discovery.error = None;
                        }
                        Err(err) => self.discovery.error = Some(validate::sentence(&err)),
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.discovery.scanning = false;
                    self.discovery.rx = None;
                    self.discovery.error = Some("The scan task stopped unexpectedly.".into());
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
        }
    }

    fn discovery_issues(&self) -> (Option<String>, Option<String>) {
        let d = &self.discovery;
        let target = if d.target.trim().parse::<std::net::IpAddr>().is_err() {
            Some("Use an IP address, e.g. 255.255.255.255 or 192.168.1.255.".to_string())
        } else {
            None
        };
        let payload = codec::parse_hex(&d.payload_hex)
            .err()
            .map(|e| validate::sentence(&e.to_string()));
        (target, payload)
    }

    fn start_scan(&mut self) {
        let payload = match codec::parse_hex(&self.discovery.payload_hex) {
            Ok(bytes) => bytes,
            Err(err) => {
                self.discovery.error = Some(validate::sentence(&err.to_string()));
                return;
            }
        };
        let request = ScanRequest {
            target: self.discovery.target.trim().to_string(),
            port: self.discovery.port,
            payload,
            timeout_ms: self.discovery.timeout_ms,
            local_port: self.discovery.local_port,
        };
        let (tx, rx) = std::sync::mpsc::channel();
        self.rt.spawn(async move {
            let result = termbridge_core::discovery::udp_scan(request)
                .await
                .map_err(|err| err.to_string());
            let _ = tx.send(result);
        });
        self.discovery.rx = Some(rx);
        self.discovery.scanning = true;
        self.discovery.error = None;
    }

    pub(crate) fn discovery_page(&mut self, ui: &mut egui::Ui) {
        let p = self.palette;
        let (target_issue, payload_issue) = self.discovery_issues();
        let valid = target_issue.is_none() && payload_issue.is_none() && self.discovery.port > 0;
        let mut scan = false;
        let mut use_target: Option<String> = None;
        page(ui, 980.0, |ui, width| {
            page_header(
                ui,
                &p,
                "UDP discovery",
                "Broadcast a probe datagram and list every device that answers, with round-trip time and payload.",
            );
            widgets::card(ui, &p, |ui| {
                ui.set_width(width - 30.0);
                widgets::section_title(ui, &p, "Probe");
                ui.add_space(6.0);
                let d = &mut self.discovery;
                form_row(ui, &p, "Target address", |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut d.target)
                            .desired_width(200.0)
                            .font(egui::TextStyle::Monospace),
                    );
                    if widgets::ghost(ui, &p, "Broadcast").clicked() {
                        d.target = "255.255.255.255".into();
                    }
                });
                if let Some(msg) = &target_issue {
                    issue_line(ui, &p, msg, false);
                }
                form_row(ui, &p, "Port", |ui| {
                    ui.add(egui::DragValue::new(&mut d.port).range(1..=65535));
                    ui.label(RichText::new("Local port").size(12.5).color(p.muted));
                    ui.add(egui::DragValue::new(&mut d.local_port).range(0..=65535));
                    ui.label(RichText::new("0 = automatic").size(12.0).color(p.faint));
                });
                form_row(ui, &p, "Payload (hex)", |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut d.payload_hex)
                            .desired_width(320.0)
                            .font(egui::TextStyle::Monospace)
                            .hint_text("Optional. Empty sends a zero-length datagram."),
                    );
                });
                if let Some(msg) = &payload_issue {
                    issue_line(ui, &p, msg, false);
                }
                form_row(ui, &p, "Wait for replies", |ui| {
                    ui.add(
                        egui::DragValue::new(&mut d.timeout_ms)
                            .range(50..=30_000)
                            .speed(50)
                            .suffix(" ms"),
                    );
                });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.add_space(138.0);
                    let text = if d.scanning {
                        "Scanning…"
                    } else {
                        "Run scan"
                    };
                    if ui
                        .add_enabled(valid && !d.scanning, widgets::primary_button(&p, text))
                        .clicked()
                    {
                        scan = true;
                    }
                    if widgets::secondary(ui, &p, "Clear results").clicked() {
                        d.results.clear();
                        d.error = None;
                        d.last_run = None;
                    }
                    if d.scanning {
                        ui.spinner();
                    }
                });
                if let Some(err) = &d.error {
                    issue_line(ui, &p, err, false);
                }
            });
            ui.add_space(14.0);
            widgets::card(ui, &p, |ui| {
                ui.set_width(width - 30.0);
                ui.horizontal(|ui| {
                    widgets::section_title(ui, &p, "Responders");
                    widgets::badge(ui, &p, &self.discovery.results.len().to_string(), p.accent);
                    if let Some((at, n)) = self.discovery.last_run {
                        ui.label(
                            RichText::new(format!(
                                "last scan {}s ago, {n} replies",
                                at.elapsed().as_secs()
                            ))
                            .size(12.0)
                            .color(p.faint),
                        );
                    }
                });
                ui.add_space(6.0);
                if self.discovery.results.is_empty() {
                    ui.add_space(14.0);
                    ui.vertical_centered(|ui| {
                        ui.label(
                            RichText::new("No responses yet. Run a scan to look for devices on the local network.")
                                .size(12.5)
                                .color(p.faint),
                        );
                    });
                    ui.add_space(14.0);
                    return;
                }
                egui::Grid::new("discovery_results")
                    .num_columns(5)
                    .striped(true)
                    .min_col_width(60.0)
                    .spacing([18.0, 8.0])
                    .show(ui, |ui| {
                        for header in ["Address", "Bytes", "RTT", "Payload", ""] {
                            ui.label(RichText::new(header).font(strong_font(12.0)).color(p.muted));
                        }
                        ui.end_row();
                        for response in &self.discovery.results {
                            ui.label(
                                RichText::new(&response.addr)
                                    .monospace()
                                    .color(p.text_strong),
                            );
                            ui.label(response.bytes.to_string());
                            ui.label(format!("{} ms", response.rtt_ms));
                            ui.add(
                                egui::Label::new(
                                    RichText::new(&response.ascii).monospace().color(p.rx),
                                )
                                .truncate(),
                            )
                            .on_hover_text(&response.hex);
                            if widgets::ghost(ui, &p, "Open UDP session").clicked() {
                                use_target = Some(response.addr.clone());
                            }
                            ui.end_row();
                        }
                    });
            });
        });
        if scan {
            self.start_scan();
        }
        if let Some(addr) = use_target {
            if let Ok(sock) = addr.parse::<std::net::SocketAddr>() {
                let cfg = UdpConfig {
                    remote_host: Some(sock.ip().to_string()),
                    remote_port: Some(sock.port()),
                    ..UdpConfig::default()
                };
                self.forms.open_from(&SessionConfig::Udp(cfg));
            }
        }
    }

    // ------------------------------------------------------------------ MCP

    pub(crate) fn start_mcp(&mut self) {
        let addr = match validate::socket_addr(&self.mcp.bind) {
            Ok(addr) => addr,
            Err(err) => {
                self.mcp.error = Some(err);
                return;
            }
        };
        let listener = match self.rt.block_on(tokio::net::TcpListener::bind(addr)) {
            Ok(listener) => listener,
            Err(err) => {
                self.mcp.error = Some(format!("Could not bind {addr}: {err}."));
                return;
            }
        };
        let server = Arc::new(McpServer::new(self.manager.clone(), self.config.clone()));
        let (tx, rx) = tokio::sync::oneshot::channel();
        let task = self.rt.spawn(async move {
            let shutdown = async {
                let _ = rx.await;
            };
            if let Err(err) = termbridge_mcp::serve_http_listener(server, listener, shutdown).await
            {
                tracing::error!("MCP HTTP server stopped: {err}");
            }
        });
        self.mcp.shutdown = Some(tx);
        self.mcp.task = Some(task);
        self.mcp.running = true;
        self.mcp.started = Some(Instant::now());
        self.mcp.error = None;
        let bind = self.mcp.bind.trim().to_string();
        self.config.update(|cfg| cfg.mcp.bind = bind);
        let _ = self.config.save();
        self.toast_ok(format!("MCP server listening on http://{addr}/mcp"));
    }

    fn stop_mcp(&mut self) {
        if let Some(tx) = self.mcp.shutdown.take() {
            let _ = tx.send(());
        }
        if let Some(task) = self.mcp.task.take() {
            task.abort();
        }
        self.mcp.running = false;
        self.mcp.started = None;
        self.toast_ok("MCP server stopped");
    }

    pub(crate) fn mcp_page(&mut self, ui: &mut egui::Ui) {
        let p = self.palette;
        let bind_result = validate::socket_addr(&self.mcp.bind);
        let mut start = false;
        let mut stop = false;
        let exe = std::env::current_exe()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|_| "termbridge".to_string());
        let stdio = serde_json::to_string_pretty(&serde_json::json!({
            "mcpServers": { "termbridge": { "command": exe, "args": ["mcp", "--transport", "stdio"] } }
        }))
        .unwrap_or_default();
        let http = serde_json::to_string_pretty(&serde_json::json!({
            "mcpServers": { "termbridge": { "type": "http", "url": format!("http://{}/mcp", self.mcp.bind.trim()) } }
        }))
        .unwrap_or_default();
        let claude_cmd = format!("claude mcp add termbridge -- \"{exe}\" mcp --transport stdio");
        let tools = termbridge_mcp::tools::definitions();

        page(ui, 980.0, |ui, width| {
            page_header(
                ui,
                &p,
                "MCP server",
                "Let AI agents list ports, open sessions, send data and read responses. The HTTP endpoint shares the sessions you see in this window.",
            );
            widgets::card(ui, &p, |ui| {
                ui.set_width(width - 30.0);
                ui.horizontal(|ui| {
                    widgets::section_title(ui, &p, "HTTP endpoint");
                    if self.mcp.running {
                        widgets::status_pill(ui, &p, "Running", p.ok);
                    } else {
                        widgets::status_pill(ui, &p, "Stopped", p.faint);
                    }
                });
                ui.add_space(6.0);
                form_row(ui, &p, "Bind address", |ui| {
                    ui.add_enabled(
                        !self.mcp.running,
                        egui::TextEdit::singleline(&mut self.mcp.bind)
                            .desired_width(200.0)
                            .font(egui::TextStyle::Monospace),
                    );
                    if self.mcp.running {
                        if widgets::danger(ui, &p, "Stop server").clicked() {
                            stop = true;
                        }
                    } else if ui
                        .add_enabled(
                            bind_result.is_ok(),
                            widgets::primary_button(&p, "Start server"),
                        )
                        .clicked()
                    {
                        start = true;
                    }
                });
                match &bind_result {
                    Err(e) => issue_line(ui, &p, e, false),
                    Ok(addr) if !addr.ip().is_loopback() => issue_line(
                        ui,
                        &p,
                        "This address is reachable from other machines and the endpoint has no authentication.",
                        true,
                    ),
                    _ => {}
                }
                if let Some(err) = &self.mcp.error {
                    issue_line(ui, &p, err, false);
                }
                if self.mcp.running {
                    form_row(ui, &p, "Endpoint", |ui| {
                        let url = format!("http://{}/mcp", self.mcp.bind.trim());
                        ui.label(RichText::new(&url).monospace().color(p.accent));
                        if widgets::ghost(ui, &p, "Copy").clicked() {
                            ui.ctx().copy_text(url);
                        }
                        if let Some(started) = self.mcp.started {
                            ui.label(
                                RichText::new(format!(
                                    "up {}",
                                    widgets::human_duration(started.elapsed().as_secs())
                                ))
                                .size(12.0)
                                .color(p.faint),
                            );
                        }
                    });
                }
                form_row(ui, &p, "On launch", |ui| {
                    if ui
                        .checkbox(
                            &mut self.mcp.autostart,
                            "Start the HTTP endpoint when the app opens",
                        )
                        .changed()
                    {
                        let v = self.mcp.autostart;
                        self.config.update(|c| c.mcp.autostart = v);
                        let _ = self.config.save();
                    }
                });
            });
            ui.add_space(14.0);
            widgets::card(ui, &p, |ui| {
                ui.set_width(width - 30.0);
                widgets::section_title(ui, &p, "Connect a client");
                widgets::hint(
                    ui,
                    &p,
                    "stdio runs a separate headless process per client. HTTP shares this window's sessions.",
                );
                ui.add_space(8.0);
                for (title, body) in [
                    ("Claude Code", claude_cmd.as_str()),
                    ("Claude Desktop / Cursor (stdio)", stdio.as_str()),
                    ("HTTP clients", http.as_str()),
                ] {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(title).font(strong_font(13.0)).color(p.text));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if widgets::ghost(ui, &p, "Copy").clicked() {
                                ui.ctx().copy_text(body.to_string());
                            }
                        });
                    });
                    egui::Frame::new()
                        .fill(p.term_bg)
                        .stroke(egui::Stroke::new(1.0, p.border))
                        .corner_radius(egui::CornerRadius::same(8))
                        .inner_margin(egui::Margin::symmetric(12, 10))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.add(
                                egui::Label::new(
                                    RichText::new(body).monospace().size(12.0).color(p.text),
                                )
                                .wrap()
                                .selectable(true),
                            );
                        });
                    ui.add_space(6.0);
                }
            });
            ui.add_space(14.0);
            widgets::card(ui, &p, |ui| {
                ui.set_width(width - 30.0);
                ui.horizontal(|ui| {
                    widgets::section_title(ui, &p, "Tools");
                    widgets::badge(ui, &p, &tools.len().to_string(), p.accent);
                });
                ui.add_space(6.0);
                egui::Grid::new("mcp_tools")
                    .num_columns(2)
                    .striped(true)
                    .spacing([18.0, 7.0])
                    .show(ui, |ui| {
                        for tool in &tools {
                            let name = tool["name"].as_str().unwrap_or_default();
                            let desc = tool["description"].as_str().unwrap_or_default();
                            let desc = desc.split(". ").next().unwrap_or(desc);
                            ui.label(RichText::new(name).monospace().size(12.5).color(p.accent));
                            ui.label(RichText::new(desc).size(12.5).color(p.muted));
                            ui.end_row();
                        }
                    });
            });
        });
        if start {
            self.start_mcp();
        }
        if stop {
            self.stop_mcp();
        }
    }

    // ------------------------------------------------------------- settings

    pub(crate) fn settings_page(&mut self, ui: &mut egui::Ui) {
        let p = self.palette;
        let mut save = false;
        let mut revert = false;
        let mut apply_theme = None;
        let macro_issues: Vec<Option<String>> = self
            .settings
            .macros
            .iter()
            .map(|m| validate::macro_def(&m.name, &m.data, m.hex))
            .collect();
        let dir_issue = {
            let d = self.settings.log_dir.trim();
            (!d.is_empty() && !std::path::Path::new(d).is_dir())
                .then(|| "Folder does not exist.".to_string())
        };
        let valid = macro_issues.iter().all(Option::is_none) && dir_issue.is_none();
        let dirty = self.settings.dirty();

        page(ui, 980.0, |ui, width| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    page_header(ui, &p, "Settings", "Stored in the configuration file shown below and shared with the TUI and headless modes.");
                });
            });
            let s = &mut self.settings;
            widgets::card(ui, &p, |ui| {
                ui.set_width(width - 30.0);
                widgets::section_title(ui, &p, "Appearance and terminal");
                ui.add_space(6.0);
                form_row(ui, &p, "Theme", |ui| {
                    if widgets::segmented(
                        ui,
                        &p,
                        &mut s.theme,
                        &[(Theme::Dark, "Dark"), (Theme::Light, "Light")],
                    ) {
                        apply_theme = Some(s.theme);
                    }
                });
                form_row(ui, &p, "Timestamps", |ui| {
                    ui.checkbox(&mut s.show_timestamps, "Show in new sessions");
                });
                form_row(ui, &p, "Ring buffer", |ui| {
                    ui.add(
                        egui::DragValue::new(&mut s.buffer_kib)
                            .range((MIN_BUFFER / 1024) as u64..=(MAX_BUFFER / 1024) as u64)
                            .speed(64)
                            .suffix(" KiB"),
                    );
                    ui.label(
                        RichText::new("per session, applies after restart")
                            .size(12.0)
                            .color(p.faint),
                    );
                });
                form_row(ui, &p, "Log folder", |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut s.log_dir)
                            .desired_width(360.0)
                            .font(egui::TextStyle::Monospace)
                            .hint_text("Default: Documents"),
                    );
                });
                if let Some(msg) = &dir_issue {
                    issue_line(ui, &p, msg, false);
                }
            });
            ui.add_space(14.0);
            widgets::card(ui, &p, |ui| {
                ui.set_width(width - 30.0);
                ui.horizontal(|ui| {
                    widgets::section_title(ui, &p, "Macros");
                    widgets::badge(ui, &p, &format!("{}/12", s.macros.len()), p.accent);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_enabled(s.macros.len() < 12, egui::Button::new("Add macro"))
                            .clicked()
                        {
                            s.macros.push(MacroDraft {
                                name: format!("Macro {}", s.macros.len() + 1),
                                data: String::new(),
                                hex: false,
                            });
                        }
                    });
                });
                widgets::hint(ui, &p, "Bound to F1 to F12 in order. Text macros accept the same escapes as the composer.");
                ui.add_space(8.0);
                let mut remove = None;
                let mut swap = None;
                // Fixed column widths keep every row aligned.
                const KEY_W: f32 = 34.0;
                const NAME_W: f32 = 150.0;
                const HEX_W: f32 = 40.0;
                const TAIL_W: f32 = 150.0;
                let data_w =
                    (ui.available_width() - KEY_W - NAME_W - HEX_W - TAIL_W - 40.0).max(180.0);
                let header = |ui: &mut egui::Ui, text: &str, w: f32| {
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(w, 18.0), egui::Sense::hover());
                    ui.painter().text(
                        rect.left_center(),
                        egui::Align2::LEFT_CENTER,
                        text,
                        strong_font(12.0),
                        p.muted,
                    );
                };
                ui.horizontal(|ui| {
                    header(ui, "Key", KEY_W);
                    header(ui, "Name", NAME_W);
                    header(ui, "Data", data_w);
                    header(ui, "Hex", HEX_W);
                });
                let n = s.macros.len();
                for (i, m) in s.macros.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        let (rect, _) =
                            ui.allocate_exact_size(egui::vec2(KEY_W, 28.0), egui::Sense::hover());
                        ui.painter().text(
                            rect.left_center(),
                            egui::Align2::LEFT_CENTER,
                            format!("F{}", i + 1),
                            egui::FontId::monospace(13.0),
                            p.text_strong,
                        );
                        ui.add_sized(
                            [NAME_W, 28.0],
                            egui::TextEdit::singleline(&mut m.name).char_limit(24),
                        );
                        ui.add_sized(
                            [data_w, 28.0],
                            egui::TextEdit::singleline(&mut m.data)
                                .font(egui::TextStyle::Monospace),
                        );
                        ui.allocate_ui_with_layout(
                            egui::vec2(HEX_W, 28.0),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                ui.set_min_width(HEX_W);
                                ui.checkbox(&mut m.hex, "");
                            },
                        );
                        if ui
                            .add_enabled(
                                i > 0,
                                egui::Button::new("↑").min_size(egui::vec2(28.0, 26.0)),
                            )
                            .on_hover_text("Move up")
                            .clicked()
                        {
                            swap = Some((i, i - 1));
                        }
                        if ui
                            .add_enabled(
                                i + 1 < n,
                                egui::Button::new("↓").min_size(egui::vec2(28.0, 26.0)),
                            )
                            .on_hover_text("Move down")
                            .clicked()
                        {
                            swap = Some((i, i + 1));
                        }
                        if widgets::ghost(ui, &p, "Remove").clicked() {
                            remove = Some(i);
                        }
                    });
                    if let Some(Some(msg)) = macro_issues.get(i) {
                        ui.horizontal(|ui| {
                            ui.add_space(KEY_W + NAME_W + 16.0);
                            ui.label(RichText::new(msg).size(11.5).color(p.error));
                        });
                    }
                }
                if let Some((a, b)) = swap {
                    s.macros.swap(a, b);
                }
                if let Some(i) = remove {
                    s.macros.remove(i);
                }
            });
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(valid && dirty, widgets::primary_button(&p, "Save changes"))
                    .on_disabled_hover_text(if !valid {
                        "Fix the highlighted fields first"
                    } else {
                        "Nothing to save"
                    })
                    .clicked()
                {
                    save = true;
                }
                if ui
                    .add_enabled(dirty, egui::Button::new("Discard"))
                    .clicked()
                {
                    revert = true;
                }
                if dirty {
                    widgets::status_pill(ui, &p, "Unsaved changes", p.warn);
                }
            });
            ui.add_space(14.0);
            widgets::card(ui, &p, |ui| {
                ui.set_width(width - 30.0);
                widgets::section_title(ui, &p, "About");
                ui.add_space(4.0);
                form_row(ui, &p, "Version", |ui| {
                    ui.label(format!(
                        "Termbridge {}  ·  {} {}",
                        env!("CARGO_PKG_VERSION"),
                        std::env::consts::OS,
                        std::env::consts::ARCH
                    ));
                });
                form_row(ui, &p, "Config file", |ui| {
                    let path = self.config.path().display().to_string();
                    ui.label(RichText::new(&path).monospace().size(12.0));
                    if widgets::ghost(ui, &p, "Copy").clicked() {
                        ui.ctx().copy_text(path);
                    }
                });
            });
        });

        if let Some(theme) = apply_theme {
            let dark = matches!(theme, Theme::Dark);
            self.set_dark(ui.ctx(), dark);
            if let Some(saved) = self.settings.saved.as_mut() {
                saved.0 = theme;
            }
        }
        if revert {
            let cfg = self.config.get();
            self.settings = SettingsState::from_config(&cfg);
        }
        if save {
            let s = &self.settings;
            let theme = s.theme;
            let show = s.show_timestamps;
            let buffer = (s.buffer_kib as usize * 1024).clamp(MIN_BUFFER, MAX_BUFFER);
            let log_dir = {
                let d = s.log_dir.trim();
                (!d.is_empty()).then(|| std::path::PathBuf::from(d))
            };
            let macros: Vec<MacroDef> = s
                .macros
                .iter()
                .map(|m| MacroDef::new(m.name.trim(), &m.data, m.hex))
                .collect();
            self.config.update(|c| {
                c.theme = theme;
                c.show_timestamps = show;
                c.buffer_bytes = buffer;
                c.log_dir = log_dir;
                c.macros = macros;
            });
            match self.config.save() {
                Ok(()) => {
                    self.settings.saved = Some(self.settings.snapshot());
                    self.toast_ok("Settings saved");
                }
                Err(err) => self.toast_err(format!("Could not save settings: {err}")),
            }
        }
    }
}

// -------------------------------------------------------------------- tools

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum InputKind {
    Text,
    Hex,
    Dec,
    Base64,
}

pub struct ToolsState {
    input: String,
    kind: InputKind,
}

impl Default for ToolsState {
    fn default() -> Self {
        Self {
            input: "01 03 00 00 00 0A".into(),
            kind: InputKind::Hex,
        }
    }
}

fn decode_tool_input(input: &str, kind: InputKind) -> Result<Vec<u8>, String> {
    use base64::Engine;
    match kind {
        InputKind::Text => {
            codec::parse_escapes(input).map_err(|e| validate::sentence(&e.to_string()))
        }
        InputKind::Hex => codec::parse_hex(input).map_err(|e| validate::sentence(&e.to_string())),
        InputKind::Dec => input
            .split(|c: char| c.is_whitespace() || c == ',' || c == ';')
            .filter(|t| !t.is_empty())
            .map(|t| {
                t.parse::<u8>()
                    .map_err(|_| format!("'{t}' is not a byte value (0 to 255)."))
            })
            .collect(),
        InputKind::Base64 => base64::engine::general_purpose::STANDARD
            .decode(input.trim())
            .map_err(|e| format!("Invalid Base64: {e}.")),
    }
}

impl App {
    pub(crate) fn tools_page(&mut self, ui: &mut egui::Ui) {
        use base64::Engine;
        use termbridge_core::checksum::{self, ChecksumKind};
        let p = self.palette;
        let decoded = decode_tool_input(&self.tools.input, self.tools.kind);
        let mut copied: Option<String> = None;
        page(ui, 980.0, |ui, width| {
            page_header(
                ui,
                &p,
                "Tools",
                "Convert data between representations and compute protocol checksums.",
            );
            widgets::card(ui, &p, |ui| {
                ui.set_width(width - 30.0);
                widgets::section_title(ui, &p, "Input");
                ui.add_space(6.0);
                form_row(ui, &p, "Interpret as", |ui| {
                    widgets::segmented(
                        ui,
                        &p,
                        &mut self.tools.kind,
                        &[
                            (InputKind::Text, "Text"),
                            (InputKind::Hex, "Hex"),
                            (InputKind::Dec, "Decimal"),
                            (InputKind::Base64, "Base64"),
                        ],
                    );
                });
                form_row(ui, &p, "Data", |ui| {
                    ui.add(
                        egui::TextEdit::multiline(&mut self.tools.input)
                            .desired_rows(3)
                            .desired_width(width - 30.0 - 150.0)
                            .font(egui::TextStyle::Monospace),
                    );
                });
                match &decoded {
                    Ok(bytes) => issue_line_ok(ui, &p, &format!("{} bytes", bytes.len())),
                    Err(e) => issue_line(ui, &p, e, false),
                }
            });
            let Ok(bytes) = decoded.as_ref() else { return };
            ui.add_space(14.0);
            widgets::card(ui, &p, |ui| {
                ui.set_width(width - 30.0);
                widgets::section_title(ui, &p, "Representations");
                ui.add_space(6.0);
                let binary: String = bytes
                    .iter()
                    .map(|b| format!("{b:08b}"))
                    .collect::<Vec<_>>()
                    .join(" ");
                let rows = [
                    ("Text", codec::escape_bytes(bytes)),
                    ("Hex", codec::to_hex(bytes)),
                    (
                        "Hex (C array)",
                        bytes
                            .iter()
                            .map(|b| format!("0x{b:02X}"))
                            .collect::<Vec<_>>()
                            .join(", "),
                    ),
                    ("Decimal", codec::to_dec(bytes)),
                    ("Binary", binary),
                    (
                        "Base64",
                        base64::engine::general_purpose::STANDARD.encode(bytes),
                    ),
                ];
                for (label, value) in rows {
                    value_row(ui, &p, label, &value, &mut copied);
                }
            });
            ui.add_space(14.0);
            widgets::card(ui, &p, |ui| {
                ui.set_width(width - 30.0);
                widgets::section_title(ui, &p, "Checksums");
                widgets::hint(ui, &p, "Shown in the byte order they are appended to a frame. Modbus RTU CRC is low byte first.");
                ui.add_space(6.0);
                for kind in &ChecksumKind::ALL[1..] {
                    let sum = checksum::compute(*kind, bytes);
                    value_row(ui, &p, kind.label(), &codec::to_hex(&sum), &mut copied);
                }
                ui.add_space(4.0);
                let frame = checksum::append(ChecksumKind::Crc16Modbus, bytes);
                value_row(
                    ui,
                    &p,
                    "Frame + Modbus CRC",
                    &codec::to_hex(&frame),
                    &mut copied,
                );
            });
        });
        if let Some(text) = copied {
            ui.ctx().copy_text(text);
            self.toast_ok("Copied to clipboard");
        }
    }
}

fn issue_line_ok(ui: &mut egui::Ui, p: &crate::theme::Palette, msg: &str) {
    ui.horizontal(|ui| {
        ui.add_space(138.0);
        ui.label(RichText::new(msg).size(12.0).color(p.faint));
    });
}

fn value_row(
    ui: &mut egui::Ui,
    p: &crate::theme::Palette,
    label: &str,
    value: &str,
    copied: &mut Option<String>,
) {
    ui.horizontal(|ui| {
        widgets::field_label(ui, p, label, 130.0);
        let w = (ui.available_width() - 70.0).max(120.0);
        ui.allocate_ui_with_layout(
            egui::vec2(w, 22.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.set_min_width(w);
                ui.add(
                    egui::Label::new(
                        RichText::new(value)
                            .monospace()
                            .size(12.5)
                            .color(p.text_strong),
                    )
                    .truncate()
                    .selectable(true),
                )
                .on_hover_text(value);
            },
        );
        if widgets::ghost(ui, p, "Copy").clicked() {
            *copied = Some(value.to_string());
        }
    });
}
