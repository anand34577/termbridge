//! Session workspace: header, metrics, line controls, terminal and composer.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui::{self, CornerRadius, Margin, RichText, Stroke, Vec2};
use serial_mcp_core::checksum::{self, ChecksumKind};
use serial_mcp_core::codec::{self, ViewFormat};
use serial_mcp_core::config::{AppConfig, MacroDef};
use serial_mcp_core::logging::LogFormat;
use serial_mcp_core::responder::{RuleDef, RuleStatus, MAX_RULES};
use serial_mcp_core::session::{Parity, SessionConfig};
use serial_mcp_core::types::SessionKind;
use serial_mcp_core::{ControlOp, SessionInfo, SessionManager};

use crate::forms::{flow_label, kind_short};
use crate::terminal::{self, LineFilter, TerminalBuffer};
use crate::theme::Palette;
use crate::widgets::{self, strong_font};
use serial_mcp_core::validate;

/// Largest file accepted by "Send file".
pub const MAX_SEND_FILE: u64 = 16 * 1024 * 1024;
const HISTORY_LEN: usize = 100;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Eol {
    None,
    Cr,
    Lf,
    CrLf,
}

impl Eol {
    fn bytes(self) -> &'static [u8] {
        match self {
            Eol::None => b"",
            Eol::Cr => b"\r",
            Eol::Lf => b"\n",
            Eol::CrLf => b"\r\n",
        }
    }
}

/// Things the session view asks the app to do.
#[derive(Default)]
pub struct Action {
    pub close: bool,
    pub ping: Option<(String, u16)>,
    pub toast_ok: Option<String>,
    pub toast_err: Option<String>,
}

pub struct SessionView {
    pub id: String,
    pub label: String,
    pub kind: SessionKind,
    pub term: TerminalBuffer,
    pub info: Option<SessionInfo>,
    pub ping: Option<String>,
    filter: LineFilter,
    follow: bool,
    wrap: bool,
    input: String,
    hex_send: bool,
    eol: Eol,
    history: Vec<String>,
    history_pos: Option<usize>,
    repeat: bool,
    repeat_ms: u64,
    last_repeat: Instant,
    log_path: String,
    log_error: Option<String>,
    log_format: LogFormat,
    checksum: ChecksumKind,
    rules: Vec<RuleDef>,
    rules_live: Vec<RuleStatus>,
    rules_error: Option<String>,
    /// True once the user edited the rule draft, so live updates stop
    /// overwriting it (and an emptied list can be applied).
    rules_dirty: bool,
    file_path: String,
    break_on: bool,
    echo_on: bool,
    /// Folder used by "Save" and the default recording path.
    export_dir: PathBuf,
    last_refresh: Instant,
    rate_sample: Option<(Instant, u64, u64)>,
    rx_rate: f64,
    tx_rate: f64,
    rx_hist: std::collections::VecDeque<f32>,
    tx_hist: std::collections::VecDeque<f32>,
    rx_flash: Option<Instant>,
    tx_flash: Option<Instant>,
    visible: Vec<usize>,
    visible_key: (usize, u64, String, bool, bool, bool),
}

impl SessionView {
    pub fn new(info: SessionInfo, config: &AppConfig) -> Self {
        let echo_on = match &info.config {
            SessionConfig::TcpServer(cfg) => cfg.echo,
            SessionConfig::Udp(cfg) => cfg.echo,
            _ => false,
        };
        let log_path = default_log_path(config, &info.label);
        Self {
            id: info.id.clone(),
            label: info.label.clone(),
            kind: info.kind,
            term: TerminalBuffer::new(ViewFormat::Ascii, config.show_timestamps),
            info: Some(info),
            ping: None,
            filter: LineFilter::default(),
            follow: true,
            wrap: false,
            input: String::new(),
            hex_send: false,
            eol: Eol::None,
            history: Vec::new(),
            history_pos: None,
            repeat: false,
            repeat_ms: 1000,
            last_repeat: Instant::now(),
            log_path,
            log_error: None,
            log_format: LogFormat::Text,
            checksum: ChecksumKind::None,
            rules: Vec::new(),
            rules_live: Vec::new(),
            rules_error: None,
            rules_dirty: false,
            file_path: String::new(),
            break_on: false,
            echo_on,
            export_dir: base_dir(Some(config)),
            last_refresh: Instant::now() - Duration::from_secs(1),
            rate_sample: None,
            rx_rate: 0.0,
            tx_rate: 0.0,
            rx_hist: std::collections::VecDeque::from(vec![0.0; 60]),
            tx_hist: std::collections::VecDeque::from(vec![0.0; 60]),
            rx_flash: None,
            tx_flash: None,
            visible: Vec::new(),
            visible_key: (0, 0, String::new(), false, false, false),
        }
    }

    pub fn connected(&self) -> bool {
        self.info.as_ref().is_some_and(|i| i.connected)
    }

    fn recording(&self) -> Option<&serial_mcp_core::session::RecordingInfo> {
        self.info.as_ref().and_then(|i| i.recording.as_ref())
    }

    pub fn has_error(&self) -> bool {
        self.info.as_ref().is_some_and(|i| i.error.is_some())
    }

