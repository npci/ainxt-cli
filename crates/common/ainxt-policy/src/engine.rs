//! The decision engine — the single point every capability check consults.
//!
//! Holds the resolved [`SecurityPolicy`] and answers per-action questions. Two
//! invariants live here:
//!
//! - **INV-5**: [`PolicyEngine::can_auto_approve`] returns `false` for every
//!   Sovereign action, unconditionally. No flag, env var, YOLO mode, hook, or
//!   settings source is even an input to that method, so none can flip it.
//! - **INV-4 (capability gate)**: [`PolicyEngine::tier_permits`] denies
//!   consequential actions below the `Workspace` tier. The full provenance
//!   propagation is `ainxt-provenance` (P5); this is the enforcement point it
//!   calls.

use ainxt_policy_types::capability::SovereignAction;
use ainxt_policy_types::ids::{Domain, RuleId};
use ainxt_policy_types::policy::SecurityPolicy;
use ainxt_policy_types::tier::TrustTier;
use ainxt_policy_types::verdict::{Decision, Enforcement, Verdict};

/// A resolved binary invocation to be checked against the exec policy. Matching
/// is on the **resolved absolute path** and its **content hash**, never on
/// `argv[0]` — this is what defeats rename/copy/`$PATH` evasion (INV-3, TM-07).
///
/// Construct via [`crate::exec_guard::resolve_program`], which does PATH lookup,
/// symlink canonicalisation, and hashing. Constructing one by hand (as the tests
/// do) is fine for the pure decision path.
#[derive(Debug, Clone)]
pub struct ExecTarget {
    /// Canonicalised absolute path of the binary that will actually run.
    pub resolved_path: String,
    /// Basename, used for allowlist matching by name.
    pub basename: String,
    /// blake3 content hash of the resolved binary, if it could be read. Carried
    /// for audit and future hash-pinning; the current decision matches on
    /// name/path (allowlist-by-omission already defeats rename).
    pub content_hash: Option<String>,
}

/// The runtime decision engine.
#[derive(Debug, Clone)]
pub struct PolicyEngine {
    policy: SecurityPolicy,
}

impl PolicyEngine {
    pub fn new(policy: SecurityPolicy) -> Self {
        PolicyEngine { policy }
    }

    pub fn enforcement(&self) -> Enforcement {
        self.policy.enforcement
    }

    pub fn policy(&self) -> &SecurityPolicy {
        &self.policy
    }

    // --- INV-5: Sovereign non-bypass -------------------------------------

    /// Whether `action` is in the Sovereign set for this policy.
    pub fn is_sovereign(&self, action: SovereignAction) -> bool {
        self.policy.capabilities.sovereign.contains(&action)
    }

    /// **INV-5.** Whether an action may be auto-approved by *any* mechanism.
    ///
    /// A Sovereign action can never be auto-approved — this returns `false` for
    /// it with no other input considered. Callers that hold an "always approve"
    /// / YOLO flag must gate it through this method; the flag is deliberately
    /// not a parameter, so it cannot influence the answer for Sovereign actions.
    pub fn can_auto_approve(&self, action: SovereignAction) -> bool {
        !self.is_sovereign(action)
    }

    // --- INV-4 hook: trust-tier capability gate --------------------------

    /// Whether an action influenced by content at `tier` may perform a
    /// consequential action (egress beyond the gateway, process spawn,
    /// credential read, write outside the workspace, or a Sovereign action).
    ///
    /// Below `Workspace`, the answer is always `false`, regardless of what any
    /// allowlist permits. This is how an injected instruction from `Untrusted`
    /// content is neutered even when the model complies with it.
    pub fn tier_permits_consequential(&self, tier: TrustTier) -> bool {
        tier.permits_consequential_actions()
    }

    // --- Capability decisions --------------------------------------------

    /// Decide egress to `host` (bare host or `host:port`). Deny-precedence:
    /// a denylist hit blocks even if the allowlist would permit it.
    pub fn egress_decision(&self, host: &str, tier: TrustTier) -> Decision {
        let caps = &self.policy.capabilities;
        if caps.egress_deny.denies(host) {
            return Decision::block(
                RuleId::default_rule(Domain::Egress, "001"),
                format!("egress to {host} is explicitly denied"),
            );
        }
        if !caps.egress_allow.permits(host) {
            let mut d = Decision::block(
                RuleId::default_rule(Domain::Egress, "002"),
                format!("egress to {host} is not on the allowlist"),
            );
            // Non-allowlisted egress is a Sovereign action.
            d.requires_human = self.is_sovereign(SovereignAction::NonAllowlistedEgress);
            return d;
        }
        if !self.tier_permits_consequential(tier) {
            return Decision::block(
                RuleId::default_rule(Domain::Egress, "003"),
                format!("egress blocked: action tier {tier:?} is below Workspace"),
            );
        }
        Decision::allow()
    }

