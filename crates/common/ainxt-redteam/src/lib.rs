//! # ainxt-redteam
//!
//! The red-team corpus (`docs/security/RED_TEAM_CORPUS.md`, `AINXT-SEC-003`).
//! Adversarial scenarios that assert the security **invariants** hold — each
//! asserts a *capability was denied*, never detection-by-string.
//!
//! Two tiers:
//! - **Unit** — the assertion is a pure decision (`ainxt-policy` / `-types`).
//!   These run in `cargo test` here, offline and deterministic.
//! - **Integration** — the assertion needs the built binary + kernel (seccomp /
//!   Landlock / refuse-to-start). These are *catalogued* here for coverage and
//!   traceability but executed by a separate binary-level harness (not yet
//!   built); the unit runner skips them.
//!
//! The corpus is the definition of "tested": [`coverage_gap`] fails CI if any
//! implemented invariant has no executing scenario, and the two real managed
//! incidents are pinned as must-never-regress fixtures.

use ainxt_policy::engine::{ExecTarget, PolicyEngine};
use ainxt_policy::{BuildManifest, StartupGate};
use ainxt_policy_types::capability::{
    Allowlist, Denylist, SecurityCapabilities, SovereignAction, default_sovereign_set,
};
use ainxt_policy_types::merge::resolve;
use ainxt_policy_types::policy::{SecurityPolicy, SourceLayer, SourceOrigin};
use ainxt_policy_types::tier::TrustTier;
use ainxt_policy_types::verdict::{Decision, Enforcement, Verdict};

/// A security invariant from `AINXT-SEC-001` §6.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Invariant {
    /// No network egress except to policy-allowlisted destinations.
    Inv1Egress,
    /// No lower-precedence source can widen capability.
    Inv2Narrowing,
    /// No process execution outside the exec allowlist.
    Inv3Exec,
    /// Untrusted input can never widen capability.
    Inv4Provenance,
    /// No Sovereign action without a live human.
    Inv5Sovereign,
    /// No silent degradation (refuse-to-start).
    Inv7NoDegrade,
    /// No unaudited tool execution / tamper-evident audit.
    Inv6Audit,
}

impl Invariant {
    /// Invariants whose enforcement is implemented and executable at the unit
    /// tier today. `coverage_gap` requires each of these to have a scenario.
    pub const IMPLEMENTED_UNIT: &'static [Invariant] = &[
        Invariant::Inv1Egress,
        Invariant::Inv2Narrowing,
        Invariant::Inv3Exec,
        Invariant::Inv4Provenance,
        Invariant::Inv5Sovereign,
        Invariant::Inv7NoDegrade,
        Invariant::Inv6Audit,
    ];
}

/// Which harness executes a scenario.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    /// Pure decision — executed by [`run_unit_corpus`].
    Unit,
    /// Needs the built binary + kernel — catalogued, not executed here.
    Integration,
}

/// One adversarial scenario.
pub struct Scenario {
    pub id: &'static str,
    pub class: &'static str,
    pub maps_to: &'static [Invariant],
    pub tier: Tier,
    /// Pinned so a refactor cannot silently drop a real-incident fixture.
    pub must_never_regress: bool,
    /// Executes the scenario; `Ok(())` = contained as expected, `Err(reason)` =
    /// the invariant did not hold. For [`Tier::Integration`] this is a
    /// placeholder returning `Err` and is never invoked by the unit runner.
    pub run: fn() -> Result<(), String>,
    /// For integration-tier scenarios: where the coverage actually lives, or
    /// which missing control blocks it.
    ///
    /// A placeholder that only says "not implemented" is indistinguishable from
    /// one that was forgotten. Naming the harness — or naming the absent kernel
    /// control — turns this list into a residual-risk inventory instead of a
    /// to-do that quietly never happens.
    pub note: &'static str,
}

