//! Subscription tier checks, credit-limit upsells, and auto-topup handling.

use super::queue::maybe_drain_queue;
use crate::app::actions::Effect;
use crate::app::agent::AgentId;
use crate::app::agent_view::AgentView;
use crate::app::app_view::AppView;
use crate::scrollback::block::RenderBlock;
use std::time::Duration;
use ainxt_telemetry::events::{SuperAinxtUpsell, SuperAinxtUpsellClicked};
use ainxt_telemetry::session_ctx::log_event;

/// How long the pager auto-checks subscription status before stopping.
/// After this, the user can still manually check via the [Refresh] button.
pub(super) const PAYWALL_AUTO_CHECK_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// Whether the user is at the highest subscription tier (SuperAinxt Heavy).
///
/// Returns `true` only when `subscription_tier` **positively matches** a
/// known max-tier identifier. When the tier is unknown (`None`) or any
/// other value, returns `false` — the user gets the Q&A modal so lower-
/// tier users always see the upgrade option.
pub(super) fn is_max_tier(subscription_tier: Option<&str>) -> bool {
    let Some(t) = subscription_tier else {
        return false; // Unknown — default to Q&A.
    };
    // Normalize: lowercase + spaces→underscores to match both JWT-derived
    // keys ("superainxt_heavy") and CCP display names ("SuperAinxt Heavy").
    t.to_ascii_lowercase().replace(' ', "_") == "superainxt_heavy"
}

/// URL for upgrading the subscription tier.
/// Resolved at runtime — respects `AINXT_URL_SUBSCRIBE` env override.
pub(crate) fn upsell_url_upgrade() -> std::borrow::Cow<'static, str> {
    ainxt_env::url_subscribe()
}

/// URL for managing pay-as-you-go / on-demand spending / purchasing credits.
/// Resolved at runtime — respects `AINXT_URL_USAGE` env override.
pub(crate) fn upsell_url_payg() -> std::borrow::Cow<'static, str> {
    ainxt_env::url_usage()
}

// Keep the constants for callers that need a `&str` in const context (e.g. tests).
pub(crate) const UPSELL_URL_UPGRADE: &str = ainxt_env::URL_SUBSCRIBE;
pub(crate) const UPSELL_URL_PAYG: &str = ainxt_env::URL_USAGE;

/// Billing mode for credit-limit upsell copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CreditLimitUpsellMode {
    /// Unified usage pool — suggest purchasing prepaid credits.
    UnifiedCredits,
    /// Legacy on-demand / PAYG (`enabled` = on-demand cap already active).
    LegacyPayg { enabled: bool },
}

/// Resolve upsell copy mode from credits config.
///
/// Prefers explicit `is_unified_billing_user` (`Option` — do not treat a
/// missing field as legacy). Positive `pay_as_you_go` (on-demand cap &gt; 0)
/// only selects legacy when the unified flag is absent. Unknown defaults to
/// unified (buy credits) so pool users never get “enable on-demand” wrongly.
pub(super) fn credit_limit_upsell_mode(
    balance: Option<&crate::views::credit_bar::CreditBalance>,
) -> CreditLimitUpsellMode {
    match balance {
        Some(b) if b.is_unified_billing_user == Some(true) => CreditLimitUpsellMode::UnifiedCredits,
        Some(b) if b.is_unified_billing_user == Some(false) => CreditLimitUpsellMode::LegacyPayg {
            enabled: b.pay_as_you_go,
        },
        // Flag absent: only treat as legacy PAYG when we have a positive
        // on-demand cap (pay_as_you_go is derived from cap &gt; 0).
        Some(b) if b.pay_as_you_go => CreditLimitUpsellMode::LegacyPayg { enabled: true },
        _ => CreditLimitUpsellMode::UnifiedCredits,
    }
}

/// Whether an API / retry error is a credit-limit / spend-block denial.
///
/// - **402** Payment Required — always credit/spend block on this surface
///   (Build pool and IC spend blocks); no message filter.
/// - **403** — only when the body contains "run out of credits" (legacy IC
///   spend wording); other 403s (content-safety, ZDR, …) are excluded.
pub(crate) fn is_credit_limit_error(http_status: Option<u16>, message: &str) -> bool {
    let m = message.to_ascii_lowercase();
    let legacy = m.contains("run out of credits");
    match http_status {
        Some(402) => true,
        Some(403) if legacy => true,
        // Retry notifications embed "status 402" / "status 403" in the body
        // without a separate status field.
        None | Some(_) => m.contains("status 402") || (m.contains("status 403") && legacy),
    }
}

