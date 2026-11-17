//! Small helpers shared by the core, CLI and GUI.

use chrono::{Local, TimeZone, Utc};

pub fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

/// Format a millisecond timestamp. `local` selects the machine time zone.
pub fn format_ts(ts_ms: i64, local: bool) -> String {
    if local {
        match Local.timestamp_millis_opt(ts_ms) {
            chrono::LocalResult::Single(dt) => dt.format("%H:%M:%S%.3f").to_string(),
            _ => ts_ms.to_string(),
        }
    } else {
        match Utc.timestamp_millis_opt(ts_ms) {
            chrono::LocalResult::Single(dt) => dt.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string(),
            _ => ts_ms.to_string(),
        }
    }
}

/// Compact byte count for status bars.
pub fn human_bytes(bytes: usize) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0usize;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}