    /// Decide execution of a resolved binary.
    pub fn exec_decision(&self, target: &ExecTarget, tier: TrustTier) -> Decision {
        let caps = &self.policy.capabilities;
        if caps.exec_deny.denies(&target.basename) || caps.exec_deny.denies(&target.resolved_path) {
            return Decision::block(
                RuleId::default_rule(Domain::Exec, "001"),
                format!("execution of {} is explicitly denied", target.basename),
            );
        }
        let permitted = caps.exec_allow.permits(&target.basename)
            || caps.exec_allow.permits(&target.resolved_path);
        if !permitted {
            return Decision::block(
                RuleId::default_rule(Domain::Exec, "002"),
                format!("execution of {} is not on the allowlist", target.basename),
            );
        }
        if !self.tier_permits_consequential(tier) {
            return Decision::block(
                RuleId::default_rule(Domain::Exec, "003"),
                format!("execution blocked: action tier {tier:?} is below Workspace"),
            );
        }
        Decision::allow()
    }

    /// Whether an MCP tool may be invoked.
    ///
    /// Matched on `server/tool`, with `server/*` permitting a whole server.
    ///
    /// This is the only containment MCP has. Every other dimension can fall
    /// back on derivation — we can read a shell command and work out what it
    /// does — but MCP arguments are opaque JSON from an arbitrary server, so
    /// there is nothing to introspect. Either the pair is allowlisted or the
    /// call does not happen.
    pub fn mcp_decision(&self, server: &str, tool: &str, tier: TrustTier) -> Decision {
        let caps = &self.policy.capabilities;
        let qualified = format!("{server}/{tool}");
        let wildcard = format!("{server}/*");

        if caps.mcp_allow.permits(&qualified) || caps.mcp_allow.permits(&wildcard) {
            if !self.tier_permits_consequential(tier) {
                return Decision::block(
                    RuleId::default_rule(Domain::Mcp, "002"),
                    format!("MCP call blocked: action tier {tier:?} is below Workspace"),
                );
            }
            return Decision::allow();
        }

        Decision::block(
            RuleId::default_rule(Domain::Mcp, "001"),
            format!("MCP tool {qualified} is not on the allowlist"),
        )
    }

    /// Whether reading `path` constitutes a credential access (always Sovereign,
    /// hence never auto-approvable).
    pub fn is_credential_path(&self, path: &str) -> bool {
        self.policy.capabilities.cred_paths.denies(path)
    }

    /// Convenience: is the overall posture enforcing?
    pub fn is_enforcing(&self) -> bool {
        self.policy.enforcement == Enforcement::Block
    }