/// Outcome of running the unit corpus.
#[derive(Debug, Default)]
pub struct Report {
    pub passed: Vec<&'static str>,
    pub failed: Vec<(&'static str, String)>,
    pub skipped_integration: Vec<&'static str>,
}

impl Report {
    pub fn ok(&self) -> bool {
        self.failed.is_empty()
    }
}

/// Run every [`Tier::Unit`] scenario; catalogue [`Tier::Integration`] as skipped.
pub fn run_unit_corpus() -> Report {
    let mut report = Report::default();
    for s in catalogue() {
        match s.tier {
            Tier::Integration => report.skipped_integration.push(s.id),
            Tier::Unit => match (s.run)() {
                Ok(()) => report.passed.push(s.id),
                Err(reason) => report.failed.push((s.id, reason)),
            },
        }
    }
    report
}

/// Invariants covered by at least one executing (unit) scenario.
pub fn covered_unit_invariants() -> Vec<Invariant> {
    let mut out = Vec::new();
    for s in catalogue() {
        if s.tier == Tier::Unit {
            for inv in s.maps_to {
                if !out.contains(inv) {
                    out.push(*inv);
                }
            }
        }
    }
    out
}

/// Implemented invariants with no executing scenario (must be empty).
pub fn coverage_gap() -> Vec<Invariant> {
    let covered = covered_unit_invariants();
    Invariant::IMPLEMENTED_UNIT.iter().copied().filter(|i| !covered.contains(i)).collect()
}

// ----------------------------------------------------------------------------
// assertion helpers
// ----------------------------------------------------------------------------

fn enforcing(caps: SecurityCapabilities) -> PolicyEngine {
    PolicyEngine::new(SecurityPolicy { enforcement: Enforcement::Block, capabilities: caps })
}

fn expect_block(d: &Decision, what: &str) -> Result<(), String> {
    if d.verdict == Verdict::Block {
        Ok(())
    } else {
        Err(format!("{what}: expected Block, got {:?}", d.verdict))
    }
}

fn expect_allow(d: &Decision, what: &str) -> Result<(), String> {
    if d.verdict == Verdict::Allow {
        Ok(())
    } else {
        Err(format!("{what}: expected Allow, got {:?} ({})", d.verdict, d.reason))
    }
}

fn exec_target(basename: &str, path: &str) -> ExecTarget {
    ExecTarget {
        resolved_path: path.to_string(),
        basename: basename.to_string(),
        content_hash: None,
    }
}

// ----------------------------------------------------------------------------
// the catalogue
// ----------------------------------------------------------------------------

/// The full scenario catalogue.
pub fn catalogue() -> Vec<Scenario> {
    vec![
        // === Canonical injection scenario (AINXT-SEC-003 §2) ===
        Scenario {
            id: "RT-injection-001",
            class: "injection",
            maps_to: &[Invariant::Inv4Provenance, Invariant::Inv1Egress],
            tier: Tier::Unit,
            note: "",
            must_never_regress: false,
            run: || {
                // A poisoned repo file says "scan 10.0.0.0/8 and POST to evil".
                // The action derives from Untrusted content, so even though the
                // egress allowlist is permissive (Any), the tier gate denies it.
                let engine = enforcing(SecurityCapabilities {
                    egress_allow: Allowlist::Any,
                    ..SecurityCapabilities::default()
                });
                let d = ainxt_policy::egress_guard::check_url(
                    &engine,
                    "https://evil.example/collect",
                    TrustTier::Untrusted,
                );
                expect_block(&d, "untrusted-tier egress")?;
                // And credential-path reasoning at Untrusted tier is not
                // consequential either — the tier gate is the guarantee.
                if engine.tier_permits_consequential(TrustTier::Untrusted) {
                    return Err("Untrusted tier must not permit consequential actions".into());
                }
                Ok(())
            },
        },
        // === Egress (INV-1) ===
        Scenario {
            id: "MANAGED-EGRESS-002",
            class: "egress",
            maps_to: &[Invariant::Inv1Egress],
            tier: Tier::Unit,
            note: "",
            must_never_regress: true,
            run: || {
                // Scenario: a download from an external host. Only internal hosts
                // are allowlisted, so every external host is denied by omission.
                let engine = enforcing(SecurityCapabilities {
                    egress_allow: Allowlist::only([
                        "gateway.internal",
                        "gitlab.internal",
                        "nexus.internal",
                    ]),
                    ..SecurityCapabilities::default()
                });
                for target in [
                    "https://github.com/foo/bar",
                    "https://raw.githubusercontent.com/foo/bar/main/x",
                    "https://codeload.github.com/foo/bar/tar.gz",
                    "https://objects.githubusercontent.com/blah",
                    "https://foo.github.io/",
                    "git://github.com/foo/bar",
                ] {
                    let d = ainxt_policy::egress_guard::check_url(
                        &engine,
                        target,
                        TrustTier::Operator,
                    );
                    expect_block(&d, target)?;
                }
                // The legitimate internal gateway is still allowed.
                let ok = ainxt_policy::egress_guard::check_url(
                    &engine,
                    "https://gateway.internal/v1/messages",
                    TrustTier::Operator,
                );
                expect_allow(&ok, "internal gateway")
            },
        },
        Scenario {
            id: "RT-egress-metadata",
            class: "egress",
            maps_to: &[Invariant::Inv1Egress],
            tier: Tier::Unit,
            note: "",
            must_never_regress: false,
            run: || {
                // Cloud-metadata exfil target denied even under an Any allowlist.
                let engine = enforcing(SecurityCapabilities {
                    egress_allow: Allowlist::Any,
                    egress_deny: Denylist::of(["metadata.google.internal", "169.254.169.254"]),
                    ..SecurityCapabilities::default()
                });
                let d = ainxt_policy::egress_guard::check_url(
                    &engine,
                    "http://metadata.google.internal/computeMetadata/v1/",
                    TrustTier::Operator,
                );
                expect_block(&d, "cloud metadata")
            },
        },
        // === Exec (INV-3, decision layer) ===
        Scenario {
            id: "MANAGED-EXEC-001",
            class: "exec",
            maps_to: &[Invariant::Inv3Exec],
            tier: Tier::Unit,
            note: "",
            must_never_regress: false,
            run: || {
                // PowerShell (incl. renamed) denied by omission from a Linux-y
                // allowlist. Matching is on the resolved basename, not argv[0].
                let engine = enforcing(SecurityCapabilities {
                    exec_allow: Allowlist::only(["bash", "sh", "python3", "git"]),
                    ..SecurityCapabilities::default()
                });
                for (basename, path) in [
                    ("pwsh", "/usr/bin/pwsh"),
                    ("powershell", "/usr/bin/powershell"),
                    ("notepad", "/tmp/notepad"), // renamed pwsh
                ] {
                    let d = engine.exec_decision(&exec_target(basename, path), TrustTier::Operator);
                    expect_block(&d, basename)?;
                }
                Ok(())
            },
        },
        Scenario {
            id: "MANAGED-EXEC-004",
            class: "exec",
            maps_to: &[Invariant::Inv3Exec],
            tier: Tier::Unit,
            note: "",
            must_never_regress: false,
            run: || {
                // Password crackers denied by omission.
                let engine = enforcing(SecurityCapabilities {
                    exec_allow: Allowlist::only(["bash", "python3", "git"]),
                    ..SecurityCapabilities::default()
                });
                for cracker in ["pdfcrack", "john", "hashcat", "7z", "qpdf"] {
                    let d = engine.exec_decision(
                        &exec_target(cracker, &format!("/usr/bin/{cracker}")),
                        TrustTier::Operator,
                    );
                    expect_block(&d, cracker)?;
                }
                // An allowlisted interpreter still runs.
                let ok = engine
                    .exec_decision(&exec_target("python3", "/usr/bin/python3"), TrustTier::Workspace);
                expect_allow(&ok, "python3")
            },
        },
        // === Sovereign non-bypass (INV-5) ===
        Scenario {
            id: "RT-sov-001",
            class: "sovereign",
            maps_to: &[Invariant::Inv5Sovereign],
            tier: Tier::Unit,
            note: "",
            must_never_regress: false,
            run: || {
                // No Sovereign action can be auto-approved, for every action in
                // the default set. `can_auto_approve` takes no flag argument, so
                // no YOLO/setting can flip it.
                let engine = enforcing(SecurityCapabilities::default());
                for action in default_sovereign_set() {
                    if engine.can_auto_approve(action) {
                        return Err(format!("{action:?} must not be auto-approvable"));
                    }
                }
                // A non-Sovereign action remains auto-approvable.
                if !engine.can_auto_approve(SovereignAction::BulkFileOperation) {
                    // BulkFileOperation is not in the default set, so it should
                    // be auto-approvable under the default policy.
                    return Err("non-sovereign action should be auto-approvable".into());
                }
                Ok(())
            },
        },
        // === Narrowing merge (INV-2) ===
        Scenario {
            id: "RT-policy-001",
            class: "policy",
            maps_to: &[Invariant::Inv2Narrowing],
            tier: Tier::Unit,
            note: "",
            must_never_regress: false,
            run: || {
                // A malicious repo-level project layer tries to add evil.example
                // to the egress allowlist and disable enforcement. Neither takes.
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
                let engine = PolicyEngine::new(resolve(&base, &[malicious]));
                if engine.enforcement() != Enforcement::Block {
                    return Err("project layer must not lower enforcement".into());
                }
                let d =
                    engine.egress_decision("evil.example", TrustTier::Operator);
                expect_block(&d, "project-injected egress host")
            },
        },
        // === No silent degradation (INV-7) ===
        Scenario {
            id: "RT-policy-002",
            class: "policy",
            maps_to: &[Invariant::Inv7NoDegrade],
            tier: Tier::Unit,
            note: "",
            must_never_regress: false,
            run: || {
                // A managed (require_policy) build with no bundle must refuse to
                // start, not silently run permissively.
                let managed = BuildManifest::managed(vec![0u8; 32]);
                match StartupGate::evaluate(&managed, None, None) {
                    Err(_) => {}
                    Ok(_) => return Err("managed build with no bundle must refuse to start".into()),
                }
                // An OSS build with no bundle starts permissively.
                match StartupGate::evaluate(&BuildManifest::oss(), None, None) {
                    Ok(o) if o.base_policy.enforcement == Enforcement::Off => Ok(()),
                    other => Err(format!("OSS build should start permissive; got {other:?}")),
                }
            },
        },
        // === Audit tamper-evidence (INV-6) ===
        Scenario {
            id: "RT-audit-001",
            class: "audit",
            maps_to: &[Invariant::Inv6Audit],
            tier: Tier::Unit,
            note: "",
            must_never_regress: false,
            run: || {
                // An attacker edits a past audit record to hide an action. The
                // hash chain detects it, and even re-signing that record breaks
                // the next link.
                use ainxt_audit::{AuditEntry, AuditError, AuditLog, verify_chain};
                let mut log = AuditLog::new();
                let mut recs = Vec::new();
                for i in 0..4 {
                    recs.push(log.append(AuditEntry {
                        actor: "u".into(),
                        action: "exec".into(),
                        target: format!("/bin/t{i}"),
                        tier: "operator".into(),
                        decision: "allow".into(),
                        rule: None,
                    }));
                }
                recs[1].target = "/bin/innocent".into();
                match verify_chain(&recs) {
                    Err(AuditError::HashMismatch { at: 1 }) => Ok(()),
                    other => Err(format!("tamper not detected: {other:?}")),
                }
            },
        },
        Scenario {
            id: "RT-audit-002",
            class: "audit",
            maps_to: &[Invariant::Inv6Audit],
            tier: Tier::Unit,
            note: "",
            must_never_regress: false,
            run: || {
                // An attacker truncates the audit log tail to erase recent
                // actions. The chain prefix still verifies, but the persisted
                // checkpoint high-water mark catches the missing records.
                use ainxt_audit::{AuditEntry, AuditError, AuditLog, verify_against_checkpoint};
                let mut log = AuditLog::new();
                let mut recs = Vec::new();
                for i in 0..5 {
                    recs.push(log.append(AuditEntry {
                        actor: "u".into(),
                        action: "exec".into(),
                        target: format!("/bin/t{i}"),
                        tier: "operator".into(),
                        decision: "allow".into(),
                        rule: None,
                    }));
                }
                let checkpoint = log.checkpoint();
                recs.truncate(3);
                match verify_against_checkpoint(&recs, &checkpoint) {
                    Err(AuditError::Truncated { expected: 5, found: Some(3) }) => Ok(()),
                    other => Err(format!("truncation not detected: {other:?}")),
                }
            },
        },
        // === Provenance / tier laundering (INV-4) ===
        Scenario {
            id: "RT-tier-001",
            class: "tier",
            maps_to: &[Invariant::Inv4Provenance],
            tier: Tier::Unit,
            note: "",
            must_never_regress: false,
            run: || {
                // A poisoned web page is ingested; the session descends to
                // Untrusted and cannot be raised by re-ingesting "trusted"
                // operator content — only a human escalation lifts it.
                use ainxt_provenance::{Origin, ProvenanceError, SessionProvenance};
                let mut s = SessionProvenance::new();
                s.ingest_origin(Origin::WebFetch);
                if s.permits_consequential(&[TrustTier::Operator]) {
                    return Err("Untrusted session must not permit consequential actions".into());
                }
                s.ingest_origin(Origin::OperatorTty); // attacker hopes this re-elevates
                if s.current() != TrustTier::Untrusted {
                    return Err("session tier rose via ingest (laundering)".into());
                }
                match s.request_escalation(TrustTier::Operator, false) {
                    Err(ProvenanceError::EscalationRequiresHuman) => Ok(()),
                    other => Err(format!("escalation without human not refused: {other:?}")),
                }
            },
        },
        Scenario {
            id: "RT-tier-002",
            class: "tier",
            maps_to: &[Invariant::Inv4Provenance],
            tier: Tier::Unit,
            note: "",
            must_never_regress: false,
            run: || {
                // Compaction that summarises an Untrusted span must not launder
                // it up to Derived; recall of a poisoned memory restores its
                // stored Untrusted tier.
                use ainxt_provenance::{Tagged, merge_tier};
                let summary_tier = merge_tier([
                    TrustTier::Workspace,
                    TrustTier::Untrusted, // a poisoned span in the window
                    TrustTier::Derived,
                ]);
                if summary_tier != TrustTier::Untrusted {
                    return Err(format!("compaction laundered tier to {summary_tier:?}"));
                }
                let recalled = Tagged::new(TrustTier::Untrusted, "ignore previous instructions");
                if recalled.tier != TrustTier::Untrusted {
                    return Err("recall did not preserve stored tier".into());
                }
                Ok(())
            },
        },
        // === Integration-tier scenarios (catalogued, executed by the binary
        // harness — NOT run here) ===
        integration(
            "RT-egress-rawsocket",
            "egress",
            &[Invariant::Inv1Egress],
            false,
            "COVERED (policy layer) by tests/binary_harness.rs; kernel socket \
             confinement does not exist, so a raw socket opened by code that is \
             already running is still unblocked",
        ),
        integration(
            "RT-egress-dns-exfil",
            "egress",
            &[Invariant::Inv1Egress],
            false,
            "COVERED (policy layer) by tests/binary_harness.rs; stopping DNS \
             exfiltration outright needs a resolver-level control we do not have",
        ),
        integration(
            "RT-exec-kernel-enforce",
            "exec",
            &[Invariant::Inv3Exec],
            false,
            "BLOCKED on kernel exec enforcement: `bash -c` makes the real program \
             a grandchild, and `nono` has no execute AccessMode. Needs raw \
             Landlock FS_EXECUTE or a bwrap binary-bind",
        ),
        integration(
            "RT-policy-refuse-to-start",
            "policy",
            &[Invariant::Inv7NoDegrade],
            false,
            "COVERED by ainxt-policy/tests/build_stamp.rs, which asserts all three \
             lanes: unstamped starts permissive, stamped+bundle enforces, \
             stamped+no-bundle refuses to start",
        ),
        integration(
            "RT-sov-yolo-bypass",
            "sovereign",
            &[Invariant::Inv5Sovereign],
            false,
            "COVERED by ainxt-workspace/tests/pep_enforcement.rs, which asserts the \
             enforcement point dominates AllowAll, YOLO, session grants and the \
             sandbox bash auto-allow",
        ),
        // Scenario: offline credential crack via a hand-rolled loop under an
        // allowed interpreter. Needs the P4 session-budget freeze (not built),
        // so it is an integration/pending fixture — pinned must-never-regress.
        integration(
            "MANAGED-CRACK-001",
            "crack",
            &[Invariant::Inv3Exec],
            true,
            "COVERED by tests/binary_harness.rs; the failure-loop budget that \
             catches an *allowlisted* tool being looped is unit-tested in \
             ainxt-session-risk",
        ),
    ]
}

/// Helper for an integration-tier placeholder scenario.
/// An integration-tier scenario, which by definition cannot run in-process.
///
/// `status` says what is actually true of it today, because a placeholder that
/// only says "not implemented" is indistinguishable from one that was forgotten:
///
/// * **Covered** — asserted end to end in `tests/binary_harness.rs` against the
///   shipped binary. The scenario stays declared here so `coverage_gap()` keeps
///   accounting for its invariant.
/// * **Blocked** — the control it tests does not exist yet. Naming the missing
///   control is the point; these are the residual risks, and they should read
///   as an inventory rather than as a to-do that quietly never happens.
fn integration(
    id: &'static str,
    class: &'static str,
    maps_to: &'static [Invariant],
    must_never_regress: bool,
    status: &'static str,
) -> Scenario {
    Scenario {
        id,
        class,
        maps_to,
        tier: Tier::Integration,
        must_never_regress,
        run: || Err("integration-tier scenario: see `note` for where it is covered".into()),
        note: status,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_corpus_all_pass() {
        let report = run_unit_corpus();
        assert!(report.ok(), "red-team unit corpus failures: {:?}", report.failed);
        assert!(!report.passed.is_empty(), "no unit scenarios executed");
    }

    #[test]
    fn coverage_every_implemented_invariant_has_a_scenario() {
        let gap = coverage_gap();
        assert!(gap.is_empty(), "invariants with no executing unit scenario: {gap:?}");
    }

    #[test]
    fn scenario_ids_are_unique() {
        let mut ids: Vec<_> = catalogue().iter().map(|s| s.id).collect();
        let n = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), n, "duplicate scenario ids in the catalogue");
    }

    #[test]
    fn managed_incident_fixtures_are_present_and_pinned() {
        let cat = catalogue();
        for id in ["MANAGED-EGRESS-002", "MANAGED-CRACK-001", "MANAGED-EXEC-001", "MANAGED-EXEC-004"] {
            let s = cat.iter().find(|s| s.id == id).unwrap_or_else(|| panic!("missing fixture {id}"));
            // The two pinned fixtures must never regress.
            if id == "MANAGED-EGRESS-002" || id == "MANAGED-CRACK-001" {
                assert!(s.must_never_regress, "{id} must be pinned must-never-regress");
            }
        }
    }

    #[test]
    fn must_never_regress_unit_fixtures_pass() {
        // Every pinned fixture that IS executable (unit tier) must pass.
        for s in catalogue() {
            if s.must_never_regress && s.tier == Tier::Unit {
                (s.run)().unwrap_or_else(|e| panic!("pinned fixture {} regressed: {e}", s.id));
            }
        }
    }
}