    /// Pull new traffic and refresh counters. Called every frame for the
    /// selected session and periodically for the others.
    pub fn poll(&mut self, manager: &SessionManager, selected: bool) {
        if selected {
            match manager.read_since(&self.id, self.term.last_seq, 512 * 1024) {
                Ok(batch) if !batch.is_empty() => self.term.ingest(&batch),
                Ok(batch) => self.term.last_seq = batch.next_seq,
                Err(_) => {}
            }
            self.term.flush_idle();
        }
        let interval = if selected { 150 } else { 600 };
        if self.last_refresh.elapsed() >= Duration::from_millis(interval) {
            self.last_refresh = Instant::now();
            if let Ok(info) = manager.info(&self.id) {
                let now = Instant::now();
                if let Some((t, rx, tx)) = self.rate_sample {
                    let dt = now.duration_since(t).as_secs_f64().max(0.001);
                    let rx_now = (info.rx_bytes.saturating_sub(rx)) as f64 / dt;
                    let tx_now = (info.tx_bytes.saturating_sub(tx)) as f64 / dt;
                    self.rx_rate = self.rx_rate * 0.5 + rx_now * 0.5;
                    self.tx_rate = self.tx_rate * 0.5 + tx_now * 0.5;
                    if info.rx_bytes > rx {
                        self.rx_flash = Some(now);
                    }
                    if info.tx_bytes > tx {
                        self.tx_flash = Some(now);
                    }
                    self.rx_hist.push_back(self.rx_rate as f32);
                    self.tx_hist.push_back(self.tx_rate as f32);
                    while self.rx_hist.len() > 60 {
                        self.rx_hist.pop_front();
                    }
                    while self.tx_hist.len() > 60 {
                        self.tx_hist.pop_front();
                    }
                }
                self.rate_sample = Some((now, info.rx_bytes, info.tx_bytes));
                self.info = Some(info);
                if let Ok(live) = manager.auto_responses(&self.id) {
                    self.rules_live = live;
                }
            }
        }
        if self.repeat
            && self.connected()
            && self.last_repeat.elapsed() >= Duration::from_millis(self.repeat_ms.max(10))
        {
            self.last_repeat = Instant::now();
            if let Ok(bytes) = self.encoded() {
                if manager.write(&self.id, &bytes).is_err() {
                    self.repeat = false;
                }
            } else {
                self.repeat = false;
            }
        }
    }

    pub fn wants_fast_repaint(&self) -> bool {
        self.repeat
    }

    fn encoded(&self) -> Result<Vec<u8>, String> {
        let bytes = validate::payload(&self.input, self.hex_send)?;
        let mut bytes = checksum::append(self.checksum, &bytes);
        bytes.extend_from_slice(self.eol.bytes());
        Ok(bytes)
    }

    fn send(&mut self, manager: &SessionManager, action: &mut Action) {
        match self.encoded() {
            Ok(bytes) => match manager.write(&self.id, &bytes) {
                Ok(_) => {
                    let entry = self.input.clone();
                    self.history.retain(|h| h != &entry);
                    self.history.push(entry);
                    if self.history.len() > HISTORY_LEN {
                        self.history.remove(0);
                    }
                    self.history_pos = None;
                    self.input.clear();
                }
                Err(err) => action.toast_err = Some(validate::sentence(&err.to_string())),
            },
            Err(err) => action.toast_err = Some(err),
        }
    }

    pub fn send_macro(&self, manager: &SessionManager, mac: &MacroDef) -> Result<(), String> {
        let bytes = codec::parse_input(&mac.data, mac.hex)
            .map_err(|e| format!("Macro {}: {}", mac.name, validate::sentence(&e.to_string())))?;
        manager
            .write(&self.id, &bytes)
            .map(|_| ())
            .map_err(|e| validate::sentence(&e.to_string()))
    }

    pub fn set_file_path(&mut self, path: String) {
        self.file_path = path;
    }