/// Well-known error code CCP returns (HTTP 429, flat body
/// `{"code": "...", "error": "..."}`) when a free-tier user exhausts the
/// free usage quota. Kept in sync with the shared well-known error code
/// `SUBSCRIPTION_FREE_USAGE_EXHAUSTED`. sampling-types' `parse_error_bytes` prepends the flat
/// `code` to the flattened message, so the code reaches the pager embedded
/// in `RetryState::Exhausted.reason` and the -32003 error's data string.
pub(crate) const FREE_USAGE_EXHAUSTED_ERROR_CODE: &str = "subscription:free-usage-exhausted";

/// Whether a rate-limit error is the free-usage-quota exhaustion (paywall)
/// rather than transient throttling. Text-sniff on the flattened message,
/// same precedent as [`is_credit_limit_error`].
pub(crate) fn is_free_usage_exhausted_error(reason: &str) -> bool {
    reason.contains(FREE_USAGE_EXHAUSTED_ERROR_CODE)
}

/// Whether a rate-limited (-32003) ACP error is the free-usage exhaustion.
/// `data` may be a bare string or the `{message, promptUsage?}` object
/// `attach_prompt_usage` produces — always read via the shared detail helper.
pub(crate) fn acp_error_is_free_usage_exhausted(err: &agent_client_protocol::Error) -> bool {
    err.data
        .as_ref()
        .and_then(ainxt_shell::sampling::error::error_detail_from_data)
        .as_deref()
        .is_some_and(is_free_usage_exhausted_error)
}

/// User-facing message for free-usage exhaustion. Shown by headless mode and
/// `format_acp_error` in place of auth-aware rate-limit copy. Deliberately
/// promises no reset duration — the quota window is backend-config-driven.
///
/// A function rather than a `const`, for two reasons. `concat!` only accepts
/// literal tokens, so it cannot splice `ainxt_env::URL_SUBSCRIBE` (a `const` in
/// another crate) — the previous `const` form did not compile at all. And as a
/// function it can resolve the URL through [`upsell_url_upgrade`], so this string
/// honours `AINXT_URL_SUBSCRIBE` exactly like the UI buttons do, instead of baking
/// in a URL a fork would have to patch. Every caller already wanted a `String`,
/// and the neighbouring `rate_limited_user_message` is a function too.
pub(crate) fn free_usage_user_message() -> String {
    format!(
        "You\u{2019}ve reached your free ainxt usage limit for now. \
         Subscribe for much higher limits, or try again later: {}",
        upsell_url_upgrade(),
    )
}

