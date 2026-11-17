//! Session manager: one live serial / TCP / UDP connection per session.
//!
//! Each session owns a worker (a thread for serial ports, a Tokio task for
//! sockets), a bounded ring buffer of recent traffic, byte counters, optional
//! file logging and a command channel for writes and control operations.

mod serial;
mod tcp;
mod udp;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc};

use crate::error::{CoreError, Result};
use crate::logging::{FileSink, LogFormat};
use crate::responder::{Responder, RuleDef, RuleStatus};
use crate::ring::{ReadBatch, RingBuffer};
use crate::types::{ClientInfo, ControlOp, Direction, ModemLines, SessionEvent, SessionKind};

pub use serial::{FlowControl, Parity, SerialConfig};
pub use tcp::{TcpClientConfig, TcpServerConfig};
pub use udp::UdpConfig;

/// Configuration snapshot stored with a session.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionConfig {
    Serial(SerialConfig),
    TcpClient(TcpClientConfig),
    TcpServer(TcpServerConfig),
    Udp(UdpConfig),
}

impl SessionConfig {
    pub fn kind(&self) -> SessionKind {
        match self {
            SessionConfig::Serial(_) => SessionKind::Serial,
            SessionConfig::TcpClient(_) => SessionKind::TcpClient,
            SessionConfig::TcpServer(_) => SessionKind::TcpServer,
            SessionConfig::Udp(_) => SessionKind::Udp,
        }
    }
}

/// Commands accepted by a session worker.
pub(crate) enum Command {
    Write(Vec<u8>),
    Close,
    SetRts(bool),
    SetDtr(bool),
    SetBreak(bool),
    SetEcho(bool),
}

/// Live status of a session, updated by its worker.
pub struct SessionStatus {
    pub connected: AtomicBool,
    pub closed: AtomicBool,
    pub tx_bytes: AtomicU64,
    pub rx_bytes: AtomicU64,
    pub tx_packets: AtomicU64,
    pub rx_packets: AtomicU64,
    pub peer: Mutex<Option<String>>,
    pub clients: Mutex<Vec<ClientInfo>>,
    pub modem: Mutex<ModemLines>,
    pub error: Mutex<Option<String>>,
}

impl Default for SessionStatus {
    fn default() -> Self {
        Self {
            connected: AtomicBool::new(false),
            closed: AtomicBool::new(false),
            tx_bytes: AtomicU64::new(0),
            rx_bytes: AtomicU64::new(0),
            tx_packets: AtomicU64::new(0),
            rx_packets: AtomicU64::new(0),
            peer: Mutex::new(None),
            clients: Mutex::new(Vec::new()),
            modem: Mutex::new(ModemLines::default()),
            error: Mutex::new(None),
        }
    }
}

/// JSON-friendly snapshot of a session.
#[derive(Debug, Clone, Serialize)]
pub struct SessionInfo {
    pub id: String,
    pub kind: SessionKind,
    pub label: String,
    pub created_at: DateTime<Utc>,
    pub connected: bool,
    pub peer: Option<String>,
    pub clients: Vec<ClientInfo>,
    pub tx_bytes: u64,
    pub rx_bytes: u64,
    pub tx_packets: u64,
    pub rx_packets: u64,
    pub modem: ModemLines,
    pub error: Option<String>,
    pub config: SessionConfig,
    pub log_bytes: usize,
    pub next_seq: u64,
    /// Active file recording, if any.
    pub recording: Option<RecordingInfo>,
    /// Number of enabled auto-response rules.
    pub auto_responses: usize,
}

/// Where and how a session is being recorded.
#[derive(Debug, Clone, Serialize)]
pub struct RecordingInfo {
    pub path: String,
    pub format: LogFormat,
    pub bytes: u64,
}

/// A registered session.
pub struct Session {
    pub id: String,
    pub kind: SessionKind,
    pub label: String,
    pub created_at: DateTime<Utc>,
    pub config: SessionConfig,
    pub status: Arc<SessionStatus>,
    pub log: Arc<RingBuffer>,
    pub(crate) tx: mpsc::UnboundedSender<Command>,
    pub logfile: Arc<Mutex<Option<FileSink>>>,
    pub responder: Arc<Mutex<Responder>>,
    handle: Mutex<Option<SessionTask>>,
}

enum SessionTask {
    Thread(std::thread::JoinHandle<()>),
    Task(tokio::task::JoinHandle<()>),
}