    fn send_file(&mut self, manager: &SessionManager) -> Result<String, String> {
        let path = PathBuf::from(self.file_path.trim());
        if self.file_path.trim().is_empty() {
            return Err("Enter a file path, or drop a file onto the window.".into());
        }
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
                widgets::human_count(meta.len()),
                widgets::human_count(MAX_SEND_FILE)
            ));
        }
        let data = std::fs::read(&path).map_err(|e| format!("Cannot read file: {e}."))?;
        for chunk in data.chunks(1024) {
            manager
                .write(&self.id, chunk)
                .map_err(|e| validate::sentence(&e.to_string()))?;
        }
        Ok(format!(
            "Queued {} from {}",
            widgets::human_count(data.len() as u64),
            path.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default()
        ))
    }

    fn refresh_visible(&mut self) {
        let key = (
            self.term.lines.len(),
            self.term.trimmed + self.term.last_seq,
            self.filter.query.clone(),
            self.filter.hide_rx,
            self.filter.hide_tx,
            self.filter.hide_info,
        );
        if key == self.visible_key {
            return;
        }
        self.visible_key = key;
        self.visible = self
            .term
            .lines
            .iter()
            .enumerate()
            .filter(|(_, l)| self.filter.matches(l))
            .map(|(i, _)| i)
            .collect();
    }

    pub fn clear(&mut self, manager: &SessionManager) {
        let _ = manager.clear_log(&self.id);
        self.term.clear_view();
    }

    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        manager: &SessionManager,
        p: &Palette,
        macros: &[MacroDef],
    ) -> Action {
        let mut action = Action::default();
        let info = self.info.clone();
        // Short windows get a one-line metric strip so the terminal keeps room.
        let compact = ui.available_height() < 640.0;

        egui::Panel::top(("session_top", &self.id))
            .frame(egui::Frame::new().inner_margin(Margin {
                left: 0,
                right: 0,
                top: 0,
                bottom: 10,
            }))
            .show_separator_line(false)
            .resizable(false)
            .show(ui, |ui| {
                self.header(ui, p, info.as_ref(), &mut action, compact);
                if compact {
                    ui.add_space(8.0);
                } else {
                    ui.add_space(12.0);
                    self.metrics(ui, p, info.as_ref());
                    ui.add_space(10.0);
                }
                self.controls(ui, manager, p, info.as_ref(), &mut action);
            });

        egui::Panel::bottom(("session_bottom", &self.id))
            .frame(egui::Frame::new().inner_margin(Margin {
                left: 0,
                right: 0,
                top: 10,
                bottom: 0,
            }))
            .show_separator_line(false)
            .resizable(false)
            .show(ui, |ui| {
                self.composer(ui, manager, p, macros, &mut action);
            });

        egui::CentralPanel::default()
            .frame(egui::Frame::new())
            .show(ui, |ui| {
                self.toolbar(ui, manager, p, &mut action);
                ui.add_space(8.0);
                self.refresh_visible();
                terminal::show(ui, &self.term, &self.visible, p, self.follow, self.wrap);
            });

        action
    }

    fn header(
        &mut self,
        ui: &mut egui::Ui,
        p: &Palette,
        info: Option<&SessionInfo>,
        action: &mut Action,
        compact: bool,
    ) {
        ui.horizontal(|ui| {
            widgets::badge(ui, p, kind_short(self.kind), p.kind_color(self.kind));
            let title = ui.label(
                RichText::new(&self.label)
                    .font(strong_font(19.0))
                    .color(p.text_strong),
            );
            if compact {
                if let Some(info) = info {
                    title.on_hover_text(describe(&info.config, info));
                }
            }
            let (text, color) = status(info, self.kind, p);
            let anim = match text {
                "Connecting" | "Reconnecting" => widgets::PillAnim::Busy,
                "Connected" | "Listening" | "Bound" => widgets::PillAnim::None,
                _ => widgets::PillAnim::None,
            };
            widgets::status_pill_anim(ui, p, text, color, anim);
            ui.add_space(6.0);
            let lit =
                |t: Option<Instant>| t.is_some_and(|t| t.elapsed() < Duration::from_millis(180));
            widgets::activity_led(ui, p, "RX", p.rx, lit(self.rx_flash));
            widgets::activity_led(ui, p, "TX", p.tx, lit(self.tx_flash));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if widgets::danger(ui, p, "Close session")
                    .on_hover_text("Ctrl+W")
                    .clicked()
                {
                    action.close = true;
                }
            });
        });
        if let Some(info) = info {
            ui.horizontal(|ui| {
                ui.add_space(2.0);
                if compact {
                    self.metrics_inline(ui, p, info);
                } else {
                    ui.label(
                        RichText::new(describe(&info.config, info))
                            .size(12.5)
                            .color(p.muted),
                    );
                }
                if let Some(err) = &info.error {
                    ui.add(
                        egui::Label::new(
                            RichText::new(validate::sentence(err))
                                .size(12.5)
                                .color(p.error),
                        )
                        .truncate(),
                    );
                }
            });
        }
    }

    fn metrics(&self, ui: &mut egui::Ui, p: &Palette, info: Option<&SessionInfo>) {
        let Some(info) = info else { return };
        let gap = 10.0;
        let count = 4.0;
        let width = ((ui.available_width() - gap * (count - 1.0)) / count - 1.0).max(120.0);
        let uptime = (chrono::Utc::now() - info.created_at).num_seconds().max(0) as u64;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            let rx_hist: Vec<f32> = self.rx_hist.iter().copied().collect();
            let tx_hist: Vec<f32> = self.tx_hist.iter().copied().collect();
            widgets::stat_tile_spark(
                ui,
                p,
                width,
                "Received",
                &widgets::human_count(info.rx_bytes),
                &widgets::human_rate(self.rx_rate),
                p.rx,
                Some(&rx_hist),
            );
            widgets::stat_tile_spark(
                ui,
                p,
                width,
                "Sent",
                &widgets::human_count(info.tx_bytes),
                &widgets::human_rate(self.tx_rate),
                p.tx,
                Some(&tx_hist),
            );
            widgets::stat_tile(
                ui,
                p,
                width,
                "Packets in / out",
                &format!("{} / {}", info.rx_packets, info.tx_packets),
                &format!(
                    "buffer {}",
                    serial_mcp_core::util::human_bytes(info.log_bytes)
                ),
                p.accent,
            );
            let (caption, value) = match info.kind {
                SessionKind::TcpServer => ("Clients", info.clients.len().to_string()),
                _ => ("Uptime", widgets::human_duration(uptime)),
            };
            widgets::stat_tile(
                ui,
                p,
                width,
                caption,
                &value,
                &format!(
                    "opened {}",
                    info.created_at
                        .with_timezone(&chrono::Local)
                        .format("%H:%M:%S")
                ),
                p.nvt,
            );
        });
    }

    /// One-line metrics shown under the title in short windows.
    fn metrics_inline(&self, ui: &mut egui::Ui, p: &Palette, info: &SessionInfo) {
        ui.spacing_mut().item_spacing.x = 5.0;
        let uptime = (chrono::Utc::now() - info.created_at).num_seconds().max(0) as u64;
        let item = |ui: &mut egui::Ui, caption: &str, value: String, color: egui::Color32| {
            widgets::dot(ui, color, false);
            ui.label(RichText::new(caption).size(12.0).color(p.muted));
            ui.label(
                RichText::new(value)
                    .font(strong_font(12.5))
                    .color(p.text_strong),
            );
            ui.add_space(12.0);
        };
        item(
            ui,
            "RX",
            format!(
                "{}  {}",
                widgets::human_count(info.rx_bytes),
                widgets::human_rate(self.rx_rate)
            ),
            p.rx,
        );
        item(
            ui,
            "TX",
            format!(
                "{}  {}",
                widgets::human_count(info.tx_bytes),
                widgets::human_rate(self.tx_rate)
            ),
            p.tx,
        );
        item(
            ui,
            "Packets",
            format!("{} / {}", info.rx_packets, info.tx_packets),
            p.accent,
        );
        match info.kind {
            SessionKind::TcpServer => item(ui, "Clients", info.clients.len().to_string(), p.nvt),
            _ => item(ui, "Up", widgets::human_duration(uptime), p.nvt),
        }
    }

    fn controls(
        &mut self,
        ui: &mut egui::Ui,
        manager: &SessionManager,
        p: &Palette,
        info: Option<&SessionInfo>,
        action: &mut Action,
    ) {
        widgets::bar(ui, p, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                match self.kind {
                    SessionKind::Serial => {
                        let modem = info.map(|i| i.modem).unwrap_or_default();
                        ui.label(RichText::new("Outputs").size(12.0).color(p.muted));
                        let mut rts = modem.rts;
                        if widgets::toggle_chip(ui, p, "RTS", &mut rts).changed() {
                            if let Err(e) = manager.set_control(&self.id, ControlOp::Rts(rts)) {
                                action.toast_err = Some(validate::sentence(&e.to_string()));
                            }
                        }
                        let mut dtr = modem.dtr;
                        if widgets::toggle_chip(ui, p, "DTR", &mut dtr).changed() {
                            if let Err(e) = manager.set_control(&self.id, ControlOp::Dtr(dtr)) {
                                action.toast_err = Some(validate::sentence(&e.to_string()));
                            }
                        }
                        if widgets::toggle_chip(ui, p, "Break", &mut self.break_on).changed() {
                            if let Err(e) =
                                manager.set_control(&self.id, ControlOp::Break(self.break_on))
                            {
                                action.toast_err = Some(validate::sentence(&e.to_string()));
                                self.break_on = false;
                            }
                        }
                        widgets::vdivider(ui, p);
                        ui.label(RichText::new("Inputs").size(12.0).color(p.muted));
                        widgets::led(ui, p, "CTS", modem.cts);
                        widgets::led(ui, p, "DSR", modem.dsr);
                        widgets::led(ui, p, "RI", modem.ri);
                        widgets::led(ui, p, "CD", modem.cd);
                    }
                    SessionKind::TcpServer | SessionKind::Udp => {
                        if widgets::toggle_chip(ui, p, "Echo", &mut self.echo_on).changed() {
                            if let Err(e) =
                                manager.set_control(&self.id, ControlOp::Echo(self.echo_on))
                            {
                                action.toast_err = Some(validate::sentence(&e.to_string()));
                            }
                        }
                        widgets::vdivider(ui, p);
                        if let Some(info) = info {
                            if info.kind == SessionKind::TcpServer {
                                if info.clients.is_empty() {
                                    ui.label(
                                        RichText::new("No clients connected")
                                            .size(12.5)
                                            .color(p.faint),
                                    );
                                }
                                for client in &info.clients {
                                    widgets::led(
                                        ui,
                                        p,
                                        &format!("#{} {}", client.id, client.peer),
                                        true,
                                    );
                                }
                            } else {
                                ui.label(RichText::new("Last peer").size(12.0).color(p.muted));
                                ui.label(
                                    RichText::new(
                                        info.peer.clone().unwrap_or_else(|| "none".into()),
                                    )
                                    .monospace()
                                    .color(p.text),
                                );
                            }
                        }
                    }
                    SessionKind::TcpClient => {
                        if widgets::secondary(ui, p, "Measure latency").clicked() {
                            if let Some(SessionConfig::TcpClient(cfg)) = info.map(|i| &i.config) {
                                action.ping = Some((cfg.host.clone(), cfg.port));
                                self.ping = Some("measuring…".into());
                            }
                        }
                        if let Some(ping) = &self.ping {
                            ui.label(RichText::new(ping).monospace().color(p.muted));
                        }
                        if let Some(peer) = info.and_then(|i| i.peer.clone()) {
                            widgets::vdivider(ui, p);
                            ui.label(RichText::new("Peer").size(12.0).color(p.muted));
                            ui.label(RichText::new(peer).monospace());
                        }
                    }
                }

                // Recording and auto-reply, right-aligned.
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    self.recording_control(ui, manager, p, action);
                    self.auto_reply_control(ui, manager, p, action);
                });
            });
        });
    }

    fn recording_control(
        &mut self,
        ui: &mut egui::Ui,
        manager: &SessionManager,
        p: &Palette,
        action: &mut Action,
    ) {
        if let Some(rec) = self.recording().cloned() {
            if widgets::secondary(ui, p, "Stop")
                .on_hover_text(&rec.path)
                .clicked()
            {
                let _ = manager.set_logging(&self.id, None, false);
                action.toast_ok = Some(format!("Saved recording to {}", rec.path));
            }
            widgets::status_pill_anim(
                ui,
                p,
                &format!(
                    "REC {}  {}",
                    rec.format.as_str().to_uppercase(),
                    widgets::human_count(rec.bytes)
                ),
                p.error,
                widgets::PillAnim::Pulse,
            )
            .on_hover_text(&rec.path);
            return;
        }
        ui.menu_button(RichText::new("Record").size(13.0), |ui| {
            ui.set_min_width(400.0);
            ui.label(RichText::new("Record traffic to a file").font(strong_font(13.5)));
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Format").size(12.5).color(p.muted));
                let before = self.log_format;
                widgets::segmented(
                    ui,
                    p,
                    &mut self.log_format,
                    &[(LogFormat::Text, "Text log"), (LogFormat::Csv, "CSV"), (LogFormat::Raw, "Raw RX bytes")],
                );
                if before != self.log_format {
                    // Keep the extension in step with the format.
                    let path = std::path::Path::new(self.log_path.trim());
                    if path.extension().is_some_and(|e| e == before.extension()) {
                        self.log_path = path
                            .with_extension(self.log_format.extension())
                            .display()
                            .to_string();
                    }
                }
            });
            ui.add(
                egui::TextEdit::singleline(&mut self.log_path)
                    .desired_width(390.0)
                    .font(egui::TextStyle::Monospace),
            );
            if let Some(err) = &self.log_error {
                ui.label(RichText::new(err).size(12.0).color(p.error));
            }
            let help = match self.log_format {
                LogFormat::Text => "One line per chunk with timestamp, direction and escaped data.",
                LogFormat::Csv => "timestamp, direction, length, hex, text. Opens in any spreadsheet.",
                LogFormat::Raw => "Received bytes exactly as they arrived, for firmware dumps and binary captures.",
            };
            ui.label(RichText::new(help).size(12.0).color(p.muted));
            ui.label(RichText::new("Existing files are appended to.").size(12.0).color(p.faint));
            if widgets::primary(ui, p, "Start recording").clicked() {
                match validate::log_path(&self.log_path) {
                    Ok(path) => match manager.set_logging_with(&self.id, Some(path.clone()), true, self.log_format) {
                        Ok(()) => {
                            self.log_error = None;
                            self.info = manager.info(&self.id).ok();
                            action.toast_ok = Some(format!("Recording to {}", path.display()));
                            ui.close();
                        }
                        Err(e) => self.log_error = Some(validate::sentence(&e.to_string())),
                    },
                    Err(e) => self.log_error = Some(e),
                }
            }
        });
    }

    fn auto_reply_control(
        &mut self,
        ui: &mut egui::Ui,
        manager: &SessionManager,
        p: &Palette,
        action: &mut Action,
    ) {
        let active = self.rules_live.iter().filter(|r| r.def.enabled).count();
        let label = if active > 0 {
            format!("Auto-reply ({active})")
        } else {
            "Auto-reply".to_string()
        };
        let text = RichText::new(label).size(13.0);
        let text = if active > 0 {
            text.color(p.accent)
        } else {
            text
        };
        ui.menu_button(text, |ui| {
            ui.set_min_width(620.0);
            ui.label(RichText::new("Auto-reply rules").font(strong_font(13.5)));
            ui.label(
                RichText::new("When received data contains the pattern, the reply is sent at once. Text fields accept escapes such as \\r and \\x06.")
                    .size(12.0)
                    .color(p.muted),
            );
            ui.add_space(6.0);
            if !self.rules_dirty {
                let live: Vec<RuleDef> = self.rules_live.iter().map(|r| r.def.clone()).collect();
                if self.rules != live {
                    self.rules = live;
                }
            }
            let before = self.rules.clone();
            let mut remove = None;
            for (i, rule) in self.rules.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    ui.checkbox(&mut rule.enabled, "");
                    ui.add(
                        egui::TextEdit::singleline(&mut rule.pattern)
                            .desired_width(190.0)
                            .font(egui::TextStyle::Monospace)
                            .hint_text("Pattern, e.g. AT\\r"),
                    );
                    ui.label(RichText::new("→").color(p.muted));
                    ui.add(
                        egui::TextEdit::singleline(&mut rule.reply)
                            .desired_width(190.0)
                            .font(egui::TextStyle::Monospace)
                            .hint_text("Reply, e.g. OK\\r\\n"),
                    );
                    ui.checkbox(&mut rule.hex, "Hex");
                    let hits = self
                        .rules_live
                        .get(i)
                        .filter(|l| l.def.pattern == rule.pattern)
                        .map(|l| l.hits)
                        .unwrap_or(0);
                    ui.label(RichText::new(format!("{hits} hits")).size(11.5).color(p.faint));
                    if widgets::ghost(ui, p, "Remove").clicked() {
                        remove = Some(i);
                    }
                });
            }
            if let Some(i) = remove {
                self.rules.remove(i);
            }
            if self.rules != before {
                self.rules_dirty = true;
            }
            if let Some(err) = &self.rules_error {
                ui.label(RichText::new(err).size(12.0).color(p.error));
            }
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(self.rules.len() < MAX_RULES, egui::Button::new("Add rule"))
                    .clicked()
                {
                    self.rules.push(RuleDef::default());
                }
                let dirty = self.rules != self.rules_live.iter().map(|r| r.def.clone()).collect::<Vec<_>>();
                if ui.add_enabled(dirty, widgets::primary_button(p, "Apply rules")).clicked() {
                    match manager.set_auto_responses(&self.id, &self.rules) {
                        Ok(live) => {
                            let n = live.iter().filter(|r| r.def.enabled).count();
                            self.rules_live = live;
                            self.rules_error = None;
                            self.rules_dirty = false;
                            action.toast_ok = Some(match n {
                                0 => "Auto-reply off".to_string(),
                                1 => "1 auto-reply rule active".to_string(),
                                n => format!("{n} auto-reply rules active"),
                            });
                        }
                        Err(e) => self.rules_error = Some(validate::sentence(&e.to_string())),
                    }
                }
                if dirty {
                    ui.label(RichText::new("Not applied yet").size(12.0).color(p.warn));
                }
            });
        });
    }

    fn toolbar(
        &mut self,
        ui: &mut egui::Ui,
        manager: &SessionManager,
        p: &Palette,
        action: &mut Action,
    ) {
        let narrow = ui.available_width() < 1000.0;
        ui.horizontal(|ui| {
            let mut format = self.term.format;
            if widgets::segmented(
                ui,
                p,
                &mut format,
                &[
                    (ViewFormat::Ascii, "Text"),
                    (ViewFormat::Hex, "Hex"),
                    (ViewFormat::Dec, "Dec"),
                ],
            ) {
                self.term.set_format(format);
            }
            widgets::vdivider(ui, p);
            let mut show_rx = !self.filter.hide_rx;
            let mut show_tx = !self.filter.hide_tx;
            let mut show_info = !self.filter.hide_info;
            ui.checkbox(&mut self.follow, "Follow");
            if narrow {
                ui.menu_button(RichText::new("View").size(13.0), |ui| {
                    ui.checkbox(&mut self.term.show_ts, "Timestamps");
                    ui.checkbox(&mut self.wrap, "Wrap long lines");
                    ui.separator();
                    ui.checkbox(&mut show_rx, RichText::new("Received (RX)").color(p.rx));
                    ui.checkbox(&mut show_tx, RichText::new("Sent (TX)").color(p.tx));
                    ui.checkbox(&mut show_info, "Events");
                });
            } else {
                ui.checkbox(&mut self.term.show_ts, "Time");
                ui.checkbox(&mut self.wrap, "Wrap");
                widgets::vdivider(ui, p);
                ui.checkbox(&mut show_rx, RichText::new("RX").color(p.rx));
                ui.checkbox(&mut show_tx, RichText::new("TX").color(p.tx));
                ui.checkbox(&mut show_info, RichText::new("Events").color(p.muted));
            }
            self.filter.hide_rx = !show_rx;
            self.filter.hide_tx = !show_tx;
            self.filter.hide_info = !show_info;

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                self.toolbar_actions(ui, manager, p, action, narrow);
            });
        });
    }

    fn toolbar_actions(
        &mut self,
        ui: &mut egui::Ui,
        manager: &SessionManager,
        p: &Palette,
        action: &mut Action,
        narrow: bool,
    ) {
        let mut clear = false;
        let mut save = false;
        let mut copy = false;
        if narrow {
            ui.menu_button(RichText::new("Actions").size(13.0), |ui| {
                copy = ui.button("Copy visible lines").clicked();
                save = ui.button("Save to text file").clicked();
                ui.separator();
                clear = ui.button("Clear terminal  (Ctrl+L)").clicked();
                if copy || save || clear {
                    ui.close();
                }
            });
        } else {
            clear = widgets::ghost(ui, p, "Clear")
                .on_hover_text("Ctrl+L")
                .clicked();
            save = widgets::ghost(ui, p, "Save")
                .on_hover_text("Save the visible lines to a text file")
                .clicked();
            copy = widgets::ghost(ui, p, "Copy").clicked();
        }
        if clear {
            self.clear(manager);
        }
        if save {
            let path = default_export_path(&self.label, &self.export_dir);
            match std::fs::write(&path, self.term.text(&self.filter)) {
                Ok(()) => action.toast_ok = Some(format!("Saved {}", path.display())),
                Err(e) => action.toast_err = Some(format!("Could not save: {e}.")),
            }
        }
        if copy {
            ui.ctx().copy_text(self.term.text(&self.filter));
            action.toast_ok = Some(format!("Copied {} lines", self.visible.len()));
        }
        let count = if self.filter.is_active() {
            format!("{} of {}", self.visible.len(), self.term.lines.len())
        } else {
            format!("{} lines", self.term.lines.len())
        };
        ui.label(RichText::new(count).size(12.0).color(p.faint));
        let search = ui.add(
            egui::TextEdit::singleline(&mut self.filter.query)
                .id(egui::Id::unique(("filter", &self.id)))
                .desired_width(if narrow { 130.0 } else { 180.0 })
                .hint_text(if narrow {
                    "Filter"
                } else {
                    "Filter lines  (Ctrl+F)"
                }),
        );
        if ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::F)) {
            search.request_focus();
        }
    }

    fn composer(
        &mut self,
        ui: &mut egui::Ui,
        manager: &SessionManager,
        p: &Palette,
        macros: &[MacroDef],
        action: &mut Action,
    ) {
        widgets::card(ui, p, |ui| {
            ui.set_width(ui.available_width());
            let preview = self.encoded();
            // Row 1: input + send
            ui.horizontal(|ui| {
                let send_w = 92.0;
                let input_id = egui::Id::unique(("composer", &self.id));
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.input)
                        .id(input_id)
                        .desired_width(ui.available_width() - send_w - 8.0)
                        .font(egui::TextStyle::Monospace)
                        .margin(Margin::symmetric(10, 7))
                        .hint_text(if self.hex_send {
                            "Hex bytes, e.g. 01 03 00 00 00 0A C5 CD"
                        } else {
                            "Type a command.  \\r \\n \\t \\xNN  $0D  \\#013 escapes are supported"
                        }),
                );
                let enter = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if response.has_focus() {
                    let (up, down) = ui.input(|i| {
                        (
                            i.key_pressed(egui::Key::ArrowUp),
                            i.key_pressed(egui::Key::ArrowDown),
                        )
                    });
                    if up && !self.history.is_empty() {
                        let pos = match self.history_pos {
                            Some(p) if p > 0 => p - 1,
                            Some(p) => p,
                            None => self.history.len() - 1,
                        };
                        self.history_pos = Some(pos);
                        self.input = self.history[pos].clone();
                    }
                    if down {
                        match self.history_pos {
                            Some(p) if p + 1 < self.history.len() => {
                                self.history_pos = Some(p + 1);
                                self.input = self.history[p + 1].clone();
                            }
                            Some(_) => {
                                self.history_pos = None;
                                self.input.clear();
                            }
                            None => {}
                        }
                    }
                }
                let can_send = preview.is_ok() && self.connected();
                let clicked = ui
                    .add_enabled(
                        can_send,
                        widgets::primary_button(p, "Send").min_size(Vec2::new(send_w, 32.0)),
                    )
                    .on_hover_text("Enter")
                    .clicked();
                if (clicked || enter) && can_send {
                    self.send(manager, action);
                    ui.memory_mut(|m| m.request_focus(input_id));
                } else if enter && !self.input.is_empty() {
                    if let Err(e) = &preview {
                        action.toast_err = Some(e.clone());
                    } else if !self.connected() {
                        action.toast_err = Some("The session is not connected.".into());
                    }
                    ui.memory_mut(|m| m.request_focus(input_id));
                }
            });

            // Row 2: encoding options, preview, repeat, file.
            ui.add_space(2.0);
            let narrow = ui.available_width() < 860.0;
            ui.horizontal_wrapped(|ui| {
                let mut hex = self.hex_send;
                widgets::segmented(ui, p, &mut hex, &[(false, "Text"), (true, "Hex")]);
                self.hex_send = hex;
                if !narrow {
                    ui.label(RichText::new("Line end").size(12.0).color(p.muted));
                }
                widgets::segmented(
                    ui,
                    p,
                    &mut self.eol,
                    &[
                        (Eol::None, "None"),
                        (Eol::Cr, "CR"),
                        (Eol::Lf, "LF"),
                        (Eol::CrLf, "CRLF"),
                    ],
                );
                egui::ComboBox::from_id_salt(("checksum", &self.id))
                    .width(118.0)
                    .selected_text(if self.checksum == ChecksumKind::None {
                        "No checksum".to_string()
                    } else {
                        self.checksum.label().to_string()
                    })
                    .show_ui(ui, |ui| {
                        for kind in ChecksumKind::ALL {
                            let label = if kind == ChecksumKind::None {
                                "No checksum"
                            } else {
                                kind.label()
                            };
                            ui.selectable_value(&mut self.checksum, kind, label);
                        }
                    })
                    .response
                    .on_hover_text("Appended to the data, before the line ending");
                if !narrow {
                    widgets::vdivider(ui, p);
                }
                ui.horizontal(|ui| {
                    let label = if narrow { "Repeat" } else { "Repeat every" };
                    if ui.checkbox(&mut self.repeat, label).changed() && self.repeat {
                        if preview.is_err() {
                            self.repeat = false;
                            action.toast_err =
                                Some("Enter valid data before enabling repeat.".into());
                        }
                        self.last_repeat = Instant::now();
                    }
                    ui.add(
                        egui::DragValue::new(&mut self.repeat_ms)
                            .range(10..=3_600_000)
                            .speed(10)
                            .suffix(" ms"),
                    );
                });
                if !narrow {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        preview_label(ui, p, &preview, self.input.is_empty());
                    });
                }
            });
            if narrow && !self.input.is_empty() {
                ui.horizontal(|ui| preview_label(ui, p, &preview, false));
            }

            // Row 3: macros and file sending.
            ui.add_space(2.0);
            ui.horizontal_wrapped(|ui| {
                if macros.is_empty() {
                    ui.label(
                        RichText::new("No macros. Add them in Settings.")
                            .size(12.0)
                            .color(p.faint),
                    );
                } else {
                    ui.label(RichText::new("Macros").size(12.0).color(p.muted));
                }
                for (index, mac) in macros.iter().enumerate().take(12) {
                    let text = RichText::new(format!("F{}  {}", index + 1, mac.name)).size(12.5);
                    let button = egui::Button::new(text)
                        .fill(p.raised)
                        .stroke(Stroke::new(1.0, p.border))
                        .corner_radius(CornerRadius::same(6))
                        .min_size(Vec2::new(0.0, 26.0));
                    if ui
                        .add_enabled(self.connected(), button)
                        .on_hover_text(format!(
                            "{}{}",
                            if mac.hex { "hex: " } else { "" },
                            mac.data
                        ))
                        .clicked()
                    {
                        if let Err(e) = self.send_macro(manager, mac) {
                            action.toast_err = Some(e);
                        }
                    }
                }
                widgets::vdivider(ui, p);
                ui.menu_button(RichText::new("Send file").size(13.0), |ui| {
                    ui.set_min_width(380.0);
                    ui.label(RichText::new("File to send").font(strong_font(13.0)));
                    ui.add(
                        egui::TextEdit::singleline(&mut self.file_path)
                            .desired_width(360.0)
                            .font(egui::TextStyle::Monospace)
                            .hint_text("Path, or drop a file onto the window"),
                    );
                    ui.label(
                        RichText::new(format!(
                            "Raw bytes are sent in 1 KiB chunks. Limit {}.",
                            widgets::human_count(MAX_SEND_FILE)
                        ))
                        .size(12.0)
                        .color(p.muted),
                    );
                    if widgets::primary(ui, p, "Send file").clicked() {
                        match self.send_file(manager) {
                            Ok(msg) => {
                                action.toast_ok = Some(msg);
                                ui.close();
                            }
                            Err(e) => action.toast_err = Some(e),
                        }
                    }
                });
            });
        });
    }
}

