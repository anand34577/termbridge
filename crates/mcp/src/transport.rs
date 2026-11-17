//! MCP transports: newline-delimited stdio and streamable HTTP.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::server::McpServer;

/// Serve MCP over stdin/stdout. Logs must go to stderr.
pub async fn serve_stdio(server: Arc<McpServer>) -> anyhow::Result<()> {
    let stdin = tokio::io::stdin();
    let mut lines = BufReader::new(stdin).lines();
    let mut stdout = tokio::io::stdout();
    tracing::info!("MCP stdio transport ready");
    while let Some(line) = lines.next_line().await? {
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }
        if let Some(response) = server.handle_line(&line).await {
            stdout.write_all(response.as_bytes()).await?;
            stdout.write_all(b"\n").await?;
            stdout.flush().await?;
        }
    }
    Ok(())
}

/// Serve MCP over HTTP until the process exits.
pub async fn serve_http(server: Arc<McpServer>, bind: SocketAddr) -> anyhow::Result<()> {
    serve_http_with_shutdown(server, bind, std::future::pending::<()>()).await
}

/// Serve MCP over HTTP with a graceful shutdown signal.
pub async fn serve_http_with_shutdown<F>(
    server: Arc<McpServer>,
    bind: SocketAddr,
    shutdown: F,
) -> anyhow::Result<()>
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    let listener = tokio::net::TcpListener::bind(bind).await?;
    tracing::info!("MCP HTTP transport on http://{bind}/mcp");
    serve_http_listener(server, listener, shutdown).await
}

/// Serve MCP over an already-bound listener. Useful for GUIs that want to
/// report bind errors synchronously.
pub async fn serve_http_listener<F>(
    server: Arc<McpServer>,
    listener: tokio::net::TcpListener,
    shutdown: F,
) -> anyhow::Result<()>
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    let app = Router::new()
        .route("/health", get(health))
        .route("/mcp", post(mcp_post).options(mcp_options))
        .route("/", post(mcp_post).options(mcp_options))
        .layer(DefaultBodyLimit::max(16 * 1024 * 1024))
        .with_state(server);

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await?;
    Ok(())
}

async fn health(State(server): State<Arc<McpServer>>) -> impl IntoResponse {
    Json(json!({
        "status": "ok",
        "name": crate::SERVER_NAME,
        "version": crate::VERSION,
        "protocolVersion": crate::PROTOCOL_VERSION,
        "sessions": server.manager.list().len()
    }))
}

async fn mcp_post(State(server): State<Arc<McpServer>>, body: Bytes) -> Response {
    let text = String::from_utf8_lossy(&body).to_string();
    match server.handle_line(&text).await {
        Some(response) => {
            let mut res = (StatusCode::OK, response).into_response();
            set_cors(res.headers_mut());
            res.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            );
            res
        }
        None => {
            let mut res = StatusCode::ACCEPTED.into_response();
            set_cors(res.headers_mut());
            res
        }
    }
}

async fn mcp_options() -> Response {
    let mut res = StatusCode::NO_CONTENT.into_response();
    set_cors(res.headers_mut());
    res.headers_mut().insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("POST, GET, OPTIONS"),
    );
    res.headers_mut().insert(
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static("content-type, mcp-session-id, mcp-protocol-version"),
    );
    res
}

fn set_cors(headers: &mut axum::http::HeaderMap) {
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static("*"),
    );
    headers.insert(
        header::ACCESS_CONTROL_EXPOSE_HEADERS,
        HeaderValue::from_static("mcp-session-id"),
    );
}
