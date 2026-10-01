//! What one transcript `message.usage` object bills, read the way the API
//! bills it. Two rules, both verified against real transcripts:
//!
//! 1. Fable-class multi-pass turns (server-side tool iterations) carry an
//!    `iterations` array: the top-level cache fields aggregate all passes,
//!    but top-level input/output reflect only the FINAL pass. Billing is
//!    the iteration sum (input 4 at top level vs 142,478 summed).
//! 2. Cache writes split into 5-minute and 1-hour tiers under
//!    `cache_creation`; any write not covered by the split bills at the
//!    5-minute rate, the default TTL.

use serde::Deserialize;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Tokens {
    pub input: u64,
    pub output: u64,
    pub write_5m: u64,
    pub write_1h: u64,
    pub read: u64,
}

impl Tokens {
    /// Every token the call billed.
    pub fn total(&self) -> u64 {
        self.input + self.output + self.write_5m + self.write_1h + self.read
    }

    /// Tokens the model read as input: uncached, cache writes and cache reads.
    pub fn context(&self) -> u64 {
        self.input + self.write_5m + self.write_1h + self.read
    }

    pub fn add(&mut self, other: &Tokens) {
        self.input += other.input;
        self.output += other.output;
        self.write_5m += other.write_5m;
        self.write_1h += other.write_1h;
        self.read += other.read;
    }
}

/// `usage.speed`. Anything other than "standard" or "fast" is kept verbatim
/// so pricing can refuse it instead of guessing.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Speed {
    #[default]
    Standard,
    Fast,
    Other(String),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Billed {
    pub tokens: Tokens,
    pub speed: Speed,
}

#[derive(Deserialize, Default)]
pub struct RawUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
    cache_creation: Option<RawCacheCreation>,
    iterations: Option<Vec<RawIteration>>,
    speed: Option<String>,
}

#[derive(Deserialize, Default)]
struct RawIteration {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
}

#[derive(Deserialize, Default)]
struct RawCacheCreation {
    #[serde(default)]
    ephemeral_5m_input_tokens: u64,
    #[serde(default)]
    ephemeral_1h_input_tokens: u64,
}

impl RawUsage {
    pub fn billed(&self) -> Billed {
        let (input, output) = match self.iterations.as_deref() {
            Some(iters) if !iters.is_empty() => (
                iters.iter().map(|i| i.input_tokens).sum(),
                iters.iter().map(|i| i.output_tokens).sum(),
            ),
            _ => (self.input_tokens, self.output_tokens),
        };
        let split = self.cache_creation.as_ref();
        let write_1h = split.map_or(0, |s| s.ephemeral_1h_input_tokens);
        let write_5m = split.map_or(0, |s| s.ephemeral_5m_input_tokens)
            + self.cache_creation_input_tokens.saturating_sub(
                split.map_or(0, |s| s.ephemeral_5m_input_tokens + s.ephemeral_1h_input_tokens),
            );
        let speed = match self.speed.as_deref() {
            None | Some("standard") => Speed::Standard,
            Some("fast") => Speed::Fast,
            Some(other) => Speed::Other(other.to_string()),
        };
        Billed {
            tokens: Tokens { input, output, write_5m, write_1h, read: self.cache_read_input_tokens },
            speed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn billed(json: &str) -> Billed {
        serde_json::from_str::<RawUsage>(json).unwrap().billed()
    }

    #[test]
    fn iterations_sum_input_and_output() {
        let b = billed(
            r#"{"input_tokens":4,"output_tokens":10,"cache_read_input_tokens":7,
                "iterations":[{"input_tokens":100,"output_tokens":20},{"input_tokens":4,"output_tokens":10}]}"#,
        );
        assert_eq!(b.tokens, Tokens { input: 104, output: 30, write_5m: 0, write_1h: 0, read: 7 });
    }

    #[test]
    fn unsplit_cache_writes_bill_at_5m() {
        let b = billed(
            r#"{"cache_creation_input_tokens":100,
                "cache_creation":{"ephemeral_5m_input_tokens":10,"ephemeral_1h_input_tokens":60}}"#,
        );
        assert_eq!((b.tokens.write_5m, b.tokens.write_1h), (40, 60));
        assert_eq!(billed(r#"{"cache_creation_input_tokens":9}"#).tokens.write_5m, 9);
    }

    #[test]
    fn speed_is_read_and_unknown_speeds_are_kept() {
        assert_eq!(billed(r#"{"speed":"standard"}"#).speed, Speed::Standard);
        assert_eq!(billed(r#"{"speed":"fast"}"#).speed, Speed::Fast);
        assert_eq!(billed(r#"{"speed":"turbo"}"#).speed, Speed::Other("turbo".into()));
    }
}
