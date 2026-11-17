//! Per-session recording to disk in text, CSV or raw binary form.

use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::codec;
use crate::error::{CoreError, Result};
use crate::types::Direction;
use crate::util;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    /// `[timestamp] RX escaped-data` lines, easy to read and grep.
    #[default]
    Text,
    /// `timestamp,direction,length,hex,text` rows for spreadsheets and scripts.
    Csv,
    /// Received bytes only, written exactly as they arrived.
    Raw,
}

impl LogFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            LogFormat::Text => "text",
            LogFormat::Csv => "csv",
            LogFormat::Raw => "raw",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            LogFormat::Text => "log",
            LogFormat::Csv => "csv",
            LogFormat::Raw => "bin",
        }
    }

    pub fn parse(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "" | "text" | "txt" | "log" => Ok(LogFormat::Text),
            "csv" => Ok(LogFormat::Csv),
            "raw" | "bin" | "binary" => Ok(LogFormat::Raw),
            other => Err(CoreError::InvalidInput(format!(
                "unknown log format '{other}' (expected text, csv or raw)"
            ))),
        }
    }
}

/// Appends session traffic to a file.
pub struct FileSink {
    file: BufWriter<File>,
    path: PathBuf,
    format: LogFormat,
    bytes: u64,
}

impl FileSink {
    pub fn open(path: &Path, append: bool) -> std::io::Result<Self> {
        Self::open_with(path, append, LogFormat::Text)
    }

    pub fn open_with(path: &Path, append: bool, format: LogFormat) -> std::io::Result<Self> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let existing = append && path.metadata().map(|m| m.len() > 0).unwrap_or(false);
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .append(append)
            .truncate(!append)
            .open(path)?;
        let mut sink = Self {
            file: BufWriter::new(file),
            path: path.to_path_buf(),
            format,
            bytes: 0,
        };
        if format == LogFormat::Csv && !existing {
            writeln!(sink.file, "timestamp,direction,length,hex,text")?;
        }
        Ok(sink)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn format(&self) -> LogFormat {
        self.format
    }

    /// Payload bytes written so far.
    pub fn bytes_written(&self) -> u64 {
        self.bytes
    }

    pub fn write_chunk(&mut self, ts_ms: i64, dir: Direction, data: &[u8]) -> std::io::Result<()> {
        let tag = match dir {
            Direction::Rx => "RX",
            Direction::Tx => "TX",
            Direction::Info => "INFO",
            Direction::Error => "ERROR",
            Direction::Nvt => "NVT",
        };
        match self.format {
            LogFormat::Text => {
                let ts = util::format_ts(ts_ms, false);
                writeln!(self.file, "[{ts}] {tag} {}", codec::escape_bytes(data))?;
            }
            LogFormat::Csv => {
                let ts = util::format_ts(ts_ms, false);
                let text = codec::escape_bytes(data).replace('"', "\"\"");
                writeln!(
                    self.file,
                    "{ts},{tag},{},{},\"{text}\"",
                    data.len(),
                    codec::to_hex(data)
                )?;
            }
            LogFormat::Raw => {
                if dir != Direction::Rx {
                    return Ok(());
                }
                self.file.write_all(data)?;
            }
        }
        if matches!(dir, Direction::Rx | Direction::Tx) {
            self.bytes += data.len() as u64;
        }
        // Flush every chunk (a write to the OS, not an fsync) so a log is
        // readable while the session is still open and idle.
        self.file.flush()?;
        Ok(())
    }

    pub fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

impl Drop for FileSink {
    fn drop(&mut self) {
        let _ = self.file.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_each_format() {
        let dir = std::env::temp_dir().join(format!("serial-mcp-log-{}", std::process::id()));
        for format in [LogFormat::Text, LogFormat::Csv, LogFormat::Raw] {
            let path = dir.join(format!("t.{}", format.extension()));
            {
                let mut sink = FileSink::open_with(&path, false, format).unwrap();
                sink.write_chunk(0, Direction::Rx, b"A\r\n").unwrap();
                sink.write_chunk(0, Direction::Tx, b"B").unwrap();
            }
            let data = std::fs::read(&path).unwrap();
            match format {
                LogFormat::Raw => assert_eq!(data, b"A\r\n"),
                LogFormat::Csv => {
                    let text = String::from_utf8(data).unwrap();
                    assert!(text.starts_with("timestamp,direction"));
                    assert!(text.contains(",RX,3,41 0D 0A,"));
                }
                LogFormat::Text => {
                    let text = String::from_utf8(data).unwrap();
                    assert!(text.contains("RX A\\r\\n"));
                }
            }
        }
        let _ = std::fs::remove_dir_all(dir);
    }
}
