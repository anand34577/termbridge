//! Serial port enumeration and TCP reachability probes.

use std::time::{Duration, Instant};

use serde::Serialize;
use tokio::net::TcpStream;

use crate::error::{CoreError, Result};

/// Serial port description suitable for JSON output.
#[derive(Debug, Clone, Serialize)]
pub struct PortInfo {
    pub port_name: String,
    /// `usb`, `pci` or `unknown`.
    pub kind: String,
    pub vid: Option<u16>,
    pub pid: Option<u16>,
    pub serial_number: Option<String>,
    pub manufacturer: Option<String>,
    pub product: Option<String>,
}

/// Enumerate serial ports visible to the operating system.
pub fn list_ports() -> Result<Vec<PortInfo>> {
    let ports = serialport::available_ports()?;
    let mut out: Vec<PortInfo> = ports
        .into_iter()
        .map(|p| match p.port_type {
            serialport::SerialPortType::UsbPort(info) => PortInfo {
                port_name: p.port_name,
                kind: "usb".into(),
                vid: Some(info.vid),
                pid: Some(info.pid),
                serial_number: info.serial_number,
                manufacturer: info.manufacturer,
                product: info.product,
            },
            serialport::SerialPortType::PciPort => PortInfo {
                port_name: p.port_name,
                kind: "pci".into(),
                vid: None,
                pid: None,
                serial_number: None,
                manufacturer: None,
                product: None,
            },
            _ => PortInfo {
                port_name: p.port_name,
                kind: "unknown".into(),
                vid: None,
                pid: None,
                serial_number: None,
                manufacturer: None,
                product: None,
            },
        })
        .collect();
    out.sort_by(|a, b| a.port_name.cmp(&b.port_name));
    Ok(out)
}

/// Result of a TCP reachability probe (a "ping" for TCP).
#[derive(Debug, Clone, Serialize)]
pub struct ProbeResult {
    pub host: String,
    pub port: u16,
    pub connected: bool,
    pub rtt_ms: u128,
    pub banner: Option<String>,
    pub banner_hex: Option<String>,
}

/// Try to open a TCP connection and optionally read a short banner.
pub async fn tcp_probe(
    host: &str,
    port: u16,
    timeout: Duration,
    read_banner: bool,
) -> Result<ProbeResult> {
    let timeout = timeout.min(Duration::from_secs(600));
    let start = Instant::now();
    let stream = tokio::time::timeout(timeout, TcpStream::connect((host, port)))
        .await
        .map_err(|_| CoreError::Timeout(timeout.as_millis() as u64))?
        .map_err(|e| CoreError::Connect(format!("{host}:{port}: {e}")))?;
    let rtt_ms = start.elapsed().as_millis();

    let mut banner = None;
    let mut banner_hex = None;
    if read_banner {
        use tokio::io::AsyncReadExt;
        let mut stream = stream;
        let mut buf = vec![0u8; 512];
        if let Ok(Ok(n)) =
            tokio::time::timeout(Duration::from_millis(250), stream.read(&mut buf)).await
        {
            if n > 0 {
                banner = Some(crate::codec::display_bytes(&buf[..n]));
                banner_hex = Some(crate::codec::to_hex(&buf[..n]));
            }
        }
    }

    Ok(ProbeResult {
        host: host.to_string(),
        port,
        connected: true,
        rtt_ms,
        banner,
        banner_hex,
    })
}