/// Open the credit-limit upsell on the given agent.
///
/// **`max_tier = false`** (default): shows the Q&A question modal with
/// two options ("Upgrade tier" + buy-credits or PAYG). Each option's `id`
/// carries the target URL so the submit handler is position-independent.
///
/// **`max_tier = true`** (positively identified as SuperAinxt Heavy):
/// pushes an inline scrollback card (`CreditLimitBlock`) with a single
/// continue action. No Q&A modal — the user can't upgrade further.
pub(super) fn open_credit_limit_upsell(
    agent: &mut AgentView,
    mode: CreditLimitUpsellMode,
    max_tier: bool,
) {
    use crate::scrollback::blocks::CreditLimitCardAction;

    let (
        heading,
        upgrade_tier_desc,
        secondary_label,
        secondary_desc,
        card_action,
        second_choice,
        payg_telemetry,
    ): (
        &str,
        &str,
        &str,
        &str,
        CreditLimitCardAction,
        ainxt_telemetry::events::CreditLimitChoice,
        bool,
    ) = match mode {
        CreditLimitUpsellMode::UnifiedCredits => (
            "You hit your weekly limit.",
            "Upgrade to a higher tier for more usage",
            "Buy more credits",
            "Purchase credits to keep using ainxt",
            CreditLimitCardAction::PurchaseCredits,
            ainxt_telemetry::events::CreditLimitChoice::PurchaseCredits,
            false,
        ),
        CreditLimitUpsellMode::LegacyPayg { enabled: true } => (
            "You\u{2019}ve hit your spending cap.",
            "Upgrade to a higher tier for more credits",
            "Increase limit",
            "Raise your pay-as-you-go spending cap",
            CreditLimitCardAction::IncreasePaygLimit,
            ainxt_telemetry::events::CreditLimitChoice::PayAsYouGo,
            true,
        ),
        CreditLimitUpsellMode::LegacyPayg { enabled: false } => (
            "You\u{2019}ve hit the credit limit for your plan.",
            "Upgrade to a higher tier for more credits",
            "Pay as you go",
            "Enable pay-as-you-go credits for on-demand usage",
            CreditLimitCardAction::EnablePayg,
            ainxt_telemetry::events::CreditLimitChoice::PayAsYouGo,
            false,
        ),
    };
    let unified_billing = matches!(mode, CreditLimitUpsellMode::UnifiedCredits);

    // ── Max tier: inline scrollback card ─────────────────────────
    if max_tier {
        use crate::scrollback::block::RenderBlock;
        log_event(ainxt_telemetry::events::CreditLimitUpsellShown {
            surface: ainxt_telemetry::events::CreditLimitUpsellSurface::InlineCard,
            max_tier: true,
            pay_as_you_go: payg_telemetry,
            unified_billing,
        });
        agent.scrollback.push_block(RenderBlock::credit_limit_card(
            heading,
            card_action,
            // By value, not by reference: `credit_limit_card` takes `impl Into<String>`, and
            // `String: From<Cow<'_, str>>` exists while `String: From<&Cow<'_, str>>` does not.
            upsell_url_payg(),
        ));
        return;
    }

    log_event(ainxt_telemetry::events::CreditLimitUpsellShown {
        surface: ainxt_telemetry::events::CreditLimitUpsellSurface::QuestionModal,
        max_tier: false,
        pay_as_you_go: payg_telemetry,
        unified_billing,
    });

    // ── Default: Q&A question modal with two options ────────────────
    use crate::views::question_view::{LocalQuestionKind, QuestionViewState};
    use ainxt_tools::implementations::ainxt_build::ask_user_question::{
        Question, QuestionOption,
    };

    if agent.question_view.is_some() {
        return;
    }

    let question = Question {
        question: heading.into(),
        options: vec![
            QuestionOption {
                label: "Upgrade tier".into(),
                description: upgrade_tier_desc.into(),
                preview: None,
                id: Some(upsell_url_upgrade().into_owned()),
            },
            QuestionOption {
                label: secondary_label.into(),
                description: secondary_desc.into(),
                preview: None,
                id: Some(upsell_url_payg().into_owned()),
            },
        ],
        multi_select: Some(false),
        id: None,
    };

    let stashed = agent.prompt.stash();
    let state = QuestionViewState::new(
        format!("credit-limit-upsell-{}", uuid::Uuid::new_v4()),
        vec![question],
        stashed,
    )
    .with_local_kind(LocalQuestionKind::CreditLimitUpsell {
        choices: vec![
            ainxt_telemetry::events::CreditLimitChoice::UpgradeTier,
            second_choice,
        ],
    })
    .with_no_freeform();
    agent.question_view = Some(state);
    agent.prompt.set_text("");
}

/// Open the free-usage paywall on the given agent: a Q&A modal in the
/// [`open_credit_limit_upsell`] style with two upgrade options. Each
/// option's `id` carries its target URL so the submit handler is
/// position-independent.
///
/// Driver-only by construction (called from the PromptResponse handler,
/// which viewers never receive). `auth_method` feeds the
/// `SuperAinxtUpsellShown` funnel event.
pub(super) fn open_free_usage_upsell(agent: &mut AgentView, auth_method: Option<String>) {
    open_subscribe_upsell(agent, UpsellReason::FreeUsageLimit, auth_method);
}

/// Open the SuperAinxt upsell for a tier-restricted slash command
/// (`/usage`, `/imagine`, …). Returns whether the modal opened (`false`
/// when another question modal is already up) so the caller can decide
/// whether to consume the input that triggered it.
pub(super) fn open_restricted_command_upsell(
    agent: &mut AgentView,
    auth_method: Option<String>,
) -> bool {
    open_subscribe_upsell(agent, UpsellReason::RestrictedCommand, auth_method)
}

