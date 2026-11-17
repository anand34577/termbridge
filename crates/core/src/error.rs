use thiserror::Error;

/// Errors produced by the transport core.
#[derive(Debug, Error)]
pub enum CoreError {
    #[error("serial port error: {0}")]
    Serial(#[from] serialport::Error),

    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),

    #[error("session not found: {0}")]
    SessionNotFound(String),

    #[error("session is closed: {0}")]
    SessionClosed(String),

    #[error("invalid input: {0}")]
    InvalidInput(String),

    #[error("unsupported operation: {0}")]
    Unsupported(String),

    #[error("timed out after {0} ms")]
    Timeout(u64),

    #[error("connection failed: {0}")]
    Connect(String),

    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, CoreError>;
