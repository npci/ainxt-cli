//! The narrowing-only merge algebra — invariant **INV-2**.
//!
//! Given a stack of [`SourceLayer`]s in any order, [`resolve`] produces a single
//! [`SecurityPolicy`] that is **no wider than any individual layer**. This is the
//! whole reason repo-level settings (which an attacker can ship in a cloned
//! repo) cannot escalate capability: merge is the componentwise *meet*, not
//! override.
//!
//! The algebra is a commutative monoid under narrowing, so source *order does
//! not affect capability* — order matters only for explanation/attribution, not
//! for the security outcome. The property tests below assert exactly that:
//! permuting the layers never changes the resolved capability set.

use std::collections::BTreeSet;

use crate::capability::{Denylist, SecurityCapabilities, SovereignAction};
use crate::policy::{SecurityPolicy, SourceLayer};

/// Merge two capability sets by taking the narrowest of each dimension.
pub fn narrow_capabilities(
    a: &SecurityCapabilities,
    b: &SecurityCapabilities,
) -> SecurityCapabilities {
    SecurityCapabilities {
        egress_allow: a.egress_allow.meet(&b.egress_allow),
        egress_deny: a.egress_deny.union(&b.egress_deny),
        exec_allow: a.exec_allow.meet(&b.exec_allow),
        exec_deny: a.exec_deny.union(&b.exec_deny),
        write_allow: a.write_allow.meet(&b.write_allow),
        mcp_allow: a.mcp_allow.meet(&b.mcp_allow),
        cred_paths: a.cred_paths.union(&b.cred_paths),
        sovereign: a.sovereign.union(&b.sovereign).copied().collect(),
    }
}

/// Resolve a stack of source layers into one effective policy.
///
/// `base` is the starting point — typically [`SecurityPolicy::oss_default`] or,
/// under `require_policy`, the signed bundle's floor. Every layer can only
/// narrow it further.
pub fn resolve(base: &SecurityPolicy, layers: &[SourceLayer]) -> SecurityPolicy {
    let mut enforcement = base.enforcement;
    let mut capabilities = base.capabilities.clone();

    for layer in layers {
        if let Some(e) = layer.enforcement {
            enforcement = enforcement.narrow(e);
        }
        capabilities = narrow_capabilities(&capabilities, &layer.capabilities);
    }

    SecurityPolicy { enforcement, capabilities }
}

/// Whether `wide` permits everything `narrow` permits and possibly more — i.e.
/// `narrow ⊑ wide` in the capability lattice. Used by the INV-2 property tests
/// and available to `ainxt-policy` for assertions.
pub fn is_narrower_or_equal(narrow: &SecurityCapabilities, wide: &SecurityCapabilities) -> bool {
    allow_subset(&narrow.egress_allow, &wide.egress_allow)
        && allow_subset(&narrow.exec_allow, &wide.exec_allow)
        && allow_subset(&narrow.write_allow, &wide.write_allow)
        && allow_subset(&narrow.mcp_allow, &wide.mcp_allow)
        && deny_superset(&narrow.egress_deny, &wide.egress_deny)
        && deny_superset(&narrow.exec_deny, &wide.exec_deny)
        && deny_superset(&narrow.cred_paths, &wide.cred_paths)
        && sovereign_superset(&narrow.sovereign, &wide.sovereign)
}

/// `narrow` permits a subset of what `wide` permits.
fn allow_subset(
    narrow: &crate::capability::Allowlist,
    wide: &crate::capability::Allowlist,
) -> bool {
    use crate::capability::Allowlist::*;
    match (narrow, wide) {
        // Any permits everything: it can only be a subset of another Any.
        (Any, Any) => true,
        (Any, Only(_)) => false,
        (Only(_), Any) => true,
        (Only(n), Only(w)) => n.is_subset(w),
    }
}

/// A narrower policy denies at least as much (superset of denials).
fn deny_superset(narrow: &Denylist, wide: &Denylist) -> bool {
    wide.0.is_subset(&narrow.0)
}

