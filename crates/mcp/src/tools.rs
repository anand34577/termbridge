//! MCP tool definitions and dispatch.

use std::time::{Duration, Instant};

use base64::Engine;
use serde_json::{json, Value};

use termbridge_core::checksum::{self, ChecksumKind};
use termbridge_core::codec::{self, ViewFormat};
use termbridge_core::config::MacroDef;
use termbridge_core::logging::LogFormat;
use termbridge_core::responder::RuleDef;
use termbridge_core::{
    net, CoreError, Result, SerialConfig, SessionInfo, TcpClientConfig, TcpServerConfig, UdpConfig,
};

use crate::server::McpServer;
use crate::{PROTOCOL_VERSION, SUPPORTED_PROTOCOL_VERSIONS, VERSION};

pub fn is_known(name: &str) -> bool {
    definitions().iter().any(|d| d["name"] == name)
}

/// Tool result on success.
pub fn ok(structured: Value) -> Value {
    let text = serde_json::to_string_pretty(&structured).unwrap_or_default();
    json!({
        "content": [{ "type": "text", "text": text }],
        "structuredContent": structured,
        "isError": false
    })
}

/// Tool result on execution failure. Protocol-level errors use JSON-RPC errors.
pub fn tool_error(message: impl Into<String>) -> Value {
    json!({
        "content": [{ "type": "text", "text": message.into() }],
        "isError": true
    })
}

