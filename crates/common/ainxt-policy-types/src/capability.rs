//! The capability lattice.
//!
//! Each dimension is a lattice element whose *meet* (greatest lower bound) is
//! the narrowest of two values. The merge algebra in [`crate::merge`] is nothing
//! but the componentwise meet, which is exactly why a lower-precedence source
//! can never widen capability (INV-2).

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// An allowlist over string entries (hosts, binaries, write roots).
///
/// The critical modelling choice for INV-2 is that "absent" and "empty" are
/// **different**:
///
/// - [`Allowlist::Any`] — this source imposes no constraint on the dimension.
///   It is the top of the lattice; meeting it with anything yields the other.
/// - [`Allowlist::Only`] — only these entries are permitted, possibly none.
///   `Only(∅)` permits nothing and is the bottom.
///
/// Meet is set intersection with `Any` as identity, which guarantees the result
/// is never wider than either input.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Allowlist {
    /// No constraint from this source (lattice top).
    #[default]
    Any,
    /// Only these entries are permitted.
    Only(BTreeSet<String>),
}

impl Allowlist {
    /// Build an `Only` allowlist from entries.
    pub fn only<I, S>(entries: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Allowlist::Only(entries.into_iter().map(Into::into).collect())
    }

    /// The permit-nothing allowlist (lattice bottom).
    pub fn nothing() -> Self {
        Allowlist::Only(BTreeSet::new())
    }

    /// Narrowing merge: intersection with `Any` as identity.
    ///
    /// `Any ∧ x = x`; `Only(a) ∧ Only(b) = Only(a ∩ b)`. Commutative and
    /// associative; the result is `⊆` both inputs' permitted sets.
    pub fn meet(&self, other: &Allowlist) -> Allowlist {
        match (self, other) {
            (Allowlist::Any, o) => o.clone(),
            (s, Allowlist::Any) => s.clone(),
            (Allowlist::Only(a), Allowlist::Only(b)) => {
                Allowlist::Only(a.intersection(b).cloned().collect())
            }
        }
    }

    /// Whether `entry` is permitted by this allowlist.
    pub fn permits(&self, entry: &str) -> bool {
        match self {
            Allowlist::Any => true,
            Allowlist::Only(set) => set.contains(entry),
        }
    }
}

/// A denylist. Merge is **union** — any source may add a denial, none may remove
/// one. A denylist entry always wins over an allowlist entry (deny-precedence),
/// which is enforced where the two are consulted together in `ainxt-policy`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Denylist(pub BTreeSet<String>);

impl Denylist {
    pub fn of<I, S>(entries: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Denylist(entries.into_iter().map(Into::into).collect())
    }

    /// Narrowing merge: union.
    pub fn union(&self, other: &Denylist) -> Denylist {
        Denylist(self.0.union(&other.0).cloned().collect())
    }

    pub fn denies(&self, entry: &str) -> bool {
        self.0.contains(entry)
    }
}

/// An action that no automatic approval mechanism may ever cover
/// (`AINXT-SEC-001` §5.5.1). The set **unions** on merge; a source may extend it
/// but never shrink it.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum SovereignAction {
    Persistence,
    CredentialAccess,
    PrivilegeEscalation,
    GitHistoryRewrite,
    ForcePush,
    ProtectedBranchWrite,
    PackagePublish,
    BulkFileOperation,
    NonAllowlistedEgress,
    SecurityConfigChange,
}

/// The full set of security capabilities after merge — the effective policy a
/// decision consults. Every field's merge is narrowing (see [`crate::merge`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecurityCapabilities {
    /// Hosts the egress broker will permit (`host:port` or bare host).
    #[serde(default)]
    pub egress_allow: Allowlist,
    /// Destinations always denied regardless of allowlist.
    #[serde(default)]
    pub egress_deny: Denylist,
    /// Binaries permitted to execute, by basename or absolute path.
    #[serde(default)]
    pub exec_allow: Allowlist,
    /// Binaries always denied.
    #[serde(default)]
    pub exec_deny: Denylist,
    /// Filesystem roots writable without a Sovereign approval.
    #[serde(default)]
    pub write_allow: Allowlist,
    /// Paths whose read is a credential access (always Sovereign).
    #[serde(default)]
    pub cred_paths: Denylist,
    /// MCP tools permitted, as `server/tool` (or `server/*`).
    ///
    /// MCP arguments are opaque JSON that cannot be introspected, so unlike
    /// every other dimension there is no derivation to fall back on: an
    /// allowlist over the `(server, tool)` pair is the *only* containment
    /// available. Without it, MCP is a hole wide enough for every incident
    /// class the rest of this struct exists to close.
    #[serde(default)]
    pub mcp_allow: Allowlist,
    /// The Sovereign action set.
    #[serde(default)]
    pub sovereign: BTreeSet<SovereignAction>,
}

impl Default for SecurityCapabilities {
    fn default() -> Self {
        // The OSS built-in default: permissive on allowlists (`Any`), with only
        // the safety-critical Sovereign actions pinned. a managed deployment's bundle narrows
        // this; it can never be widened past it.
        SecurityCapabilities {
            mcp_allow: Allowlist::Any,
            egress_allow: Allowlist::Any,
            egress_deny: Denylist::default(),
            exec_allow: Allowlist::Any,
            exec_deny: Denylist::default(),
            write_allow: Allowlist::Any,
            cred_paths: Denylist::default(),
            sovereign: default_sovereign_set(),
        }
    }
}

/// The Sovereign actions the OSS engine pins even with no bundle. These are the
/// universally dangerous ones; a bundle may add more but never remove these.
pub fn default_sovereign_set() -> BTreeSet<SovereignAction> {
    use SovereignAction::*;
    [
        Persistence,
        CredentialAccess,
        PrivilegeEscalation,
        SecurityConfigChange,
    ]
    .into_iter()
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn any_is_meet_identity() {
        let only = Allowlist::only(["a", "b"]);
        assert_eq!(Allowlist::Any.meet(&only), only);
        assert_eq!(only.meet(&Allowlist::Any), only);
    }

    #[test]
    fn meet_is_intersection() {
        let a = Allowlist::only(["a", "b", "c"]);
        let b = Allowlist::only(["b", "c", "d"]);
        assert_eq!(a.meet(&b), Allowlist::only(["b", "c"]));
    }

    #[test]
    fn meet_never_widens() {
        let a = Allowlist::only(["a"]);
        let b = Allowlist::only(["b"]);
        // Disjoint allowlists meet to nothing — never to their union.
        assert_eq!(a.meet(&b), Allowlist::nothing());
    }

    #[test]
    fn empty_only_permits_nothing() {
        assert!(!Allowlist::nothing().permits("anything"));
        assert!(Allowlist::Any.permits("anything"));
    }

    #[test]
    fn denylist_union_grows() {
        let a = Denylist::of(["x"]);
        let b = Denylist::of(["y"]);
        let u = a.union(&b);
        assert!(u.denies("x") && u.denies("y"));
    }
}