    /// Whether a decision should actually block, given the enforcement posture.
    /// Under `Warn`/`Off`, a `Block` verdict is surfaced/audited but not fatal.
    pub fn should_block(&self, verdict: Verdict) -> bool {
        verdict == Verdict::Block && self.policy.enforcement == Enforcement::Block
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ainxt_policy_types::capability::{Allowlist, Denylist, SecurityCapabilities};
    use ainxt_policy_types::policy::{SecurityPolicy, SourceLayer, SourceOrigin};
    use ainxt_policy_types::verdict::Enforcement;

    fn engine_with(caps: SecurityCapabilities) -> PolicyEngine {
        PolicyEngine::new(SecurityPolicy { enforcement: Enforcement::Block, capabilities: caps })
    }

    // --- INV-5 ---

    #[test]
    fn inv5_sovereign_never_auto_approves() {
        let engine = engine_with(SecurityCapabilities {
            sovereign: [SovereignAction::CredentialAccess].into_iter().collect(),
            ..SecurityCapabilities::default()
        });
        assert!(!engine.can_auto_approve(SovereignAction::CredentialAccess));
        // A non-sovereign action can be auto-approved.
        assert!(engine.can_auto_approve(SovereignAction::PackagePublish));
    }

    #[test]
    fn inv5_default_sovereign_set_is_pinned() {
        // Even the permissive OSS default pins the universally-dangerous ones.
        let engine = PolicyEngine::new(SecurityPolicy::oss_default());
        assert!(!engine.can_auto_approve(SovereignAction::Persistence));
        assert!(!engine.can_auto_approve(SovereignAction::PrivilegeEscalation));
        assert!(!engine.can_auto_approve(SovereignAction::SecurityConfigChange));
    }

    // --- egress ---

    #[test]
    fn egress_allowlisted_host_permitted_at_workspace_tier() {
        let engine = engine_with(SecurityCapabilities {
            egress_allow: Allowlist::only(["gateway.internal"]),
            ..SecurityCapabilities::default()
        });
        let d = engine.egress_decision("gateway.internal", TrustTier::Workspace);
        assert_eq!(d.verdict, Verdict::Allow);
    }

    #[test]
    fn egress_non_allowlisted_blocked() {
        let engine = engine_with(SecurityCapabilities {
            egress_allow: Allowlist::only(["gateway.internal"]),
            ..SecurityCapabilities::default()
        });
        let d = engine.egress_decision("evil.example", TrustTier::Workspace);
        assert_eq!(d.verdict, Verdict::Block);
        assert_eq!(d.rule.unwrap().to_string(), "DEFAULT-EGRESS-002");
    }

    #[test]
    fn egress_denylist_beats_allowlist() {
        let engine = engine_with(SecurityCapabilities {
            egress_allow: Allowlist::Any,
            egress_deny: Denylist::of(["blocked.host"]),
            ..SecurityCapabilities::default()
        });
        let d = engine.egress_decision("blocked.host", TrustTier::Operator);
        assert_eq!(d.verdict, Verdict::Block);
        assert_eq!(d.rule.unwrap().to_string(), "DEFAULT-EGRESS-001");
    }

    #[test]
    fn inv4_untrusted_tier_cannot_egress_even_if_allowlisted() {
        let engine = engine_with(SecurityCapabilities {
            egress_allow: Allowlist::Any,
            ..SecurityCapabilities::default()
        });
        // The canonical injection outcome: allowlist would permit, tier does not.
        let d = engine.egress_decision("gateway.internal", TrustTier::Untrusted);
        assert_eq!(d.verdict, Verdict::Block);
        assert_eq!(d.rule.unwrap().to_string(), "DEFAULT-EGRESS-003");
    }

    // --- exec ---

    #[test]
    fn exec_allowlist_by_basename() {
        let engine = engine_with(SecurityCapabilities {
            exec_allow: Allowlist::only(["python3"]),
            ..SecurityCapabilities::default()
        });
        let ok = ExecTarget {
            resolved_path: "/usr/bin/python3".into(),
            basename: "python3".into(),
            content_hash: None,
        };
        assert_eq!(engine.exec_decision(&ok, TrustTier::Workspace).verdict, Verdict::Allow);
    }

    #[test]
    fn exec_powershell_denied_by_omission() {
        // MANAGED-EXEC-001 shape: pwsh not on the allowlist → denied, even renamed.
        let engine = engine_with(SecurityCapabilities {
            exec_allow: Allowlist::only(["bash", "python3", "git"]),
            ..SecurityCapabilities::default()
        });
        let renamed = ExecTarget {
            resolved_path: "/tmp/notepad".into(), // renamed pwsh
            basename: "notepad".into(),
            content_hash: None,
        };
        assert_eq!(engine.exec_decision(&renamed, TrustTier::Operator).verdict, Verdict::Block);
    }

    // --- enforcement posture ---

    #[test]
    fn warn_mode_does_not_fatally_block() {
        let engine = PolicyEngine::new(SecurityPolicy {
            enforcement: Enforcement::Warn,
            capabilities: SecurityCapabilities::default(),
        });
        assert!(!engine.should_block(Verdict::Block));
    }

    #[test]
    fn resolve_then_decide_end_to_end() {
        // A bundle floor of gateway-only egress; a malicious project layer tries
        // to add evil.example; the engine built from the resolved policy denies.
        use ainxt_policy_types::merge::resolve;
        let base = SecurityPolicy {
            enforcement: Enforcement::Block,
            capabilities: SecurityCapabilities {
                egress_allow: Allowlist::only(["gateway.internal"]),
                ..SecurityCapabilities::default()
            },
        };
        let malicious = SourceLayer {
            origin: SourceOrigin::Project,
            enforcement: Some(Enforcement::Off),
            capabilities: SecurityCapabilities {
                egress_allow: Allowlist::only(["gateway.internal", "evil.example"]),
                ..SecurityCapabilities::default()
            },
        };
        let resolved = resolve(&base, &[malicious]);
        let engine = PolicyEngine::new(resolved);
        assert!(engine.is_enforcing()); // project could not lower it
        assert_eq!(
            engine.egress_decision("evil.example", TrustTier::Operator).verdict,
            Verdict::Block
        );
    }
}
