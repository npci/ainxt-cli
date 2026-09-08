//! Published per-token pricing, used only to fill the gaps the gateway leaves.
//!
//! # The gateway is authoritative; this table is the fallback
//!
//! The gateway prices every call it can and reports the result on the wire (the
//! `ainxt_cost_in_usd_ticks` / `ainxt_cost_priced` extension on Anthropic
//! `message_delta.usage`, and `cost_in_usd_ticks` on the OpenAI-compatible
//! backends). When a cost arrives, the CLI stores it verbatim and never adjusts
//! it — that figure is what reconciles with the platform's billing.
//!
//! Not every deployment stamps a cost, though: older gateways, pool/OAuth
//! paths, and stock upstream servers all report usage without a price. The CLI
//! is the only place a user sees a running total, so a silent "unavailable"
//! there leaves them with no budget signal at all. For those calls we price the
//! tokens from the models' published public rates and label the result an
//! estimate.
//!
//! The two figures are kept in separate fields all the way to the screen (see
//! [`crate::usage::UsageTotals`]): an exact gateway sum that can be reconciled
//! with an invoice, and an estimated remainder that is always rendered with a
//! `~` so it is never mistaken for one.
//!
//! Rates are dollars per million tokens. In-house / self-hosted models carry no
//! per-token billing and settle at zero. An unrecognized model falls back to a
//! mid-range rate, which keeps budget awareness meaningful; the total is flagged
//! as an estimate either way.

/// Ticks per USD used throughout the ledger (`cost = USD * 1e10`).
///
/// Integer ticks, not floats: a session ledger sums thousands of per-call
/// values, and float addition would drift from the platform's own total.
pub const COST_TICKS_PER_USD: f64 = 1e10;

/// Dollars-per-million-tokens for one model family.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TokenRate {
    /// Fresh (uncached) input tokens.
    pub input_per_m: f64,
    /// Generated output tokens.
    pub output_per_m: f64,
    /// Cached-read input tokens (served from prompt cache).
    pub cache_read_per_m: f64,
    /// Cache-write (`cache_creation`) input tokens. Providers that support
    /// prompt caching charge a premium to populate it; those that don't bill
    /// these at the plain input rate, which is what a `None` here means.
    pub cache_write_per_m: Option<f64>,
}

impl TokenRate {
    const fn new(input_per_m: f64, output_per_m: f64, cache_read_per_m: f64) -> Self {
        Self {
            input_per_m,
            output_per_m,
            cache_read_per_m,
            cache_write_per_m: None,
        }
    }

    /// Anthropic-style rate: 5-minute cache writes bill at 1.25x input.
    const fn with_cache_write(mut self, cache_write_per_m: f64) -> Self {
        self.cache_write_per_m = Some(cache_write_per_m);
        self
    }

    /// In-house / self-hosted models: no per-token billing.
    pub const FREE: TokenRate = TokenRate::new(0.0, 0.0, 0.0);

    /// Effective cache-write rate, falling back to the input rate.
    fn cache_write_rate(&self) -> f64 {
        self.cache_write_per_m.unwrap_or(self.input_per_m)
    }
}

// Published public rates (USD per million tokens). Grouped by pricing tier so
// several model ids can share one entry. Update these when providers change
// their public pricing.
const SONNET: TokenRate = TokenRate::new(3.0, 15.0, 0.3).with_cache_write(3.75);
const OPUS_HEAVY: TokenRate = TokenRate::new(15.0, 75.0, 1.5).with_cache_write(18.75);
const OPUS_LIGHT: TokenRate = TokenRate::new(5.0, 25.0, 0.5).with_cache_write(6.25);
const HAIKU: TokenRate = TokenRate::new(1.0, 5.0, 0.1).with_cache_write(1.25);
const GPT_FLAGSHIP: TokenRate = TokenRate::new(2.5, 10.0, 1.25);
const GPT_MINI: TokenRate = TokenRate::new(0.15, 0.6, 0.075);
const GEMINI_FLASH: TokenRate = TokenRate::new(0.15, 0.6, 0.0375);
const GEMINI_PRO: TokenRate = TokenRate::new(1.25, 10.0, 0.31);

