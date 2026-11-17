//! Byte formatting and input parsing.
//!
//! Input parsing accepts `\#013` (decimal), `$0d` (hex), `##`, `$$` and `<<`
//! escapes, plus the usual `\r`, `\n`, `\t`, `\0` and `\xNN` sequences.

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};

/// How bytes are rendered in a terminal view or an MCP response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ViewFormat {
    #[default]
    Ascii,
    Hex,
    Dec,
}

impl ViewFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            ViewFormat::Ascii => "ascii",
            ViewFormat::Hex => "hex",
            ViewFormat::Dec => "dec",
        }
    }

    pub fn parse(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "ascii" | "text" | "raw" => Ok(ViewFormat::Ascii),
            "hex" => Ok(ViewFormat::Hex),
            "dec" | "decimal" => Ok(ViewFormat::Dec),
            other => Err(CoreError::InvalidInput(format!(
                "unknown view format '{other}' (expected ascii, hex or dec)"
            ))),
        }
    }
}

/// Parse user input into bytes.
pub fn parse_input(input: &str, hex: bool) -> Result<Vec<u8>> {
    if hex {
        parse_hex(input)
    } else {
        parse_escapes(input)
    }
}

/// Parse a hex string. Whitespace and `, ; : - _` separators are ignored and
/// `0x` prefixes are allowed.
pub fn parse_hex(input: &str) -> Result<Vec<u8>> {
    let mut digits = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c.is_whitespace() || matches!(c, ',' | ';' | ':' | '-' | '_') {
            continue;
        }
        if c == '0' && matches!(chars.peek(), Some('x') | Some('X')) {
            chars.next();
            continue;
        }
        if c.is_ascii_hexdigit() {
            digits.push(c);
        } else {
            return Err(CoreError::InvalidInput(format!(
                "invalid hex character '{c}'"
            )));
        }
    }
    if !digits.len().is_multiple_of(2) {
        return Err(CoreError::InvalidInput(
            "hex input has an odd number of digits".to_string(),
        ));
    }
    let mut out = Vec::with_capacity(digits.len() / 2);
    let bytes = digits.as_bytes();
    for pair in bytes.chunks(2) {
        let s = std::str::from_utf8(pair)
            .map_err(|_| CoreError::InvalidInput("invalid hex input".into()))?;
        out.push(
            u8::from_str_radix(s, 16)
                .map_err(|_| CoreError::InvalidInput(format!("invalid hex byte '{s}'")))?,
        );
    }
    Ok(out)
}

/// Parse ASCII input with escape sequences.
pub fn parse_escapes(input: &str) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(input.len());
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\\' => {
                i += 1;
                if i >= chars.len() {
                    out.push(b'\\');
                    break;
                }
                match chars[i] {
                    '#' => {
                        i += 1;
                        let mut value: u32 = 0;
                        let mut digits = 0;
                        while digits < 3 && i < chars.len() && chars[i].is_ascii_digit() {
                            value = value * 10 + chars[i].to_digit(10).unwrap_or(0);
                            i += 1;
                            digits += 1;
                        }
                        if digits == 0 || value > 255 {
                            return Err(CoreError::InvalidInput(
                                "\\# escape expects a decimal value 0..255".into(),
                            ));
                        }
                        out.push(value as u8);
                    }
                    'x' | 'X' => {
                        i += 1;
                        let mut value = 0u32;
                        let mut digits = 0;
                        while digits < 2 && i < chars.len() && chars[i].is_ascii_hexdigit() {
                            value = value * 16 + chars[i].to_digit(16).unwrap_or(0);
                            i += 1;
                            digits += 1;
                        }
                        if digits != 2 {
                            return Err(CoreError::InvalidInput(
                                "\\x escape expects two hex digits".into(),
                            ));
                        }
                        out.push(value as u8);
                    }
                    'r' => {
                        out.push(b'\r');
                        i += 1;
                    }
                    'n' => {
                        out.push(b'\n');
                        i += 1;
                    }
                    't' => {
                        out.push(b'\t');
                        i += 1;
                    }
                    '0' => {
                        out.push(0);
                        i += 1;
                    }
                    '\\' => {
                        out.push(b'\\');
                        i += 1;
                    }
                    other => {
                        // Unknown escape: keep the character itself.
                        push_char(&mut out, other);
                        i += 1;
                    }
                }
            }
            '$' => {
                if i + 1 < chars.len() && chars[i + 1] == '$' {
                    out.push(b'$');
                    i += 2;
                    continue;
                }
                if i + 2 < chars.len()
                    && chars[i + 1].is_ascii_hexdigit()
                    && chars[i + 2].is_ascii_hexdigit()
                {
                    let hi = chars[i + 1].to_digit(16).unwrap_or(0) as u8;
                    let lo = chars[i + 2].to_digit(16).unwrap_or(0) as u8;
                    out.push((hi << 4) | lo);
                    i += 3;
                } else {
                    out.push(b'$');
                    i += 1;
                }
            }
            '#' => {
                if i + 1 < chars.len() && chars[i + 1] == '#' {
                    out.push(b'#');
                    i += 2;
                } else {
                    out.push(b'#');
                    i += 1;
                }
            }
            '<' => {
                if i + 1 < chars.len() && chars[i + 1] == '<' {
                    out.push(b'<');
                    i += 2;
                } else {
                    out.push(b'<');
                    i += 1;
                }
            }
            other => {
                push_char(&mut out, other);
                i += 1;
            }
        }
    }
    Ok(out)
}