impl Session {
    pub fn info(&self) -> SessionInfo {
        SessionInfo {
            id: self.id.clone(),
            kind: self.kind,
            label: self.label.clone(),
            created_at: self.created_at,
            connected: self.status.connected.load(Ordering::SeqCst),
            peer: self
                .status
                .peer
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone(),
            clients: self
                .status
                .clients
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone(),
            tx_bytes: self.status.tx_bytes.load(Ordering::Relaxed),
            rx_bytes: self.status.rx_bytes.load(Ordering::Relaxed),
            tx_packets: self.status.tx_packets.load(Ordering::Relaxed),
            rx_packets: self.status.rx_packets.load(Ordering::Relaxed),
            modem: *self.status.modem.lock().unwrap_or_else(|e| e.into_inner()),
            error: self
                .status
                .error
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone(),
            config: self.config.clone(),
            log_bytes: self.log.len_bytes(),
            next_seq: self.log.next_seq(),
            recording: self
                .logfile
                .lock()
                .unwrap()
                .as_ref()
                .map(|sink| RecordingInfo {
                    path: sink.path().display().to_string(),
                    format: sink.format(),
                    bytes: sink.bytes_written(),
                }),
            auto_responses: self
                .responder
                .lock()
                .unwrap()
                .status()
                .iter()
                .filter(|r| r.def.enabled)
                .count(),
        }
    }
}

/// Context handed to a worker. All logging, counters and events go through it.
#[derive(Clone)]
pub(crate) struct WorkerCtx {
    pub id: String,
    pub status: Arc<SessionStatus>,
    pub log: Arc<RingBuffer>,
    pub logfile: Arc<Mutex<Option<FileSink>>>,
    pub responder: Arc<Mutex<Responder>>,
    pub commands: mpsc::UnboundedSender<Command>,
    pub events: broadcast::Sender<SessionEvent>,
}

impl WorkerCtx {
    pub fn is_closed(&self) -> bool {
        self.status.closed.load(Ordering::SeqCst)
    }

    pub fn push(&self, dir: Direction, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        match dir {
            Direction::Rx => {
                self.status
                    .rx_bytes
                    .fetch_add(data.len() as u64, Ordering::Relaxed);
                self.status.rx_packets.fetch_add(1, Ordering::Relaxed);
            }
            Direction::Tx => {
                self.status
                    .tx_bytes
                    .fetch_add(data.len() as u64, Ordering::Relaxed);
                self.status.tx_packets.fetch_add(1, Ordering::Relaxed);
            }
            _ => {}
        }
        self.log.push(dir, data);
        if let Ok(mut guard) = self.logfile.lock() {
            if let Some(sink) = guard.as_mut() {
                if let Err(err) = sink.write_chunk(crate::util::now_ms(), dir, data) {
                    tracing::warn!("log write failed for {}: {err}", self.id);
                }
            }
        }
        let _ = self.events.send(SessionEvent::Data {
            id: self.id.clone(),
            dir,
            len: data.len(),
        });
        if dir == Direction::Rx {
            let replies = match self.responder.lock() {
                Ok(mut responder) => responder.feed(data),
                Err(_) => Vec::new(),
            };
            for reply in replies {
                let _ = self.commands.send(Command::Write(reply));
            }
        }
    }

    pub fn info(&self, message: impl Into<String>) {
        let message = message.into();
        self.log.push(Direction::Info, message.as_bytes());
        let _ = self.events.send(SessionEvent::Info {
            id: self.id.clone(),
            message,
        });
    }

    pub fn error(&self, message: impl Into<String>) {
        let message = message.into();
        *self.status.error.lock().unwrap_or_else(|e| e.into_inner()) = Some(message.clone());
        self.log.push(Direction::Error, message.as_bytes());
        let _ = self.events.send(SessionEvent::Error {
            id: self.id.clone(),
            message,
        });
    }

    pub fn set_connected(&self, peer: Option<String>) {
        self.status.connected.store(true, Ordering::SeqCst);
        *self.status.error.lock().unwrap_or_else(|e| e.into_inner()) = None;
        *self.status.peer.lock().unwrap_or_else(|e| e.into_inner()) = peer.clone();
        let _ = self.events.send(SessionEvent::Connected {
            id: self.id.clone(),
            peer,
        });
    }