/// Estimate applied when a model id matches no known family. A mid-range rate
/// (rather than zero) keeps budget awareness meaningful; every total built from
/// this table is flagged an estimate regardless.
const FALLBACK: TokenRate = OPUS_LIGHT;

/// True when a model id is an in-house / self-hosted model (no per-token cost).
///
/// The gateway prefixes its self-hosted models with `local:`; we also recognize
/// the common in-house family names in case a deployment lists them unprefixed.
pub fn is_in_house(model: &str) -> bool {
    let m = model.trim().to_ascii_lowercase();
    const PREFIXES: [&str; 9] = [
        "local:", "local-", "local/", "ollama:", "ollama/", "qwen", "glm", "kimi", "gemma",
    ];
    if PREFIXES.iter().any(|p| m.starts_with(p)) {
        return true;
    }
    // Other self-hosted families that may appear unprefixed.
    for fam in ["llama", "mistral", "deepseek", "phi"] {
        if m.starts_with(fam) {
            return true;
        }
    }
    m.ends_with("-inhouse") || m.ends_with("-in-house") || m.ends_with("-internal")
}

/// Resolve the published rate for a model id.
///
/// Returns `(rate, recognized)`; `recognized == false` means the id matched no
/// known family and [`FALLBACK`] was used, so the figure is especially rough.
pub fn rate_for(model: &str) -> (TokenRate, bool) {
    if is_in_house(model) {
        return (TokenRate::FREE, true);
    }
    let m = model.to_ascii_lowercase();
    let contains = |needle: &str| m.contains(needle);

    // Anthropic Claude families.
    if contains("opus") {
        // Newer Opus tiers price lower than the original heavy tier.
        let light = contains("4-5")
            || contains("4.5")
            || contains("4-6")
            || contains("4.6")
            || contains("4-7")
            || contains("4.7")
            || contains("4-8")
            || contains("4.8")
            || contains("5");
        return (if light { OPUS_LIGHT } else { OPUS_HEAVY }, true);
    }
    if contains("sonnet") {
        return (SONNET, true);
    }
    if contains("haiku") {
        return (HAIKU, true);
    }
    // OpenAI GPT families.
    if contains("gpt") || m.starts_with("o1") || m.starts_with("o3") || m.starts_with("o4") {
        return (
            if contains("mini") || contains("nano") {
                GPT_MINI
            } else {
                GPT_FLAGSHIP
            },
            true,
        );
    }
    // Google Gemini families.
    if contains("gemini") {
        return (
            if contains("pro") {
                GEMINI_PRO
            } else {
                GEMINI_FLASH
            },
            true,
        );
    }
    (FALLBACK, false)
}

/// Estimated USD cost of one call under a model's published rate.
///
/// `input_tokens` is the FULL prompt size (cache reads and writes included),
/// `cached_read_tokens` the cache-hit subset and `cache_write_tokens` the
/// newly-cached subset of it, and `output_tokens` the generated tokens. Each
/// bucket is billed at its own rate and the remainder at the plain input rate,
/// so the buckets partition the prompt instead of double-counting it.
pub fn call_cost_usd(
    model: &str,
    input_tokens: u64,
    output_tokens: u64,
    cached_read_tokens: u64,
    cache_write_tokens: u64,
) -> f64 {
    let (rate, _recognized) = rate_for(model);
    // Clamp the buckets to the prompt: a backend reporting inconsistent counts
    // must not produce a negative fresh remainder (and thus a bogus credit).
    let cached = cached_read_tokens.min(input_tokens);
    let written = cache_write_tokens.min(input_tokens - cached);
    let fresh = input_tokens.saturating_sub(cached).saturating_sub(written);
    let per_m = |tokens: u64, rate: f64| (tokens as f64 / 1_000_000.0) * rate;
    per_m(fresh, rate.input_per_m)
        + per_m(output_tokens, rate.output_per_m)
        + per_m(cached, rate.cache_read_per_m)
        + per_m(written, rate.cache_write_rate())
}

