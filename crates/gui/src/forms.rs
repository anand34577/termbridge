//! "New connection" dialog: transport picker plus validated forms.

use eframe::egui::{self, Color32, CornerRadius, Margin, RichText, Stroke, Vec2};
use serial_mcp_core::net::PortInfo;
use serial_mcp_core::session::{FlowControl, Parity, SessionConfig};
use serial_mcp_core::types::SessionKind;
use serial_mcp_core::{SerialConfig, SessionInfo, TcpClientConfig, TcpServerConfig, UdpConfig};

use crate::theme::Palette;
use crate::widgets::{self, strong_font};
use serial_mcp_core::validate::{self, Report, Severity};

pub const BAUD_RATES: &[u32] = &[
    300, 600, 1200, 2400, 4800, 9600, 14400, 19200, 28800, 38400, 57600, 76800, 115200, 230400,
    250000, 460800, 500000, 921600, 1_000_000, 2_000_000,
];

const LABEL_W: f32 = 112.0;

pub struct ConnectForms {
    pub open: bool,
    pub kind: SessionKind,
    pub serial: SerialConfig,
    pub tcp_client: TcpClientConfig,
    pub tcp_server: TcpServerConfig,
    pub udp: UdpConfig,
    pub ports: Vec<PortInfo>,
    label: String,
    udp_remote_host: String,
    udp_remote_port: u16,
}

pub enum FormResult {
    None,
    Submit(SessionConfig),
    RefreshPorts,
}

impl ConnectForms {
    pub fn new() -> Self {
        Self {
            open: false,
            kind: SessionKind::Serial,
            serial: SerialConfig::default(),
            tcp_client: TcpClientConfig::default(),
            tcp_server: TcpServerConfig::default(),
            udp: UdpConfig::default(),
            ports: Vec::new(),
            label: String::new(),
            udp_remote_host: String::new(),
            udp_remote_port: 0,
        }
    }

    pub fn open_with(&mut self, kind: SessionKind) {
        self.kind = kind;
        self.open = true;
        self.label.clear();
    }

    /// Pre-fill the dialog from a remembered connection.
    pub fn open_from(&mut self, config: &SessionConfig) {
        let label = match config {
            SessionConfig::Serial(c) => c.label.clone(),
            SessionConfig::TcpClient(c) => c.label.clone(),
            SessionConfig::TcpServer(c) => c.label.clone(),
            SessionConfig::Udp(c) => c.label.clone(),
        };
        match config {
            SessionConfig::Serial(c) => self.serial = c.clone(),
            SessionConfig::TcpClient(c) => self.tcp_client = c.clone(),
            SessionConfig::TcpServer(c) => self.tcp_server = c.clone(),
            SessionConfig::Udp(c) => {
                self.udp = c.clone();
                self.udp_remote_host = c.remote_host.clone().unwrap_or_default();
                self.udp_remote_port = c.remote_port.unwrap_or(0);
            }
        }
        self.kind = config.kind();
        self.label = label.unwrap_or_default();
        self.open = true;
    }

    pub fn set_ports(&mut self, ports: Vec<PortInfo>) {
        if !ports.iter().any(|p| p.port_name == self.serial.port) {
            if let Some(first) = ports.first() {
                self.serial.port = first.port_name.clone();
            }
        }
        self.ports = ports;
    }

    fn sync_udp(&mut self) {
        let host = self.udp_remote_host.trim();
        self.udp.remote_host = (!host.is_empty()).then(|| host.to_string());
        self.udp.remote_port = (self.udp_remote_port != 0).then_some(self.udp_remote_port);
    }

    fn report(&mut self, open: &[SessionInfo]) -> Report {
        self.sync_udp();
        match self.kind {
            SessionKind::Serial => {
                let names: Vec<String> = self.ports.iter().map(|p| p.port_name.clone()).collect();
                validate::serial(&self.serial, &names, open)
            }
            SessionKind::TcpClient => validate::tcp_client(&self.tcp_client),
            SessionKind::TcpServer => validate::tcp_server(&self.tcp_server, open),
            SessionKind::Udp => validate::udp(&self.udp, open),
        }
    }