    pub fn set_disconnected(&self, reason: impl Into<String>) {
        let was_connected = self.status.connected.swap(false, Ordering::SeqCst);
        if was_connected {
            let _ = self.events.send(SessionEvent::Disconnected {
                id: self.id.clone(),
                reason: reason.into(),
            });
        }
    }

    pub fn set_peer(&self, peer: Option<String>) {
        *self.status.peer.lock().unwrap_or_else(|e| e.into_inner()) = peer;
    }

    pub fn set_clients(&self, clients: Vec<ClientInfo>) {
        let count = clients.len();
        *self
            .status
            .clients
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = clients;
        let _ = self.events.send(SessionEvent::ClientsChanged {
            id: self.id.clone(),
            count,
        });
    }

    pub fn set_modem(&self, lines: ModemLines) {
        let mut guard = self.status.modem.lock().unwrap_or_else(|e| e.into_inner());
        if *guard != lines {
            *guard = lines;
            drop(guard);
            let _ = self.events.send(SessionEvent::ModemChanged {
                id: self.id.clone(),
                lines,
            });
        }
    }
}

struct Inner {
    sessions: RwLock<HashMap<String, Arc<Session>>>,
    events: broadcast::Sender<SessionEvent>,
    next_id: AtomicU64,
    buffer_bytes: usize,
}

/// Entry point for all transport work.
#[derive(Clone)]
pub struct SessionManager {
    inner: Arc<Inner>,
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new()
    }
}

struct Prepared {
    id: String,
    kind: SessionKind,
    label: String,
    config: SessionConfig,
    status: Arc<SessionStatus>,
    log: Arc<RingBuffer>,
    logfile: Arc<Mutex<Option<FileSink>>>,
    responder: Arc<Mutex<Responder>>,
    tx: mpsc::UnboundedSender<Command>,
    ctx: WorkerCtx,
}

impl SessionManager {
    pub fn new() -> Self {
        Self::with_buffer(1024 * 1024)
    }

    pub fn with_buffer(buffer_bytes: usize) -> Self {
        let (events, _) = broadcast::channel(1024);
        Self {
            inner: Arc::new(Inner {
                sessions: RwLock::new(HashMap::new()),
                events,
                next_id: AtomicU64::new(1),
                buffer_bytes: buffer_bytes.max(16 * 1024),
            }),
        }
    }

    pub fn events(&self) -> broadcast::Receiver<SessionEvent> {
        self.inner.events.subscribe()
    }

    fn prepare(
        &self,
        kind: SessionKind,
        label: String,
        config: SessionConfig,
    ) -> (Prepared, mpsc::UnboundedReceiver<Command>) {
        let n = self.inner.next_id.fetch_add(1, Ordering::SeqCst);
        let id = format!("{}-{n}", kind.prefix());
        let status = Arc::new(SessionStatus::default());
        let log = Arc::new(RingBuffer::new(self.inner.buffer_bytes));
        let logfile = Arc::new(Mutex::new(None));
        let responder = Arc::new(Mutex::new(Responder::default()));
        let (tx, rx) = mpsc::unbounded_channel();
        let ctx = WorkerCtx {
            id: id.clone(),
            status: status.clone(),
            log: log.clone(),
            logfile: logfile.clone(),
            responder: responder.clone(),
            commands: tx.clone(),
            events: self.inner.events.clone(),
        };
        (
            Prepared {
                id,
                kind,
                label,
                config,
                status,
                log,
                logfile,
                responder,
                tx,
                ctx,
            },
            rx,
        )
    }

    fn finish(&self, p: Prepared, task: SessionTask) -> SessionInfo {
        let session = Arc::new(Session {
            id: p.id.clone(),
            kind: p.kind,
            label: p.label.clone(),
            created_at: Utc::now(),
            config: p.config,
            status: p.status,
            log: p.log,
            tx: p.tx,
            logfile: p.logfile,
            responder: p.responder,
            handle: Mutex::new(Some(task)),
        });
        let info = session.info();
        self.inner
            .sessions
            .write()
            .unwrap()
            .insert(p.id.clone(), session);
        let _ = self.inner.events.send(SessionEvent::Opened {
            id: p.id,
            kind: p.kind,
            label: p.label,
        });
        info
    }

