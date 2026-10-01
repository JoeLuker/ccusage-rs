//! Anthropic first-party API list prices, US dollars per million tokens.
//!
//! One row per exact model id, every rate written out: cache multipliers
//! are NOT uniform (cache reads are 0.05x input on Opus 5.5 and 0.025x on
//! Fable 5.1 / Mythos 5.1), so deriving rates from input price is wrong.
//! A model id that is not in the table is unpriced, never matched to an
//! older sibling: a point release can change price (Opus 5.5 is $4/$20,
//! Opus 5 is $5/$25). Callers must surface unpriced usage.
//!
//! Dated ids (`claude-haiku-4-5-20251001`) price as their undated id.
//! Fast-mode rows are the documented fast input/output prices with the
//! standard cache multipliers applied on top, per the pricing page.

use crate::usage::{Billed, Speed, Tokens};
use serde::Serialize;

/// Where every row below was read from, and when.
pub const SOURCE: &str = "https://platform.claude.com/docs/en/about-claude/pricing";
pub const VERIFIED: &str = "2026-10-01";

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Price {
    pub input: f64,
    pub write_5m: f64,
    pub write_1h: f64,
    pub read: f64,
    pub output: f64,
}

const fn p(input: f64, write_5m: f64, write_1h: f64, read: f64, output: f64) -> Price {
    Price { input, write_5m, write_1h, read, output }
}

struct Row {
    id: &'static str,
    standard: Price,
    fast: Option<Price>,
}

const ROWS: &[Row] = &[
    Row { id: "claude-fable-5-1", standard: p(10.0, 12.5, 20.0, 0.25, 50.0), fast: None },
    Row { id: "claude-mythos-5-1", standard: p(10.0, 12.5, 20.0, 0.25, 50.0), fast: None },
    Row { id: "claude-fable-5", standard: p(10.0, 12.5, 20.0, 1.0, 50.0), fast: None },
    Row { id: "claude-mythos-5", standard: p(10.0, 12.5, 20.0, 1.0, 50.0), fast: None },
    Row { id: "claude-opus-5-5", standard: p(4.0, 5.0, 8.0, 0.2, 20.0), fast: Some(p(8.0, 10.0, 16.0, 0.4, 40.0)) },
    Row { id: "claude-opus-5", standard: p(5.0, 6.25, 10.0, 0.5, 25.0), fast: Some(p(10.0, 12.5, 20.0, 1.0, 50.0)) },
    Row { id: "claude-opus-4-8", standard: p(5.0, 6.25, 10.0, 0.5, 25.0), fast: Some(p(10.0, 12.5, 20.0, 1.0, 50.0)) },
    Row { id: "claude-opus-4-7", standard: p(5.0, 6.25, 10.0, 0.5, 25.0), fast: None },
    Row { id: "claude-opus-4-6", standard: p(5.0, 6.25, 10.0, 0.5, 25.0), fast: None },
    Row { id: "claude-opus-4-5", standard: p(5.0, 6.25, 10.0, 0.5, 25.0), fast: None },
    Row { id: "claude-opus-4-1", standard: p(15.0, 18.75, 30.0, 1.5, 75.0), fast: None },
    Row { id: "claude-opus-4", standard: p(15.0, 18.75, 30.0, 1.5, 75.0), fast: None },
    Row { id: "claude-sonnet-5-5", standard: p(2.0, 2.5, 4.0, 0.2, 10.0), fast: None },
    Row { id: "claude-sonnet-5", standard: p(2.0, 2.5, 4.0, 0.2, 10.0), fast: None },
    Row { id: "claude-sonnet-4-6", standard: p(3.0, 3.75, 6.0, 0.3, 15.0), fast: None },
    Row { id: "claude-sonnet-4-5", standard: p(3.0, 3.75, 6.0, 0.3, 15.0), fast: None },
    Row { id: "claude-sonnet-4", standard: p(3.0, 3.75, 6.0, 0.3, 15.0), fast: None },
    Row { id: "claude-haiku-4-5", standard: p(1.0, 1.25, 2.0, 0.1, 5.0), fast: None },
    Row { id: "claude-3-5-haiku", standard: p(0.8, 1.0, 1.6, 0.08, 4.0), fast: None },
];

/// `claude-haiku-4-5-20251001` -> `claude-haiku-4-5`.
fn undated(model: &str) -> &str {
    match model.rsplit_once('-') {
        Some((head, tail)) if tail.len() == 8 && tail.bytes().all(|b| b.is_ascii_digit()) => head,
        _ => model,
    }
}

