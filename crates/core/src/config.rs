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
        // Only migrate when we are using the default location; an explicit path
        // means the caller owns the file.
        if path.ends_with("termbridge/config.toml") {
            if let Some(dir) = path.parent() {
                migrate_legacy_config_dir(dir);
            }
        }
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
    if let Ok(path) = std::env::var("TERMBRIDGE_CONFIG") {
        return PathBuf::from(path);
    }
    // The variable was named after the old binary. Keep honouring it so an
    // existing setup does not silently start reading a different file.
    if let Ok(path) = std::env::var("SERIAL_MCP_CONFIG") {
        return PathBuf::from(path);
    }
    let base = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
    base.join("termbridge").join("config.toml")
}

/// Directory this program used before it was renamed.
fn legacy_config_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|base| base.join("serial-mcp"))
}

/// Move a config directory left by the previous name into place.
///
/// Runs once, before the first load. The old directory is renamed rather than
/// copied, so a user's recent connections, macros and auto-reply rules carry
/// over without leaving two configs that can drift apart. If the new
/// directory already exists the old one is left untouched.
fn migrate_legacy_config_dir(new_dir: &Path) {
    if new_dir.exists() {
        return;
    }
    let Some(old_dir) = legacy_config_dir() else {
        return;
    };
    move_config_dir(&old_dir, new_dir);
}

/// Rename `old_dir` to `new_dir`, falling back to a copy when the rename
/// cannot cross filesystems. Does nothing if `old_dir` is absent.
fn move_config_dir(old_dir: &Path, new_dir: &Path) {
    if !old_dir.is_dir() {
        return;
    }
    if let Some(parent) = new_dir.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match std::fs::rename(old_dir, new_dir) {
        Ok(()) => tracing::info!(
            "moved configuration from {} to {}",
            old_dir.display(),
            new_dir.display()
        ),
        // A rename can fail across filesystems; copying keeps the settings.
        Err(_) => {
            let _ = std::fs::create_dir_all(new_dir);
            if let Ok(entries) = std::fs::read_dir(old_dir) {
                for entry in entries.flatten() {
                    let dest = new_dir.join(entry.file_name());
                    let _ = std::fs::copy(entry.path(), &dest);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_config() {
        let dir = std::env::temp_dir().join(format!("termbridge-test-{}", std::process::id()));
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
        let dir = std::env::temp_dir().join(format!("termbridge-recent-{}", std::process::id()));
        let path = dir.join("config.toml");
        let store = ConfigStore::load(Some(&path));
        store.update(|c| *c = cfg.clone());
        store.save().unwrap();
        assert_eq!(ConfigStore::load(Some(&path)).get().recent.len(), 2);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn moves_config_from_the_old_directory() {
        let base = std::env::temp_dir().join(format!("termbridge-move-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let old = base.join("serial-mcp");
        let new = base.join("termbridge");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("config.toml"), "show_timestamps = false\n").unwrap();

        move_config_dir(&old, &new);

        assert!(new.join("config.toml").exists(), "config did not move");
        assert!(!old.exists(), "old directory was left behind");
        let cfg = ConfigStore::load(Some(&new.join("config.toml"))).get();
        assert!(!cfg.show_timestamps, "settings were not preserved");
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn leaves_an_existing_config_directory_alone() {
        let base = std::env::temp_dir().join(format!("termbridge-keep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let old = base.join("serial-mcp");
        let new = base.join("termbridge");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::create_dir_all(&new).unwrap();
        std::fs::write(new.join("config.toml"), "show_timestamps = true\n").unwrap();

        move_config_dir(&old, &new);

        assert!(old.exists(), "the old directory should be untouched");
        assert!(
            ConfigStore::load(Some(&new.join("config.toml")))
                .get()
                .show_timestamps,
            "the existing config should not be overwritten"
        );
        let _ = std::fs::remove_dir_all(base);
    }
}
