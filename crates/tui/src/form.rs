//! Keyboard-driven "New connection" form for the TUI.

use serial_mcp_core::net::PortInfo;
use serial_mcp_core::session::{FlowControl, Parity, SessionConfig};
use serial_mcp_core::types::SessionKind;
use serial_mcp_core::validate::{self, Report};
use serial_mcp_core::{SerialConfig, SessionInfo, TcpClientConfig, TcpServerConfig, UdpConfig};

pub const BAUD_RATES: &[u32] = &[
    300, 1200, 2400, 4800, 9600, 19200, 38400, 57600, 115200, 230400, 460800, 921600, 1_000_000,
    2_000_000,
];

pub const KINDS: [SessionKind; 4] = [
    SessionKind::Serial,
    SessionKind::TcpClient,
    SessionKind::TcpServer,
    SessionKind::Udp,
];

#[derive(Clone)]
pub enum Value {
    Text(String),
    Choice(Vec<&'static str>, usize),
    Toggle(bool),
}

#[derive(Clone)]
pub struct Field {
    pub key: &'static str,
    pub label: &'static str,
    pub value: Value,
    /// Validation field name in `Report`.
    pub check: &'static str,
}

impl Field {
    fn text(key: &'static str, label: &'static str, value: impl Into<String>) -> Self {
        Self {
            key,
            label,
            value: Value::Text(value.into()),
            check: key,
        }
    }
    fn choice(
        key: &'static str,
        label: &'static str,
        opts: Vec<&'static str>,
        index: usize,
    ) -> Self {
        Self {
            key,
            label,
            value: Value::Choice(opts, index),
            check: key,
        }
    }
    fn toggle(key: &'static str, label: &'static str, on: bool) -> Self {
        Self {
            key,
            label,
            value: Value::Toggle(on),
            check: key,
        }
    }

    pub fn display(&self) -> String {
        match &self.value {
            Value::Text(t) => t.clone(),
            Value::Choice(opts, i) => opts[*i].to_string(),
            Value::Toggle(on) => if *on { "[x]" } else { "[ ]" }.to_string(),
        }
    }
}

pub struct ConnectForm {
    pub kind_index: usize,
    pub fields: Vec<Field>,
    /// 0 = transport row, 1.. = fields.
    pub focus: usize,
    pub ports: Vec<PortInfo>,
    port_cycle: usize,
}

impl ConnectForm {
    pub fn new(kind: SessionKind, ports: Vec<PortInfo>, recent: &[SessionConfig]) -> Self {
        let kind_index = KINDS.iter().position(|k| *k == kind).unwrap_or(0);
        let mut form = Self {
            kind_index,
            fields: Vec::new(),
            focus: 1,
            ports,
            port_cycle: 0,
        };
        form.rebuild(recent);
        form
    }

    pub fn kind(&self) -> SessionKind {
        KINDS[self.kind_index]
    }

