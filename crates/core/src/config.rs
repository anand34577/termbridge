//! Persistent application configuration.

use std::path::{Path, PathBuf};
use std::sync::RwLock;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    Dark,
    Light,
}

/// A send macro shown on F1..F12 in the GUI and usable over MCP.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MacroDef {
    pub name: String,
    pub data: String,
    #[serde(default)]
    pub hex: bool,
}

impl MacroDef {
    pub fn new(name: &str, data: &str, hex: bool) -> Self {
        Self {
            name: name.to_string(),
            data: data.to_string(),
            hex,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct McpSettings {
    /// Address the HTTP MCP transport binds to.
    pub bind: String,
    /// Start the HTTP transport automatically with the GUI.
    pub autostart: bool,
}

impl Default for McpSettings {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1:8765".to_string(),
            autostart: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub theme: Theme,
    pub show_timestamps: bool,
    pub buffer_bytes: usize,
    pub macros: Vec<MacroDef>,
    pub mcp: McpSettings,
    pub log_dir: Option<PathBuf>,
    /// Most recently opened connections, newest first.
    pub recent: Vec<crate::session::SessionConfig>,
    /// Colour theme of the terminal UI.
    pub tui_theme: String,
}

/// Maximum number of remembered connections.
pub const MAX_RECENT: usize = 8;
/// Smallest and largest per-session ring buffer.
pub const MIN_BUFFER: usize = 16 * 1024;
pub const MAX_BUFFER: usize = 64 * 1024 * 1024;

impl AppConfig {
    /// Clamp out-of-range values from a hand-edited or older config file.
    pub fn sanitize(&mut self) {
        self.buffer_bytes = self.buffer_bytes.clamp(MIN_BUFFER, MAX_BUFFER);
        self.macros.retain(|m| !m.name.trim().is_empty());
        self.macros.truncate(12);
        self.recent.truncate(MAX_RECENT);
        let mut seen = std::collections::HashSet::new();
        self.recent.retain(|c| seen.insert(recent_key(c)));
        if self
            .mcp
            .bind
            .trim()
            .parse::<std::net::SocketAddr>()
            .is_err()
        {
            self.mcp.bind = McpSettings::default().bind;
        }
    }

    /// Remember a connection, moving it to the front and removing duplicates.
    pub fn remember(&mut self, config: crate::session::SessionConfig) {
        let key = recent_key(&config);
        self.recent.retain(|c| recent_key(c) != key);
        self.recent.insert(0, config);
        self.recent.truncate(MAX_RECENT);
    }
}

/// Identity of a remembered connection (ignores labels and toggles).
pub fn recent_key(config: &crate::session::SessionConfig) -> String {
    use crate::session::SessionConfig as C;
    match config {
        C::Serial(c) => format!("serial:{}", c.port.to_ascii_lowercase()),
        C::TcpClient(c) => format!("tcp:{}:{}", c.host.to_ascii_lowercase(), c.port),
        C::TcpServer(c) => format!("server:{}:{}", c.bind, c.port),
        C::Udp(c) => format!(
            "udp:{}:{}:{}:{}",
            c.bind,
            c.local_port,
            c.remote_host.clone().unwrap_or_default(),
            c.remote_port.unwrap_or(0)
        ),
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            theme: Theme::Dark,
            show_timestamps: true,
            buffer_bytes: 1024 * 1024,
            macros: vec![
                MacroDef::new("CR", "\\r", false),
                MacroDef::new("AT", "AT\\r", false),
                MacroDef::new("Ping", "PING\\r", false),
            ],
            mcp: McpSettings::default(),
            log_dir: None,
            recent: Vec::new(),
            tui_theme: "Midnight".to_string(),
        }
    }
}

/// Thread-safe configuration handle backed by a TOML file.
pub struct ConfigStore {
    path: PathBuf,
    inner: RwLock<AppConfig>,
}

impl ConfigStore {
    /// Load configuration from `path`, or from the platform config directory
    /// when `None` is given. Missing or invalid files fall back to defaults.
    pub fn load(path: Option<&Path>) -> Self {
        let path = path
            .map(|p| p.to_path_buf())
            .unwrap_or_else(default_config_path);
        let mut config = match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str::<AppConfig>(&text).unwrap_or_else(|err| {
                tracing::warn!("invalid config at {}: {err}", path.display());
                AppConfig::default()
            }),
            Err(_) => AppConfig::default(),
        };
        config.sanitize();
        Self {
            path,
            inner: RwLock::new(config),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn get(&self) -> AppConfig {
        self.inner.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn update<F: FnOnce(&mut AppConfig)>(&self, f: F) {
        let mut guard = self.inner.write().unwrap_or_else(|e| e.into_inner());
        f(&mut guard);
    }

    pub fn save(&self) -> std::io::Result<()> {
        let config = self.get();
        if let Some(parent) = self.path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let text = toml::to_string_pretty(&config)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        // Write to a temporary file and rename it so a crash never leaves a
        // truncated config behind.
        let tmp = self.path.with_extension("toml.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, &self.path).or_else(|_| {
            std::fs::copy(&tmp, &self.path)?;
            std::fs::remove_file(&tmp)
        })
    }
}

pub fn default_config_path() -> PathBuf {
    if let Ok(path) = std::env::var("SERIAL_MCP_CONFIG") {
        return PathBuf::from(path);
    }
    let base = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
    base.join("serial-mcp").join("config.toml")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_config() {
        let dir = std::env::temp_dir().join(format!("serial-mcp-test-{}", std::process::id()));
        let path = dir.join("config.toml");
        let store = ConfigStore::load(Some(&path));
        store.update(|cfg| {
            cfg.show_timestamps = false;
            cfg.buffer_bytes = 32768;
        });
        store.save().unwrap();
        let reloaded = ConfigStore::load(Some(&path));
        let cfg = reloaded.get();
        assert!(!cfg.show_timestamps);
        assert_eq!(cfg.buffer_bytes, 32768);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn remembers_recent_without_duplicates() {
        use crate::session::{SessionConfig, TcpClientConfig};
        let mut cfg = AppConfig::default();
        for port in [1u16, 2, 1] {
            cfg.remember(SessionConfig::TcpClient(TcpClientConfig {
                port,
                ..Default::default()
            }));
        }
        assert_eq!(cfg.recent.len(), 2);
        let dir = std::env::temp_dir().join(format!("serial-mcp-recent-{}", std::process::id()));
        let path = dir.join("config.toml");
        let store = ConfigStore::load(Some(&path));
        store.update(|c| *c = cfg.clone());
        store.save().unwrap();
        assert_eq!(ConfigStore::load(Some(&path)).get().recent.len(), 2);
        let _ = std::fs::remove_dir_all(dir);
    }
}