/// Byte preview of the composer input, or the parse error.
fn preview_label(ui: &mut egui::Ui, p: &Palette, preview: &Result<Vec<u8>, String>, empty: bool) {
    match preview {
        Ok(bytes) => {
            let shown: Vec<u8> = bytes.iter().take(12).copied().collect();
            let more = if bytes.len() > 12 { " …" } else { "" };
            ui.label(
                RichText::new(format!(
                    "{} B  {}{more}",
                    bytes.len(),
                    codec::to_hex(&shown)
                ))
                .monospace()
                .size(12.0)
                .color(p.faint),
            );
        }
        Err(e) if !empty => {
            ui.label(RichText::new(e).size(12.0).color(p.error));
        }
        Err(_) => {}
    }
}

fn status(
    info: Option<&SessionInfo>,
    kind: SessionKind,
    p: &Palette,
) -> (&'static str, egui::Color32) {
    match info {
        Some(i) if i.connected => match kind {
            SessionKind::TcpServer => ("Listening", p.ok),
            SessionKind::Udp => ("Bound", p.ok),
            _ => ("Connected", p.ok),
        },
        Some(i) if i.error.is_some() => ("Error", p.error),
        Some(i) => match &i.config {
            SessionConfig::TcpClient(c) if c.reconnect => ("Reconnecting", p.warn),
            SessionConfig::TcpClient(_) if i.rx_bytes + i.tx_bytes == 0 => ("Connecting", p.warn),
            _ => ("Disconnected", p.muted),
        },
        None => ("Unknown", p.muted),
    }
}