    /// Open a serial port. The port is opened synchronously (on a blocking
    /// thread) so configuration errors are reported immediately.
    pub async fn open_serial(&self, cfg: SerialConfig) -> Result<SessionInfo> {
        if cfg.port.trim().is_empty() {
            return Err(CoreError::InvalidInput(
                "serial port name is required".into(),
            ));
        }
        let open_cfg = cfg.clone();
        let port = tokio::task::spawn_blocking(move || serial::open_port(&open_cfg))
            .await
            .map_err(|e| CoreError::Other(format!("serial open task failed: {e}")))??;
        let label = cfg
            .label
            .clone()
            .unwrap_or_else(|| format!("{} @ {}", cfg.port, cfg.baud));
        let (p, rx) = self.prepare(
            SessionKind::Serial,
            label,
            SessionConfig::Serial(cfg.clone()),
        );
        let ctx = p.ctx.clone();
        let thread_name = format!("serial-{}", p.id);
        let handle = std::thread::Builder::new()
            .name(thread_name)
            .spawn(move || serial::worker(port, cfg, ctx, rx))
            .map_err(CoreError::Io)?;
        // The port is already open, so the session is usable as soon as the
        // worker thread exists. This closes the race between `open_serial`
        // returning and the first `write`.
        p.status.connected.store(true, Ordering::SeqCst);
        *p.status.peer.lock().unwrap_or_else(|e| e.into_inner()) = Some(p.label.clone());
        Ok(self.finish(p, SessionTask::Thread(handle)))
    }

    /// Start a TCP client. The connection is established asynchronously; poll
    /// `wait_connected` or the session status for progress.
    pub async fn open_tcp_client(&self, cfg: TcpClientConfig) -> Result<SessionInfo> {
        if cfg.host.trim().is_empty() {
            return Err(CoreError::InvalidInput("host is required".into()));
        }
        if cfg.port == 0 {
            return Err(CoreError::InvalidInput(
                "port must be between 1 and 65535".into(),
            ));
        }
        let label = cfg
            .label
            .clone()
            .unwrap_or_else(|| format!("{}:{}", cfg.host, cfg.port));
        let (p, mut rx) = self.prepare(
            SessionKind::TcpClient,
            label,
            SessionConfig::TcpClient(cfg.clone()),
        );
        let ctx = p.ctx.clone();
        let task = tokio::spawn(async move {
            tcp::client_worker(cfg, ctx, &mut rx).await;
        });
        Ok(self.finish(p, SessionTask::Task(task)))
    }

    /// Bind and start a TCP server. Binding errors are returned immediately.
    pub async fn open_tcp_server(&self, cfg: TcpServerConfig) -> Result<SessionInfo> {
        let listener = tokio::net::TcpListener::bind((cfg.bind.as_str(), cfg.port))
            .await
            .map_err(|e| CoreError::Connect(format!("bind {}:{}: {e}", cfg.bind, cfg.port)))?;
        let local = listener
            .local_addr()
            .map(|a| a.to_string())
            .unwrap_or_else(|_| format!("{}:{}", cfg.bind, cfg.port));
        let label = cfg
            .label
            .clone()
            .unwrap_or_else(|| format!("listen {local}"));
        let (p, mut rx) = self.prepare(
            SessionKind::TcpServer,
            label,
            SessionConfig::TcpServer(cfg.clone()),
        );
        // The listener is already bound, so the session is usable immediately.
        p.status.connected.store(true, Ordering::SeqCst);
        *p.status.peer.lock().unwrap_or_else(|e| e.into_inner()) = Some(local.clone());
        let ctx = p.ctx.clone();
        let task = tokio::spawn(async move {
            tcp::server_worker(listener, cfg, ctx, &mut rx).await;
        });
        Ok(self.finish(p, SessionTask::Task(task)))
    }

