//! Per-prompt and per-session billing ledgers (not serialized).
//!
//! `total_tokens()` is input + output: Responses wire `total` is live context
//! length. Compaction and other side calls never call `record_main_loop_call`.
//!
//! # Completeness ownership
//!
//! Wire incomplete is the OR of these stores (each has a distinct role):
//!
//! - **`UsageLedger.incomplete`** — durable on the bill snapshot. Set by nested
//!   subagent incomplete fold, drain timeout, true apply-miss, and
//!   `mark_usage_incomplete`. Monotonic for a ledger instance.
//! - **Sticky (`subagent_usage_not_applied` on the coordinator)** — pin-scoped
//!   **report** signal (session-only attribution or apply-miss report). Not a
//!   second token sink; does not stain ledgers by itself.
//! - **Foreground live IDs** — fold may still land; freeze drains ≤120s or fails
//!   closed. Cancel skips multi-second drain (actor-loop safety).
//! - **Background live** — never waits; prompt report incomplete immediately;
//!   spend still folds into the session ledger at completion (no session-ledger
//!   incomplete).
//!
//! Freeze and cancel share one outcome policy: ledger marks only on fail-closed;
//! sticky and background_live are report-level only.
//!
//! Projection (`PromptUsage`) never invents tokens; it only ORs completeness
//! and scrubs costs when partial or incomplete.
//!
//! # Cost ownership
//!
//! The gateway is the **authoritative** source of cost, and its figures are
//! stored verbatim in `cost_usd_ticks` — that sum is the one that reconciles
//! with the platform's billing.
//!
//! Calls the gateway does not price (older gateways, pool/OAuth paths, stock
//! upstream servers) are priced locally from published rates into a **separate**
//! field, `estimated_cost_usd_ticks`, and counted in `cost_missing_calls`. The
//! two never mix in one number: a consumer can report the exact sum alone, or
//! the combined figure marked as an estimate, but it can never mistake one for
//! the other. See [`Self::displayable_cost`] for the combining rule and
//! [`crate::pricing`] for the rate table.

use indexmap::IndexMap;
use ainxt_sampling_types::TokenUsage;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UsageTotals {
    /// FULL prompt size: fresh + cache reads + cache writes. Display surfaces
    /// that want the fresh-only figure must subtract — see
    /// [`Self::fresh_input_tokens`].
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_read_tokens: u64,
    /// Prompt tokens written to the cache. A subset of `input_tokens`, not an
    /// addition to it.
    pub cache_write_tokens: u64,
    pub reasoning_tokens: u64,
    pub model_calls: u64,
    pub api_duration_ms: u64,
    /// Gateway-reported cost in USD ticks (1e10 per USD). `None` when no call
    /// in this row was priced by the gateway. Never locally estimated, so
    /// `Some(0)` is a real "free" (in-house models), not a missing value.
    pub cost_usd_ticks: Option<i64>,
    /// Locally estimated cost, in the same ticks, for exactly the calls the
    /// gateway did not price. Deliberately a second field rather than folded
    /// into `cost_usd_ticks`: the exact and estimated portions must stay
    /// separable so a display can label the total honestly, and so billing
    /// reconciliation can still read the gateway sum on its own.
    pub estimated_cost_usd_ticks: Option<i64>,
    /// Calls that reported usage but no gateway cost. Drives
    /// [`Self::cost_is_partial`] and counts what `estimated_cost_usd_ticks`
    /// covers.
    pub cost_missing_calls: u64,
}

/// A cost figure ready for display, with the provenance the UI must convey.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayCost {
    /// Total in USD ticks: the gateway sum plus any local estimate.
    pub ticks: i64,
    /// True when part of `ticks` was priced locally rather than by the gateway.
    /// Display surfaces must mark such a figure (conventionally a `~` prefix)
    /// so it is never read as a billable amount.
    pub estimated: bool,
}

