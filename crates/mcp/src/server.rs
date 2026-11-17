//! JSON-RPC / MCP request handling.

use std::sync::Arc;
use std::time::Instant;

use serde_json::{json, Value};

use serial_mcp_core::config::ConfigStore;
use serial_mcp_core::SessionManager;

use crate::tools;
use crate::{PROTOCOL_VERSION, SERVER_NAME, SUPPORTED_PROTOCOL_VERSIONS, VERSION};

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

/// MCP server state shared by the stdio and HTTP transports.
pub struct McpServer {
    pub manager: SessionManager,
    pub config: Arc<ConfigStore>,
    pub started: Instant,
}

impl McpServer {
    pub fn new(manager: SessionManager, config: Arc<ConfigStore>) -> Self {
        Self {
            manager,
            config,
            started: Instant::now(),
        }
    }

    /// Handle one newline-delimited JSON message and return the serialized
    /// response, or `None` for notifications.
    pub async fn handle_line(&self, line: &str) -> Option<String> {
        let value: Value = match serde_json::from_str(line) {
            Ok(value) => value,
            Err(err) => {
                return Some(
                    serde_json::to_string(&error_response(
                        Value::Null,
                        PARSE_ERROR,
                        format!("parse error: {err}"),
                    ))
                    .unwrap_or_default(),
                )
            }
        };
        let response = self.handle_json(value).await?;
        Some(serde_json::to_string(&response).unwrap_or_default())
    }

    /// Handle a single request or a JSON-RPC batch.
    pub async fn handle_json(&self, value: Value) -> Option<Value> {
        if let Some(batch) = value.as_array() {
            if batch.is_empty() {
                // JSON-RPC: an empty batch is an invalid request.
                return Some(error_response(Value::Null, INVALID_REQUEST, "empty batch"));
            }
            let mut responses = Vec::new();
            for item in batch {
                if let Some(resp) = self.handle_one(item.clone()).await {
                    responses.push(resp);
                }
            }
            if responses.is_empty() {
                None
            } else {
                Some(Value::Array(responses))
            }
        } else {
            self.handle_one(value).await
        }
    }

    async fn handle_one(&self, request: Value) -> Option<Value> {
        let id = request.get("id").cloned();
        let method = match request.get("method").and_then(|m| m.as_str()) {
            Some(method) => method.to_string(),
            None => {
                return Some(error_response(
                    id.unwrap_or(Value::Null),
                    INVALID_REQUEST,
                    "missing method",
                ))
            }
        };
        let params = request.get("params").cloned().unwrap_or(Value::Null);

        // Notifications never get a response.
        id.as_ref()?;
        let id = id.unwrap();

        let response = match method.as_str() {
            "initialize" => response(id, self.initialize(&params)),
            "ping" => response(id, json!({})),
            "tools/list" => response(id, json!({ "tools": tools::definitions() })),
            "tools/call" => {
                let name = params
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or_default();
                if !tools::is_known(name) {
                    return Some(error_response(
                        id,
                        INVALID_PARAMS,
                        format!("unknown tool: {name}"),
                    ));
                }
                let arguments = params
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                match tools::call(self, name, &arguments).await {
                    Ok(result) => response(id, result),
                    Err(err) => response(id, tools::tool_error(err.to_string())),
                }
            }
            "resources/list" => response(id, json!({ "resources": self.resource_list() })),
            "resources/read" => match self.read_resource(&params) {
                Ok(result) => response(id, result),
                Err(err) => error_response(id, INVALID_PARAMS, err),
            },
            "prompts/list" => response(id, json!({ "prompts": prompts() })),
            "prompts/get" => match self.get_prompt(&params) {
                Ok(result) => response(id, result),
                Err(err) => error_response(id, INVALID_PARAMS, err),
            },
            "logging/setLevel" => response(id, json!({})),
            other => {
                return Some(error_response(
                    id,
                    METHOD_NOT_FOUND,
                    format!("method not found: {other}"),
                ))
            }
        };
        Some(response)
    }

    fn initialize(&self, params: &Value) -> Value {
        let requested = params
            .get("protocolVersion")
            .and_then(|v| v.as_str())
            .unwrap_or(PROTOCOL_VERSION);
        let negotiated = if SUPPORTED_PROTOCOL_VERSIONS.contains(&requested) {
            requested
        } else {
            PROTOCOL_VERSION
        };
        json!({
            "protocolVersion": negotiated,
            "capabilities": {
                "tools": { "listChanged": false },
                "resources": { "subscribe": false, "listChanged": false },
                "prompts": { "listChanged": false },
                "logging": {}
            },
            "serverInfo": {
                "name": SERVER_NAME,
                "title": "Serial MCP",
                "version": VERSION
            },
            "instructions": "Serial MCP is a cross-platform serial / TCP / UDP terminal. \
        Use list_serial_ports to discover ports, open_serial / open_tcp_client / open_tcp_server / open_udp to create a session, \
        write_session to transmit, and read_session (or wait_for_data) to receive. \
        Sessions keep a ring buffer, so read_session with since_seq never loses ordering. \
        Use close_session when done. write_session and compute_checksum accept ascii, hex or base64 input."
        })
    }