    /// Recreate the fields for the current kind, pre-filled from the most
    /// recent connection of that kind when there is one.
    pub fn rebuild(&mut self, recent: &[SessionConfig]) {
        let kind = self.kind();
        let last = recent.iter().find(|c| c.kind() == kind).cloned();
        self.fields = match kind {
            SessionKind::Serial => {
                let mut c = match last {
                    Some(SessionConfig::Serial(c)) => c,
                    _ => SerialConfig::default(),
                };
                if !self.ports.iter().any(|p| p.port_name == c.port) {
                    if let Some(first) = self.ports.first() {
                        c.port = first.port_name.clone();
                    }
                }
                vec![
                    Field::text("port", "Port", c.port),
                    Field::text("baud", "Baud rate", c.baud.to_string()),
                    Field::choice(
                        "data_bits",
                        "Data bits",
                        vec!["5", "6", "7", "8"],
                        (c.data_bits.clamp(5, 8) - 5) as usize,
                    ),
                    Field::choice(
                        "parity",
                        "Parity",
                        vec!["None", "Even", "Odd", "Mark", "Space"],
                        match c.parity {
                            Parity::None => 0,
                            Parity::Even => 1,
                            Parity::Odd => 2,
                            Parity::Mark => 3,
                            Parity::Space => 4,
                        },
                    ),
                    Field::choice(
                        "stop_bits",
                        "Stop bits",
                        vec!["1", "2"],
                        if c.stop_bits == 2 { 1 } else { 0 },
                    ),
                    Field::choice(
                        "flow",
                        "Flow control",
                        vec!["None", "RTS/CTS", "XON/XOFF"],
                        match c.flow_control {
                            FlowControl::None => 0,
                            FlowControl::RtsCts => 1,
                            FlowControl::XonXoff => 2,
                        },
                    ),
                    Field::toggle("dtr", "Assert DTR", c.dtr),
                    Field::toggle("rts", "Assert RTS", c.rts),
                    Field::text("label", "Label", c.label.unwrap_or_default()),
                ]
            }
            SessionKind::TcpClient => {
                let c = match last {
                    Some(SessionConfig::TcpClient(c)) => c,
                    _ => TcpClientConfig::default(),
                };
                vec![
                    Field::text("host", "Host", c.host),
                    Field::text("port", "Port", c.port.to_string()),
                    Field::text("timeout", "Timeout (ms)", c.connect_timeout_ms.to_string()),
                    Field::toggle("nvt", "Telnet (NVT)", c.nvt),
                    Field::toggle("reconnect", "Auto-reconnect", c.reconnect),
                    Field::text("label", "Label", c.label.unwrap_or_default()),
                ]
            }
            SessionKind::TcpServer => {
                let c = match last {
                    Some(SessionConfig::TcpServer(c)) => c,
                    _ => TcpServerConfig::default(),
                };
                vec![
                    Field::text("bind", "Bind address", c.bind),
                    Field::text("port", "Port", c.port.to_string()),
                    Field::text("max_clients", "Max clients", c.max_clients.to_string()),
                    Field::toggle("echo", "Echo", c.echo),
                    Field::text("label", "Label", c.label.unwrap_or_default()),
                ]
            }
            SessionKind::Udp => {
                let c = match last {
                    Some(SessionConfig::Udp(c)) => c,
                    _ => UdpConfig::default(),
                };
                vec![
                    Field::text("bind", "Bind address", c.bind),
                    Field::text("local_port", "Local port", c.local_port.to_string()),
                    Field::text(
                        "remote_host",
                        "Remote host",
                        c.remote_host.unwrap_or_default(),
                    ),
                    Field::text(
                        "remote_port",
                        "Remote port",
                        c.remote_port.map(|p| p.to_string()).unwrap_or_default(),
                    ),
                    Field::toggle("broadcast", "Broadcast", c.broadcast),
                    Field::toggle("echo", "Echo", c.echo),
                    Field::text("label", "Label", c.label.unwrap_or_default()),
                ]
            }
        };
        self.focus = self.focus.min(self.fields.len());
    }

    fn text(&self, key: &str) -> String {
        self.fields
            .iter()
            .find(|f| f.key == key)
            .map(|f| match &f.value {
                Value::Text(t) => t.trim().to_string(),
                _ => String::new(),
            })
            .unwrap_or_default()
    }

    fn choice(&self, key: &str) -> usize {
        self.fields
            .iter()
            .find(|f| f.key == key)
            .map(|f| match &f.value {
                Value::Choice(_, i) => *i,
                _ => 0,
            })
            .unwrap_or(0)
    }

    fn toggle(&self, key: &str) -> bool {
        self.fields
            .iter()
            .find(|f| f.key == key)
            .is_some_and(|f| matches!(f.value, Value::Toggle(true)))
    }

