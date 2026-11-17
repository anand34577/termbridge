//! UDP broadcast discovery.
//!
//! The discovery payload itself is device specific, so the probe payload is
//! configurable. Every datagram received during the listen window is reported
//! with its sender address, RTT and both hex and ASCII renderings.

use std::collections::HashSet;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::net::UdpSocket;

use crate::error::Result;

/// Longest discovery / listen window accepted, so a bad timeout cannot
/// overflow `Instant` arithmetic.
const MAX_WAIT_MS: u64 = 10 * 60 * 1000;

#[derive(Debug, Clone, Deserialize)]
pub struct ScanRequest {
    /// Broadcast or unicast destination address.
    #[serde(default = "default_target")]
    pub target: String,
    #[serde(default = "default_port")]
    pub port: u16,
    /// Probe payload. Empty sends a single zero byte.
    #[serde(default)]
    pub payload: Vec<u8>,
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
    /// Local port to bind; 0 selects an ephemeral port.
    #[serde(default)]
    pub local_port: u16,
}

fn default_target() -> String {
    "255.255.255.255".to_string()
}

fn default_port() -> u16 {
    4000
}

fn default_timeout() -> u64 {
    1000
}

impl Default for ScanRequest {
    fn default() -> Self {
        Self {
            target: default_target(),
            port: default_port(),
            payload: Vec::new(),
            timeout_ms: default_timeout(),
            local_port: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ScanResponse {
    pub addr: String,
    pub bytes: usize,
    pub hex: String,
    pub ascii: String,
    pub rtt_ms: u128,
}

/// Send a probe to `target:port` and collect every response until the timeout.
pub async fn udp_scan(req: ScanRequest) -> Result<Vec<ScanResponse>> {
    let socket = UdpSocket::bind(("0.0.0.0", req.local_port)).await?;
    socket.set_broadcast(true)?;
    let payload = if req.payload.is_empty() {
        vec![0u8]
    } else {
        req.payload.clone()
    };
    let started = Instant::now();
    socket
        .send_to(&payload, (req.target.as_str(), req.port))
        .await?;

    let deadline = started + Duration::from_millis(req.timeout_ms.clamp(1, MAX_WAIT_MS));
    let mut seen: HashSet<SocketAddr> = HashSet::new();
    let mut out = Vec::new();
    let mut buf = vec![0u8; 65535];

    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, socket.recv_from(&mut buf)).await {
            Ok(Ok((n, addr))) => {
                if !seen.insert(addr) {
                    continue;
                }
                out.push(ScanResponse {
                    addr: addr.to_string(),
                    bytes: n,
                    hex: crate::codec::to_hex(&buf[..n]),
                    ascii: crate::codec::display_bytes(&buf[..n]),
                    rtt_ms: started.elapsed().as_millis(),
                });
            }
            Ok(Err(e)) => return Err(e.into()),
            Err(_) => break,
        }
    }
    Ok(out)
}

/// Listen for unsolicited UDP datagrams on a local port.
pub async fn udp_listen(port: u16, timeout: Duration) -> Result<Vec<ScanResponse>> {
    let socket = UdpSocket::bind(("0.0.0.0", port)).await?;
    let started = Instant::now();
    let deadline = started + timeout.min(Duration::from_millis(MAX_WAIT_MS));
    let mut out = Vec::new();
    let mut buf = vec![0u8; 65535];
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, socket.recv_from(&mut buf)).await {
            Ok(Ok((n, addr))) => out.push(ScanResponse {
                addr: addr.to_string(),
                bytes: n,
                hex: crate::codec::to_hex(&buf[..n]),
                ascii: crate::codec::display_bytes(&buf[..n]),
                rtt_ms: started.elapsed().as_millis(),
            }),
            Ok(Err(e)) => return Err(e.into()),
            Err(_) => break,
        }
    }
    Ok(out)
}
