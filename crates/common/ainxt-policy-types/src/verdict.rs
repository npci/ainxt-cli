//! Verdicts, enforcement levels, and the decision record.

use serde::{Deserialize, Serialize};

use crate::ids::RuleId;

/// The global enforcement posture. Monotone: a lower-precedence source may only
/// **raise** it (INV-2). `Off < Warn < Block`, so `max` is the merge operation.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Enforcement {
    /// Rules evaluated, never enforced. OSS/dev default.
    Off,
    /// Violations audited and surfaced, action still permitted. Rollout mode.
    Warn,
    /// Violations blocked. managed production default.
    Block,
}

impl Enforcement {
    /// Narrowing merge: the stricter of two postures wins.
    pub fn narrow(self, other: Enforcement) -> Enforcement {
        self.max(other)
    }
}

/// A per-finding verdict from a content or capability check.
///
/// Ordered by severity so `max` yields the strictest verdict across layers.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Allow,
    Redact,
    Warn,
    Block,
}

/// The outcome of consulting policy for a single action, suitable for both an
/// audit record and a denial message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Decision {
    pub verdict: Verdict,
    /// The rule that produced this verdict, if any (allows carry `None`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule: Option<RuleId>,
    /// Human-readable rationale, surfaced in denial messages and `policy explain`.
    pub reason: String,
    /// Whether escalating past this decision requires a live human (Sovereign).
    pub requires_human: bool,
}

impl Decision {
    pub fn allow() -> Self {
        Decision {
            verdict: Verdict::Allow,
            rule: None,
            reason: "no matching restriction".to_string(),
            requires_human: false,
        }
    }

    pub fn block(rule: RuleId, reason: impl Into<String>) -> Self {
        Decision {
            verdict: Verdict::Block,
            rule: Some(rule),
            reason: reason.into(),
            requires_human: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enforcement_narrow_is_max() {
        assert_eq!(Enforcement::Off.narrow(Enforcement::Block), Enforcement::Block);
        assert_eq!(Enforcement::Warn.narrow(Enforcement::Off), Enforcement::Warn);
        assert_eq!(Enforcement::Block.narrow(Enforcement::Warn), Enforcement::Block);
    }

    #[test]
    fn verdict_orders_by_severity() {
        assert!(Verdict::Block > Verdict::Warn);
        assert!(Verdict::Warn > Verdict::Redact);
        assert!(Verdict::Redact > Verdict::Allow);
    }
}