fn sovereign_superset(narrow: &BTreeSet<SovereignAction>, wide: &BTreeSet<SovereignAction>) -> bool {
    wide.is_subset(narrow)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::{Allowlist, Denylist, SovereignAction};
    use crate::policy::SourceOrigin;
    use crate::verdict::Enforcement;
    use pretty_assertions::assert_eq;

    fn layer(origin: SourceOrigin, caps: SecurityCapabilities, e: Option<Enforcement>) -> SourceLayer {
        SourceLayer { origin, enforcement: e, capabilities: caps }
    }

    fn caps_egress(a: Allowlist) -> SecurityCapabilities {
        SecurityCapabilities { egress_allow: a, ..SecurityCapabilities::default() }
    }

    // --- INV-2 core: a project (attacker-controllable) source cannot widen ---

    #[test]
    fn inv2_project_cannot_widen_egress() {
        // Bundle: only the gateway. Project tries to add evil.example.
        let bundle = SecurityPolicy {
            enforcement: Enforcement::Block,
            capabilities: caps_egress(Allowlist::only(["gateway.internal"])),
        };
        let malicious_project = layer(
            SourceOrigin::Project,
            caps_egress(Allowlist::only(["gateway.internal", "evil.example"])),
            None,
        );

        let resolved = resolve(&bundle, &[malicious_project]);
        // The meet of {gateway} and {gateway, evil} is {gateway}. evil is gone.
        assert_eq!(resolved.capabilities.egress_allow, Allowlist::only(["gateway.internal"]));
        assert!(!resolved.capabilities.egress_allow.permits("evil.example"));
    }

    #[test]
    fn inv2_project_cannot_lower_enforcement() {
        let bundle = SecurityPolicy {
            enforcement: Enforcement::Block,
            capabilities: SecurityCapabilities::default(),
        };
        let malicious_project =
            layer(SourceOrigin::Project, SecurityCapabilities::default(), Some(Enforcement::Off));
        let resolved = resolve(&bundle, &[malicious_project]);
        assert_eq!(resolved.enforcement, Enforcement::Block);
    }

    #[test]
    fn inv2_project_cannot_remove_a_denial() {
        let bundle = SecurityPolicy {
            enforcement: Enforcement::Block,
            capabilities: SecurityCapabilities {
                exec_deny: Denylist::of(["powershell"]),
                ..SecurityCapabilities::default()
            },
        };
        // Project supplies an empty denylist hoping to clear it.
        let resolved = resolve(&bundle, &[layer(SourceOrigin::Project, SecurityCapabilities::default(), None)]);
        assert!(resolved.capabilities.exec_deny.denies("powershell"));
    }

    #[test]
    fn inv2_project_cannot_remove_sovereign_action() {
        let bundle = SecurityPolicy {
            enforcement: Enforcement::Block,
            capabilities: SecurityCapabilities {
                sovereign: [SovereignAction::PackagePublish].into_iter().collect(),
                ..SecurityCapabilities::default()
            },
        };
        // Project supplies an empty sovereign set.
        let resolved = resolve(
            &bundle,
            &[layer(
                SourceOrigin::Project,
                SecurityCapabilities { sovereign: BTreeSet::new(), ..SecurityCapabilities::default() },
                None,
            )],
        );
        assert!(resolved.capabilities.sovereign.contains(&SovereignAction::PackagePublish));
    }

    // --- INV-2 property: order-independence ("no source order can widen") ---

    /// A tiny deterministic pseudo-random capability generator, so this test has
    /// no external proptest dependency but still exercises many stacks.
    fn pseudo_caps(seed: u64) -> SecurityCapabilities {
        let bit = |n: u32| (seed >> n) & 1 == 1;
        let egress = if bit(0) {
            Allowlist::Any
        } else {
            let mut s = BTreeSet::new();
            if bit(1) { s.insert("a".to_string()); }
            if bit(2) { s.insert("b".to_string()); }
            if bit(3) { s.insert("c".to_string()); }
            Allowlist::Only(s)
        };
        let mut deny = BTreeSet::new();
        if bit(4) { deny.insert("x".to_string()); }
        if bit(5) { deny.insert("y".to_string()); }
        let mut sov = BTreeSet::new();
        if bit(6) { sov.insert(SovereignAction::ForcePush); }
        SecurityCapabilities {
            egress_allow: egress,
            egress_deny: Denylist(deny),
            sovereign: sov,
            ..SecurityCapabilities::default()
        }
    }

    #[test]
    fn inv2_merge_is_order_independent() {
        let base = SecurityPolicy::oss_default();
        for seed in 0u64..512 {
            let l0 = layer(SourceOrigin::Managed, pseudo_caps(seed), None);
            let l1 = layer(SourceOrigin::User, pseudo_caps(seed.rotate_left(7)), None);
            let l2 = layer(SourceOrigin::Project, pseudo_caps(seed.rotate_left(13)), None);

            let forward = resolve(&base, &[l0.clone(), l1.clone(), l2.clone()]);
            let reverse = resolve(&base, &[l2, l1, l0]);
            assert_eq!(
                forward.capabilities, reverse.capabilities,
                "capability resolution must not depend on source order (seed {seed})"
            );
        }
    }

    #[test]
    fn inv2_result_is_narrower_than_every_layer() {
        let base = SecurityPolicy::oss_default();
        for seed in 0u64..512 {
            let layers = [
                layer(SourceOrigin::Managed, pseudo_caps(seed), None),
                layer(SourceOrigin::User, pseudo_caps(seed.rotate_left(11)), None),
                layer(SourceOrigin::Project, pseudo_caps(seed.rotate_left(17)), None),
            ];
            let resolved = resolve(&base, &layers);
            for l in &layers {
                assert!(
                    is_narrower_or_equal(&resolved.capabilities, &l.capabilities),
                    "resolved policy must be narrower than layer {:?} (seed {seed})",
                    l.origin
                );
            }
            // And narrower than the base.
            assert!(is_narrower_or_equal(&resolved.capabilities, &base.capabilities));
        }
    }

    #[test]
    fn narrow_capabilities_is_idempotent() {
        for seed in 0u64..256 {
            let c = pseudo_caps(seed);
            assert_eq!(narrow_capabilities(&c, &c), c, "meet with self is identity (seed {seed})");
        }
    }
}
