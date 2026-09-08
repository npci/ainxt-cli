//! Egress decision primitive (P3 → INV-1).
//!
//! Parses a target URL (or bare host) into a host and asks the
//! [`PolicyEngine`] whether egress is permitted. This is the userspace half of
//! INV-1 — it governs the CLI process's *own* outbound calls (the model/gateway
//! traffic in `ainxt-sampler`), ensuring the CLI only talks to allowlisted
//! hosts and cannot be pointed elsewhere by config.
//!
//! The *other* half — constraining sandboxed child processes (curl/git/pip
//! inside a shell) — is kernel work layered on `child_net.rs` and is tracked
//! separately; it is not this module.
//!
//! Fail-closed: an unparseable target or one with no host yields a `Block`
//! decision, so a malformed or opaque URL cannot slip past a restrictive
//! allowlist.

use ainxt_policy_types::ids::{Domain, RuleId};
use ainxt_policy_types::tier::TrustTier;
use ainxt_policy_types::verdict::Decision;

use crate::engine::PolicyEngine;

/// Decide egress for a full URL string.
pub fn check_url(engine: &PolicyEngine, url_str: &str, tier: TrustTier) -> Decision {
    match host_of(url_str) {
        Some(host) => engine.egress_decision(&host, tier),
        None => Decision::block(
            RuleId::default_rule(Domain::Egress, "004"),
            format!("egress target {url_str:?} has no parseable host"),
        ),
    }
}

/// Decide egress for a bare host (no scheme).
pub fn check_host(engine: &PolicyEngine, host: &str, tier: TrustTier) -> Decision {
    engine.egress_decision(host, tier)
}

/// Extract the lowercased host from a URL string. Returns `None` if the string
/// does not parse as a URL with a host.
pub fn host_of(url_str: &str) -> Option<String> {
    let parsed = url::Url::parse(url_str).ok()?;
    parsed.host_str().map(|h| h.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ainxt_policy_types::capability::{Allowlist, Denylist, SecurityCapabilities};
    use ainxt_policy_types::policy::SecurityPolicy;
    use ainxt_policy_types::verdict::{Enforcement, Verdict};

    fn engine_allowing(hosts: &[&str]) -> PolicyEngine {
        PolicyEngine::new(SecurityPolicy {
            enforcement: Enforcement::Block,
            capabilities: SecurityCapabilities {
                egress_allow: Allowlist::only(hosts.iter().copied()),
                ..SecurityCapabilities::default()
            },
        })
    }

    #[test]
    fn allowlisted_gateway_permitted() {
        let engine = engine_allowing(&["gateway.internal"]);
        let d = check_url(&engine, "https://gateway.internal/v1/messages", TrustTier::Workspace);
        assert_eq!(d.verdict, Verdict::Allow);
    }

    #[test]
    fn github_and_indirect_hosts_denied_by_omission() {
        // MANAGED-EGRESS-002: only the internal gateway/mirror are allowlisted, so
        // every GitHub-family host is denied without enumerating any of them.
        let engine = engine_allowing(&["gateway.internal", "gitlab.internal", "nexus.internal"]);
        for target in [
            "https://github.com/foo/bar",
            "https://raw.githubusercontent.com/foo/bar/main/x",
            "https://codeload.github.com/foo/bar/tar.gz",
            "https://objects.githubusercontent.com/blah",
            "https://foo.github.io/",
            "git://github.com/foo/bar",
        ] {
            let d = check_url(&engine, target, TrustTier::Operator);
            assert_eq!(d.verdict, Verdict::Block, "should deny {target}");
        }
    }

    #[test]
    fn explicit_denylist_beats_allowlist() {
        let engine = PolicyEngine::new(SecurityPolicy {
            enforcement: Enforcement::Block,
            capabilities: SecurityCapabilities {
                egress_allow: Allowlist::Any,
                egress_deny: Denylist::of(["metadata.google.internal"]),
                ..SecurityCapabilities::default()
            },
        });
        // Cloud-metadata exfil target, denied even under an Any allowlist.
        let d = check_url(&engine, "http://metadata.google.internal/computeMetadata/v1/", TrustTier::Operator);
        assert_eq!(d.verdict, Verdict::Block);
    }

    #[test]
    fn untrusted_tier_cannot_egress_even_if_allowlisted() {
        let engine = engine_allowing(&["gateway.internal"]);
        let d = check_url(&engine, "https://gateway.internal/v1/messages", TrustTier::Untrusted);
        assert_eq!(d.verdict, Verdict::Block);
        assert_eq!(d.rule.unwrap().to_string(), "DEFAULT-EGRESS-003");
    }

    #[test]
    fn unparseable_target_is_blocked_fail_closed() {
        let engine = engine_allowing(&["gateway.internal"]);
        let d = check_url(&engine, "not a url", TrustTier::Operator);
        assert_eq!(d.verdict, Verdict::Block);
        assert_eq!(d.rule.unwrap().to_string(), "DEFAULT-EGRESS-004");
    }

    #[test]
    fn host_is_lowercased() {
        assert_eq!(host_of("https://GateWay.Internal/x").as_deref(), Some("gateway.internal"));
    }
}