/// Which situation opened the SuperAinxt upsell modal. Controls the heading
/// and the telemetry source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum UpsellReason {
    /// Free-usage quota exhausted (429 paywall).
    FreeUsageLimit,
    /// A tier-restricted slash command was invoked.
    RestrictedCommand,
}

/// Shared builder behind [`open_free_usage_upsell`] /
/// [`open_restricted_command_upsell`]: a Q&A modal in the
/// [`open_credit_limit_upsell`] style. Upgrade options carry their target
/// URL in the option `id` (position-independent submit handling).
fn open_subscribe_upsell(
    agent: &mut AgentView,
    reason: UpsellReason,
    auth_method: Option<String>,
) -> bool {
    use crate::views::question_view::{LocalQuestionKind, QuestionViewState};
    use ainxt_tools::implementations::ainxt_build::ask_user_question::{
        Question, QuestionOption,
    };

    // Never displace an already-open question modal. Callers that consume
    // input on open must check this `false` and keep the input instead.
    if agent.question_view.is_some() {
        return false;
    }

    let (heading, source, modal_id_prefix) = match reason {
        UpsellReason::FreeUsageLimit => (
            "You hit your free usage limit.",
            SuperAinxtUpsell::FreeUsagePaywall,
            "free-usage-upsell",
        ),
        UpsellReason::RestrictedCommand => (
            "Unlock all features with SuperAinxt.",
            SuperAinxtUpsell::RestrictedCommand,
            "restricted-command-upsell",
        ),
    };

    log_event(ainxt_telemetry::events::SuperAinxtUpsellShown {
        source,
        auth_method,
    });

    let options = vec![
        QuestionOption {
            label: "Upgrade to SuperAinxt".into(),
            description: "For everyday coding and productivity tasks".into(),
            preview: None,
            id: Some(upsell_url_upgrade().into_owned()),
        },
        QuestionOption {
            label: "Upgrade to SuperAinxt Heavy".into(),
            description: "Get the most out of ainxt. Highest usage limits.".into(),
            preview: None,
            // No Heavy-specific URL exists; the /superainxt page lists
            // both plans, so both upgrade options land there.
            id: Some(upsell_url_upgrade().into_owned()),
        },
    ];
    let question = Question {
        question: heading.into(),
        options,
        multi_select: Some(false),
        id: None,
    };

    let stashed = agent.prompt.stash();
    let state = QuestionViewState::new(
        format!("{modal_id_prefix}-{}", uuid::Uuid::new_v4()),
        vec![question],
        stashed,
    )
    .with_local_kind(LocalQuestionKind::FreeUsageUpsell { source })
    .with_no_freeform();
    agent.question_view = Some(state);
    agent.prompt.set_text("");
    true
}

/// Apply an [`AutoTopupFetch`] outcome to a cached `auto_topup` slot: `Resolved`
/// sets it, `Cleared` resets it to "unknown" (no credits), and `Unchanged` keeps
/// the last-known-good value (the fetch failed).
pub(super) fn apply_auto_topup(
    slot: &mut Option<crate::views::credit_bar::AutoTopupInfo>,
    fetch: &crate::views::credit_bar::AutoTopupFetch,
) {
    use crate::views::credit_bar::AutoTopupFetch;
    match fetch {
        AutoTopupFetch::Resolved(rule) => *slot = Some(rule.clone()),
        AutoTopupFetch::Cleared => *slot = None,
        AutoTopupFetch::Unchanged => {}
    }
}

// TaskResult handlers.

pub(super) fn handle_billing_fetched(
    app: &mut AppView,
    agent_id: AgentId,
    balance: Option<crate::views::credit_bar::CreditBalance>,
    silent: bool,
    subscription_tier: Option<String>,
    autotopup: crate::views::credit_bar::AutoTopupFetch,
) -> Vec<Effect> {
    // Parse/transport failures route to `BillingError`, so a `None`
    // balance here means the response carried no billing config. Clear
    // the cached balance + polling so the status bar agrees with the
    // "No billing data available." message rather than showing a stale
    // value.
    app.credit_balance = balance.clone();
    // `Resolved` updates the cached rule, `Cleared` resets it to unknown
    // (no credits), `Unchanged` keeps the last-known-good (fetch failed).
    apply_auto_topup(&mut app.auto_topup, &autotopup);
    app.billing_poll_wanted = balance
        .as_ref()
        .map(|b| b.usage_pct >= 99.0)
        .unwrap_or(false);
    if let Some(tier) = subscription_tier {
        app.subscription_tier = Some(tier);
    }
    // Render the `/usage` summary from the now-current cached rule.
    let summary_topup = app.auto_topup.clone();
    if let Some(agent) = app.agents.get_mut(&agent_id) {
        // Gateway/chat-kind: do not attach Build coding credits.
        let mut topup = agent.auto_topup.clone();
        apply_auto_topup(&mut topup, &autotopup);
        agent.apply_credit_balance(balance.clone(), topup);
        if !silent && !agent.chat_kind {
            let msg = match &balance {
                Some(bal) => {
                    crate::views::credit_bar::format_usage_summary(bal, summary_topup.as_ref())
                }
                None => "No billing data available.".to_string(),
            };
            agent.scrollback.push_block(RenderBlock::System(
                crate::scrollback::blocks::SystemMessageBlock::new(msg),
            ));
        }
    }
    vec![]
}

