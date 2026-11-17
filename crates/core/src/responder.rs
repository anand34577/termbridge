//! Auto-responder: send a reply automatically when received data matches a pattern.
//!
//! Useful for simulating a device (answer `AT` with `OK`), keeping a link
//! alive, or acknowledging frames. Matching works across chunk boundaries.

use serde::{Deserialize, Serialize};

use crate::codec;
use crate::error::{CoreError, Result};

/// Maximum number of rules per session.
pub const MAX_RULES: usize = 32;
/// Longest accepted pattern in bytes.
pub const MAX_PATTERN: usize = 1024;

/// A rule as entered by the user (text with escapes, or hex).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RuleDef {
    pub enabled: bool,
    /// Bytes to look for in received data.
    pub pattern: String,
    /// Bytes to send back when the pattern is seen.
    pub reply: String,
    /// Interpret `pattern` and `reply` as hex instead of escaped text.
    pub hex: bool,
}

impl Default for RuleDef {
    fn default() -> Self {
        Self {
            enabled: true,
            pattern: String::new(),
            reply: String::new(),
            hex: false,
        }
    }
}

impl RuleDef {
    pub fn compile(&self) -> Result<Rule> {
        let pattern = codec::parse_input(&self.pattern, self.hex)?;
        let reply = codec::parse_input(&self.reply, self.hex)?;
        if pattern.is_empty() {
            return Err(CoreError::InvalidInput("pattern is empty".into()));
        }
        if pattern.len() > MAX_PATTERN {
            return Err(CoreError::InvalidInput(format!(
                "pattern is longer than {MAX_PATTERN} bytes"
            )));
        }
        if reply.is_empty() {
            return Err(CoreError::InvalidInput("reply is empty".into()));
        }
        Ok(Rule {
            def: self.clone(),
            pattern,
            reply,
            hits: 0,
        })
    }
}

#[derive(Debug, Clone)]
pub struct Rule {
    pub def: RuleDef,
    pub pattern: Vec<u8>,
    pub reply: Vec<u8>,
    pub hits: u64,
}

/// Snapshot of a rule with its hit counter, for UIs and MCP.
#[derive(Debug, Clone, Serialize)]
pub struct RuleStatus {
    #[serde(flatten)]
    pub def: RuleDef,
    pub hits: u64,
}

#[derive(Debug, Default)]
pub struct Responder {
    rules: Vec<Rule>,
    tail: Vec<u8>,
}

impl Responder {
    /// Replace all rules. Fails without changing anything if any rule is invalid.
    pub fn set_rules(&mut self, defs: &[RuleDef]) -> Result<()> {
        if defs.len() > MAX_RULES {
            return Err(CoreError::InvalidInput(format!(
                "at most {MAX_RULES} auto-response rules are allowed"
            )));
        }
        let mut rules = Vec::with_capacity(defs.len());
        for (index, def) in defs.iter().enumerate() {
            rules.push(
                def.compile()
                    .map_err(|e| CoreError::InvalidInput(format!("rule {}: {e}", index + 1)))?,
            );
        }
        self.rules = rules;
        self.tail.clear();
        Ok(())
    }

    pub fn status(&self) -> Vec<RuleStatus> {
        self.rules
            .iter()
            .map(|r| RuleStatus {
                def: r.def.clone(),
                hits: r.hits,
            })
            .collect()
    }

    pub fn is_active(&self) -> bool {
        self.rules.iter().any(|r| r.def.enabled)
    }

    /// Feed received bytes; returns the replies to send, in order.
    pub fn feed(&mut self, data: &[u8]) -> Vec<Vec<u8>> {
        if !self.is_active() || data.is_empty() {
            return Vec::new();
        }
        let tail_len = self.tail.len();
        let mut window = std::mem::take(&mut self.tail);
        window.extend_from_slice(data);

        // Collect (end position, rule index) so replies keep stream order.
        let mut hits: Vec<(usize, usize)> = Vec::new();
        for (index, rule) in self.rules.iter().enumerate() {
            if !rule.def.enabled || rule.pattern.len() > window.len() {
                continue;
            }
            let n = rule.pattern.len();
            let mut start = 0;
            while start + n <= window.len() {
                if window[start..start + n] == rule.pattern[..] {
                    let end = start + n;
                    // Only count matches that end in the new data; earlier
                    // ones were already handled on the previous call.
                    if end > tail_len {
                        hits.push((end, index));
                    }
                    start += n;
                } else {
                    start += 1;
                }
            }
        }
        hits.sort();
        let replies = hits
            .iter()
            .map(|&(_, index)| {
                self.rules[index].hits += 1;
                self.rules[index].reply.clone()
            })
            .collect();

        let keep = self
            .rules
            .iter()
            .map(|r| r.pattern.len())
            .max()
            .unwrap_or(1)
            .saturating_sub(1);
        let from = window.len().saturating_sub(keep);
        self.tail = window[from..].to_vec();
        replies
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(p: &str, r: &str) -> RuleDef {
        RuleDef {
            pattern: p.into(),
            reply: r.into(),
            ..Default::default()
        }
    }

    #[test]
    fn matches_across_chunks_once() {
        let mut resp = Responder::default();
        resp.set_rules(&[rule("AT\\r", "OK\\r\\n")]).unwrap();
        assert!(resp.feed(b"xxA").is_empty());
        assert_eq!(resp.feed(b"T\r"), vec![b"OK\r\n".to_vec()]);
        // The same bytes must not trigger again.
        assert!(resp.feed(b"zz").is_empty());
        assert_eq!(resp.status()[0].hits, 1);
    }

    #[test]
    fn multiple_matches_in_order() {
        let mut resp = Responder::default();
        resp.set_rules(&[rule("B", "2"), rule("A", "1")]).unwrap();
        assert_eq!(
            resp.feed(b"AB A"),
            vec![b"1".to_vec(), b"2".to_vec(), b"1".to_vec()]
        );
    }

    #[test]
    fn hex_rules_and_validation() {
        let mut resp = Responder::default();
        let def = RuleDef {
            pattern: "01 03".into(),
            reply: "06".into(),
            hex: true,
            enabled: true,
        };
        resp.set_rules(&[def]).unwrap();
        assert_eq!(resp.feed(&[0x01, 0x03]), vec![vec![0x06]]);
        assert!(resp.set_rules(&[rule("", "x")]).is_err());
        assert!(resp.set_rules(&[rule("x", "")]).is_err());
        // A failed update keeps the previous rules.
        assert_eq!(resp.status().len(), 1);
    }

    #[test]
    fn disabled_rules_do_nothing() {
        let mut resp = Responder::default();
        let mut def = rule("A", "B");
        def.enabled = false;
        resp.set_rules(&[def]).unwrap();
        assert!(resp.feed(b"AAA").is_empty());
    }
}
