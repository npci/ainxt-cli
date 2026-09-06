//! The policy document as it exists per source, and the resolved policy.

use serde::{Deserialize, Serialize};

use crate::capability::SecurityCapabilities;
use crate::verdict::Enforcement;

/// Which settings source a layer came from. Ordering is precedence, highest
/// first — `PolicyBundle` (signed, root-owned) is authoritative, repo sources
/// are attacker-controllable and rank lowest.
///
/// `Ord` is defined so that a *smaller* discriminant is *higher* precedence,
/// matching the resolution chain in `AINXT-SEC-001` §5.2.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "camelCase")]
pub enum SourceOrigin {
    /// Signed policy bundle — the authority.
    PolicyBundle,
    /// MDM/managed settings.
    Managed,
    /// User's own settings.
    User,
    /// Repo `.ainxt/settings.json` — **attacker-controllable**.
    Project,
    /// Repo local overrides — **attacker-controllable**.
    Local,
    /// CLI flags / runtime.
    Flags,
}

impl SourceOrigin {
    /// Whether this source is under the control of repository contents, and
    /// therefore of an attacker who can influence a cloned repo (entry point
    /// E-2). Such sources may only narrow, never widen — enforced structurally
    /// by the merge algebra, but exposed here for audit and explanation.
    pub fn is_attacker_controllable(self) -> bool {
        matches!(self, SourceOrigin::Project | SourceOrigin::Local)
    }
}

/// One source's contribution to policy, before merge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceLayer {
    pub origin: SourceOrigin,
    /// Absent means "this source says nothing about enforcement".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enforcement: Option<Enforcement>,
    #[serde(default)]
    pub capabilities: SecurityCapabilities,
}

impl SourceLayer {
    pub fn new(origin: SourceOrigin) -> Self {
        SourceLayer { origin, enforcement: None, capabilities: SecurityCapabilities::default() }
    }
}

/// The resolved, merged security policy a decision consults.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecurityPolicy {
    pub enforcement: Enforcement,
    pub capabilities: SecurityCapabilities,
}

impl SecurityPolicy {
    /// The permissive OSS baseline used when no bundle is present and
    /// `require_policy = false`.
    pub fn oss_default() -> Self {
        SecurityPolicy {
            enforcement: Enforcement::Off,
            capabilities: SecurityCapabilities::default(),
        }
    }
}
