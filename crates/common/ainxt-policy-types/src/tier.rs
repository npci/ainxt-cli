//! The trust lattice (`AINXT-SEC-001` §5.4.1).
//!
//! Defined here rather than in `ainxt-provenance` (P5) because it is pure schema
//! and the policy capability table is keyed on it. `ainxt-provenance` builds its
//! propagation logic on top of this type.

use serde::{Deserialize, Serialize};

/// Trust tier of a span of context, or of an action derived from spans.
///
/// Ordered least-trusted to most-trusted. `Ord` follows that order, so
/// `min` of a set of influencing spans yields the correct action tier
/// (INV-4: an action is only as trusted as its least-trusted influence).
///
/// ```
/// use ainxt_policy_types::TrustTier;
/// // The minimum influence wins — untrusted content poisons the action.
/// assert_eq!(
///     TrustTier::Operator.min(TrustTier::Untrusted),
///     TrustTier::Untrusted
/// );
/// assert!(TrustTier::Operator > TrustTier::Untrusted);
/// ```
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum TrustTier {
    /// E-1…E-11: web, MCP, tool stdout, issues, recall, subagent output, mesh.
    Untrusted,
    /// Model output.
    Derived,
    /// Repo files present at session start.
    Workspace,
    /// A human typed it into the TTY.
    Operator,
}

impl TrustTier {
    /// The most-restrictive tier — the floor of the lattice.
    pub const FLOOR: TrustTier = TrustTier::Untrusted;

    /// True if this tier may perform network egress beyond the gateway,
    /// spawn processes, read credential paths, or take a `Sovereign` action.
    ///
    /// Only `Workspace` and `Operator` clear this bar. `Untrusted` and
    /// `Derived` cannot — which is the concrete realisation of INV-4.
    pub fn permits_consequential_actions(self) -> bool {
        matches!(self, TrustTier::Workspace | TrustTier::Operator)
    }
}