impl UsageTotals {
    /// Fold one model call into a totals row.
    ///
    /// `model_id` selects the local rate used when — and only when — the
    /// gateway reported no cost for this call.
    fn from_call(
        model_id: &str,
        usage: &TokenUsage,
        api_duration_ms: Option<u64>,
        cost_usd_ticks: Option<i64>,
    ) -> Self {
        let input_tokens = u64::from(usage.prompt_tokens);
        let output_tokens = u64::from(usage.completion_tokens);
        let cached_read_tokens = u64::from(usage.cached_prompt_tokens);
        let cache_write_tokens = u64::from(usage.cache_write_tokens);
        // A gateway cost always wins and is stored untouched: it is the only
        // figure that reconciles with the platform's billing, and `Some(0)` is
        // a real "free" (in-house models), not a missing value.
        //
        // When the gateway reports nothing, price the tokens from the model's
        // published rate — but into a *separate* field. The CLI is the only
        // place a user sees a running total, so dropping these calls would
        // leave no budget signal at all; keeping the estimate apart means the
        // display can show a number without ever claiming it is billable.
        let estimated_cost_usd_ticks = cost_usd_ticks.is_none().then(|| {
            crate::pricing::call_cost_ticks(
                model_id,
                input_tokens,
                output_tokens,
                cached_read_tokens,
                cache_write_tokens,
            )
        });
        Self {
            input_tokens,
            output_tokens,
            cached_read_tokens,
            cache_write_tokens,
            reasoning_tokens: u64::from(usage.reasoning_tokens),
            model_calls: 1,
            api_duration_ms: api_duration_ms.unwrap_or(0),
            cost_usd_ticks,
            estimated_cost_usd_ticks,
            // Counts calls the *gateway* didn't price, so a partial gateway
            // total is still flagged even though we filled an estimate.
            cost_missing_calls: u64::from(cost_usd_ticks.is_none()),
        }
    }

    pub fn total_tokens(&self) -> u64 {
        self.input_tokens.saturating_add(self.output_tokens)
    }

    /// Prompt tokens the model actually had to read fresh: the full prompt
    /// minus the cached and written portions.
    ///
    /// `input_tokens` is the whole prompt, so showing it beside a cache-read
    /// column reads as if the cached tokens were charged twice. Saturating
    /// subtraction keeps this at zero if a backend ever reports buckets that
    /// exceed the total.
    pub fn fresh_input_tokens(&self) -> u64 {
        self.input_tokens
            .saturating_sub(self.cached_read_tokens)
            .saturating_sub(self.cache_write_tokens)
    }

    pub fn cost_is_partial(&self) -> bool {
        self.cost_usd_ticks.is_some() && self.cost_missing_calls > 0
    }

    /// The figure to show a user, combining the exact gateway sum with the
    /// local estimate for whatever the gateway left unpriced.
    ///
    /// `estimated` is true whenever any part of the total was priced locally,
    /// which is the signal a display must surface. Returns `None` only when
    /// there is nothing at all to show — no call has landed yet.
    pub fn displayable_cost(&self) -> Option<DisplayCost> {
        match (self.cost_usd_ticks, self.estimated_cost_usd_ticks) {
            (None, None) => None,
            (exact, estimate) => Some(DisplayCost {
                ticks: exact.unwrap_or(0).saturating_add(estimate.unwrap_or(0)),
                estimated: estimate.is_some(),
            }),
        }
    }