    /// Parse a numeric field, recording a validation error when it is not a number.
    fn number<T: std::str::FromStr>(
        &self,
        key: &'static str,
        errors: &mut Vec<(&'static str, String)>,
        empty: Option<T>,
    ) -> Option<T> {
        let text = self.text(key);
        if text.is_empty() {
            if empty.is_none() {
                errors.push((key, "Required.".into()));
            }
            return empty;
        }
        match text.parse::<T>() {
            Ok(v) => Some(v),
            Err(_) => {
                errors.push((key, "Enter a whole number in range.".into()));
                None
            }
        }
    }

    /// Build the configuration and the validation messages for display.
    pub fn build(
        &self,
        open: &[SessionInfo],
    ) -> (Option<SessionConfig>, Vec<(&'static str, String)>) {
        let mut errors: Vec<(&'static str, String)> = Vec::new();
        let label = {
            let l = self.text("label");
            (!l.is_empty()).then_some(l)
        };
        let collect = |report: Report, errors: &mut Vec<(&'static str, String)>| {
            for issue in report.issues {
                if issue.severity == validate::Severity::Error {
                    errors.push((issue.field, issue.message));
                }
            }
        };
        let config = match self.kind() {
            SessionKind::Serial => {
                let baud = self.number::<u32>("baud", &mut errors, None);
                let cfg = SerialConfig {
                    port: self.text("port"),
                    baud: baud.unwrap_or(0),
                    data_bits: 5 + self.choice("data_bits") as u8,
                    parity: [
                        Parity::None,
                        Parity::Even,
                        Parity::Odd,
                        Parity::Mark,
                        Parity::Space,
                    ][self.choice("parity")],
                    stop_bits: 1 + self.choice("stop_bits") as u8,
                    flow_control: [FlowControl::None, FlowControl::RtsCts, FlowControl::XonXoff]
                        [self.choice("flow")],
                    dtr: self.toggle("dtr"),
                    rts: self.toggle("rts"),
                    label,
                    ..SerialConfig::default()
                };
                let names: Vec<String> = self.ports.iter().map(|p| p.port_name.clone()).collect();
                if baud.is_some() {
                    collect(validate::serial(&cfg, &names, open), &mut errors);
                }
                SessionConfig::Serial(cfg)
            }
            SessionKind::TcpClient => {
                let port = self.number::<u16>("port", &mut errors, None);
                let timeout = self.number::<u64>("timeout", &mut errors, Some(3000));
                let cfg = TcpClientConfig {
                    host: self.text("host"),
                    port: port.unwrap_or(0),
                    nvt: self.toggle("nvt"),
                    reconnect: self.toggle("reconnect"),
                    connect_timeout_ms: timeout.unwrap_or(3000),
                    label,
                };
                collect(validate::tcp_client(&cfg), &mut errors);
                SessionConfig::TcpClient(cfg)
            }
            SessionKind::TcpServer => {
                let port = self.number::<u16>("port", &mut errors, None);
                let max = self.number::<usize>("max_clients", &mut errors, Some(32));
                let cfg = TcpServerConfig {
                    bind: self.text("bind"),
                    port: port.unwrap_or(0),
                    echo: self.toggle("echo"),
                    max_clients: max.unwrap_or(32),
                    label,
                };
                collect(validate::tcp_server(&cfg, open), &mut errors);
                SessionConfig::TcpServer(cfg)
            }
            SessionKind::Udp => {
                let local = self.number::<u16>("local_port", &mut errors, Some(0));
                let remote_port = self.number::<u16>("remote_port", &mut errors, Some(0));
                let host = self.text("remote_host");
                let cfg = UdpConfig {
                    bind: self.text("bind"),
                    local_port: local.unwrap_or(0),
                    remote_host: (!host.is_empty()).then_some(host),
                    remote_port: remote_port.filter(|p| *p != 0),
                    echo: self.toggle("echo"),
                    broadcast: self.toggle("broadcast"),
                    label,
                };
                collect(validate::udp(&cfg, open), &mut errors);
                SessionConfig::Udp(cfg)
            }
        };
        // Deduplicate messages per field.
        errors.dedup_by(|a, b| a.0 == b.0);
        if errors.is_empty() {
            (Some(config), errors)
        } else {
            (None, errors)
        }
    }

    pub fn next(&mut self) {
        self.focus = (self.focus + 1) % (self.fields.len() + 1);
    }

    pub fn prev(&mut self) {
        let n = self.fields.len() + 1;
        self.focus = (self.focus + n - 1) % n;
    }

    /// Left/right: change transport, cycle choices, flip toggles, or cycle
    /// detected ports / baud presets.
    pub fn cycle(&mut self, forward: bool, recent: &[SessionConfig]) {
        if self.focus == 0 {
            let n = KINDS.len();
            self.kind_index = if forward {
                (self.kind_index + 1) % n
            } else {
                (self.kind_index + n - 1) % n
            };
            self.rebuild(recent);
            self.focus = 0;
            return;
        }
        let ports: Vec<String> = self.ports.iter().map(|p| p.port_name.clone()).collect();
        let Some(field) = self.fields.get_mut(self.focus - 1) else {
            return;
        };
        match &mut field.value {
            Value::Choice(opts, i) => {
                let n = opts.len();
                *i = if forward {
                    (*i + 1) % n
                } else {
                    (*i + n - 1) % n
                };
            }
            Value::Toggle(on) => *on = !*on,
            Value::Text(t) => {
                if field.key == "port" && self.kind_index == 0 && !ports.is_empty() {
                    let n = ports.len();
                    self.port_cycle = if forward {
                        (self.port_cycle + 1) % n
                    } else {
                        (self.port_cycle + n - 1) % n
                    };
                    *t = ports[self.port_cycle].clone();
                } else if field.key == "baud" {
                    let current: u32 = t.trim().parse().unwrap_or(115200);
                    let next = if forward {
                        BAUD_RATES
                            .iter()
                            .find(|b| **b > current)
                            .copied()
                            .unwrap_or(BAUD_RATES[0])
                    } else {
                        BAUD_RATES
                            .iter()
                            .rev()
                            .find(|b| **b < current)
                            .copied()
                            .unwrap_or(*BAUD_RATES.last().unwrap())
                    };
                    *t = next.to_string();
                }
            }
        }
    }

    pub fn type_char(&mut self, c: char) {
        if self.focus == 0 {
            return;
        }
        if let Some(field) = self.fields.get_mut(self.focus - 1) {
            match &mut field.value {
                Value::Text(t) if t.chars().count() < 253 => t.push(c),
                Value::Toggle(on) if c == ' ' => *on = !*on,
                _ => {}
            }
        }
    }

    pub fn backspace(&mut self) {
        if self.focus == 0 {
            return;
        }
        if let Some(Field {
            value: Value::Text(t),
            ..
        }) = self.fields.get_mut(self.focus - 1)
        {
            t.pop();
        }
    }
}