    fn build(&self) -> SessionConfig {
        let label = {
            let l = self.label.trim();
            (!l.is_empty()).then(|| l.to_string())
        };
        match self.kind {
            SessionKind::Serial => {
                let mut c = self.serial.clone();
                c.port = c.port.trim().to_string();
                c.label = label;
                SessionConfig::Serial(c)
            }
            SessionKind::TcpClient => {
                let mut c = self.tcp_client.clone();
                c.host = c.host.trim().to_string();
                c.label = label;
                SessionConfig::TcpClient(c)
            }
            SessionKind::TcpServer => {
                let mut c = self.tcp_server.clone();
                c.bind = c.bind.trim().to_string();
                c.label = label;
                SessionConfig::TcpServer(c)
            }
            SessionKind::Udp => {
                let mut c = self.udp.clone();
                c.bind = c.bind.trim().to_string();
                c.label = label;
                SessionConfig::Udp(c)
            }
        }
    }

    /// Draw the modal. Returns what the caller should do.
    pub fn show(&mut self, ctx: &egui::Context, p: &Palette, open: &[SessionInfo]) -> FormResult {
        if !self.open {
            return FormResult::None;
        }
        let mut result = FormResult::None;
        let modal = egui::Modal::new(egui::Id::unique("new_connection"))
            .backdrop_color(Color32::from_black_alpha(if p.dark { 150 } else { 90 }))
            .frame(
                egui::Frame::new()
                    .fill(p.card)
                    .stroke(Stroke::new(1.0, p.border))
                    .corner_radius(CornerRadius::same(14))
                    .inner_margin(Margin::same(0)),
            )
            .show(ctx, |ui| {
                ui.set_width(760.0);
                ui.spacing_mut().item_spacing.y = 0.0;
                // Header
                egui::Frame::new()
                    .inner_margin(Margin {
                        left: 22,
                        right: 22,
                        top: 18,
                        bottom: 14,
                    })
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 4.0;
                        ui.label(
                            RichText::new("New connection")
                                .font(strong_font(18.0))
                                .color(p.text_strong),
                        );
                        widgets::hint(
                            ui,
                            p,
                            "Pick a transport, check the settings, then open the session.",
                        );
                    });
                separator(ui, p);

                let report = self.report(open);
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(0.0, 0.0);
                    // Transport picker
                    egui::Frame::new()
                        .fill(p.surface)
                        .inner_margin(Margin::same(12))
                        .show(ui, |ui| {
                            ui.vertical(|ui| {
                                ui.set_width(210.0);
                                ui.set_min_height(390.0);
                                ui.spacing_mut().item_spacing.y = 6.0;
                                for (kind, title, desc) in KINDS {
                                    if kind_option(ui, p, *kind, title, desc, self.kind == *kind) {
                                        self.kind = *kind;
                                    }
                                }
                            });
                        });
                    let (rect, _) =
                        ui.allocate_exact_size(Vec2::new(1.0, 414.0), egui::Sense::hover());
                    ui.painter().rect_filled(rect, 0.0, p.border);
                    // Form
                    egui::Frame::new()
                        .inner_margin(Margin {
                            left: 22,
                            right: 22,
                            top: 16,
                            bottom: 12,
                        })
                        .show(ui, |ui| {
                            ui.vertical(|ui| {
                                ui.set_width(760.0 - 235.0 - 44.0);
                                ui.spacing_mut().item_spacing = Vec2::new(8.0, 10.0);
                                let r = Some(&report);
                                match self.kind {
                                    SessionKind::Serial => {
                                        if self.serial_form(ui, p, r) {
                                            result = FormResult::RefreshPorts;
                                        }
                                    }
                                    SessionKind::TcpClient => self.tcp_client_form(ui, p, r),
                                    SessionKind::TcpServer => self.tcp_server_form(ui, p, r),
                                    SessionKind::Udp => self.udp_form(ui, p, r),
                                }
                                row(ui, p, "Label", None, |ui| {
                                    ui.add(
                                        egui::TextEdit::singleline(&mut self.label)
                                            .desired_width(240.0)
                                            .char_limit(48)
                                            .hint_text("Optional display name"),
                                    );
                                });
                            })
                        });
                });
                separator(ui, p);

                // Footer
                egui::Frame::new()
                    .inner_margin(Margin::symmetric(22, 14))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(self.summary())
                                    .monospace()
                                    .size(12.0)
                                    .color(p.muted),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    let verb = match self.kind {
                                        SessionKind::Serial => "Open port",
                                        SessionKind::TcpClient => "Connect",
                                        SessionKind::TcpServer => "Start listening",
                                        SessionKind::Udp => "Bind socket",
                                    };
                                    let enter = ui.input(|i| {
                                        i.key_pressed(egui::Key::Enter) && i.modifiers.command
                                    });
                                    let clicked = ui
                                        .add_enabled(
                                            report.is_ok(),
                                            widgets::primary_button(p, verb),
                                        )
                                        .on_hover_text("Ctrl+Enter")
                                        .on_disabled_hover_text("Fix the highlighted fields first")
                                        .clicked();
                                    if clicked || (enter && report.is_ok()) {
                                        result = FormResult::Submit(self.build());
                                        self.open = false;
                                    }
                                    if widgets::secondary(ui, p, "Cancel").clicked() {
                                        self.open = false;
                                    }
                                },
                            );
                        });
                    });
            });
        if modal.should_close() {
            self.open = false;
        }
        result
    }

    fn summary(&self) -> String {
        match self.kind {
            SessionKind::Serial => {
                let s = &self.serial;
                let parity = match s.parity {
                    Parity::None => 'N',
                    Parity::Even => 'E',
                    Parity::Odd => 'O',
                    Parity::Mark => 'M',
                    Parity::Space => 'S',
                };
                format!(
                    "{}  {} {}{}{}  {}",
                    s.port.trim(),
                    s.baud,
                    s.data_bits,
                    parity,
                    s.stop_bits,
                    flow_label(s.flow_control)
                )
            }
            SessionKind::TcpClient => format!(
                "tcp://{}:{}{}",
                self.tcp_client.host.trim(),
                self.tcp_client.port,
                if self.tcp_client.nvt { "  telnet" } else { "" }
            ),
            SessionKind::TcpServer => format!(
                "listen {}:{}  max {} clients",
                self.tcp_server.bind.trim(),
                self.tcp_server.port,
                self.tcp_server.max_clients
            ),
            SessionKind::Udp => {
                let local = if self.udp.local_port == 0 {
                    "auto".to_string()
                } else {
                    self.udp.local_port.to_string()
                };
                match (&self.udp.remote_host, self.udp.remote_port) {
                    (Some(h), Some(port)) => {
                        format!("udp {}:{local} -> {h}:{port}", self.udp.bind.trim())
                    }
                    _ => format!("udp {}:{local}", self.udp.bind.trim()),
                }
            }
        }
    }

    /// Returns true when the user asked to rescan ports.
    fn serial_form(&mut self, ui: &mut egui::Ui, p: &Palette, r: Option<&Report>) -> bool {
        let mut refresh = false;
        let ports = self.ports.clone();
        row(ui, p, "Port", r.and_then(|r| r.for_field("port")), |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.serial.port)
                    .desired_width(150.0)
                    .char_limit(128)
                    .font(egui::TextStyle::Monospace)
                    .hint_text(if cfg!(windows) {
                        "COM3"
                    } else {
                        "/dev/ttyUSB0"
                    }),
            );
            let label = if ports.is_empty() {
                "No ports found".to_string()
            } else {
                format!("Detected ({})", ports.len())
            };
            ui.add_enabled_ui(!ports.is_empty(), |ui| {
                ui.menu_button(label, |ui| {
                    ui.set_min_width(300.0);
                    for port in &ports {
                        let desc = port_description(port);
                        let text = RichText::new(format!("{}   {}", port.port_name, desc));
                        if ui
                            .selectable_label(self.serial.port == port.port_name, text)
                            .clicked()
                        {
                            self.serial.port = port.port_name.clone();
                            ui.close();
                        }
                    }
                });
            });
            if widgets::ghost(ui, p, "Rescan").clicked() {
                refresh = true;
            }
        });
        if let Some(info) = ports.iter().find(|x| x.port_name == self.serial.port) {
            let desc = port_description(info);
            if !desc.is_empty() {
                ui.horizontal(|ui| {
                    ui.add_space(LABEL_W + 8.0);
                    ui.label(RichText::new(desc).size(12.0).color(p.faint));
                });
            }
        }
        row(
            ui,
            p,
            "Baud rate",
            r.and_then(|r| r.for_field("baud")),
            |ui| {
                ui.add(
                    egui::DragValue::new(&mut self.serial.baud)
                        .range(validate::MIN_BAUD..=validate::MAX_BAUD)
                        .speed(100),
                );
                egui::ComboBox::from_id_salt("baud_presets")
                    .width(110.0)
                    .selected_text("Presets")
                    .show_ui(ui, |ui| {
                        for baud in BAUD_RATES {
                            ui.selectable_value(&mut self.serial.baud, *baud, baud.to_string());
                        }
                    });
            },
        );
        row(
            ui,
            p,
            "Data bits",
            r.and_then(|r| r.for_field("data_bits")),
            |ui| {
                widgets::segmented(
                    ui,
                    p,
                    &mut self.serial.data_bits,
                    &[(5, "5"), (6, "6"), (7, "7"), (8, "8")],
                );
            },
        );
        row(
            ui,
            p,
            "Parity",
            r.and_then(|r| r.for_field("parity")),
            |ui| {
                widgets::segmented(
                    ui,
                    p,
                    &mut self.serial.parity,
                    &[
                        (Parity::None, "None"),
                        (Parity::Even, "Even"),
                        (Parity::Odd, "Odd"),
                        (Parity::Mark, "Mark"),
                        (Parity::Space, "Space"),
                    ],
                );
            },
        );
        row(
            ui,
            p,
            "Stop bits",
            r.and_then(|r| r.for_field("stop_bits")),
            |ui| {
                widgets::segmented(ui, p, &mut self.serial.stop_bits, &[(1, "1"), (2, "2")]);
            },
        );
        row(ui, p, "Flow control", None, |ui| {
            widgets::segmented(
                ui,
                p,
                &mut self.serial.flow_control,
                &[
                    (FlowControl::None, "None"),
                    (FlowControl::RtsCts, "RTS/CTS"),
                    (FlowControl::XonXoff, "XON/XOFF"),
                ],
            );
        });
        row(ui, p, "Assert on open", None, |ui| {
            ui.checkbox(&mut self.serial.dtr, "DTR");
            ui.checkbox(&mut self.serial.rts, "RTS");
        });
        refresh
    }

    fn tcp_client_form(&mut self, ui: &mut egui::Ui, p: &Palette, r: Option<&Report>) {
        row(ui, p, "Host", r.and_then(|r| r.for_field("host")), |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.tcp_client.host)
                    .desired_width(240.0)
                    .char_limit(253)
                    .hint_text("192.168.1.50 or device.local"),
            );
        });
        row(ui, p, "Port", r.and_then(|r| r.for_field("port")), |ui| {
            ui.add(egui::DragValue::new(&mut self.tcp_client.port).range(1..=65535));
            for (port, name) in [
                (23u16, "Telnet"),
                (502, "Modbus"),
                (4001, "4001"),
                (8080, "8080"),
            ] {
                if widgets::ghost(ui, p, name).clicked() {
                    self.tcp_client.port = port;
                    self.tcp_client.nvt = port == 23;
                }
            }
        });
        row(
            ui,
            p,
            "Timeout",
            r.and_then(|r| r.for_field("timeout")),
            |ui| {
                ui.add(
                    egui::DragValue::new(&mut self.tcp_client.connect_timeout_ms)
                        .range(100..=120_000)
                        .speed(50)
                        .suffix(" ms"),
                );
            },
        );
        row(ui, p, "Options", None, |ui| {
            ui.checkbox(&mut self.tcp_client.nvt, "Telnet negotiation (NVT)");
        });
        row(ui, p, "", None, |ui| {
            ui.checkbox(&mut self.tcp_client.reconnect, "Reconnect automatically");
        });
    }

    fn tcp_server_form(&mut self, ui: &mut egui::Ui, p: &Palette, r: Option<&Report>) {
        row(
            ui,
            p,
            "Bind address",
            r.and_then(|r| r.for_field("bind")),
            |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.tcp_server.bind)
                        .desired_width(150.0)
                        .font(egui::TextStyle::Monospace),
                );
                for (addr, name) in [("0.0.0.0", "All"), ("127.0.0.1", "Local only")] {
                    if widgets::ghost(ui, p, name).clicked() {
                        self.tcp_server.bind = addr.into();
                    }
                }
            },
        );
        row(ui, p, "Port", r.and_then(|r| r.for_field("port")), |ui| {
            ui.add(egui::DragValue::new(&mut self.tcp_server.port).range(1..=65535));
        });
        row(
            ui,
            p,
            "Max clients",
            r.and_then(|r| r.for_field("max_clients")),
            |ui| {
                ui.add(egui::DragValue::new(&mut self.tcp_server.max_clients).range(1..=512));
            },
        );
        row(ui, p, "Options", None, |ui| {
            ui.checkbox(
                &mut self.tcp_server.echo,
                "Echo received data back to the client",
            );
        });
    }

    fn udp_form(&mut self, ui: &mut egui::Ui, p: &Palette, r: Option<&Report>) {
        row(
            ui,
            p,
            "Bind address",
            r.and_then(|r| r.for_field("bind")),
            |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.udp.bind)
                        .desired_width(150.0)
                        .font(egui::TextStyle::Monospace),
                );
                for (addr, name) in [("0.0.0.0", "All"), ("127.0.0.1", "Local only")] {
                    if widgets::ghost(ui, p, name).clicked() {
                        self.udp.bind = addr.into();
                    }
                }
            },
        );
        row(
            ui,
            p,
            "Local port",
            r.and_then(|r| r.for_field("local_port")),
            |ui| {
                ui.add(egui::DragValue::new(&mut self.udp.local_port).range(0..=65535));
                ui.label(
                    RichText::new("0 picks a free port")
                        .size(12.0)
                        .color(p.faint),
                );
            },
        );
        row(
            ui,
            p,
            "Remote host",
            r.and_then(|r| r.for_field("remote_host")),
            |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.udp_remote_host)
                        .desired_width(200.0)
                        .char_limit(253)
                        .hint_text("Optional default target"),
                );
            },
        );
        row(
            ui,
            p,
            "Remote port",
            r.and_then(|r| r.for_field("remote_port")),
            |ui| {
                ui.add(egui::DragValue::new(&mut self.udp_remote_port).range(0..=65535));
                ui.label(RichText::new("0 = none").size(12.0).color(p.faint));
            },
        );
        row(ui, p, "Options", None, |ui| {
            ui.checkbox(&mut self.udp.broadcast, "Allow broadcast");
            ui.checkbox(&mut self.udp.echo, "Echo to sender");
        });
    }
}