/// The rates one call bills at, or None when the model or speed is not in
/// the table.
pub fn price(model: &str, speed: &Speed) -> Option<Price> {
    let id = undated(model);
    let row = ROWS.iter().find(|row| row.id == id)?;
    match speed {
        Speed::Standard => Some(row.standard),
        Speed::Fast => row.fast,
        Speed::Other(_) => None,
    }
}

/// Dollars per token category.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct Dollars {
    pub input: f64,
    pub output: f64,
    pub write_5m: f64,
    pub write_1h: f64,
    pub read: f64,
}

impl Dollars {
    pub fn total(&self) -> f64 {
        self.input + self.output + self.write_5m + self.write_1h + self.read
    }

    pub fn add(&mut self, other: &Dollars) {
        self.input += other.input;
        self.output += other.output;
        self.write_5m += other.write_5m;
        self.write_1h += other.write_1h;
        self.read += other.read;
    }
}

impl Price {
    pub fn dollars(&self, tokens: &Tokens) -> Dollars {
        let per = |n: u64, rate: f64| n as f64 * rate / 1_000_000.0;
        Dollars {
            input: per(tokens.input, self.input),
            output: per(tokens.output, self.output),
            write_5m: per(tokens.write_5m, self.write_5m),
            write_1h: per(tokens.write_1h, self.write_1h),
            read: per(tokens.read, self.read),
        }
    }
}

/// What one call cost, or None when it cannot be priced.
pub fn cost(model: &str, billed: &Billed) -> Option<Dollars> {
    price(model, &billed.speed).map(|p| p.dollars(&billed.tokens))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_id_seen_in_the_estate_corpus_prices() {
        // The model ids present in ~/.claude/projects on 2026-10-01.
        for id in [
            "claude-sonnet-5", "claude-opus-5", "claude-opus-4-8", "claude-opus-4-6",
            "claude-opus-4-7", "claude-fable-5", "claude-opus-5-5", "claude-fable-5-1",
            "claude-haiku-4-5-20251001", "claude-sonnet-4-6", "claude-sonnet-5-5",
            "claude-opus-4-5-20251101", "claude-sonnet-4-5-20250929",
        ] {
            assert!(price(id, &Speed::Standard).is_some(), "{id} unpriced");
        }
    }

    #[test]
    fn point_releases_do_not_inherit_their_predecessor() {
        assert_eq!(price("claude-opus-5-5", &Speed::Standard).unwrap().input, 4.0);
        assert_eq!(price("claude-opus-5", &Speed::Standard).unwrap().input, 5.0);
        assert_eq!(price("claude-fable-5-1", &Speed::Standard).unwrap().read, 0.25);
        assert_eq!(price("claude-fable-5", &Speed::Standard).unwrap().read, 1.0);
        assert!(price("claude-opus-5-6", &Speed::Standard).is_none());
        assert!(price("claude-opus-50", &Speed::Standard).is_none());
        assert!(price("<synthetic>", &Speed::Standard).is_none());
    }

    #[test]
    fn rows_are_internally_consistent_with_the_documented_multipliers() {
        for row in ROWS {
            let s = row.standard;
            assert!((s.write_5m - s.input * 1.25).abs() < 1e-9, "{} 5m write", row.id);
            assert!((s.write_1h - s.input * 2.0).abs() < 1e-9, "{} 1h write", row.id);
            let read_mult = match row.id {
                "claude-fable-5-1" | "claude-mythos-5-1" => 0.025,
                "claude-opus-5-5" => 0.05,
                _ => 0.1,
            };
            assert!((s.read - s.input * read_mult).abs() < 1e-9, "{} read", row.id);
            if let Some(f) = row.fast {
                assert!((f.input / s.input - f.output / s.output).abs() < 1e-9, "{} fast", row.id);
                assert!((f.read / f.input - s.read / s.input).abs() < 1e-9, "{} fast read", row.id);
            }
        }
    }

    #[test]
    fn fast_mode_prices_only_where_documented() {
        assert_eq!(price("claude-opus-5-5", &Speed::Fast).unwrap().output, 40.0);
        assert!(price("claude-sonnet-5-5", &Speed::Fast).is_none());
        assert!(price("claude-opus-5-5", &Speed::Other("turbo".into())).is_none());
    }

    #[test]
    fn dollars_per_category() {
        let tokens = Tokens { input: 1_000_000, output: 1_000_000, write_5m: 0, write_1h: 1_000_000, read: 1_000_000 };
        let d = price("claude-opus-5-5", &Speed::Standard).unwrap().dollars(&tokens);
        assert_eq!(d, Dollars { input: 4.0, output: 20.0, write_5m: 0.0, write_1h: 8.0, read: 0.2 });
        assert!((d.total() - 32.2).abs() < 1e-9);
    }
}