    fn fold_totals(&mut self, other: &UsageTotals) {
        let Self {
            input_tokens,
            output_tokens,
            cached_read_tokens,
            cache_write_tokens,
            reasoning_tokens,
            model_calls,
            api_duration_ms,
            cost_usd_ticks,
            estimated_cost_usd_ticks,
            cost_missing_calls,
        } = other;
        self.input_tokens = self.input_tokens.saturating_add(*input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(*output_tokens);
        self.cached_read_tokens = self.cached_read_tokens.saturating_add(*cached_read_tokens);
        self.cache_write_tokens = self.cache_write_tokens.saturating_add(*cache_write_tokens);
        self.reasoning_tokens = self.reasoning_tokens.saturating_add(*reasoning_tokens);
        self.model_calls = self.model_calls.saturating_add(*model_calls);
        self.api_duration_ms = self.api_duration_ms.saturating_add(*api_duration_ms);
        self.cost_missing_calls = self.cost_missing_calls.saturating_add(*cost_missing_calls);
        self.cost_usd_ticks = merge_cost_ticks(self.cost_usd_ticks, *cost_usd_ticks);
        self.estimated_cost_usd_ticks = merge_cost_ticks(
            self.estimated_cost_usd_ticks,
            *estimated_cost_usd_ticks,
        );
    }
}

fn merge_cost_ticks(a: Option<i64>, b: Option<i64>) -> Option<i64> {
    match (a, b) {
        (None, None) => None,
        (a, b) => Some(a.unwrap_or(0).saturating_add(b.unwrap_or(0))),
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UsageLedger {
    pub totals: UsageTotals,
    pub by_model: IndexMap<String, UsageTotals>,
    /// Main-agent loop rounds for `num_turns` (subagents excluded).
    pub main_loop_model_calls: u64,
    /// Bill may under-count (drain timeout, nested subagent incomplete, apply failure).
    pub incomplete: bool,
}

impl UsageLedger {
    /// Fold one main-agent-loop model call. This is the only writer of
    /// `main_loop_model_calls` (the wire `numTurns`); side calls such as
    /// compaction must not use it.
    pub fn record_main_loop_call(
        &mut self,
        model_id: &str,
        usage: &TokenUsage,
        api_duration_ms: Option<u64>,
        cost_usd_ticks: Option<i64>,
    ) {
        let call = UsageTotals::from_call(model_id, usage, api_duration_ms, cost_usd_ticks);
        self.main_loop_model_calls = self.main_loop_model_calls.saturating_add(1);
        self.fold_entry(model_id, &call);
    }

    /// Fold subagent usage without incrementing `main_loop_model_calls`.
    pub fn record_subagent(&mut self, by_model: &[(String, UsageTotals)], incomplete: bool) {
        for (model_id, totals) in by_model {
            self.fold_entry(model_id, totals);
        }
        if incomplete {
            self.incomplete = true;
        }
    }

    pub fn mark_incomplete(&mut self) {
        self.incomplete = true;
    }

    fn fold_entry(&mut self, model_id: &str, totals: &UsageTotals) {
        self.totals.fold_totals(totals);
        self.by_model
            .entry(model_id.to_owned())
            .or_default()
            .fold_totals(totals);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tu(prompt: u32, completion: u32) -> TokenUsage {
        TokenUsage {
            prompt_tokens: prompt,
            completion_tokens: completion,
            total_tokens: 999_999,
            reasoning_tokens: 0,
            cached_prompt_tokens: 0,
            ..Default::default()
        }
    }

    #[test]
    fn fresh_input_excludes_cache_buckets() {
        // A cache-heavy Anthropic turn: 10k prompt of which 7k was read from
        // cache and 2k written to it, leaving 1k genuinely fresh.
        let usage = TokenUsage {
            prompt_tokens: 10_000,
            completion_tokens: 100,
            total_tokens: 10_100,
            reasoning_tokens: 0,
            cached_prompt_tokens: 7_000,
            cache_write_tokens: 2_000,
        };
        let mut ledger = UsageLedger::default();
        ledger.record_main_loop_call("claude-sonnet-4-6", &usage, None, Some(500));

        let t = &ledger.totals;
        assert_eq!(t.input_tokens, 10_000, "full prompt is preserved");
        assert_eq!(t.cached_read_tokens, 7_000);
        assert_eq!(t.cache_write_tokens, 2_000);
        // The displayed columns must partition the prompt, not overlap it.
        assert_eq!(t.fresh_input_tokens(), 1_000);
        assert_eq!(
            t.fresh_input_tokens() + t.cached_read_tokens + t.cache_write_tokens,
            t.input_tokens,
            "fresh + cache-read + cache-write must equal the full prompt"
        );
    }

    #[test]
    fn fresh_input_saturates_when_buckets_exceed_prompt() {
        // Defensive: a backend reporting inconsistent buckets must not
        // underflow into a huge number.
        let usage = TokenUsage {
            prompt_tokens: 100,
            completion_tokens: 1,
            total_tokens: 101,
            reasoning_tokens: 0,
            cached_prompt_tokens: 90,
            cache_write_tokens: 50,
        };
        let mut ledger = UsageLedger::default();
        ledger.record_main_loop_call("m", &usage, None, Some(1));
        assert_eq!(ledger.totals.fresh_input_tokens(), 0);
    }

    #[test]
    fn cache_write_folds_across_calls_and_models() {
        let mut ledger = UsageLedger::default();
        let mk = |p, cr, cw| TokenUsage {
            prompt_tokens: p,
            completion_tokens: 10,
            total_tokens: p + 10,
            reasoning_tokens: 0,
            cached_prompt_tokens: cr,
            cache_write_tokens: cw,
        };
        ledger.record_main_loop_call("a", &mk(1_000, 400, 100), None, Some(10));
        ledger.record_main_loop_call("a", &mk(2_000, 900, 300), None, Some(20));
        ledger.record_main_loop_call("b", &mk(500, 0, 0), None, Some(5));

        assert_eq!(ledger.totals.cache_write_tokens, 400);
        assert_eq!(ledger.by_model["a"].cache_write_tokens, 400);
        assert_eq!(ledger.by_model["a"].fresh_input_tokens(), 3_000 - 1_300 - 400);
        assert_eq!(ledger.by_model["b"].cache_write_tokens, 0);
        assert_eq!(ledger.by_model["b"].fresh_input_tokens(), 500);
    }

    #[test]
    fn unpriced_call_is_estimated_separately() {
        let mut ledger = UsageLedger::default();
        // The gateway reported no cost, so the CLI prices the call locally.
        // The estimate lands in its own field and never in the exact sum.
        ledger.record_main_loop_call("claude-sonnet-4-6", &tu(1_000_000, 0), None, None);
        assert_eq!(ledger.totals.cost_usd_ticks, None, "no gateway figure");
        // Sonnet input is $3/M, so 1M fresh input estimates at $3.
        assert_eq!(
            ledger.totals.estimated_cost_usd_ticks,
            Some(30_000_000_000)
        );
        assert_eq!(ledger.totals.cost_missing_calls, 1);
        // A wholly estimated total is still displayable, but marked estimated.
        let shown = ledger.totals.displayable_cost().expect("estimate shows");
        assert_eq!(shown.ticks, 30_000_000_000);
        assert!(shown.estimated);
        // No gateway figure exists, so there is no partial *gateway* sum.
        assert!(!ledger.totals.cost_is_partial());
    }

    #[test]
    fn unpriced_in_house_call_estimates_to_free() {
        let mut ledger = UsageLedger::default();
        // In-house models carry no per-token billing, so the local estimate is
        // a confident zero rather than an unknown.
        ledger.record_main_loop_call("local:gemma", &tu(10_000, 5_000), None, None);
        assert_eq!(ledger.totals.estimated_cost_usd_ticks, Some(0));
        let shown = ledger.totals.displayable_cost().expect("free shows");
        assert_eq!(shown.ticks, 0);
    }

    #[test]
    fn gateway_priced_zero_is_free_and_not_estimated() {
        let mut ledger = UsageLedger::default();
        // In-house model priced by the gateway at a real zero: trusted, not
        // counted as a missing price, and never overwritten by an estimate.
        ledger.record_main_loop_call("local:gemma", &tu(10, 5), None, Some(0));
        assert_eq!(ledger.totals.cost_usd_ticks, Some(0));
        assert_eq!(ledger.totals.estimated_cost_usd_ticks, None);
        assert_eq!(ledger.totals.cost_missing_calls, 0);
        assert!(!ledger.totals.cost_is_partial());
        let shown = ledger.totals.displayable_cost().expect("free shows");
        assert!(!shown.estimated, "a gateway zero is exact");
    }

    #[test]
    fn gateway_costs_sum_exactly() {
        let mut ledger = UsageLedger::default();
        ledger.record_main_loop_call("a", &tu(100, 10), Some(100), Some(30));
        ledger.record_main_loop_call("a", &tu(50, 5), Some(50), Some(70));
        // Sum is exact: every tick came from the gateway, none estimated.
        assert_eq!(ledger.totals.cost_usd_ticks, Some(100));
        assert_eq!(ledger.totals.estimated_cost_usd_ticks, None);
        assert!(!ledger.totals.cost_is_partial());
        let shown = ledger.totals.displayable_cost().expect("exact shows");
        assert_eq!(shown.ticks, 100);
        assert!(!shown.estimated);
    }

    #[test]
    fn mixed_priced_and_unpriced_keeps_the_two_sums_apart() {
        let mut ledger = UsageLedger::default();
        // One gateway-priced call and one the gateway skipped, on a model with
        // a known published rate.
        ledger.record_main_loop_call("claude-sonnet-4-6", &tu(10, 5), None, Some(70));
        ledger.record_main_loop_call("claude-sonnet-4-6", &tu(1_000_000, 0), None, None);
        // The exact sum still reports only what the gateway actually priced,
        // so billing reconciliation is unaffected by the estimate beside it.
        assert_eq!(ledger.totals.cost_usd_ticks, Some(70));
        assert_eq!(
            ledger.totals.estimated_cost_usd_ticks,
            Some(30_000_000_000)
        );
        assert!(ledger.totals.cost_is_partial());
        // The display figure combines them and is flagged as an estimate.
        let shown = ledger.totals.displayable_cost().expect("combined shows");
        assert_eq!(shown.ticks, 30_000_000_070);
        assert!(shown.estimated);
    }

    #[test]
    fn ledger_sums_partial_subagent_and_zero_cost() {
        let mut ledger = UsageLedger::default();
        ledger.record_main_loop_call("local:gemma", &tu(1, 1), None, None);

        ledger.record_main_loop_call("a", &tu(100, 10), Some(100), None);
        ledger.record_main_loop_call("a", &tu(50, 5), Some(50), Some(70));
        // A gateway-reported cost is present but some calls were unpriced, so
        // the exact total covers only those and the row is flagged partial.
        assert_eq!(ledger.totals.cost_usd_ticks, Some(70));
        assert!(ledger.totals.cost_is_partial());
        assert_eq!(ledger.main_loop_model_calls, 3);

        ledger.record_subagent(
            &[(
                "b".into(),
                UsageTotals {
                    input_tokens: 5,
                    model_calls: 1,
                    ..Default::default()
                },
            )],
            false,
        );
        assert_eq!(ledger.by_model["b"].input_tokens, 5);
        assert_eq!(ledger.main_loop_model_calls, 3);
        assert_eq!(ledger.totals.model_calls, 4);
        assert!(!ledger.incomplete);

        ledger.record_subagent(&[], true);
        assert!(ledger.incomplete);
    }
}