const KINDS: &[(SessionKind, &str, &str)] = &[
    (SessionKind::Serial, "Serial port", "RS-232 / USB UART"),
    (SessionKind::TcpClient, "TCP client", "Connect to a host"),
    (SessionKind::TcpServer, "TCP server", "Accept connections"),
    (SessionKind::Udp, "UDP", "Datagrams, broadcast"),
];

pub fn kind_short(kind: SessionKind) -> &'static str {
    match kind {
        SessionKind::Serial => "SER",
        SessionKind::TcpClient => "TCP",
        SessionKind::TcpServer => "SRV",
        SessionKind::Udp => "UDP",
    }
}

fn kind_option(
    ui: &mut egui::Ui,
    p: &Palette,
    kind: SessionKind,
    title: &str,
    desc: &str,
    selected: bool,
) -> bool {
    let color = p.kind_color(kind);
    let response = egui::Frame::new()
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
        .corner_radius(CornerRadius::same(9))
        .inner_margin(Margin::symmetric(10, 9))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                widgets::badge(ui, p, kind_short(kind), color);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    ui.label(
                        RichText::new(title)
                            .font(strong_font(13.5))
                            .color(if selected { p.text_strong } else { p.text }),
                    );
                    ui.label(RichText::new(desc).size(11.5).color(p.muted));
                });
            });
        })
        .response
        .interact(egui::Sense::click());
    if response.hovered() && !selected {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response.clicked()
}