    /// Bind a UDP socket and start the terminal worker.
    pub async fn open_udp(&self, cfg: UdpConfig) -> Result<SessionInfo> {
        let has_host = cfg
            .remote_host
            .as_deref()
            .is_some_and(|h| !h.trim().is_empty());
        if has_host != cfg.remote_port.is_some() {
            return Err(CoreError::InvalidInput(
                "remote_host and remote_port must be set together".into(),
            ));
        }
        let socket = tokio::net::UdpSocket::bind((cfg.bind.as_str(), cfg.local_port))
            .await
            .map_err(|e| {
                CoreError::Connect(format!("bind {}:{}: {e}", cfg.bind, cfg.local_port))
            })?;
        socket.set_broadcast(cfg.broadcast)?;
        let remote = match (&cfg.remote_host, cfg.remote_port) {
            (Some(host), Some(port)) if !host.trim().is_empty() => {
                let mut addrs = tokio::net::lookup_host((host.as_str(), port))
                    .await
                    .map_err(|e| CoreError::Connect(format!("resolve {host}:{port}: {e}")))?;
                addrs.next()
            }
            _ => None,
        };
        let local = socket
            .local_addr()
            .map(|a| a.to_string())
            .unwrap_or_else(|_| format!("{}:{}", cfg.bind, cfg.local_port));
        let label = cfg.label.clone().unwrap_or_else(|| format!("udp {local}"));
        let (p, mut rx) = self.prepare(SessionKind::Udp, label, SessionConfig::Udp(cfg.clone()));
        // The socket is already bound, so the session is usable immediately.
        p.status.connected.store(true, Ordering::SeqCst);
        *p.status.peer.lock().unwrap_or_else(|e| e.into_inner()) = Some(local.clone());
        let ctx = p.ctx.clone();
        let task = tokio::spawn(async move {
            udp::worker(socket, cfg, remote, ctx, &mut rx).await;
        });
        Ok(self.finish(p, SessionTask::Task(task)))
    }

    pub fn list(&self) -> Vec<SessionInfo> {
        let mut infos: Vec<SessionInfo> = self
            .inner
            .sessions
            .read()
            .unwrap()
            .values()
            .map(|s| s.info())
            .collect();
        infos.sort_by_key(|i| i.created_at);
        infos
    }

    pub fn get(&self, id: &str) -> Option<Arc<Session>> {
        self.inner
            .sessions
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(id)
            .cloned()
    }

    pub fn info(&self, id: &str) -> Result<SessionInfo> {
        self.get(id)
            .map(|s| s.info())
            .ok_or_else(|| CoreError::SessionNotFound(id.to_string()))
    }

