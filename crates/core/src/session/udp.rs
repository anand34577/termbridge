//! UDP terminal transport.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::net::UdpSocket;
use tokio::sync::mpsc::UnboundedReceiver;

use super::{Command, WorkerCtx};
use crate::types::Direction;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UdpConfig {
    pub bind: String,
    pub local_port: u16,
    pub remote_host: Option<String>,
    pub remote_port: Option<u16>,
    /// Echo received datagrams back to their sender.
    pub echo: bool,
    /// Allow sending to broadcast addresses.
    pub broadcast: bool,
    pub label: Option<String>,
}

impl Default for UdpConfig {
    fn default() -> Self {
        Self {
            bind: "0.0.0.0".to_string(),
            local_port: 0,
            remote_host: None,
            remote_port: None,
            echo: false,
            broadcast: false,
            label: None,
        }
    }
}

pub(crate) async fn worker(
    socket: UdpSocket,
    cfg: UdpConfig,
    remote: Option<SocketAddr>,
    ctx: WorkerCtx,
    rx: &mut UnboundedReceiver<Command>,
) {
    let local = socket
        .local_addr()
        .map(|a| a.to_string())
        .unwrap_or_else(|_| format!("{}:{}", cfg.bind, cfg.local_port));
    let echo = Arc::new(AtomicBool::new(cfg.echo));
    let mut last_peer: Option<SocketAddr> = None;

    ctx.set_connected(Some(local.clone()));
    ctx.info(match remote {
        Some(addr) => format!("UDP {local} -> {addr}"),
        None => format!("UDP {local} (no remote configured)"),
    });

    let mut buf = vec![0u8; 65535];

    loop {
        tokio::select! {
            cmd = rx.recv() => match cmd {
                Some(Command::Write(data)) => {
                    let target = remote.or(last_peer);
                    match target {
                        Some(addr) => match socket.send_to(&data, addr).await {
                            Ok(_) => ctx.push(Direction::Tx, &data),
                            Err(err) => ctx.error(format!("UDP send to {addr} failed: {err}")),
                        },
                        None => ctx.error("no UDP destination: set a remote host/port or wait for an incoming datagram"),
                    }
                }
                Some(Command::SetEcho(value)) => {
                    echo.store(value, Ordering::SeqCst);
                    ctx.info(format!("echo {}", if value { "on" } else { "off" }));
                }
                Some(Command::Close) | None => break,
                _ => {}
            },
            received = socket.recv_from(&mut buf) => match received {
                Ok((n, src)) => {
                    last_peer = Some(src);
                    ctx.set_peer(Some(src.to_string()));
                    ctx.push(Direction::Rx, &buf[..n]);
                    if echo.load(Ordering::SeqCst)
                        && socket.send_to(&buf[..n], src).await.is_ok()
                    {
                        ctx.push(Direction::Tx, &buf[..n]);
                    }
                }
                Err(err) => {
                    ctx.error(format!("UDP receive failed: {err}"));
                    break;
                }
            }
        }
    }

    ctx.set_disconnected("closed");
    ctx.info("UDP socket closed");
}