/// Labelled form row with an optional validation message underneath.
fn row(
    ui: &mut egui::Ui,
    p: &Palette,
    label: &str,
    issue: Option<&validate::Issue>,
    add: impl FnOnce(&mut egui::Ui),
) {
    ui.horizontal(|ui| {
        widgets::field_label(ui, p, label, LABEL_W);
        add(ui);
    });
    if let Some(issue) = issue {
        let color = match issue.severity {
            Severity::Error => p.error,
            Severity::Warning => p.warn,
        };
        ui.horizontal(|ui| {
            ui.add_space(LABEL_W + 8.0);
            ui.add(egui::Label::new(RichText::new(&issue.message).size(12.0).color(color)).wrap());
        });
    }
}

fn separator(ui: &mut egui::Ui, p: &Palette) {
    let (rect, _) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 0.0, p.border);
}

pub fn port_description(port: &PortInfo) -> String {
    let mut parts = Vec::new();
    if let Some(product) = &port.product {
        parts.push(product.clone());
    } else if let Some(m) = &port.manufacturer {
        parts.push(m.clone());
    }
    if let (Some(vid), Some(pid)) = (port.vid, port.pid) {
        parts.push(format!("{vid:04X}:{pid:04X}"));
    }
    if parts.is_empty() && !port.kind.is_empty() && !port.kind.eq_ignore_ascii_case("unknown") {
        parts.push(port.kind.clone());
    }
    parts.join("  ")
}

pub fn flow_label(value: FlowControl) -> &'static str {
    match value {
        FlowControl::None => "no flow control",
        FlowControl::RtsCts => "RTS/CTS",
        FlowControl::XonXoff => "XON/XOFF",
    }
}
