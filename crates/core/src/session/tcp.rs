//! TCP client and TCP server transports.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

use super::{Command, WorkerCtx};
use crate::nvt::NvtParser;
use crate::types::{ClientInfo, Direction};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TcpClientConfig {
    pub host: String,
    pub port: u16,
    /// Enable Telnet / NVT negotiation handling.
    pub nvt: bool,
    /// Reconnect automatically after a disconnect.
    pub reconnect: bool,
    pub connect_timeout_ms: u64,
    pub label: Option<String>,
}

impl Default for TcpClientConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 23,
            nvt: false,
            reconnect: false,
            connect_timeout_ms: 3000,
            label: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TcpServerConfig {
    pub bind: String,
    pub port: u16,
    /// Echo every received byte back to the sending client.
    pub echo: bool,
    pub max_clients: usize,
    pub label: Option<String>,
}

impl Default for TcpServerConfig {
    fn default() -> Self {
        Self {
            bind: "0.0.0.0".to_string(),
            port: 9000,
            echo: false,
            max_clients: 32,
            label: None,
        }
    }
}

pub(crate) async fn client_worker(
    cfg: TcpClientConfig,
    ctx: WorkerCtx,
    rx: &mut UnboundedReceiver<Command>,
) {
    let addr = format!("{}:{}", cfg.host, cfg.port);
    let mut backoff_ms: u64 = 500;

    loop {
        if ctx.is_closed() {
            break;
        }
        ctx.info(format!("connecting to {addr}"));
        let connect = tokio::time::timeout(
            Duration::from_millis(cfg.connect_timeout_ms.max(1)),
            TcpStream::connect(&addr),
        )
        .await;

        let stream = match connect {
            Ok(Ok(stream)) => stream,
            Ok(Err(err)) => {
                ctx.error(format!("connect to {addr} failed: {err}"));
                if !cfg.reconnect {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                backoff_ms = (backoff_ms * 2).min(5000);
                continue;
            }
            Err(_) => {
                ctx.error(format!(
                    "connect to {addr} timed out after {} ms",
                    cfg.connect_timeout_ms
                ));
                if !cfg.reconnect {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                backoff_ms = (backoff_ms * 2).min(5000);
                continue;
            }
        };

        let _ = stream.set_nodelay(true);
        let peer = stream.peer_addr().ok().map(|a| a.to_string());
        ctx.set_connected(peer.clone());
        ctx.info(format!(
            "connected to {}",
            peer.clone().unwrap_or_else(|| addr.clone())
        ));
        backoff_ms = 500;

        let (mut reader, mut writer) = stream.into_split();
        let mut nvt = NvtParser::new();
        let mut buf = vec![0u8; 16384];
        let mut user_closed = false;

        loop {
            tokio::select! {
                cmd = rx.recv() => match cmd {
                    Some(Command::Write(data)) => {
                        if writer.write_all(&data).await.is_err() {
                            break;
                        }
                        let _ = writer.flush().await;
                        ctx.push(Direction::Tx, &data);
                    }
                    Some(Command::Close) | None => {
                        user_closed = true;
                        let _ = writer.shutdown().await;
                        break;
                    }
                    _ => {}
                },
                read = reader.read(&mut buf) => match read {
                    Ok(0) => {
                        ctx.info("peer closed the connection");
                        break;
                    }
                    Ok(n) => {
                        if cfg.nvt {
                            let out = nvt.feed(&buf[..n]);
                            for event in out.events {
                                ctx.push(Direction::Nvt, event.as_bytes());
                            }
                            if !out.replies.is_empty()
                                && writer.write_all(&out.replies).await.is_ok()
                            {
                                ctx.push(Direction::Tx, &out.replies);
                            }
                            if !out.data.is_empty() {
                                ctx.push(Direction::Rx, &out.data);
                            }
                        } else {
                            ctx.push(Direction::Rx, &buf[..n]);
                        }
                    }
                    Err(err) => {
                        ctx.error(format!("read failed: {err}"));
                        break;
                    }
                }
            }
        }

        ctx.set_disconnected(if user_closed {
            "closed"
        } else {
            "disconnected"
        });
        if user_closed || ctx.is_closed() || !cfg.reconnect {
            break;
        }
        ctx.info("reconnecting...");
        tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
        backoff_ms = (backoff_ms * 2).min(5000);
    }

    ctx.set_disconnected("closed");
}

enum ClientCmd {
    Data(Vec<u8>),
    Close,
}

struct ClientSlot {
    peer: String,
    since: chrono::DateTime<Utc>,
    tx: UnboundedSender<ClientCmd>,
}

type Clients = Arc<Mutex<HashMap<u64, ClientSlot>>>;

fn clients_info(clients: &Clients) -> Vec<ClientInfo> {
    let mut out: Vec<ClientInfo> = clients
        .lock()
        .unwrap()
        .iter()
        .map(|(id, slot)| ClientInfo {
            id: *id,
            peer: slot.peer.clone(),
            connected_at: slot.since,
        })
        .collect();
    out.sort_by_key(|c| c.id);
    out
}

pub(crate) async fn server_worker(
    listener: TcpListener,
    cfg: TcpServerConfig,
    ctx: WorkerCtx,
    rx: &mut UnboundedReceiver<Command>,
) {
    let local = listener
        .local_addr()
        .map(|a| a.to_string())
        .unwrap_or_else(|_| format!("{}:{}", cfg.bind, cfg.port));
    let clients: Clients = Arc::new(Mutex::new(HashMap::new()));
    let next_client = Arc::new(AtomicU64::new(1));
    let echo = Arc::new(AtomicBool::new(cfg.echo));

    ctx.set_connected(Some(local.clone()));
    ctx.info(format!("listening on {local}"));

    loop {
        tokio::select! {
            cmd = rx.recv() => match cmd {
                Some(Command::Write(data)) => {
                    let senders: Vec<UnboundedSender<ClientCmd>> = clients
                        .lock()
                        .unwrap()
                        .values()
                        .map(|slot| slot.tx.clone())
                        .collect();
                    if senders.is_empty() {
                        ctx.info("no clients connected");
                    } else {
                        for tx in senders {
                            let _ = tx.send(ClientCmd::Data(data.clone()));
                        }
                        ctx.push(Direction::Tx, &data);
                    }
                }
                Some(Command::SetEcho(value)) => {
                    echo.store(value, Ordering::SeqCst);
                    ctx.info(format!("echo {}", if value { "on" } else { "off" }));
                }
                Some(Command::Close) | None => break,
                _ => {}
            },
            accepted = listener.accept() => match accepted {
                Ok((stream, addr)) => {
                    let count = clients.lock().unwrap_or_else(|e| e.into_inner()).len();
                    if count >= cfg.max_clients {
                        drop(stream);
                        ctx.info(format!(
                            "rejected {addr}: maximum of {} clients reached",
                            cfg.max_clients
                        ));
                        continue;
                    }
                    let client_id = next_client.fetch_add(1, Ordering::SeqCst);
                    let (client_tx, client_rx) = mpsc::unbounded_channel();
                    clients.lock().unwrap_or_else(|e| e.into_inner()).insert(
                        client_id,
                        ClientSlot {
                            peer: addr.to_string(),
                            since: Utc::now(),
                            tx: client_tx,
                        },
                    );
                    ctx.set_clients(clients_info(&clients));
                    ctx.info(format!("client {addr} connected"));
                    let client_ctx = ctx.clone();
                    let client_clients = clients.clone();
                    let client_echo = echo.clone();
                    tokio::spawn(async move {
                        client_task(
                            client_id,
                            addr,
                            stream,
                            client_ctx,
                            client_clients,
                            client_echo,
                            client_rx,
                        )
                        .await;
                    });
                }
                Err(err) => {
                    ctx.error(format!("accept failed: {err}"));
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        }
    }

    for slot in clients.lock().unwrap_or_else(|e| e.into_inner()).values() {
        let _ = slot.tx.send(ClientCmd::Close);
    }
    clients.lock().unwrap_or_else(|e| e.into_inner()).clear();
    ctx.set_clients(Vec::new());
    ctx.set_disconnected("closed");
    ctx.info("server closed");
}

async fn client_task(
    id: u64,
    addr: std::net::SocketAddr,
    stream: TcpStream,
    ctx: WorkerCtx,
    clients: Clients,
    echo: Arc<AtomicBool>,
    mut rx: UnboundedReceiver<ClientCmd>,
) {
    let (mut reader, mut writer) = stream.into_split();
    let mut buf = vec![0u8; 16384];
    loop {
        tokio::select! {
            cmd = rx.recv() => match cmd {
                Some(ClientCmd::Data(data)) => {
                    if writer.write_all(&data).await.is_err() {
                        break;
                    }
                    let _ = writer.flush().await;
                }
                Some(ClientCmd::Close) | None => break,
            },
            read = reader.read(&mut buf) => match read {
                Ok(0) => break,
                Ok(n) => {
                    ctx.push(Direction::Rx, &buf[..n]);
                    if echo.load(Ordering::SeqCst) && writer.write_all(&buf[..n]).await.is_ok() {
                        let _ = writer.flush().await;
                        ctx.push(Direction::Tx, &buf[..n]);
                    }
                }
                Err(_) => break,
            }
        }
    }
    clients
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&id);
    ctx.set_clients(clients_info(&clients));
    ctx.info(format!("client {addr} disconnected"));
}
