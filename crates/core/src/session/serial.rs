//! Serial port transport.

use std::io::Write;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::UnboundedReceiver;

use super::{Command, WorkerCtx};
use crate::error::{CoreError, Result};
use crate::types::{Direction, ModemLines};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Parity {
    #[default]
    None,
    Even,
    Odd,
    Mark,
    Space,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FlowControl {
    #[default]
    None,
    RtsCts,
    XonXoff,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SerialConfig {
    pub port: String,
    pub baud: u32,
    pub data_bits: u8,
    pub parity: Parity,
    pub stop_bits: u8,
    pub flow_control: FlowControl,
    /// Initial DTR output state.
    pub dtr: bool,
    /// Initial RTS output state.
    pub rts: bool,
    pub read_timeout_ms: u64,
    pub label: Option<String>,
}

impl Default for SerialConfig {
    fn default() -> Self {
        Self {
            port: if cfg!(windows) {
                "COM1".to_string()
            } else {
                "/dev/ttyUSB0".to_string()
            },
            baud: 115200,
            data_bits: 8,
            parity: Parity::None,
            stop_bits: 1,
            flow_control: FlowControl::None,
            dtr: false,
            rts: false,
            read_timeout_ms: 10,
            label: None,
        }
    }
}

fn data_bits(value: u8) -> serialport::DataBits {
    match value {
        5 => serialport::DataBits::Five,
        6 => serialport::DataBits::Six,
        7 => serialport::DataBits::Seven,
        _ => serialport::DataBits::Eight,
    }
}

fn stop_bits(value: u8) -> serialport::StopBits {
    if value == 2 {
        serialport::StopBits::Two
    } else {
        serialport::StopBits::One
    }
}

fn parity(value: Parity) -> serialport::Parity {
    match value {
        Parity::None => serialport::Parity::None,
        Parity::Even => serialport::Parity::Even,
        Parity::Odd => serialport::Parity::Odd,
        // serialport 4.x does not expose mark/space parity on all platforms.
        Parity::Mark | Parity::Space => serialport::Parity::None,
    }
}

fn flow_control(value: FlowControl) -> serialport::FlowControl {
    match value {
        FlowControl::None => serialport::FlowControl::None,
        FlowControl::RtsCts => serialport::FlowControl::Hardware,
        FlowControl::XonXoff => serialport::FlowControl::Software,
    }
}

pub(crate) fn open_port(cfg: &SerialConfig) -> Result<Box<dyn serialport::SerialPort>> {
    if matches!(cfg.parity, Parity::Mark | Parity::Space) {
        return Err(CoreError::Unsupported(
            "mark/space parity is not supported by the serial backend".into(),
        ));
    }
    let mut port = serialport::new(&cfg.port, cfg.baud)
        .data_bits(data_bits(cfg.data_bits))
        .parity(parity(cfg.parity))
        .stop_bits(stop_bits(cfg.stop_bits))
        .flow_control(flow_control(cfg.flow_control))
        .timeout(Duration::from_millis(cfg.read_timeout_ms.max(1)))
        .open()
        .map_err(|e| CoreError::Connect(format!("open {}: {e}", cfg.port)))?;
    // Some platforms / drivers do not support these lines; a failure here is
    // not fatal for the terminal itself.
    let _ = port.write_data_terminal_ready(cfg.dtr);
    let _ = port.write_request_to_send(cfg.rts);
    Ok(port)
}

pub(crate) fn worker(
    mut port: Box<dyn serialport::SerialPort>,
    cfg: SerialConfig,
    ctx: WorkerCtx,
    mut rx: UnboundedReceiver<Command>,
) {
    ctx.set_connected(Some(cfg.port.clone()));
    ctx.info(format!(
        "{} open at {} baud, {}{}{}, flow {}",
        cfg.port,
        cfg.baud,
        cfg.data_bits,
        match cfg.parity {
            Parity::None => "N",
            Parity::Even => "E",
            Parity::Odd => "O",
            Parity::Mark => "M",
            Parity::Space => "S",
        },
        cfg.stop_bits,
        match cfg.flow_control {
            FlowControl::None => "none",
            FlowControl::RtsCts => "RTS/CTS",
            FlowControl::XonXoff => "XON/XOFF",
        }
    ));

    let mut lines = ModemLines {
        rts: cfg.rts,
        dtr: cfg.dtr,
        ..ModemLines::default()
    };
    ctx.set_modem(lines);
    let mut buf = [0u8; 8192];
    let mut last_poll = Instant::now();

    loop {
        let mut user_closed = false;
        while let Ok(cmd) = rx.try_recv() {
            match cmd {
                Command::Write(data) => match port.write_all(&data).and_then(|_| port.flush()) {
                    Ok(()) => ctx.push(Direction::Tx, &data),
                    Err(err) => ctx.error(format!("serial write failed: {err}")),
                },
                Command::Close => {
                    user_closed = true;
                    break;
                }
                Command::SetRts(value) => {
                    let _ = port.write_request_to_send(value);
                    lines.rts = value;
                    ctx.set_modem(lines);
                }
                Command::SetDtr(value) => {
                    let _ = port.write_data_terminal_ready(value);
                    lines.dtr = value;
                    ctx.set_modem(lines);
                }
                Command::SetBreak(value) => {
                    let _ = if value {
                        port.set_break()
                    } else {
                        port.clear_break()
                    };
                }
                Command::SetEcho(_) => {}
            }
        }
        if user_closed || ctx.is_closed() {
            break;
        }

        match port.read(&mut buf) {
            Ok(0) => {}
            Ok(n) => ctx.push(Direction::Rx, &buf[..n]),
            Err(err) => {
                if err.kind() == std::io::ErrorKind::TimedOut {
                    // Normal read timeout; poll again.
                } else {
                    ctx.error(format!("serial read failed: {err}"));
                    break;
                }
            }
        }

        if last_poll.elapsed() >= Duration::from_millis(250) {
            last_poll = Instant::now();
            let mut next = lines;
            next.cts = port.read_clear_to_send().unwrap_or(false);
            next.dsr = port.read_data_set_ready().unwrap_or(false);
            next.ri = port.read_ring_indicator().unwrap_or(false);
            next.cd = port.read_carrier_detect().unwrap_or(false);
            if next != lines {
                lines = next;
                ctx.set_modem(lines);
            }
        }
    }

    ctx.set_disconnected("serial port closed");
    ctx.info("serial port closed");
}