    pub fn close(&self, id: &str) -> Result<()> {
        let session = self
            .inner
            .sessions
            .write()
            .unwrap()
            .remove(id)
            .ok_or_else(|| CoreError::SessionNotFound(id.to_string()))?;
        session.status.closed.store(true, Ordering::SeqCst);
        let _ = session.tx.send(Command::Close);
        if let Some(sink) = session
            .logfile
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_mut()
        {
            let _ = sink.flush();
        }
        if let Some(task) = session
            .handle
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            match task {
                // A TCP server owns per-client tasks. Let its worker observe
                // Close and shut them down instead of aborting it and leaking
                // the client sockets.
                SessionTask::Task(handle) if session.kind == SessionKind::TcpServer => {
                    drop(handle);
                }
                SessionTask::Task(handle) => handle.abort(),
                // The serial thread observes Command::Close and exits; dropping
                // the handle detaches it.
                SessionTask::Thread(_handle) => {}
            }
        }
        let _ = self
            .inner
            .events
            .send(SessionEvent::Closed { id: id.to_string() });
        Ok(())
    }

    pub fn close_all(&self) {
        let ids: Vec<String> = self
            .inner
            .sessions
            .read()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        for id in ids {
            let _ = self.close(&id);
        }
    }

    pub fn write(&self, id: &str, data: &[u8]) -> Result<usize> {
        let session = self
            .get(id)
            .ok_or_else(|| CoreError::SessionNotFound(id.to_string()))?;
        if session.status.closed.load(Ordering::SeqCst) {
            return Err(CoreError::SessionClosed(id.to_string()));
        }
        if !session.status.connected.load(Ordering::SeqCst) {
            return Err(CoreError::Connect(format!("session {id} is not connected")));
        }
        session
            .tx
            .send(Command::Write(data.to_vec()))
            .map_err(|_| CoreError::SessionClosed(id.to_string()))?;
        Ok(data.len())
    }

    pub fn read_since(&self, id: &str, since_seq: u64, max_bytes: usize) -> Result<ReadBatch> {
        let session = self
            .get(id)
            .ok_or_else(|| CoreError::SessionNotFound(id.to_string()))?;
        Ok(session.log.read_since(since_seq, max_bytes))
    }

    pub fn tail(&self, id: &str, max_bytes: usize) -> Result<ReadBatch> {
        let session = self
            .get(id)
            .ok_or_else(|| CoreError::SessionNotFound(id.to_string()))?;
        Ok(session.log.tail(max_bytes))
    }

    pub fn clear_log(&self, id: &str) -> Result<()> {
        let session = self
            .get(id)
            .ok_or_else(|| CoreError::SessionNotFound(id.to_string()))?;
        session.log.clear();
        Ok(())
    }

    pub fn set_control(&self, id: &str, op: ControlOp) -> Result<()> {
        let session = self
            .get(id)
            .ok_or_else(|| CoreError::SessionNotFound(id.to_string()))?;
        let command = match op {
            ControlOp::Rts(v) => {
                if session.kind != SessionKind::Serial {
                    return Err(CoreError::Unsupported(
                        "RTS is only available on serial sessions".into(),
                    ));
                }
                Command::SetRts(v)
            }
            ControlOp::Dtr(v) => {
                if session.kind != SessionKind::Serial {
                    return Err(CoreError::Unsupported(
                        "DTR is only available on serial sessions".into(),
                    ));
                }
                Command::SetDtr(v)
            }
            ControlOp::Break(v) => {
                if session.kind != SessionKind::Serial {
                    return Err(CoreError::Unsupported(
                        "break is only available on serial sessions".into(),
                    ));
                }
                Command::SetBreak(v)
            }
            ControlOp::Echo(v) => {
                if !matches!(session.kind, SessionKind::TcpServer | SessionKind::Udp) {
                    return Err(CoreError::Unsupported(
                        "echo is only available on TCP server and UDP sessions".into(),
                    ));
                }
                Command::SetEcho(v)
            }
        };
        session
            .tx
            .send(command)
            .map_err(|_| CoreError::SessionClosed(id.to_string()))?;
        Ok(())
    }

    /// Enable (`Some(path)`) or disable (`None`) text file logging for a session.
    pub fn set_logging(&self, id: &str, path: Option<PathBuf>, append: bool) -> Result<()> {
        self.set_logging_with(id, path, append, LogFormat::Text)
    }

    /// Replace the auto-response rules of a session. Invalid rules are
    /// rejected and the previous rules stay active.
    pub fn set_auto_responses(&self, id: &str, rules: &[RuleDef]) -> Result<Vec<RuleStatus>> {
        let session = self
            .get(id)
            .ok_or_else(|| CoreError::SessionNotFound(id.to_string()))?;
        let mut responder = session.responder.lock().unwrap_or_else(|e| e.into_inner());
        responder.set_rules(rules)?;
        Ok(responder.status())
    }

    pub fn auto_responses(&self, id: &str) -> Result<Vec<RuleStatus>> {
        let session = self
            .get(id)
            .ok_or_else(|| CoreError::SessionNotFound(id.to_string()))?;
        let status = session
            .responder
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .status();
        Ok(status)
    }

    /// Enable or disable recording in the given format.
    pub fn set_logging_with(
        &self,
        id: &str,
        path: Option<PathBuf>,
        append: bool,
        format: LogFormat,
    ) -> Result<()> {
        let session = self
            .get(id)
            .ok_or_else(|| CoreError::SessionNotFound(id.to_string()))?;
        let mut guard = session.logfile.lock().unwrap_or_else(|e| e.into_inner());
        match path {
            Some(path) => {
                if let Some(sink) = guard.as_mut() {
                    let _ = sink.flush();
                }
                *guard = Some(FileSink::open_with(&path, append, format)?);
            }
            None => {
                if let Some(sink) = guard.as_mut() {
                    let _ = sink.flush();
                }
                *guard = None;
            }
        }
        Ok(())
    }

    /// Wait until a session reports `connected`, fails, or the timeout expires.
    pub async fn wait_connected(&self, id: &str, timeout: Duration) -> Result<()> {
        let start = std::time::Instant::now();
        loop {
            let session = self
                .get(id)
                .ok_or_else(|| CoreError::SessionNotFound(id.to_string()))?;
            if session.status.connected.load(Ordering::SeqCst) {
                return Ok(());
            }
            if let Some(err) = session
                .status
                .error
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
            {
                return Err(CoreError::Connect(err));
            }
            if session.status.closed.load(Ordering::SeqCst) {
                return Err(CoreError::SessionClosed(id.to_string()));
            }
            if start.elapsed() >= timeout {
                return Err(CoreError::Timeout(timeout.as_millis() as u64));
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}
