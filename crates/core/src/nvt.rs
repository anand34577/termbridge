//! Minimal Telnet / NVT (Network Virtual Terminal) option negotiation.
//!
//! The parser strips IAC sequences from the application data stream, records
//! readable negotiation events for the terminal view, and answers every
//! option request with WONT/DONT. That is the correct behaviour for a plain
//! terminal that does not implement any Telnet options.

pub const IAC: u8 = 255;
pub const DONT: u8 = 254;
pub const DO: u8 = 253;
pub const WONT: u8 = 252;
pub const WILL: u8 = 251;
pub const SB: u8 = 250;
pub const SE: u8 = 240;

/// Result of feeding bytes through the parser.
#[derive(Debug, Default)]
pub struct NvtOutput {
    /// Application data with negotiation sequences removed.
    pub data: Vec<u8>,
    /// Bytes that should be written back to the peer.
    pub replies: Vec<u8>,
    /// Human-readable negotiation events.
    pub events: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Data,
    Iac,
    Negotiate(u8),
    Subnegotiate,
    SubnegotiateIac,
}

/// Longest subnegotiation payload kept for the event log.
const MAX_SUB: usize = 4096;

/// Stateful Telnet negotiation parser.
#[derive(Debug)]
pub struct NvtParser {
    state: State,
    sub: Vec<u8>,
}

impl Default for NvtParser {
    fn default() -> Self {
        Self::new()
    }
}

impl NvtParser {
    pub fn new() -> Self {
        Self {
            state: State::Data,
            sub: Vec::new(),
        }
    }

    pub fn feed(&mut self, input: &[u8]) -> NvtOutput {
        let mut out = NvtOutput::default();
        for &b in input {
            match self.state {
                State::Data => {
                    if b == IAC {
                        self.state = State::Iac;
                    } else {
                        out.data.push(b);
                    }
                }
                State::Iac => match b {
                    IAC => {
                        out.data.push(IAC);
                        self.state = State::Data;
                    }
                    DO | DONT | WILL | WONT => {
                        self.state = State::Negotiate(b);
                    }
                    SB => {
                        self.sub.clear();
                        self.state = State::Subnegotiate;
                    }
                    other => {
                        out.events.push(format!("IAC {}", command_name(other)));
                        self.state = State::Data;
                    }
                },
                State::Negotiate(verb) => {
                    let opt = option_name(b);
                    out.events
                        .push(format!("IAC {} {}", command_name(verb), opt));
                    match verb {
                        DO => {
                            out.replies.push(IAC);
                            out.replies.push(WONT);
                            out.replies.push(b);
                        }
                        WILL => {
                            out.replies.push(IAC);
                            out.replies.push(DONT);
                            out.replies.push(b);
                        }
                        _ => {}
                    }
                    self.state = State::Data;
                }
                State::Subnegotiate => {
                    if b == IAC {
                        self.state = State::SubnegotiateIac;
                    } else if self.sub.len() < MAX_SUB {
                        self.sub.push(b);
                    }
                }
                State::SubnegotiateIac => {
                    if b == SE {
                        out.events
                            .push(format!("IAC SB {} IAC SE", crate::codec::to_hex(&self.sub)));
                        self.state = State::Data;
                    } else {
                        if self.sub.len() + 2 <= MAX_SUB {
                            self.sub.push(IAC);
                            self.sub.push(b);
                        }
                        self.state = State::Subnegotiate;
                    }
                }
            }
        }
        out
    }
}

fn command_name(cmd: u8) -> String {
    match cmd {
        DO => "DO".into(),
        DONT => "DONT".into(),
        WILL => "WILL".into(),
        WONT => "WONT".into(),
        SB => "SB".into(),
        SE => "SE".into(),
        IAC => "IAC".into(),
        other => format!("0x{other:02X}"),
    }
}

fn option_name(opt: u8) -> String {
    match opt {
        0 => "BINARY".into(),
        1 => "ECHO".into(),
        3 => "SUPPRESS-GO-AHEAD".into(),
        5 => "STATUS".into(),
        24 => "TERMINAL-TYPE".into(),
        31 => "NAWS".into(),
        32 => "TERMINAL-SPEED".into(),
        34 => "LINEMODE".into(),
        39 => "NEW-ENVIRON".into(),
        44 => "COM-PORT-OPTION".into(),
        other => format!("0x{other:02X}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passes_plain_data() {
        let mut p = NvtParser::new();
        let out = p.feed(b"hello");
        assert_eq!(out.data, b"hello");
        assert!(out.replies.is_empty());
        assert!(out.events.is_empty());
    }

    #[test]
    fn strips_negotiation_and_replies() {
        let mut p = NvtParser::new();
        let out = p.feed(&[IAC, DO, 1, b'x', IAC, WILL, 3]);
        assert_eq!(out.data, b"x");
        assert_eq!(out.replies, vec![IAC, WONT, 1, IAC, DONT, 3]);
        assert_eq!(out.events.len(), 2);
    }

    #[test]
    fn handles_escaped_iac() {
        let mut p = NvtParser::new();
        let out = p.feed(&[IAC, IAC]);
        assert_eq!(out.data, vec![IAC]);
    }

    #[test]
    fn handles_subnegotiation() {
        let mut p = NvtParser::new();
        let out = p.feed(&[IAC, SB, 24, 0, b'V', b'T', IAC, SE, b'z']);
        assert_eq!(out.data, b"z");
        assert_eq!(out.events.len(), 1);
    }
}