fn push_char(out: &mut Vec<u8>, c: char) {
    let mut buf = [0u8; 4];
    out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
}

/// Uppercase hex pairs separated by spaces, e.g. `48 65 6C 6C 6F`.
pub fn to_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 3);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        out.push_str(&format!("{b:02X}"));
    }
    out
}

/// Decimal byte values separated by spaces.
pub fn to_dec(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 4);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        out.push_str(&b.to_string());
    }
    out
}

/// Escape bytes so the result is printable and unambiguous inside JSON text.
/// Printable ASCII is kept, `\` is doubled and control bytes become `\xNN`.
pub fn escape_bytes(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    for &b in bytes {
        match b {
            b'\\' => out.push_str("\\\\"),
            b'\r' => out.push_str("\\r"),
            b'\n' => out.push_str("\\n"),
            b'\t' => out.push_str("\\t"),
            0x20..=0x7E => out.push(b as char),
            _ => out.push_str(&format!("\\x{b:02X}")),
        }
    }
    out
}

/// Human-friendly rendering of raw bytes for a terminal pane.
///
/// `\r`, `\r\n` and `\n` all start a new line, so devices that only send CR
/// (many bootloaders and console ports) do not collapse into one long line.
pub fn display_bytes(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                // Consume the LF of a CRLF pair so it produces one line break.
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push('\n');
            }
            '\n' | '\t' => out.push(c),
            c if c.is_control() => out.push('.'),
            c => out.push(c),
        }
    }
    out
}

/// Render a chunk according to the requested view format.
pub fn format_bytes(bytes: &[u8], format: ViewFormat) -> String {
    match format {
        ViewFormat::Ascii => display_bytes(bytes),
        ViewFormat::Hex => to_hex(bytes),
        ViewFormat::Dec => to_dec(bytes),
    }
}

/// Classic hex dump with offset, hex bytes and an ASCII column.
pub fn hexdump(bytes: &[u8], width: usize) -> String {
    let width = width.clamp(1, 64);
    let mut out = String::new();
    for (line, chunk) in bytes.chunks(width).enumerate() {
        out.push_str(&format!("{:08X}  ", line * width));
        for i in 0..width {
            match chunk.get(i) {
                Some(b) => out.push_str(&format!("{b:02X} ")),
                None => out.push_str("   "),
            }
        }
        out.push(' ');
        for &b in chunk {
            out.push(if (0x20..=0x7E).contains(&b) {
                b as char
            } else {
                '.'
            });
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_escapes() {
        assert_eq!(parse_escapes("AT\\r\\n").unwrap(), b"AT\r\n");
        assert_eq!(parse_escapes("\\#013\\#010").unwrap(), b"\r\n");
        assert_eq!(parse_escapes("$0d$0A").unwrap(), b"\r\n");
        assert_eq!(parse_escapes("##$$<<").unwrap(), b"#$<");
        assert_eq!(parse_escapes("\\x41\\x42").unwrap(), b"AB");
        assert_eq!(parse_escapes("plain").unwrap(), b"plain");
        assert!(parse_escapes("\\x4").is_err());
        assert!(parse_escapes("\\x").is_err());
        assert!(parse_escapes("\\#256").is_err());
    }

    #[test]
    fn parses_hex() {
        assert_eq!(parse_hex("48 65 6C 6C 6F").unwrap(), b"Hello");
        assert_eq!(parse_hex("0x48,0x65").unwrap(), b"He");
        assert_eq!(parse_hex("48656c6c6f").unwrap(), b"Hello");
        assert!(parse_hex("4").is_err());
        assert!(parse_hex("zz").is_err());
    }

    #[test]
    fn formats_and_escapes() {
        assert_eq!(to_hex(&[0x00, 0xFF]), "00 FF");
        assert_eq!(to_dec(&[0, 255]), "0 255");
        assert_eq!(escape_bytes(b"a\r\n\x01"), "a\\r\\n\\x01");
        assert_eq!(display_bytes(b"a\r\nb"), "a\nb");
        assert_eq!(display_bytes(b"a\rb"), "a\nb");
        assert_eq!(display_bytes(b"a\nb"), "a\nb");
    }

    #[test]
    fn dumps_hex() {
        let dump = hexdump(b"Hello", 8);
        assert!(dump.contains("48 65 6C 6C 6F"));
        assert!(dump.contains("Hello"));
    }
}