fn describe(config: &SessionConfig, info: &SessionInfo) -> String {
    match config {
        SessionConfig::Serial(c) => {
            let parity = match c.parity {
                Parity::None => 'N',
                Parity::Even => 'E',
                Parity::Odd => 'O',
                Parity::Mark => 'M',
                Parity::Space => 'S',
            };
            format!(
                "{}  ·  {} baud  ·  {}{}{}  ·  {}",
                c.port,
                c.baud,
                c.data_bits,
                parity,
                c.stop_bits,
                flow_label(c.flow_control)
            )
        }
        SessionConfig::TcpClient(c) => format!(
            "tcp://{}:{}{}{}",
            c.host,
            c.port,
            if c.nvt { "  ·  telnet" } else { "" },
            if c.reconnect {
                "  ·  auto-reconnect"
            } else {
                ""
            }
        ),
        SessionConfig::TcpServer(c) => format!(
            "Listening on {}  ·  up to {} clients{}",
            info.peer
                .clone()
                .unwrap_or_else(|| format!("{}:{}", c.bind, c.port)),
            c.max_clients,
            if c.echo { "  ·  echo" } else { "" }
        ),
        SessionConfig::Udp(c) => {
            let target = match (&c.remote_host, c.remote_port) {
                (Some(h), Some(port)) => format!("  ·  target {h}:{port}"),
                _ => "  ·  replies to last sender".into(),
            };
            format!("Local {}:{}{}", c.bind, c.local_port, target)
        }
    }
}

fn safe_name(label: &str) -> String {
    let mut name: String = label
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    while name.contains("__") {
        name = name.replace("__", "_");
    }
    let trimmed: String = name.trim_matches('_').chars().take(48).collect();
    if trimmed.is_empty() {
        "session".to_string()
    } else {
        trimmed
    }
}

fn base_dir(config: Option<&AppConfig>) -> PathBuf {
    config
        .and_then(|c| c.log_dir.clone())
        .filter(|d| d.is_dir())
        .or_else(dirs_documents)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

fn dirs_documents() -> Option<PathBuf> {
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))?;
    let docs = PathBuf::from(home).join("Documents");
    docs.is_dir().then_some(docs)
}

fn default_log_path(config: &AppConfig, label: &str) -> String {
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    base_dir(Some(config))
        .join(format!("{}-{stamp}.log", safe_name(label)))
        .display()
        .to_string()
}

fn default_export_path(label: &str, dir: &std::path::Path) -> PathBuf {
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    dir.join(format!("{}-{stamp}.txt", safe_name(label)))
}