    fn resource_list(&self) -> Vec<Value> {
        let mut resources = vec![
            json!({
                "uri": "serial-mcp://info",
                "name": "Server information",
                "description": "Version, platform, configuration path and uptime.",
                "mimeType": "application/json"
            }),
            json!({
                "uri": "serial-mcp://ports",
                "name": "Serial ports",
                "description": "Serial ports currently visible to the operating system.",
                "mimeType": "application/json"
            }),
            json!({
                "uri": "serial-mcp://sessions",
                "name": "Sessions",
                "description": "All live sessions with counters and status.",
                "mimeType": "application/json"
            }),
        ];
        for session in self.manager.list() {
            resources.push(json!({
                "uri": format!("serial-mcp://sessions/{}/status", session.id),
                "name": format!("{} status", session.label),
                "mimeType": "application/json"
            }));
            resources.push(json!({
                "uri": format!("serial-mcp://sessions/{}/log", session.id),
                "name": format!("{} log", session.label),
                "description": "Most recent 64 KiB of session traffic.",
                "mimeType": "application/json"
            }));
        }
        resources
    }

    fn read_resource(&self, params: &Value) -> Result<Value, String> {
        let uri = params
            .get("uri")
            .and_then(|u| u.as_str())
            .ok_or_else(|| "missing uri".to_string())?;
        let text = if uri == "serial-mcp://info" {
            serde_json::to_string_pretty(&tools::server_info(self)).unwrap_or_default()
        } else if uri == "serial-mcp://ports" {
            let ports = serial_mcp_core::net::list_ports().map_err(|e| e.to_string())?;
            serde_json::to_string_pretty(&json!({ "ports": ports })).unwrap_or_default()
        } else if uri == "serial-mcp://sessions" {
            serde_json::to_string_pretty(&json!({ "sessions": self.manager.list() }))
                .unwrap_or_default()
        } else if let Some(rest) = uri.strip_prefix("serial-mcp://sessions/") {
            let (id, what) = rest
                .split_once('/')
                .ok_or_else(|| format!("invalid session resource: {uri}"))?;
            let info = self.manager.info(id).map_err(|e| e.to_string())?;
            if what == "status" {
                serde_json::to_string_pretty(&info).unwrap_or_default()
            } else if what == "log" {
                let batch = self
                    .manager
                    .tail(id, 64 * 1024)
                    .map_err(|e| e.to_string())?;
                serde_json::to_string_pretty(&tools::render_batch(
                    &batch,
                    serial_mcp_core::codec::ViewFormat::Ascii,
                ))
                .unwrap_or_default()
            } else {
                return Err(format!("unknown session resource: {uri}"));
            }
        } else {
            return Err(format!("unknown resource: {uri}"));
        };
        Ok(json!({
            "contents": [{ "uri": uri, "mimeType": "application/json", "text": text }]
        }))
    }

    fn get_prompt(&self, params: &Value) -> Result<Value, String> {
        let name = params
            .get("name")
            .and_then(|n| n.as_str())
            .ok_or_else(|| "missing prompt name".to_string())?;
        let args = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let text = match name {
            "diagnose_serial_link" => {
                let port = args
                    .get("port")
                    .and_then(|v| v.as_str())
                    .unwrap_or("<PORT>");
                let baud = args.get("baud").and_then(|v| v.as_u64()).unwrap_or(115200);
                format!(
                    "Diagnose the serial link on {port} at {baud} baud.\n\
1. Call list_serial_ports and confirm the port exists.\n\
2. open_serial with port={port}, baud={baud}, 8 data bits, no parity, 1 stop bit.\n\
3. write_session with data \"\\r\\n\" and read_session with timeout_ms=1000.\n\
4. If nothing arrives, try toggling DTR/RTS with set_control_lines and send \"AT\\r\".\n\
5. Report the exact bytes received (hex and ascii), the modem line state and any errors.\n\
6. close_session when finished."
                )
            }
            "echo_test" => {
                let transport = args
                    .get("transport")
                    .and_then(|v| v.as_str())
                    .unwrap_or("tcp");
                format!(
                    "Run a loopback echo test over {transport}.\n\
For TCP: open_tcp_server on port 9100 with echo=true, then open_tcp_client to 127.0.0.1:9100, \
write_session \"hello\", wait_for_data for the same bytes, and close both sessions.\n\
For UDP: open_udp with local_port=9101 and echo=true, then send a datagram to 127.0.0.1:9101 \
from a second UDP session and verify the echo.\n\
Report throughput counters from get_session and close everything afterwards."
                )
            }
            other => return Err(format!("unknown prompt: {other}")),
        };
        Ok(json!({
            "description": name,
            "messages": [{
                "role": "user",
                "content": { "type": "text", "text": text }
            }]
        }))
    }
}

fn response(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error_response(id: Value, code: i64, message: impl Into<String>) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message.into() }
    })
}

fn prompts() -> Vec<Value> {
    vec![
        json!({
            "name": "diagnose_serial_link",
            "title": "Diagnose a serial link",
            "description": "Open a serial port, probe it and report the raw response.",
            "arguments": [
                { "name": "port", "description": "Serial port name, e.g. COM3 or /dev/ttyUSB0", "required": true },
                { "name": "baud", "description": "Baud rate (default 115200)", "required": false }
            ]
        }),
        json!({
            "name": "echo_test",
            "title": "Loopback echo test",
            "description": "Verify TCP or UDP loopback with the built-in echo server.",
            "arguments": [
                { "name": "transport", "description": "tcp or udp", "required": false }
            ]
        }),
    ]
}