pub async fn call(server: &McpServer, name: &str, args: &Value) -> Result<Value> {
    match name {
        "list_serial_ports" => {
            let ports = tokio::task::spawn_blocking(net::list_ports)
                .await
                .map_err(|e| CoreError::Other(format!("port enumeration failed: {e}")))??;
            Ok(ok(json!({ "count": ports.len(), "ports": ports })))
        }
        "open_serial" => {
            let mut cfg = SerialConfig::default();
            cfg.port = require_str(args, "port")?;
            cfg.baud = u32_arg(args, "baud", cfg.baud)?;
            cfg.data_bits = u8_arg(args, "data_bits", cfg.data_bits)?;
            cfg.parity = parse_parity(&str_arg(args, "parity").unwrap_or_else(|| "none".into()))?;
            cfg.stop_bits = u8_arg(args, "stop_bits", cfg.stop_bits)?;
            cfg.flow_control =
                parse_flow(&str_arg(args, "flow_control").unwrap_or_else(|| "none".into()))?;
            cfg.dtr = bool_arg(args, "dtr").unwrap_or(cfg.dtr);
            cfg.rts = bool_arg(args, "rts").unwrap_or(cfg.rts);
            cfg.label = str_arg(args, "label");
            let report = termbridge_core::validate::serial(&cfg, &[], &server.manager.list());
            if let Some(message) = first_error(&report) {
                return Err(CoreError::InvalidInput(message));
            }
            let info = server.manager.open_serial(cfg).await?;
            Ok(ok(json!({ "session": info })))
        }
        "open_tcp_client" => {
            let mut cfg = TcpClientConfig::default();
            cfg.host = require_str(args, "host")?;
            cfg.port = u16_arg(args, "port")?;
            cfg.nvt = bool_arg(args, "nvt").unwrap_or(cfg.nvt);
            cfg.reconnect = bool_arg(args, "reconnect").unwrap_or(cfg.reconnect);
            cfg.connect_timeout_ms =
                u64_arg(args, "connect_timeout_ms").unwrap_or(cfg.connect_timeout_ms);
            cfg.label = str_arg(args, "label");
            let report = termbridge_core::validate::tcp_client(&cfg);
            if let Some(message) = first_error(&report) {
                return Err(CoreError::InvalidInput(message));
            }
            let info = server.manager.open_tcp_client(cfg.clone()).await?;
            if cfg.reconnect {
                // Keep retrying in the background; report the first attempt.
                let _ = server
                    .manager
                    .wait_connected(&info.id, Duration::from_millis(300))
                    .await;
                let info = server.manager.info(&info.id).unwrap_or(info);
                return Ok(ok(json!({ "session": info })));
            }
            let wait = Duration::from_millis(cfg.connect_timeout_ms.clamp(100, 120_000) + 500);
            match server.manager.wait_connected(&info.id, wait).await {
                Ok(()) => {
                    let info = server.manager.info(&info.id).unwrap_or(info);
                    Ok(ok(json!({ "session": info })))
                }
                Err(err) => {
                    // Do not leak a dead session when the connect failed.
                    let _ = server.manager.close(&info.id);
                    Err(err)
                }
            }
        }
        "open_tcp_server" => {
            let mut cfg = TcpServerConfig::default();
            cfg.bind = str_arg(args, "bind").unwrap_or(cfg.bind);
            cfg.port = u16_arg(args, "port")?;
            cfg.echo = bool_arg(args, "echo").unwrap_or(cfg.echo);
            cfg.max_clients = u64_arg(args, "max_clients")
                .map(|v| {
                    usize::try_from(v)
                        .map_err(|_| CoreError::InvalidInput("max_clients is out of range".into()))
                })
                .transpose()?
                .unwrap_or(cfg.max_clients);
            cfg.label = str_arg(args, "label");
            let report = termbridge_core::validate::tcp_server(&cfg, &server.manager.list());
            if let Some(message) = first_error(&report) {
                return Err(CoreError::InvalidInput(message));
            }
            let info = server.manager.open_tcp_server(cfg).await?;
            Ok(ok(json!({ "session": info })))
        }
        "open_udp" => {
            let mut cfg = UdpConfig::default();
            cfg.bind = str_arg(args, "bind").unwrap_or(cfg.bind);
            cfg.local_port = opt_u16_arg(args, "local_port")?.unwrap_or(0);
            cfg.remote_host = str_arg(args, "remote_host");
            cfg.remote_port = opt_u16_arg(args, "remote_port")?;
            cfg.echo = bool_arg(args, "echo").unwrap_or(cfg.echo);
            cfg.broadcast = bool_arg(args, "broadcast").unwrap_or(cfg.broadcast);
            cfg.label = str_arg(args, "label");
            let report = termbridge_core::validate::udp(&cfg, &server.manager.list());
            if let Some(message) = first_error(&report) {
                return Err(CoreError::InvalidInput(message));
            }
            let info = server.manager.open_udp(cfg).await?;
            Ok(ok(json!({ "session": info })))
        }
        "list_sessions" => {
            let sessions = server.manager.list();
            Ok(ok(json!({ "count": sessions.len(), "sessions": sessions })))
        }
        "get_session" => {
            let id = session_id(args)?;
            let info = server.manager.info(&id)?;
            let mut out = json!({ "session": info });
            if bool_arg(args, "include_log").unwrap_or(false) {
                let max =
                    (u64_arg(args, "log_bytes").unwrap_or(4096) as usize).clamp(1, MAX_READ_BYTES);
                let format = view_format(args)?;
                let batch = server.manager.tail(&id, max)?;
                out["log"] = render_batch(&batch, format);
            }
            Ok(ok(out))
        }
        "write_session" => {
            let id = session_id(args)?;
            let format = str_arg(args, "format").unwrap_or_else(|| "ascii".into());
            let data = require_str(args, "data")?;
            let mut bytes = match parse_input_format(&format)? {
                InputFormat::Ascii => codec::parse_escapes(&data)?,
                InputFormat::Hex => codec::parse_hex(&data)?,
                InputFormat::Base64 => base64::engine::general_purpose::STANDARD
                    .decode(data.trim())
                    .map_err(|e| CoreError::InvalidInput(format!("invalid base64: {e}")))?,
            };
            if let Some(kind) = str_arg(args, "checksum") {
                let kind = ChecksumKind::parse(&kind)?;
                bytes = checksum::append(kind, &bytes);
            }
            match parse_eol(&str_arg(args, "eol").unwrap_or_else(|| "none".into()))? {
                Eol::None => {}
                Eol::Cr => bytes.push(b'\r'),
                Eol::Lf => bytes.push(b'\n'),
                Eol::CrLf => bytes.extend_from_slice(b"\r\n"),
            }
            server.manager.write(&id, &bytes)?;
            Ok(ok(json!({
                "session_id": id,
                "bytes_written": bytes.len(),
                "hex": codec::to_hex(&bytes)
            })))
        }
        "read_session" => {
            let id = session_id(args)?;
            let since = u64_arg(args, "since_seq").unwrap_or(0);
            let max =
                (u64_arg(args, "max_bytes").unwrap_or(65536) as usize).clamp(1, MAX_READ_BYTES);
            let timeout_ms = u64_arg(args, "timeout_ms").unwrap_or(0);
            let format = view_format(args)?;
            let deadline = Instant::now() + timeout_duration(timeout_ms);
            loop {
                let batch = server.manager.read_since(&id, since, max)?;
                if !batch.is_empty() || timeout_ms == 0 || Instant::now() >= deadline {
                    let mut out = render_batch(&batch, format);
                    out["session_id"] = json!(id);
                    return Ok(ok(out));
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        }
        "wait_for_data" => {
            let id = session_id(args)?;
            let since = u64_arg(args, "since_seq").unwrap_or(0);
            let max =
                (u64_arg(args, "max_bytes").unwrap_or(65536) as usize).clamp(1, MAX_READ_BYTES);
            let timeout_ms = u64_arg(args, "timeout_ms").unwrap_or(2000);
            let format = view_format(args)?;
            let pattern = match str_arg(args, "pattern") {
                Some(text) => {
                    let pf = str_arg(args, "pattern_format").unwrap_or_else(|| "ascii".into());
                    match parse_input_format(&pf)? {
                        InputFormat::Hex => codec::parse_hex(&text)?,
                        InputFormat::Ascii => codec::parse_escapes(&text)?,
                        InputFormat::Base64 => {
                            return Err(CoreError::InvalidInput(
                                "pattern_format must be ascii or hex".into(),
                            ))
                        }
                    }
                }
                None => Vec::new(),
            };
            let deadline = Instant::now() + timeout_duration(timeout_ms);
            let mut chunks: Vec<termbridge_core::ring::Chunk> = Vec::new();
            let mut buffered = 0usize;
            let mut next_seq = since;
            loop {
                let batch = server.manager.read_since(&id, next_seq, max)?;
                if !batch.is_empty() {
                    next_seq = batch.next_seq;
                    for chunk in batch.chunks {
                        buffered += chunk.data.len();
                        chunks.push(chunk);
                    }
                    // Bound memory for long waits on a busy link.
                    while buffered > max && chunks.len() > 1 {
                        let removed = chunks.remove(0);
                        buffered -= removed.data.len();
                    }
                }
                let all: Vec<u8> = chunks.iter().flat_map(|c| c.data.iter().copied()).collect();
                let matched = if pattern.is_empty() {
                    !all.is_empty()
                } else {
                    contains(&all, &pattern)
                };
                if matched || Instant::now() >= deadline {
                    let rendered = render_chunks(&chunks, format);
                    return Ok(ok(json!({
                        "session_id": id,
                        "matched": matched,
                        "timeout": !matched,
                        "bytes": all.len(),
                        "text": rendered["text"],
                        "chunks": rendered["chunks"],
                        "next_seq": next_seq
                    })));
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        }
        "close_session" => {
            let id = session_id(args)?;
            server.manager.close(&id)?;
            Ok(ok(json!({ "closed": id })))
        }
        "set_control_lines" => {
            let id = session_id(args)?;
            let mut applied = Vec::new();
            if let Some(value) = bool_arg(args, "rts") {
                server
                    .manager
                    .set_control(&id, termbridge_core::ControlOp::Rts(value))?;
                applied.push(format!("rts={value}"));
            }
            if let Some(value) = bool_arg(args, "dtr") {
                server
                    .manager
                    .set_control(&id, termbridge_core::ControlOp::Dtr(value))?;
                applied.push(format!("dtr={value}"));
            }
            if let Some(value) = bool_arg(args, "break") {
                server
                    .manager
                    .set_control(&id, termbridge_core::ControlOp::Break(value))?;
                applied.push(format!("break={value}"));
            }
            if let Some(value) = bool_arg(args, "echo") {
                server
                    .manager
                    .set_control(&id, termbridge_core::ControlOp::Echo(value))?;
                applied.push(format!("echo={value}"));
            }
            if applied.is_empty() {
                return Err(CoreError::InvalidInput(
                    "provide at least one of rts, dtr, break or echo".into(),
                ));
            }
            Ok(ok(json!({ "session_id": id, "applied": applied })))
        }
        "set_logging" => {
            let id = session_id(args)?;
            let enabled = bool_arg(args, "enabled").unwrap_or(true);
            if enabled {
                let path = require_str(args, "path")?;
                let append = bool_arg(args, "append").unwrap_or(false);
                let format = LogFormat::parse(&str_arg(args, "format").unwrap_or_default())?;
                server
                    .manager
                    .set_logging_with(&id, Some(path.clone().into()), append, format)?;
                Ok(ok(json!({
                    "session_id": id,
                    "logging": true,
                    "path": path,
                    "format": format.as_str()
                })))
            } else {
                server.manager.set_logging(&id, None, false)?;
                Ok(ok(json!({ "session_id": id, "logging": false })))
            }
        }
        "clear_session_log" => {
            let id = session_id(args)?;
            server.manager.clear_log(&id)?;
            Ok(ok(json!({ "session_id": id, "cleared": true })))
        }
        "udp_scan" => {
            let request = termbridge_core::discovery::ScanRequest {
                target: str_arg(args, "target")
                    .or_else(|| str_arg(args, "broadcast"))
                    .unwrap_or_else(|| "255.255.255.255".into()),
                port: opt_u16_arg(args, "port")?.unwrap_or(4000),
                payload: match str_arg(args, "payload_hex") {
                    Some(hex) => codec::parse_hex(&hex)?,
                    None => Vec::new(),
                },
                timeout_ms: u64_arg(args, "timeout_ms").unwrap_or(1000),
                local_port: opt_u16_arg(args, "local_port")?.unwrap_or(0),
            };
            let responses = termbridge_core::discovery::udp_scan(request).await?;
            Ok(ok(
                json!({ "count": responses.len(), "responses": responses }),
            ))
        }
        "udp_listen" => {
            let port = u16_arg(args, "port")?;
            let timeout_ms = u64_arg(args, "timeout_ms").unwrap_or(2000);
            let responses =
                termbridge_core::discovery::udp_listen(port, timeout_duration(timeout_ms)).await?;
            Ok(ok(
                json!({ "count": responses.len(), "responses": responses }),
            ))
        }
        "tcp_probe" => {
            let host = require_str(args, "host")?;
            let port = u16_arg(args, "port")?;
            let timeout_ms = u64_arg(args, "timeout_ms").unwrap_or(2000);
            let banner = bool_arg(args, "read_banner").unwrap_or(true);
            let result = net::tcp_probe(&host, port, timeout_duration(timeout_ms), banner).await?;
            Ok(ok(serde_json::to_value(result).unwrap_or_default()))
        }
        "send_file" => {
            let id = session_id(args)?;
            let path = require_str(args, "path")?;
            let chunk_size = u64_arg(args, "chunk_size").unwrap_or(4096).clamp(1, 65536) as usize;
            let delay_ms = u64_arg(args, "delay_ms").unwrap_or(0);
            let meta = std::fs::metadata(&path)
                .map_err(|e| CoreError::InvalidInput(format!("read {path}: {e}")))?;
            if !meta.is_file() {
                return Err(CoreError::InvalidInput(format!("{path} is not a file")));
            }
            if meta.len() > MAX_SEND_FILE {
                return Err(CoreError::InvalidInput(format!(
                    "{path} is larger than the {} MiB limit",
                    MAX_SEND_FILE / (1024 * 1024)
                )));
            }
            let bytes = std::fs::read(&path)
                .map_err(|e| CoreError::InvalidInput(format!("read {path}: {e}")))?;
            let mut sent = 0usize;
            for chunk in bytes.chunks(chunk_size) {
                server.manager.write(&id, chunk)?;
                sent += chunk.len();
                if delay_ms > 0 {
                    tokio::time::sleep(Duration::from_millis(delay_ms.min(60_000))).await;
                }
            }
            Ok(ok(json!({
                "session_id": id,
                "path": path,
                "bytes_sent": sent,
                "chunks": sent.div_ceil(chunk_size)
            })))
        }
        "list_macros" => {
            let cfg = server.config.get();
            Ok(ok(json!({ "macros": cfg.macros })))
        }
        "run_macro" => {
            let id = session_id(args)?;
            let name = require_str(args, "name")?;
            let cfg = server.config.get();
            let mac: MacroDef = cfg
                .macros
                .iter()
                .find(|m| m.name.eq_ignore_ascii_case(&name))
                .cloned()
                .ok_or_else(|| CoreError::InvalidInput(format!("unknown macro: {name}")))?;
            let bytes = codec::parse_input(&mac.data, mac.hex)?;
            server.manager.write(&id, &bytes)?;
            Ok(ok(json!({
                "session_id": id,
                "macro": mac.name,
                "bytes_written": bytes.len(),
                "hex": codec::to_hex(&bytes)
            })))
        }
        "get_server_info" => Ok(ok(server_info(server))),
        "set_auto_responses" => {
            let id = session_id(args)?;
            let rules: Vec<RuleDef> = match args.get("rules") {
                Some(value) => serde_json::from_value(value.clone()).map_err(|e| {
                    CoreError::InvalidInput(format!(
                        "rules must be a list of {{pattern, reply, hex?, enabled?}}: {e}"
                    ))
                })?,
                None => return Err(CoreError::InvalidInput("rules is required".into())),
            };
            let status = server.manager.set_auto_responses(&id, &rules)?;
            Ok(ok(json!({ "session_id": id, "rules": status })))
        }
        "get_auto_responses" => {
            let id = session_id(args)?;
            let status = server.manager.auto_responses(&id)?;
            Ok(ok(json!({ "session_id": id, "rules": status })))
        }
        "compute_checksum" => {
            let data = require_str(args, "data")?;
            let format = str_arg(args, "format").unwrap_or_else(|| "ascii".into());
            let bytes = match parse_input_format(&format)? {
                InputFormat::Ascii => codec::parse_escapes(&data)?,
                InputFormat::Hex => codec::parse_hex(&data)?,
                InputFormat::Base64 => base64::engine::general_purpose::STANDARD
                    .decode(data.trim())
                    .map_err(|e| CoreError::InvalidInput(format!("invalid base64: {e}")))?,
            };
            let kinds: Vec<ChecksumKind> = match str_arg(args, "algorithm") {
                Some(name) => vec![ChecksumKind::parse(&name)?],
                None => ChecksumKind::ALL[1..].to_vec(),
            };
            let results: Vec<Value> = kinds
                .iter()
                .map(|kind| {
                    let sum = checksum::compute(*kind, &bytes);
                    json!({
                        "algorithm": kind,
                        "label": kind.label(),
                        "checksum_hex": codec::to_hex(&sum),
                        "frame_hex": codec::to_hex(&checksum::append(*kind, &bytes)),
                    })
                })
                .collect();
            Ok(ok(json!({
                "input_length": bytes.len(),
                "input_hex": codec::to_hex(&bytes),
                "results": results
            })))
        }
        other => Err(CoreError::Unsupported(format!("unknown tool: {other}"))),
    }
}

pub fn server_info(server: &McpServer) -> Value {
    json!({
        "name": crate::SERVER_NAME,
        "version": VERSION,
        "core_version": termbridge_core::VERSION,
        "protocol_version": PROTOCOL_VERSION,
        "supported_protocol_versions": SUPPORTED_PROTOCOL_VERSIONS,
        "platform": {
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "family": std::env::consts::FAMILY
        },
        "config_path": server.config.path().display().to_string(),
        "uptime_secs": server.started.elapsed().as_secs(),
        "sessions": server.manager.list().len(),
        "transports": ["stdio", "http"]
    })
}

/// Render a batch of chunks into text plus a structured chunk list.
pub fn render_batch(batch: &termbridge_core::ring::ReadBatch, format: ViewFormat) -> Value {
    let chunks = render_chunks(&batch.chunks, format);
    json!({
        "text": chunks["text"],
        "chunks": chunks["chunks"],
        "next_seq": batch.next_seq,
        "dropped_chunks": batch.dropped_chunks,
        "bytes": batch.bytes()
    })
}

fn render_chunks(chunks: &[termbridge_core::ring::Chunk], format: ViewFormat) -> Value {
    let mut text = String::new();
    let mut items = Vec::with_capacity(chunks.len());
    for chunk in chunks {
        let rendered = match chunk.dir {
            termbridge_core::Direction::Info
            | termbridge_core::Direction::Error
            | termbridge_core::Direction::Nvt => codec::escape_bytes(&chunk.data),
            _ => codec::format_bytes(&chunk.data, format),
        };
        // A chunk that ends with a line break should not add a blank line.
        let rendered = rendered.strip_suffix('\n').unwrap_or(&rendered);
        text.push_str(&format!(
            "[{}] {} {}\n",
            termbridge_core::util::format_ts(chunk.ts_ms, false),
            chunk.dir.as_str(),
            rendered
        ));
        items.push(json!({
            "seq": chunk.seq,
            "ts": termbridge_core::util::format_ts(chunk.ts_ms, false),
            "dir": chunk.dir.as_str(),
            "bytes": chunk.data.len(),
            "hex": codec::to_hex(&chunk.data),
            "text": codec::escape_bytes(&chunk.data)
        }));
    }
    json!({ "text": text, "chunks": items })
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }
    if needle.len() > haystack.len() {
        return false;
    }
    haystack.windows(needle.len()).any(|w| w == needle)
}

fn missing(key: &str) -> CoreError {
    CoreError::InvalidInput(format!("missing required argument '{key}'"))
}

/// Largest file `send_file` accepts, matching the desktop app.
const MAX_SEND_FILE: u64 = 16 * 1024 * 1024;
/// Largest single read response, so a client cannot ask for unbounded memory.
const MAX_READ_BYTES: usize = 16 * 1024 * 1024;
/// Longest single blocking wait accepted from a client.
const MAX_TIMEOUT_MS: u64 = 10 * 60 * 1000;

fn timeout_duration(ms: u64) -> Duration {
    Duration::from_millis(ms.min(MAX_TIMEOUT_MS))
}

fn first_error(report: &termbridge_core::validate::Report) -> Option<String> {
    report
        .issues
        .iter()
        .find(|i| i.severity == termbridge_core::validate::Severity::Error)
        .map(|i| format!("{}: {}", i.field, i.message))
}

fn u16_arg(args: &Value, key: &str) -> Result<u16> {
    let value = u64_arg(args, key).ok_or_else(|| missing(key))?;
    u16::try_from(value)
        .map_err(|_| CoreError::InvalidInput(format!("{key} must be between 0 and 65535")))
}

fn opt_u16_arg(args: &Value, key: &str) -> Result<Option<u16>> {
    match u64_arg(args, key) {
        Some(value) => u16::try_from(value)
            .map(Some)
            .map_err(|_| CoreError::InvalidInput(format!("{key} must be between 0 and 65535"))),
        None => Ok(None),
    }
}

fn u32_arg(args: &Value, key: &str, default: u32) -> Result<u32> {
    match u64_arg(args, key) {
        Some(value) => u32::try_from(value)
            .map_err(|_| CoreError::InvalidInput(format!("{key} is out of range"))),
        None => Ok(default),
    }
}

fn u8_arg(args: &Value, key: &str, default: u8) -> Result<u8> {
    match u64_arg(args, key) {
        Some(value) => u8::try_from(value)
            .map_err(|_| CoreError::InvalidInput(format!("{key} is out of range"))),
        None => Ok(default),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum InputFormat {
    Ascii,
    Hex,
    Base64,
}

fn parse_input_format(value: &str) -> Result<InputFormat> {
    match value.trim().to_ascii_lowercase().as_str() {
        "ascii" | "text" | "raw" => Ok(InputFormat::Ascii),
        "hex" => Ok(InputFormat::Hex),
        "base64" | "b64" => Ok(InputFormat::Base64),
        other => Err(CoreError::InvalidInput(format!(
            "unknown input format '{other}' (expected ascii, hex or base64)"
        ))),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Eol {
    None,
    Cr,
    Lf,
    CrLf,
}

fn parse_eol(value: &str) -> Result<Eol> {
    match value.trim().to_ascii_lowercase().as_str() {
        "" | "none" => Ok(Eol::None),
        "cr" => Ok(Eol::Cr),
        "lf" => Ok(Eol::Lf),
        "crlf" => Ok(Eol::CrLf),
        other => Err(CoreError::InvalidInput(format!(
            "unknown eol '{other}' (expected none, cr, lf or crlf)"
        ))),
    }
}

fn parse_parity(value: &str) -> Result<termbridge_core::session::Parity> {
    use termbridge_core::session::Parity;
    match value.trim().to_ascii_lowercase().as_str() {
        "" | "none" => Ok(Parity::None),
        "even" => Ok(Parity::Even),
        "odd" => Ok(Parity::Odd),
        "mark" => Ok(Parity::Mark),
        "space" => Ok(Parity::Space),
        other => Err(CoreError::InvalidInput(format!(
            "unknown parity '{other}' (expected none, even, odd, mark or space)"
        ))),
    }
}

fn parse_flow(value: &str) -> Result<termbridge_core::session::FlowControl> {
    use termbridge_core::session::FlowControl;
    match value.trim().to_ascii_lowercase().as_str() {
        "" | "none" => Ok(FlowControl::None),
        "rts_cts" | "rtscts" | "hardware" => Ok(FlowControl::RtsCts),
        "xon_xoff" | "xonxoff" | "software" => Ok(FlowControl::XonXoff),
        other => Err(CoreError::InvalidInput(format!(
            "unknown flow_control '{other}' (expected none, rts_cts or xon_xoff)"
        ))),
    }
}

fn str_arg(args: &Value, key: &str) -> Option<String> {
    args.get(key).and_then(|v| match v {
        Value::String(s) => Some(s.clone()),
        Value::Null => None,
        other => Some(other.to_string()),
    })
}

fn require_str(args: &Value, key: &str) -> Result<String> {
    str_arg(args, key).ok_or_else(|| missing(key))
}

fn u64_arg(args: &Value, key: &str) -> Option<u64> {
    args.get(key).and_then(|v| match v {
        Value::Number(n) => n.as_u64(),
        Value::String(s) => s.trim().parse::<u64>().ok(),
        _ => None,
    })
}

fn bool_arg(args: &Value, key: &str) -> Option<bool> {
    args.get(key).and_then(|v| match v {
        Value::Bool(b) => Some(*b),
        Value::String(s) => match s.to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" | "on" => Some(true),
            "false" | "0" | "no" | "off" => Some(false),
            _ => None,
        },
        Value::Number(n) => n.as_u64().map(|n| n != 0),
        _ => None,
    })
}

fn session_id(args: &Value) -> Result<String> {
    str_arg(args, "session_id")
        .or_else(|| str_arg(args, "session"))
        .ok_or_else(|| missing("session_id"))
}

fn view_format(args: &Value) -> Result<ViewFormat> {
    match str_arg(args, "format") {
        Some(value) => ViewFormat::parse(&value),
        None => Ok(ViewFormat::Ascii),
    }
}

/// Tool catalogue exposed through `tools/list`.
pub fn definitions() -> Vec<Value> {
    vec![
        json!({
            "name": "list_serial_ports",
            "title": "List serial ports",
            "description": "Enumerate serial ports visible to the operating system, including USB VID/PID and product names.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
        }),
        json!({
            "name": "open_serial",
            "title": "Open a serial port",
            "description": "Open a serial port as a terminal session. Returns a session_id used by write_session, read_session and close_session.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "port": { "type": "string", "description": "Port name, e.g. COM3 or /dev/ttyUSB0" },
                    "baud": { "type": "integer", "minimum": 50, "maximum": 4000000, "default": 115200 },
                    "data_bits": { "type": "integer", "enum": [5, 6, 7, 8], "default": 8 },
                    "parity": { "type": "string", "enum": ["none", "even", "odd", "mark", "space"], "default": "none" },
                    "stop_bits": { "type": "integer", "enum": [1, 2], "default": 1 },
                    "flow_control": { "type": "string", "enum": ["none", "rts_cts", "xon_xoff"], "default": "none" },
                    "dtr": { "type": "boolean", "default": false },
                    "rts": { "type": "boolean", "default": false },
                    "label": { "type": "string" }
                },
                "required": ["port"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "open_tcp_client",
            "title": "Open a TCP client",
            "description": "Connect to a TCP server. Waits for the connection (up to connect_timeout_ms) and fails if it cannot be established, unless reconnect=true, in which case the session keeps retrying in the background. Optionally negotiates Telnet/NVT options and strips them from the data stream.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "host": { "type": "string" },
                    "port": { "type": "integer", "minimum": 1, "maximum": 65535 },
                    "nvt": { "type": "boolean", "default": false, "description": "Handle Telnet IAC negotiation" },
                    "reconnect": { "type": "boolean", "default": false },
                    "connect_timeout_ms": { "type": "integer", "minimum": 100, "maximum": 120000, "default": 3000 },
                    "label": { "type": "string" }
                },
                "required": ["host", "port"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "open_tcp_server",
            "title": "Open a TCP server",
            "description": "Listen on a TCP port and accept multiple clients. Writes are broadcast to every connected client.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "bind": { "type": "string", "default": "0.0.0.0" },
                    "port": { "type": "integer", "minimum": 0, "maximum": 65535, "description": "0 binds an ephemeral port" },
                    "echo": { "type": "boolean", "default": false, "description": "Echo received bytes back to the sender" },
                    "max_clients": { "type": "integer", "minimum": 1, "maximum": 512, "default": 32 },
                    "label": { "type": "string" }
                },
                "required": ["port"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "open_udp",
            "title": "Open a UDP terminal",
            "description": "Bind a local UDP port and optionally set a default remote endpoint. Supports broadcast and echo.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "bind": { "type": "string", "default": "0.0.0.0" },
                    "local_port": { "type": "integer", "minimum": 0, "maximum": 65535, "default": 0, "description": "0 selects an ephemeral port" },
                    "remote_host": { "type": "string" },
                    "remote_port": { "type": "integer", "minimum": 1, "maximum": 65535 },
                    "echo": { "type": "boolean", "default": false },
                    "broadcast": { "type": "boolean", "default": false },
                    "label": { "type": "string" }
                },
                "additionalProperties": false
            }
        }),
        json!({
            "name": "list_sessions",
            "title": "List sessions",
            "description": "List all live sessions with transport, peer, counters, modem lines and errors.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
        }),
        json!({
            "name": "get_session",
            "title": "Get session status",
            "description": "Return detailed status for one session, optionally including the most recent log data.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string" },
                    "include_log": { "type": "boolean", "default": false },
                    "log_bytes": { "type": "integer", "minimum": 1, "maximum": 16777216, "default": 4096 },
                    "format": { "type": "string", "enum": ["ascii", "hex", "dec"], "default": "ascii" }
                },
                "required": ["session_id"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "write_session",
            "title": "Write to a session",
            "description": "Transmit bytes. ASCII input supports \\r \\n \\t \\xNN, \\#ddd and $hh escapes; hex and base64 are also accepted.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string" },
                    "data": { "type": "string" },
                    "format": { "type": "string", "enum": ["ascii", "hex", "base64"], "default": "ascii" },
                    "checksum": { "type": "string", "enum": ["none", "crc16_modbus", "crc16_ccitt", "crc16_xmodem", "crc32", "lrc", "xor8", "sum8"], "description": "Append a checksum of the data (before the line ending)." },
                    "eol": { "type": "string", "enum": ["none", "cr", "lf", "crlf"], "default": "none" }
                },
                "required": ["session_id", "data"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "read_session",
            "title": "Read from a session",
            "description": "Read buffered traffic since a sequence cursor. Blocks up to timeout_ms for new data. Pass next_seq back as since_seq to avoid duplicates.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string" },
                    "since_seq": { "type": "integer", "default": 0 },
                    "max_bytes": { "type": "integer", "minimum": 1, "default": 65536 },
                    "timeout_ms": { "type": "integer", "minimum": 0, "maximum": 600000, "default": 0 },
                    "format": { "type": "string", "enum": ["ascii", "hex", "dec"], "default": "ascii" }
                },
                "required": ["session_id"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "wait_for_data",
            "title": "Wait for matching data",
            "description": "Wait until new traffic arrives or a byte pattern is seen. Useful for request/response devices.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string" },
                    "since_seq": { "type": "integer", "default": 0 },
                    "pattern": { "type": "string", "description": "Optional pattern; empty matches any data" },
                    "pattern_format": { "type": "string", "enum": ["ascii", "hex"], "default": "ascii" },
                    "max_bytes": { "type": "integer", "minimum": 1, "default": 65536 },
                    "timeout_ms": { "type": "integer", "minimum": 0, "maximum": 600000, "default": 2000 },
                    "format": { "type": "string", "enum": ["ascii", "hex", "dec"], "default": "ascii" }
                },
                "required": ["session_id"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "close_session",
            "title": "Close a session",
            "description": "Close a session and release its serial port, socket or listener.",
            "inputSchema": {
                "type": "object",
                "properties": { "session_id": { "type": "string" } },
                "required": ["session_id"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "set_control_lines",
            "title": "Set control lines",
            "description": "Set RS-232 RTS/DTR/break on a serial session, or echo on a TCP server / UDP session.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string" },
                    "rts": { "type": "boolean" },
                    "dtr": { "type": "boolean" },
                    "break": { "type": "boolean" },
                    "echo": { "type": "boolean" }
                },
                "required": ["session_id"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "set_logging",
            "title": "Configure file logging",
            "description": "Start or stop recording session traffic to a file. Formats: text (readable log), csv (timestamp,direction,length,hex,text) or raw (received bytes only).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string" },
                    "enabled": { "type": "boolean", "default": true },
                    "path": { "type": "string" },
                    "append": { "type": "boolean", "default": false },
                    "format": { "type": "string", "enum": ["text", "csv", "raw"], "default": "text" }
                },
                "required": ["session_id"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "clear_session_log",
            "title": "Clear the ring buffer",
            "description": "Discard buffered traffic for a session without closing it.",
            "inputSchema": {
                "type": "object",
                "properties": { "session_id": { "type": "string" } },
                "required": ["session_id"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "udp_scan",
            "title": "UDP broadcast discovery",
            "description": "Send a UDP probe to a broadcast address and list every responder with hex/ascii payloads.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "target": { "type": "string", "default": "255.255.255.255" },
                    "port": { "type": "integer", "minimum": 1, "maximum": 65535, "default": 4000 },
                    "payload_hex": { "type": "string" },
                    "timeout_ms": { "type": "integer", "minimum": 1, "maximum": 600000, "default": 1000 },
                    "local_port": { "type": "integer", "minimum": 0, "maximum": 65535, "default": 0 }
                },
                "additionalProperties": false
            }
        }),
        json!({
            "name": "udp_listen",
            "title": "Listen for UDP datagrams",
            "description": "Collect unsolicited UDP datagrams on a local port for a limited time.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "port": { "type": "integer", "minimum": 1, "maximum": 65535 },
                    "timeout_ms": { "type": "integer", "minimum": 1, "maximum": 600000, "default": 2000 }
                },
                "required": ["port"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "tcp_probe",
            "title": "Probe a TCP endpoint",
            "description": "Measure connect latency and optionally read a short banner (a TCP ping).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "host": { "type": "string" },
                    "port": { "type": "integer", "minimum": 1, "maximum": 65535 },
                    "timeout_ms": { "type": "integer", "minimum": 1, "maximum": 600000, "default": 2000 },
                    "read_banner": { "type": "boolean", "default": true }
                },
                "required": ["host", "port"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "send_file",
            "title": "Send a file",
            "description": "Stream a local file (up to 16 MiB) to a session in chunks, with an optional delay between chunks.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string" },
                    "path": { "type": "string" },
                    "chunk_size": { "type": "integer", "minimum": 1, "maximum": 65536, "default": 4096 },
                    "delay_ms": { "type": "integer", "minimum": 0, "maximum": 60000, "default": 0 }
                },
                "required": ["session_id", "path"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "list_macros",
            "title": "List send macros",
            "description": "List the F1..F12 send macros stored in the configuration file.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
        }),
        json!({
            "name": "run_macro",
            "title": "Run a send macro",
            "description": "Send a configured macro to a session.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string" },
                    "name": { "type": "string" }
                },
                "required": ["session_id", "name"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "get_server_info",
            "title": "Get server information",
            "description": "Version, platform, protocol revisions, configuration path and uptime.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
        }),
        json!({
            "name": "set_auto_responses",
            "title": "Configure auto-responses",
            "description": "Replace a session's auto-response rules. When received data contains a pattern, the reply is sent immediately (device simulation, keep-alives, ACKs). Pass an empty list to remove all rules.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string" },
                    "rules": {
                        "type": "array",
                        "maxItems": 32,
                        "items": {
                            "type": "object",
                            "properties": {
                                "pattern": { "type": "string", "description": "Bytes to match (escaped text, or hex when hex=true)" },
                                "reply": { "type": "string", "description": "Bytes to send back" },
                                "hex": { "type": "boolean", "default": false },
                                "enabled": { "type": "boolean", "default": true }
                            },
                            "required": ["pattern", "reply"]
                        }
                    }
                },
                "required": ["session_id", "rules"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "get_auto_responses",
            "title": "List auto-responses",
            "description": "Show a session's auto-response rules with hit counters.",
            "inputSchema": {
                "type": "object",
                "properties": { "session_id": { "type": "string" } },
                "required": ["session_id"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "compute_checksum",
            "title": "Compute checksums",
            "description": "Compute CRC-16 (Modbus, CCITT, XMODEM), CRC-32, LRC, XOR or SUM-8 over data. Omit algorithm to get all of them.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "data": { "type": "string" },
                    "format": { "type": "string", "enum": ["ascii", "hex", "base64"], "default": "ascii" },
                    "algorithm": { "type": "string", "enum": ["crc16_modbus", "crc16_ccitt", "crc16_xmodem", "crc32", "lrc", "xor8", "sum8"] }
                },
                "required": ["data"],
                "additionalProperties": false
            }
        }),
    ]
}

/// Convenience used by tests and the CLI.
pub fn session_summary(info: &SessionInfo) -> Value {
    json!({
        "id": info.id,
        "kind": info.kind,
        "label": info.label,
        "connected": info.connected,
        "peer": info.peer,
        "tx_bytes": info.tx_bytes,
        "rx_bytes": info.rx_bytes
    })
}
