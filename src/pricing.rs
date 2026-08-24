//! Static pricing table — per-MTok USD, Anthropic first-party API rates.
//!
//! Baked in so the tool works offline (no LiteLLM fetch like the TS
//! ccusage). Cache pricing follows the API's published multipliers over
//! base input: 5m cache writes 1.25x, 1h cache writes 2x, cache reads
//! 0.1x. Where a transcript carries the ephemeral_5m/1h breakdown we
//! price each tier exactly; a remainder without breakdown is priced at
//! the 5m rate (the default TTL), which is also what the TS ccusage
//! assumes for all writes.

pub struct ModelPrice {
    /// Substring matched against the model id, longest match wins.
    pub needle: &'static str,
    pub input_per_mtok: f64,
    pub output_per_mtok: f64,
}

/// Ordered table; first match after longest-needle sort wins.
/// Model ids in transcripts look like "claude-opus-5", "claude-sonnet-4-6",
/// or dated legacy ids like "claude-3-5-sonnet-20241022".
pub const PRICES: &[ModelPrice] = &[
    ModelPrice { needle: "fable-5", input_per_mtok: 10.0, output_per_mtok: 50.0 },
    ModelPrice { needle: "mythos-5", input_per_mtok: 10.0, output_per_mtok: 50.0 },
    ModelPrice { needle: "opus-5", input_per_mtok: 5.0, output_per_mtok: 25.0 },
    ModelPrice { needle: "opus-4-8", input_per_mtok: 5.0, output_per_mtok: 25.0 },
    ModelPrice { needle: "opus-4-7", input_per_mtok: 5.0, output_per_mtok: 25.0 },
    ModelPrice { needle: "opus-4-6", input_per_mtok: 5.0, output_per_mtok: 25.0 },
    ModelPrice { needle: "opus-4-5", input_per_mtok: 5.0, output_per_mtok: 25.0 },
    ModelPrice { needle: "opus-4-1", input_per_mtok: 15.0, output_per_mtok: 75.0 },
    ModelPrice { needle: "opus-4", input_per_mtok: 15.0, output_per_mtok: 75.0 },
    ModelPrice { needle: "3-opus", input_per_mtok: 15.0, output_per_mtok: 75.0 },
    ModelPrice { needle: "sonnet-5", input_per_mtok: 3.0, output_per_mtok: 15.0 },
    ModelPrice { needle: "sonnet-4-6", input_per_mtok: 3.0, output_per_mtok: 15.0 },
    ModelPrice { needle: "sonnet-4-5", input_per_mtok: 3.0, output_per_mtok: 15.0 },
    ModelPrice { needle: "sonnet-4", input_per_mtok: 3.0, output_per_mtok: 15.0 },
    ModelPrice { needle: "3-7-sonnet", input_per_mtok: 3.0, output_per_mtok: 15.0 },
    ModelPrice { needle: "3-5-sonnet", input_per_mtok: 3.0, output_per_mtok: 15.0 },
    ModelPrice { needle: "haiku-4-5", input_per_mtok: 1.0, output_per_mtok: 5.0 },
    ModelPrice { needle: "3-5-haiku", input_per_mtok: 0.8, output_per_mtok: 4.0 },
    ModelPrice { needle: "3-haiku", input_per_mtok: 0.25, output_per_mtok: 1.25 },
];

const CACHE_WRITE_5M_MULT: f64 = 1.25;
const CACHE_WRITE_1H_MULT: f64 = 2.0;
const CACHE_READ_MULT: f64 = 0.1;

pub fn price_for(model: &str) -> Option<&'static ModelPrice> {
    // Longest needle wins so "opus-4-8" beats "opus-4".
    PRICES
        .iter()
        .filter(|p| model.contains(p.needle))
        .max_by_key(|p| p.needle.len())
}

/// Cost in USD for one usage record. Returns None for unknown models
/// (surfaced to the user rather than silently priced at zero).
#[allow(clippy::too_many_arguments)]
pub fn cost_usd(
    model: &str,
    input: u64,
    output: u64,
    cache_write: u64,
    cache_write_5m: u64,
    cache_write_1h: u64,
    cache_read: u64,
) -> Option<f64> {
    let p = price_for(model)?;
    let mtok = 1_000_000.0;
    // Prefer the explicit 5m/1h breakdown; any un-broken-down remainder
    // is priced at the 5m rate.
    let tiered = cache_write_5m + cache_write_1h;
    let remainder = cache_write.saturating_sub(tiered);
    let write_cost = (cache_write_5m as f64) * p.input_per_mtok * CACHE_WRITE_5M_MULT / mtok
        + (cache_write_1h as f64) * p.input_per_mtok * CACHE_WRITE_1H_MULT / mtok
        + (remainder as f64) * p.input_per_mtok * CACHE_WRITE_5M_MULT / mtok;
    Some(
        (input as f64) * p.input_per_mtok / mtok
            + (output as f64) * p.output_per_mtok / mtok
            + write_cost
            + (cache_read as f64) * p.input_per_mtok * CACHE_READ_MULT / mtok,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn longest_needle_wins() {
        assert_eq!(price_for("claude-opus-4-8").unwrap().input_per_mtok, 5.0);
        assert_eq!(price_for("claude-opus-4-1-20250805").unwrap().input_per_mtok, 15.0);
        assert_eq!(price_for("claude-fable-5").unwrap().output_per_mtok, 50.0);
        assert!(price_for("<synthetic>").is_none());
    }

    #[test]
    fn cache_tiers_priced_distinctly() {
        // 1M tokens of one class at a time on opus-5 ($5 in / $25 out).
        let c = cost_usd("claude-opus-5", 0, 0, 1_000_000, 0, 1_000_000, 0).unwrap();
        // 1h write = 2x input rate = $10
        assert!((c - 10.0).abs() < 1e-9, "{c}");
        let c5 = cost_usd("claude-opus-5", 0, 0, 1_000_000, 1_000_000, 0, 0).unwrap();
        assert!((c5 - 6.25).abs() < 1e-9, "{c5}");
        let cr = cost_usd("claude-opus-5", 0, 0, 0, 0, 0, 1_000_000).unwrap();
        assert!((cr - 0.5).abs() < 1e-9, "{cr}");
    }

    #[test]
    fn breakdown_remainder_at_5m_rate() {
        // total write 2M, only 1M attributed to 1h -> remainder 1M at 1.25x.
        let c = cost_usd("claude-opus-5", 0, 0, 2_000_000, 0, 1_000_000, 0).unwrap();
        assert!((c - (10.0 + 6.25)).abs() < 1e-9, "{c}");
    }
}
