//! Input validation shared by the connection forms, composer and settings.
//!
//! Validators return a list of `Issue`s keyed by field name so forms can show
//! the message right under the offending input and disable the submit button.

use std::net::{IpAddr, SocketAddr};

use crate::codec;
use crate::session::Parity;
use crate::session::SessionConfig;
use crate::{SerialConfig, SessionInfo, TcpClientConfig, TcpServerConfig, UdpConfig};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Debug, Clone)]
pub struct Issue {
    pub field: &'static str,
    pub message: String,
    pub severity: Severity,
}

#[derive(Debug, Default, Clone)]
pub struct Report {
    pub issues: Vec<Issue>,
}

impl Report {
    fn error(&mut self, field: &'static str, message: impl Into<String>) {
        self.issues.push(Issue {
            field,
            message: message.into(),
            severity: Severity::Error,
        });
    }

    fn warn(&mut self, field: &'static str, message: impl Into<String>) {
        self.issues.push(Issue {
            field,
            message: message.into(),
            severity: Severity::Warning,
        });
    }

    pub fn is_ok(&self) -> bool {
        !self.issues.iter().any(|i| i.severity == Severity::Error)
    }

    pub fn for_field(&self, field: &str) -> Option<&Issue> {
        self.issues
            .iter()
            .filter(|i| i.field == field)
            .min_by_key(|i| matches!(i.severity, Severity::Warning))
    }
}

pub const MIN_BAUD: u32 = 50;
pub const MAX_BAUD: u32 = 4_000_000;

pub fn serial(cfg: &SerialConfig, known_ports: &[String], open: &[SessionInfo]) -> Report {
    let mut r = Report::default();
    let port = cfg.port.trim();
    if port.is_empty() {
        r.error("port", "Choose or type a serial port.");
    } else if port.chars().any(char::is_whitespace) {
        r.error("port", "Port names cannot contain spaces.");
    } else if !known_ports.is_empty() && !known_ports.iter().any(|p| p == port) {
        r.warn("port", "Not in the detected list. It will still be tried.");
    }
    if open.iter().any(|s| match &s.config {
        SessionConfig::Serial(c) => c.port.eq_ignore_ascii_case(port),
        _ => false,
    }) {
        r.error("port", "This port is already open in another session.");
    }
    if !(MIN_BAUD..=MAX_BAUD).contains(&cfg.baud) {
        r.error(
            "baud",
            format!("Baud rate must be between {MIN_BAUD} and {MAX_BAUD}."),
        );
    }
    if !(5..=8).contains(&cfg.data_bits) {
        r.error("data_bits", "Data bits must be 5, 6, 7 or 8.");
    }
    if !(1..=2).contains(&cfg.stop_bits) {
        r.error("stop_bits", "Stop bits must be 1 or 2.");
    }
    if matches!(cfg.parity, Parity::Mark | Parity::Space) {
        r.error(
            "parity",
            "Mark/space parity is not supported by the serial backend. Use none, even or odd.",
        );
    }
    r
}

pub fn tcp_client(cfg: &TcpClientConfig) -> Report {
    let mut r = Report::default();
    host(&mut r, "host", &cfg.host);
    if cfg.port == 0 {
        r.error("port", "Port must be between 1 and 65535.");
    }
    if cfg.connect_timeout_ms < 100 || cfg.connect_timeout_ms > 120_000 {
        r.error("timeout", "Timeout must be between 100 ms and 120 s.");
    }
    r
}

pub fn tcp_server(cfg: &TcpServerConfig, open: &[SessionInfo]) -> Report {
    let mut r = Report::default();
    bind_ip(&mut r, "bind", &cfg.bind);
    // Port 0 asks the OS for an ephemeral port, which is useful for tests and
    // for agents that do not care which port they get.
    if cfg.port != 0 && cfg.port < 1024 && !cfg!(windows) {
        r.warn(
            "port",
            "Ports below 1024 usually need administrator rights.",
        );
    }
    if cfg.port != 0
        && open.iter().any(|s| match &s.config {
            SessionConfig::TcpServer(c) => c.port == cfg.port,
            _ => false,
        })
    {
        r.error("port", "A TCP server session already listens on this port.");
    }
    if cfg.max_clients == 0 || cfg.max_clients > 512 {
        r.error("max_clients", "Allow between 1 and 512 clients.");
    }
    r
}

pub fn udp(cfg: &UdpConfig, open: &[SessionInfo]) -> Report {
    let mut r = Report::default();
    bind_ip(&mut r, "bind", &cfg.bind);
    if cfg.local_port != 0
        && open.iter().any(|s| match &s.config {
            SessionConfig::Udp(c) => c.local_port == cfg.local_port,
            _ => false,
        })
    {
        r.error("local_port", "A UDP session already uses this local port.");
    }
    let has_host = cfg
        .remote_host
        .as_deref()
        .is_some_and(|h| !h.trim().is_empty());
    match (has_host, cfg.remote_port) {
        (true, None) => r.error(
            "remote_port",
            "Set a remote port, or clear the remote host.",
        ),
        (false, Some(_)) => r.error(
            "remote_host",
            "Set a remote host, or clear the remote port.",
        ),
        (true, Some(_)) => {
            let h = cfg.remote_host.as_deref().unwrap_or_default();
            host(&mut r, "remote_host", h);
            if is_broadcast(h) && !cfg.broadcast {
                r.error(
                    "remote_host",
                    "This is a broadcast address. Enable Broadcast to send to it.",
                );
            }
        }
        (false, None) => {}
    }
    r
}

