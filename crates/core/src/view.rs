//! Incremental terminal view model shared by the GUI and the TUI.
//!
//! Traffic chunks from the session ring buffer are turned into display lines
//! (ASCII lines split on `\n`, hex dump rows, or decimal rows). Each line keeps
//! its timestamp and direction separately so the renderer can draw a gutter.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::codec::{self, ViewFormat};
use crate::ring::ReadBatch;
use crate::types::Direction;
use crate::util;

/// Maximum number of rendered lines kept per session view.
pub const MAX_LINES: usize = 20_000;
/// Longest unterminated ASCII line before it is force-flushed.
const MAX_PARTIAL: usize = 4096;

pub struct TermLine {
    pub ts: String,
    pub dir: Direction,
    pub text: String,
}

pub struct TerminalBuffer {
    pub lines: VecDeque<TermLine>,
    pub last_seq: u64,
    pub format: ViewFormat,
    pub show_ts: bool,
    /// Lines evicted because of `MAX_LINES`.
    pub trimmed: u64,
    partial: String,
    partial_dir: Direction,
    partial_ts: i64,
    last_ingest: Instant,
}

impl TerminalBuffer {
    pub fn new(format: ViewFormat, show_ts: bool) -> Self {
        Self {
            lines: VecDeque::new(),
            last_seq: 0,
            format,
            show_ts,
            trimmed: 0,
            partial: String::new(),
            partial_dir: Direction::Rx,
            partial_ts: 0,
            last_ingest: Instant::now(),
        }
    }

    /// Drop rendered lines and re-read the ring buffer from the start.
    pub fn reset(&mut self) {
        self.lines.clear();
        self.partial.clear();
        self.partial_dir = Direction::Rx;
        self.last_ingest = Instant::now();
        self.last_seq = 0;
        self.trimmed = 0;
    }

    /// Clear the view without re-reading old data.
    pub fn clear_view(&mut self) {
        self.lines.clear();
        self.partial.clear();
        self.trimmed = 0;
    }

    pub fn set_format(&mut self, format: ViewFormat) {
        if format != self.format {
            self.format = format;
            self.reset();
        }
    }

    pub fn ingest(&mut self, batch: &ReadBatch) {
        self.last_ingest = Instant::now();
        for chunk in &batch.chunks {
            let ts = util::format_ts(chunk.ts_ms, true);
            match chunk.dir {
                Direction::Info | Direction::Error | Direction::Nvt => {
                    self.flush_partial();
                    self.push(ts, chunk.dir, codec::escape_bytes(&chunk.data));
                }
                dir => match self.format {
                    ViewFormat::Ascii => {
                        // A direction change terminates the pending line so RX
                        // and TX never share a row.
                        if !self.partial.is_empty() && self.partial_dir != dir {
                            self.flush_partial();
                        }
                        if self.partial.is_empty() {
                            self.partial_dir = dir;
                            self.partial_ts = chunk.ts_ms;
                        }
                        self.partial.push_str(&codec::display_bytes(&chunk.data));
                        while let Some(pos) = self.partial.find('\n') {
                            let line: String = self.partial.drain(..=pos).collect();
                            let line = line.trim_end_matches(['\n', '\r']).to_string();
                            let ts = util::format_ts(self.partial_ts, true);
                            self.push(ts, dir, line);
                            self.partial_ts = chunk.ts_ms;
                        }
                        if self.partial.len() > MAX_PARTIAL {
                            self.flush_partial();
                        }
                    }
                    ViewFormat::Hex => {
                        self.flush_partial();
                        for line in codec::hexdump(&chunk.data, 16).lines() {
                            self.push(ts.clone(), dir, line.to_string());
                        }
                    }
                    ViewFormat::Dec => {
                        self.flush_partial();
                        let text = codec::to_dec(&chunk.data);
                        for line in wrap(&text, 96) {
                            self.push(ts.clone(), dir, line);
                        }
                    }
                },
            }
        }
        self.last_seq = batch.next_seq;
        self.trim();
    }

    fn flush_partial(&mut self) {
        if self.partial.is_empty() {
            return;
        }
        let text = std::mem::take(&mut self.partial);
        let ts = util::format_ts(self.partial_ts, true);
        self.push(ts, self.partial_dir, text);
    }

    fn push(&mut self, ts: String, dir: Direction, text: String) {
        self.lines.push_back(TermLine { ts, dir, text });
    }

    fn trim(&mut self) {
        while self.lines.len() > MAX_LINES {
            self.lines.pop_front();
            self.trimmed += 1;
        }
    }

    /// Flush an unterminated line after a short idle period so prompts and
    /// partial responses are visible even when the device sends no newline.
    pub fn flush_idle(&mut self) {
        if self.partial.is_empty() || self.last_ingest.elapsed() < Duration::from_millis(150) {
            return;
        }
        self.flush_partial();
        self.trim();
    }

    /// Plain-text export of the (optionally filtered) view.
    pub fn text(&self, filter: &LineFilter) -> String {
        let mut out = String::new();
        for line in self.lines.iter().filter(|l| filter.matches(l)) {
            if self.show_ts {
                out.push('[');
                out.push_str(&line.ts);
                out.push_str("] ");
            }
            out.push_str(dir_tag(line.dir));
            out.push(' ');
            out.push_str(&line.text);
            out.push('\n');
        }
        out
    }
}

fn wrap(text: &str, width: usize) -> Vec<String> {
    if text.is_empty() {
        return vec![String::new()];
    }
    let mut out = Vec::new();
    let mut line = String::new();
    for word in text.split(' ') {
        if !line.is_empty() && line.len() + 1 + word.len() > width {
            out.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        out.push(line);
    }
    out
}

pub fn dir_tag(dir: Direction) -> &'static str {
    match dir {
        Direction::Rx => "RX",
        Direction::Tx => "TX",
        Direction::Info => "SYS",
        Direction::Error => "ERR",
        Direction::Nvt => "IAC",
    }
}

/// Which lines are visible.
#[derive(Clone, Default)]
pub struct LineFilter {
    pub query: String,
    pub hide_rx: bool,
    pub hide_tx: bool,
    pub hide_info: bool,
}

impl LineFilter {
    pub fn is_active(&self) -> bool {
        !self.query.trim().is_empty() || self.hide_rx || self.hide_tx || self.hide_info
    }

    pub fn matches(&self, line: &TermLine) -> bool {
        match line.dir {
            Direction::Rx if self.hide_rx => return false,
            Direction::Tx if self.hide_tx => return false,
            Direction::Info | Direction::Error | Direction::Nvt if self.hide_info => return false,
            _ => {}
        }
        let q = self.query.trim();
        q.is_empty() || line.text.to_lowercase().contains(&q.to_lowercase())
    }
}
