//! MCP protocol conformance tests.

use std::sync::Arc;

use serde_json::{json, Value};

use serial_mcp_core::config::ConfigStore;
use serial_mcp_core::SessionManager;
use serial_mcp_mcp::McpServer;

fn test_server() -> McpServer {
    let dir = std::env::temp_dir().join(format!("serial-mcp-mcp-test-{}", std::process::id()));
    let config = Arc::new(ConfigStore::load(Some(&dir.join("config.toml"))));
    McpServer::new(SessionManager::new(), config)
}

async fn call(server: &McpServer, id: u64, method: &str, params: Value) -> Value {
    let request = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
    let response = server
        .handle_line(&request.to_string())
        .await
        .expect("request should produce a response");
    serde_json::from_str(&response).unwrap()
}

#[tokio::test]
async fn initialize_negotiates_protocol() {
    let server = test_server();
    let response = call(
        &server,
        1,
        "initialize",
        json!({
            "protocolVersion": "2025-03-26",
            "capabilities": {},
            "clientInfo": { "name": "test", "version": "0.1" }
        }),
    )
    .await;
    assert_eq!(response["result"]["protocolVersion"], "2025-03-26");
    assert_eq!(response["result"]["serverInfo"]["name"], "serial-mcp");
    assert!(response["result"]["capabilities"]["tools"].is_object());

    // Unknown protocol revisions fall back to the newest supported one.
    let response = call(
        &server,
        2,
        "initialize",
        json!({ "protocolVersion": "1999-01-01", "capabilities": {} }),
    )
    .await;
    assert_eq!(
        response["result"]["protocolVersion"],
        serial_mcp_mcp::PROTOCOL_VERSION
    );
}