pub(super) fn handle_gate_refreshed(
    app: &mut AppView,
    settings: Option<ainxt_shell::util::config::RemoteSettings>,
) -> Vec<Effect> {
    let Some(rs) = settings else {
        return vec![];
    };
    app.usage_billing_redirect_url = rs.usage_billing_redirect_url.clone();
    if let Some(secs) = rs.subscription_watch_interval_secs {
        app.subscription_watch_interval_secs = Some(secs);
    }
    match AppView::gate_from_settings(&rs) {
        Some(gate) => app.impose_gate(gate),
        None => app.lift_gate(),
    }
}

/// `ainxt.dev/auth/check_subscription` completed. Meta is authoritative
/// (`apply_auth_meta` also drops any deferred gate). A failed check only
/// promotes the deferred gate it was verifying (`verify` generation);
/// generic watch/focus/paywall-chain failures never touch it.
pub(super) fn handle_check_subscription_complete(
    app: &mut AppView,
    verify: Option<u64>,
    meta: Option<serde_json::Value>,
) -> Vec<Effect> {
    let was_blocked = !app.has_access();
    let applied = match meta {
        Some(meta_val) => {
            match serde_json::from_value::<ainxt_shell::auth::AuthMeta>(meta_val) {
                Ok(auth_meta) => {
                    app.apply_auth_meta(&auth_meta);
                    true
                }
                Err(e) => {
                    // Shell sent meta we can't decode — a protocol bug, not
                    // a transient failure. The check result is lost, so a
                    // verify deferral falls through to promotion below.
                    crate::unified_log::error(
                        "subscription.check.meta_parse_failed",
                        None,
                        Some(serde_json::json!({
                            "verify": verify,
                            "error": e.to_string(),
                        })),
                    );
                    false
                }
            }
        }
        // meta: None = shell reports "not authenticated" or the check RPC
        // failed (already logged as subscription.check.rpc_failed).
        None => false,
    };
    if !applied && let Some(generation) = verify {
        app.promote_deferred_gate(generation, "check_failed");
    }
    crate::unified_log::info(
        "subscription.check.complete",
        None,
        Some(serde_json::json!({
            "verify": verify,
            "meta_applied": applied,
            "was_blocked": was_blocked,
            "gated": !app.has_access(),
            "tier": app.subscription_tier,
        })),
    );
    maybe_start_paywall_chain(app, was_blocked)
}

/// Safety net for a hung verification check: show the still-pending
/// deferred gate (err on blocking).
pub(super) fn handle_gate_verify_timeout(app: &mut AppView, generation: u64) -> Vec<Effect> {
    let was_blocked = !app.has_access();
    app.promote_deferred_gate(generation, "verify_timeout");
    maybe_start_paywall_chain(app, was_blocked)
}

/// Arm the 5s paywall auto-check chain on an ungated→gated transition, so a
/// paywall shown by verify-before-paywall self-lifts exactly like the
/// login-path one. Guarded so steady-state paywall-poller responses and
/// repeated checks can't fan out extra timers.
fn maybe_start_paywall_chain(app: &mut AppView, was_blocked: bool) -> Vec<Effect> {
    if !was_blocked && !app.has_access() && app.paywall_check_started.is_none() {
        app.paywall_check_started = Some(std::time::Instant::now());
        return vec![Effect::SchedulePaywallCheck];
    }
    vec![]
}