/// Estimated cost of one call in USD ticks (`USD * 1e10`), rounded to the
/// nearest tick. Used only for calls the gateway did not price.
pub fn call_cost_ticks(
    model: &str,
    input_tokens: u64,
    output_tokens: u64,
    cached_read_tokens: u64,
    cache_write_tokens: u64,
) -> i64 {
    let usd = call_cost_usd(
        model,
        input_tokens,
        output_tokens,
        cached_read_tokens,
        cache_write_tokens,
    );
    (usd * COST_TICKS_PER_USD).round() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_house_models_are_recognized_and_free() {
        for id in [
            "local:gemma-4-31B-it",
            "local:qwen-3.6-35B-A3B",
            "local:kimi-k2.7-code",
            "local:glm-5.1-fp8",
            "ollama:llama3.1",
            "kimi-k2.7-code",
        ] {
            assert!(is_in_house(id), "{id} should be in-house");
            assert_eq!(call_cost_ticks(id, 1_000_000, 1_000_000, 0, 0), 0);
        }
    }

    #[test]
    fn cloud_models_are_not_in_house() {
        for id in [
            "claude-sonnet-4-6",
            "claude-opus-4-7",
            "gpt-5.5",
            "gemini-2.5-flash",
        ] {
            assert!(!is_in_house(id), "{id} should not be in-house");
        }
    }

    #[test]
    fn cloud_families_resolve_to_known_rates() {
        for id in [
            "claude-sonnet-4-6",
            "claude-opus-4-7",
            "claude-haiku-4-5-20251001",
            "gpt-5.5",
            "gpt-5-mini",
            "gemini-3.5-flash",
        ] {
            let (_rate, recognized) = rate_for(id);
            assert!(recognized, "{id} should match a known family");
        }
    }

    #[test]
    fn unknown_model_uses_fallback_estimate() {
        let (rate, recognized) = rate_for("some-unknown-model-x");
        assert!(!recognized);
        assert_eq!(rate, FALLBACK);
    }

    #[test]
    fn cost_splits_prompt_into_fresh_cached_and_written() {
        // Sonnet: input 3/M, output 15/M, cache-read 0.3/M, cache-write 3.75/M.
        // 1M fresh input + 1M output = $3 + $15 = $18.
        let usd = call_cost_usd("claude-sonnet-4-6", 1_000_000, 1_000_000, 0, 0);
        assert!((usd - 18.0).abs() < 1e-9, "got {usd}");
        // Whole prompt served from cache: $0.30 + $15 = $15.30.
        let cached = call_cost_usd("claude-sonnet-4-6", 1_000_000, 1_000_000, 1_000_000, 0);
        assert!((cached - 15.30).abs() < 1e-9, "got {cached}");
        // Whole prompt written to cache at the 1.25x premium: $3.75 + $15.
        let written = call_cost_usd("claude-sonnet-4-6", 1_000_000, 1_000_000, 0, 1_000_000);
        assert!((written - 18.75).abs() < 1e-9, "got {written}");
    }

    #[test]
    fn inconsistent_buckets_never_credit_the_user() {
        // Buckets summing past the prompt must clamp, not go negative.
        let usd = call_cost_usd("claude-sonnet-4-6", 100, 0, 90, 50);
        assert!(usd > 0.0, "got {usd}");
    }

    #[test]
    fn cache_write_defaults_to_input_rate_when_unpriced() {
        // GPT rates carry no separate cache-write price, so writes bill as
        // plain input rather than silently costing nothing.
        let (rate, _) = rate_for("gpt-5.5");
        assert_eq!(rate.cache_write_per_m, None);
        let usd = call_cost_usd("gpt-5.5", 1_000_000, 0, 0, 1_000_000);
        assert!((usd - 2.5).abs() < 1e-9, "got {usd}");
    }

    #[test]
    fn opus_light_vs_heavy() {
        let (light, _) = rate_for("claude-opus-4-7");
        let (heavy, _) = rate_for("claude-opus-4");
        assert_eq!(light, OPUS_LIGHT);
        assert_eq!(heavy, OPUS_HEAVY);
    }
}