#[tokio::test]
async fn notifications_get_no_response() {
    let server = test_server();
    let response = server
        .handle_line(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
        .await;
    assert!(response.is_none());
}

#[tokio::test]
async fn tools_list_exposes_terminal_tools() {
    let server = test_server();
    let response = call(&server, 3, "tools/list", json!({})).await;
    let tools = response["result"]["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    for expected in [
        "list_serial_ports",
        "open_serial",
        "open_tcp_client",
        "open_tcp_server",
        "open_udp",
        "write_session",
        "read_session",
        "wait_for_data",
        "close_session",
        "udp_scan",
    ] {
        assert!(names.contains(&expected), "missing tool {expected}");
    }
    for tool in tools {
        assert!(tool["inputSchema"].is_object(), "tool without schema");
    }
}

#[tokio::test]
async fn tcp_server_tool_roundtrip() {
    let server = test_server();
    let response = call(
        &server,
        4,
        "tools/call",
        json!({
            "name": "open_tcp_server",
            "arguments": { "bind": "127.0.0.1", "port": 0, "echo": true }
        }),
    )
    .await;
    assert_eq!(response["result"]["isError"], false);
    let session = &response["result"]["structuredContent"]["session"];
    let id = session["id"].as_str().unwrap().to_string();
    let port = session["peer"]
        .as_str()
        .unwrap()
        .rsplit(':')
        .next()
        .unwrap()
        .parse::<u16>()
        .unwrap();

    let response = call(
        &server,
        5,
        "tools/call",
        json!({
            "name": "open_tcp_client",
            "arguments": { "host": "127.0.0.1", "port": port }
        }),
    )
    .await;
    let client = response["result"]["structuredContent"]["session"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;

    let response = call(
        &server,
        6,
        "tools/call",
        json!({
            "name": "write_session",
            "arguments": { "session_id": client, "data": "hello" }
        }),
    )
    .await;
    assert_eq!(response["result"]["isError"], false);

    let response = call(
        &server,
        7,
        "tools/call",
        json!({
            "name": "wait_for_data",
            "arguments": { "session_id": client, "pattern": "hello", "timeout_ms": 2000 }
        }),
    )
    .await;
    assert_eq!(response["result"]["structuredContent"]["matched"], true);

    let response = call(
        &server,
        8,
        "tools/call",
        json!({ "name": "close_session", "arguments": { "session_id": id } }),
    )
    .await;
    assert_eq!(response["result"]["isError"], false);
    let response = call(
        &server,
        9,
        "tools/call",
        json!({ "name": "close_session", "arguments": { "session_id": client } }),
    )
    .await;
    assert_eq!(response["result"]["isError"], false);
}

#[tokio::test]
async fn resources_and_prompts_are_available() {
    let server = test_server();
    let response = call(&server, 10, "resources/list", json!({})).await;
    let resources = response["result"]["resources"].as_array().unwrap();
    assert!(resources.iter().any(|r| r["uri"] == "serial-mcp://info"));

    let response = call(
        &server,
        11,
        "resources/read",
        json!({ "uri": "serial-mcp://info" }),
    )
    .await;
    let text = response["result"]["contents"][0]["text"].as_str().unwrap();
    assert!(text.contains("serial-mcp"));

    let response = call(&server, 12, "prompts/list", json!({})).await;
    assert!(response["result"]["prompts"].as_array().unwrap().len() >= 2);

    let response = call(
        &server,
        13,
        "prompts/get",
        json!({ "name": "echo_test", "arguments": { "transport": "tcp" } }),
    )
    .await;
    assert!(response["result"]["messages"][0]["content"]["text"]
        .as_str()
        .unwrap()
        .contains("echo"));
}

#[tokio::test]
async fn unknown_method_and_tool_are_errors() {
    let server = test_server();
    let response = call(&server, 14, "does/not/exist", json!({})).await;
    assert_eq!(response["error"]["code"], -32601);

    let response = call(
        &server,
        15,
        "tools/call",
        json!({ "name": "nope", "arguments": {} }),
    )
    .await;
    assert_eq!(response["error"]["code"], -32602);
}

#[tokio::test]
async fn batch_requests_return_an_array() {
    let server = test_server();
    let batch = json!([
        { "jsonrpc": "2.0", "id": 1, "method": "ping" },
        { "jsonrpc": "2.0", "method": "notifications/initialized" },
        { "jsonrpc": "2.0", "id": 2, "method": "ping" }
    ]);
    let response = server.handle_json(batch).await.unwrap();
    assert_eq!(response.as_array().unwrap().len(), 2);

    // An empty batch is an invalid request, not a notification.
    let response = server.handle_json(json!([])).await.unwrap();
    assert_eq!(response["error"]["code"], -32600);
}

#[tokio::test]
async fn invalid_arguments_are_rejected() {
    let server = test_server();
    for (name, arguments) in [
        (
            "open_tcp_server",
            json!({ "bind": "127.0.0.1", "port": 70000 }),
        ),
        ("open_tcp_client", json!({ "host": "127.0.0.1", "port": 0 })),
        ("open_serial", json!({ "port": "COM3", "baud": 1 })),
        ("open_udp", json!({ "remote_host": "127.0.0.1" })),
        ("udp_listen", json!({ "port": 70000 })),
        ("tcp_probe", json!({ "host": "127.0.0.1", "port": 70000 })),
    ] {
        let response = call(
            &server,
            1,
            "tools/call",
            json!({ "name": name, "arguments": arguments }),
        )
        .await;
        assert_eq!(
            response["result"]["isError"], true,
            "{name} should have been rejected: {response}"
        );
    }
    assert!(server.manager.list().is_empty());
}

#[tokio::test]
async fn tcp_connect_failure_is_reported_and_cleaned_up() {
    let server = test_server();
    let response = call(
        &server,
        1,
        "tools/call",
        json!({
            "name": "open_tcp_client",
            "arguments": { "host": "127.0.0.1", "port": 1, "connect_timeout_ms": 300 }
        }),
    )
    .await;
    assert_eq!(response["result"]["isError"], true);
    assert!(server.manager.list().is_empty());
}

#[tokio::test]
async fn read_session_cursor_returns_every_chunk() {
    let server = test_server();
    let response = call(
        &server,
        1,
        "tools/call",
        json!({
            "name": "open_tcp_server",
            "arguments": { "bind": "127.0.0.1", "port": 0 }
        }),
    )
    .await;
    let id = response["result"]["structuredContent"]["session"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let port = response["result"]["structuredContent"]["session"]["peer"]
        .as_str()
        .unwrap()
        .rsplit(':')
        .next()
        .unwrap()
        .parse::<u16>()
        .unwrap();

    let response = call(
        &server,
        2,
        "tools/call",
        json!({
            "name": "open_tcp_client",
            "arguments": { "host": "127.0.0.1", "port": port }
        }),
    )
    .await;
    let client = response["result"]["structuredContent"]["session"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    for message in ["aaa", "bbb", "ccc"] {
        let response = call(
            &server,
            3,
            "tools/call",
            json!({
                "name": "write_session",
                "arguments": { "session_id": client, "data": message }
            }),
        )
        .await;
        assert_eq!(response["result"]["isError"], false);
        tokio::time::sleep(std::time::Duration::from_millis(40)).await;
    }

    let mut since = 0u64;
    let mut received = String::new();
    for _ in 0..20 {
        let response = call(
            &server,
            4,
            "tools/call",
            json!({
                "name": "read_session",
                "arguments": { "session_id": id, "since_seq": since, "max_bytes": 3 }
            }),
        )
        .await;
        let result = &response["result"]["structuredContent"];
        since = result["next_seq"].as_u64().unwrap();
        for chunk in result["chunks"].as_array().unwrap() {
            if chunk["dir"] == "rx" {
                received.push_str(chunk["text"].as_str().unwrap());
            }
        }
        if received.len() >= 9 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert_eq!(received, "aaabbbccc");
    server.manager.close_all();
}

#[tokio::test]
async fn wait_for_data_matches_across_chunks() {
    let server = test_server();
    let response = call(
        &server,
        1,
        "tools/call",
        json!({
            "name": "open_tcp_server",
            "arguments": { "bind": "127.0.0.1", "port": 0 }
        }),
    )
    .await;
    let server_id = response["result"]["structuredContent"]["session"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let port = response["result"]["structuredContent"]["session"]["peer"]
        .as_str()
        .unwrap()
        .rsplit(':')
        .next()
        .unwrap()
        .parse::<u16>()
        .unwrap();
    call(
        &server,
        2,
        "tools/call",
        json!({
            "name": "set_auto_responses",
            "arguments": { "session_id": server_id, "rules": [
                { "pattern": "PING", "reply": "PONG" }
            ] }
        }),
    )
    .await;

    let response = call(
        &server,
        3,
        "tools/call",
        json!({
            "name": "open_tcp_client",
            "arguments": { "host": "127.0.0.1", "port": port }
        }),
    )
    .await;
    let client = response["result"]["structuredContent"]["session"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Two writes, so the reply only matches when chunks are joined.
    for part in ["PI", "NG"] {
        call(
            &server,
            4,
            "tools/call",
            json!({
                "name": "write_session",
                "arguments": { "session_id": client, "data": part }
            }),
        )
        .await;
        tokio::time::sleep(std::time::Duration::from_millis(40)).await;
    }
    let response = call(
        &server,
        5,
        "tools/call",
        json!({
            "name": "wait_for_data",
            "arguments": { "session_id": client, "pattern": "PONG", "timeout_ms": 2000 }
        }),
    )
    .await;
    assert_eq!(response["result"]["structuredContent"]["matched"], true);
    server.manager.close_all();
}

#[tokio::test]
async fn checksum_and_auto_response_tools() {
    let server = test_server();
    let response = call(
        &server,
        1,
        "tools/call",
        json!({ "name": "compute_checksum", "arguments": {
            "data": "01 03 00 00 00 0A", "format": "hex", "algorithm": "crc16_modbus" } }),
    )
    .await;
    let result = &response["result"]["structuredContent"]["results"][0];
    assert_eq!(result["checksum_hex"], "C5 CD");
    assert_eq!(result["frame_hex"], "01 03 00 00 00 0A C5 CD");

    let response = call(
        &server,
        2,
        "tools/call",
        json!({ "name": "open_tcp_server", "arguments": { "bind": "127.0.0.1", "port": 0 } }),
    )
    .await;
    let id = response["result"]["structuredContent"]["session"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let response = call(
        &server,
        3,
        "tools/call",
        json!({ "name": "set_auto_responses", "arguments": { "session_id": id,
            "rules": [{ "pattern": "PING", "reply": "PONG\r\n" }] } }),
    )
    .await;
    assert_eq!(response["result"]["isError"], false);
    assert_eq!(
        response["result"]["structuredContent"]["rules"][0]["hits"],
        0
    );

    // Invalid rules are reported as tool errors, not protocol errors.
    let response = call(
        &server,
        4,
        "tools/call",
        json!({ "name": "set_auto_responses", "arguments": { "session_id": id,
            "rules": [{ "pattern": "", "reply": "x" }] } }),
    )
    .await;
    assert_eq!(response["result"]["isError"], true);
    server.manager.close_all();
}
