//! Termbridge core: cross-platform serial, TCP and UDP terminal transports.

pub mod checksum;
pub mod codec;
pub mod config;
pub mod discovery;
pub mod error;
pub mod logging;
pub mod net;
pub mod nvt;
pub mod responder;
pub mod ring;
pub mod session;
pub mod types;
pub mod util;
pub mod validate;
pub mod view;

pub use error::{CoreError, Result};
pub use session::{
    SerialConfig, SessionConfig, SessionInfo, SessionManager, TcpClientConfig, TcpServerConfig,
    UdpConfig,
};
pub use types::{ClientInfo, ControlOp, Direction, ModemLines, SessionEvent, SessionKind};

/// Version of the core crate.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
