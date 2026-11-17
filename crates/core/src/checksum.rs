//! Checksums commonly used by serial and industrial protocols.
//!
//! Every algorithm returns the bytes in the order they are normally appended
//! to a frame (Modbus RTU CRC is little-endian, everything else big-endian).

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ChecksumKind {
    #[default]
    None,
    /// CRC-16/MODBUS: poly 0x8005 (reflected 0xA001), init 0xFFFF, appended low byte first.
    Crc16Modbus,
    /// CRC-16/CCITT-FALSE: poly 0x1021, init 0xFFFF.
    Crc16Ccitt,
    /// CRC-16/XMODEM: poly 0x1021, init 0x0000.
    Crc16Xmodem,
    /// CRC-32 (IEEE 802.3 / zlib).
    Crc32,
    /// Modbus ASCII LRC: two's complement of the 8-bit sum.
    Lrc,
    /// XOR of all bytes (NMEA style).
    Xor8,
    /// 8-bit sum modulo 256.
    Sum8,
}

impl ChecksumKind {
    pub const ALL: [ChecksumKind; 8] = [
        ChecksumKind::None,
        ChecksumKind::Crc16Modbus,
        ChecksumKind::Crc16Ccitt,
        ChecksumKind::Crc16Xmodem,
        ChecksumKind::Crc32,
        ChecksumKind::Lrc,
        ChecksumKind::Xor8,
        ChecksumKind::Sum8,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ChecksumKind::None => "None",
            ChecksumKind::Crc16Modbus => "CRC-16 Modbus",
            ChecksumKind::Crc16Ccitt => "CRC-16 CCITT",
            ChecksumKind::Crc16Xmodem => "CRC-16 XMODEM",
            ChecksumKind::Crc32 => "CRC-32",
            ChecksumKind::Lrc => "LRC",
            ChecksumKind::Xor8 => "XOR",
            ChecksumKind::Sum8 => "SUM-8",
        }
    }

    pub fn parse(value: &str) -> Result<Self> {
        let v = value
            .trim()
            .to_ascii_lowercase()
            .replace(['-', '_', ' ', '/'], "");
        Ok(match v.as_str() {
            "" | "none" => ChecksumKind::None,
            "crc16modbus" | "modbus" | "crc16" => ChecksumKind::Crc16Modbus,
            "crc16ccitt" | "ccitt" | "crc16ccittfalse" => ChecksumKind::Crc16Ccitt,
            "crc16xmodem" | "xmodem" => ChecksumKind::Crc16Xmodem,
            "crc32" => ChecksumKind::Crc32,
            "lrc" => ChecksumKind::Lrc,
            "xor" | "xor8" => ChecksumKind::Xor8,
            "sum" | "sum8" => ChecksumKind::Sum8,
            other => {
                return Err(CoreError::InvalidInput(format!(
                    "unknown checksum '{other}' (expected none, crc16_modbus, crc16_ccitt, crc16_xmodem, crc32, lrc, xor8 or sum8)"
                )))
            }
        })
    }
}

/// Checksum bytes for `data`, in append order. Empty for `None`.
pub fn compute(kind: ChecksumKind, data: &[u8]) -> Vec<u8> {
    match kind {
        ChecksumKind::None => Vec::new(),
        ChecksumKind::Crc16Modbus => crc16_modbus(data).to_le_bytes().to_vec(),
        ChecksumKind::Crc16Ccitt => crc16_ccitt(data, 0xFFFF).to_be_bytes().to_vec(),
        ChecksumKind::Crc16Xmodem => crc16_ccitt(data, 0x0000).to_be_bytes().to_vec(),
        ChecksumKind::Crc32 => crc32(data).to_be_bytes().to_vec(),
        ChecksumKind::Lrc => vec![lrc(data)],
        ChecksumKind::Xor8 => vec![data.iter().fold(0u8, |a, b| a ^ b)],
        ChecksumKind::Sum8 => vec![data.iter().fold(0u8, |a, b| a.wrapping_add(*b))],
    }
}

/// `data` followed by its checksum.
pub fn append(kind: ChecksumKind, data: &[u8]) -> Vec<u8> {
    let mut out = data.to_vec();
    out.extend(compute(kind, data));
    out
}

pub fn crc16_modbus(data: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &b in data {
        crc ^= b as u16;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xA001
            } else {
                crc >> 1
            };
        }
    }
    crc
}

pub fn crc16_ccitt(data: &[u8], init: u16) -> u16 {
    let mut crc = init;
    for &b in data {
        crc ^= (b as u16) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
        }
    }
    crc
}

pub fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

pub fn lrc(data: &[u8]) -> u8 {
    data.iter()
        .fold(0u8, |a, b| a.wrapping_add(*b))
        .wrapping_neg()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Standard check values for the ASCII string "123456789".
    const CHECK: &[u8] = b"123456789";

    #[test]
    fn known_vectors() {
        assert_eq!(crc16_modbus(CHECK), 0x4B37);
        assert_eq!(crc16_ccitt(CHECK, 0xFFFF), 0x29B1);
        assert_eq!(crc16_ccitt(CHECK, 0x0000), 0x31C3);
        assert_eq!(crc32(CHECK), 0xCBF4_3926);
    }

    #[test]
    fn modbus_frame() {
        // Read holding registers: 01 03 00 00 00 0A -> CRC C5 CD
        let frame = append(
            ChecksumKind::Crc16Modbus,
            &[0x01, 0x03, 0x00, 0x00, 0x00, 0x0A],
        );
        assert_eq!(&frame[6..], &[0xC5, 0xCD]);
    }

    #[test]
    fn simple_sums() {
        assert_eq!(compute(ChecksumKind::Xor8, &[0x01, 0x02, 0x04]), vec![0x07]);
        assert_eq!(compute(ChecksumKind::Sum8, &[0xFF, 0x02]), vec![0x01]);
        assert_eq!(lrc(&[0x01, 0x03]), 0xFC);
        assert!(compute(ChecksumKind::None, b"x").is_empty());
    }

    #[test]
    fn parses_names() {
        assert_eq!(
            ChecksumKind::parse("CRC16-Modbus").unwrap(),
            ChecksumKind::Crc16Modbus
        );
        assert_eq!(ChecksumKind::parse("xor").unwrap(), ChecksumKind::Xor8);
        assert!(ChecksumKind::parse("md5").is_err());
    }
}