/// Validate data typed into the composer. Returns the encoded bytes on success.
pub fn payload(input: &str, hex: bool) -> Result<Vec<u8>, String> {
    if input.is_empty() {
        return Err("Nothing to send.".into());
    }
    codec::parse_input(input, hex).map_err(|e| sentence(&e.to_string()))
}

pub fn macro_def(name: &str, data: &str, hex: bool) -> Option<String> {
    if name.trim().is_empty() {
        return Some("Give the macro a name.".into());
    }
    if name.chars().count() > 24 {
        return Some("Keep macro names to 24 characters.".into());
    }
    if data.is_empty() {
        return Some("Macro data is empty.".into());
    }
    codec::parse_input(data, hex)
        .err()
        .map(|e| sentence(&e.to_string()))
}

pub fn socket_addr(value: &str) -> Result<SocketAddr, String> {
    value
        .trim()
        .parse::<SocketAddr>()
        .map_err(|_| "Use IP:port, for example 127.0.0.1:8765.".to_string())
}

pub fn log_path(value: &str) -> Result<std::path::PathBuf, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err("Enter a file path for the log.".into());
    }
    let path = std::path::PathBuf::from(trimmed);
    if path.is_dir() {
        return Err("That path is a folder. Add a file name.".into());
    }
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            return Err(format!("Folder {} does not exist.", parent.display()));
        }
    }
    Ok(path)
}

fn host(r: &mut Report, field: &'static str, value: &str) {
    let v = value.trim();
    if v.is_empty() {
        r.error(field, "Enter a host name or IP address.");
        return;
    }
    if v.parse::<IpAddr>().is_ok() {
        return;
    }
    let valid = v.len() <= 253
        && v.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        });
    if !valid {
        r.error(field, "Not a valid host name or IP address.");
    }
}

fn bind_ip(r: &mut Report, field: &'static str, value: &str) {
    match value.trim().parse::<IpAddr>() {
        Ok(_) => {}
        Err(_) => r.error(
            field,
            "Bind to an IP address: 0.0.0.0 (all), 127.0.0.1 (local) or an interface IP.",
        ),
    }
}

fn is_broadcast(host: &str) -> bool {
    match host.trim().parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) => ip.is_broadcast() || ip.octets()[3] == 255,
        _ => false,
    }
}

/// Capitalise an error message and end it with a full stop.
pub fn sentence(text: &str) -> String {
    let mut chars = text.trim().chars();
    let mut out = match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => return String::new(),
    };
    if !out.ends_with('.') {
        out.push('.');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_validation() {
        let mut r = Report::default();
        host(&mut r, "h", "192.168.1.10");
        host(&mut r, "h", "device.local");
        host(&mut r, "h", "::1");
        assert!(r.is_ok());
        let mut bad = Report::default();
        host(&mut bad, "h", "bad host");
        assert!(!bad.is_ok());
        let mut bad = Report::default();
        host(&mut bad, "h", "-x.com");
        assert!(!bad.is_ok());
    }

    #[test]
    fn udp_remote_pairs() {
        let mut cfg = UdpConfig {
            remote_host: Some("10.0.0.1".into()),
            remote_port: None,
            ..Default::default()
        };
        assert!(!udp(&cfg, &[]).is_ok());
        cfg.remote_port = Some(4000);
        assert!(udp(&cfg, &[]).is_ok());
        cfg.remote_host = Some("255.255.255.255".into());
        cfg.broadcast = false;
        assert!(!udp(&cfg, &[]).is_ok());
        cfg.broadcast = true;
        assert!(udp(&cfg, &[]).is_ok());
    }

    #[test]
    fn serial_ranges() {
        let mut cfg = SerialConfig {
            port: "COM3".into(),
            ..Default::default()
        };
        assert!(serial(&cfg, &[], &[]).is_ok());
        cfg.baud = 10;
        assert!(!serial(&cfg, &[], &[]).is_ok());
        cfg.baud = 115200;
        cfg.parity = Parity::Mark;
        assert!(!serial(&cfg, &[], &[]).is_ok());
    }

    #[test]
    fn tcp_server_allows_an_ephemeral_port() {
        let cfg = TcpServerConfig {
            bind: "127.0.0.1".into(),
            port: 0,
            ..Default::default()
        };
        assert!(tcp_server(&cfg, &[]).is_ok());
        let cfg = TcpServerConfig {
            bind: "not-an-ip".into(),
            port: 9000,
            ..Default::default()
        };
        assert!(!tcp_server(&cfg, &[]).is_ok());
    }

    #[test]
    fn payload_parsing() {
        assert_eq!(payload("AT\\r", false).unwrap(), b"AT\r");
        assert_eq!(payload("01 02 ff", true).unwrap(), vec![1, 2, 255]);
        assert!(payload("zz", true).is_err());
        assert!(payload("", false).is_err());
    }

    #[test]
    fn socket_addr_parsing() {
        assert!(socket_addr("127.0.0.1:8765").is_ok());
        assert!(socket_addr("localhost").is_err());
    }
}
