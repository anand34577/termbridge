//! Model Context Protocol server for Termbridge.
//!
//! Implements JSON-RPC 2.0 over stdio (newline-delimited) and streamable HTTP
//! (`POST /mcp`). Tools expose serial, TCP and UDP terminals to AI agents.

pub mod server;
pub mod tools;
pub mod transport;

pub use server::McpServer;
pub use transport::{serve_http, serve_http_listener, serve_http_with_shutdown, serve_stdio};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Newest protocol revision implemented by this server.
pub const PROTOCOL_VERSION: &str = "2025-06-18";

/// Protocol revisions the server can speak.
pub const SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &["2024-11-05", "2025-03-26", "2025-06-18"];

pub const SERVER_NAME: &str = "termbridge";