pub(super) fn handle_credit_limit_recheck_complete(
    app: &mut AppView,
    agent_id: AgentId,
    meta: Option<serde_json::Value>,
) -> Vec<Effect> {
    let old_tier = app.subscription_tier.clone();
    if let Some(meta_val) = meta
        && let Ok(auth_meta) = serde_json::from_value::<ainxt_shell::auth::AuthMeta>(meta_val)
    {
        app.apply_auth_meta(&auth_meta);
    }
    let tier_changed = app.subscription_tier != old_tier && app.subscription_tier.is_some();

    let Some(agent) = app.agents.get_mut(&agent_id) else {
        return vec![];
    };

    // If the user already submitted another prompt while the
    // recheck was in flight, don't retry the stashed one — they've
    // moved on. The tier update (above) still takes effect.
    let user_moved_on = !agent.session.state.is_idle() || !agent.session.pending_prompts.is_empty();

    if tier_changed && !user_moved_on {
        if let Some(prompt) = agent.credit_limit_stashed_prompt.take() {
            let tier_name = app.subscription_tier.as_deref().unwrap_or("a higher tier");
            agent.scrollback.push_block(RenderBlock::system(format!(
                "Subscription upgraded to {tier_name}. Retrying\u{2026}"
            )));
            agent.session.enqueue_in_flight_prompt_front(prompt);
        }
    } else if !user_moved_on {
        let balance = agent
            .credit_balance
            .as_ref()
            .or(app.credit_balance.as_ref());
        let mode = credit_limit_upsell_mode(balance);
        let max_tier = is_max_tier(app.subscription_tier.as_deref());
        open_credit_limit_upsell(agent, mode, max_tier);
    }
    // Either way, drop the stashed prompt.
    agent.credit_limit_stashed_prompt = None;

    let mut effects = maybe_drain_queue(agent);
    effects.push(Effect::FetchBilling {
        agent_id,
        silent: true,
    });
    effects
}

// Action handlers.

pub(super) fn dispatch_open_subscribe_url(app: &mut AppView) -> Vec<Effect> {
    log_event(SuperAinxtUpsellClicked {
        source: SuperAinxtUpsell::WelcomeScreen,
        auth_method: app.login_method_id.as_ref().map(|id| id.0.to_string()),
    });
    // Remote settings' `gate_url` wins; otherwise fall back to the RESOLVER, not
    // the compile-time constant. Using `ainxt_env::URL_SUBSCRIBE` here meant this
    // button silently ignored `AINXT_URL_SUBSCRIBE` — the override that
    // `upsell_url_upgrade()` exists to honour and that `env.example` and RUN.md
    // both document. With the constant empty in this build (audit risk R42) the
    // button opened nothing at all.
    let configured: String = app
        .gate
        .as_ref()
        .and_then(|g| g.url.as_deref())
        .map(str::to_owned)
        .unwrap_or_else(|| upsell_url_upgrade().into_owned());
    let url = configured.as_str();
    // Funnel attribution: tag CLI-originated SuperAinxt upsell clicks
    // with `referrer=ainxt-build`, matching the OAuth consent flow and
    // ainxt.dev/cli marketing links. Applied even when the URL came from
    // remote settings's `gate_url`, so we don't depend on the remote flag
    // being correctly configured. If the URL already specifies a
    // referrer it's left alone.
    let url = crate::app::link_opener::ensure_query_param(url, "referrer", "ainxt-build");
    super::ctx::open_url_or_show(app, &url);
    vec![]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_usage_dual_read_string_and_wrapped_object_data() {
        let free = "subscription:free-usage-exhausted quota hit";
        let string_err = agent_client_protocol::Error::new(-32003, "Rate limited").data(free);
        assert!(acp_error_is_free_usage_exhausted(&string_err));

        // attach_prompt_usage wraps string data as {"message": ..., "promptUsage": ...}.
        let wrapped =
            agent_client_protocol::Error::new(-32003, "Rate limited").data(serde_json::json!({
                "message": free,
                "promptUsage": { "inputTokens": 1, "outputTokens": 0, "numTurns": 1 }
            }));
        assert!(acp_error_is_free_usage_exhausted(&wrapped));
        assert!(!wrapped.data.as_ref().unwrap().is_string());

        let other = agent_client_protocol::Error::new(-32003, "Rate limited").data("throttled");
        assert!(!acp_error_is_free_usage_exhausted(&other));
    }
}
