//! Process startup: install the audit chain and the enforcement point.
//!
//! Runs immediately after `ainxt_policy::bootstrap::initialize`, and
//! deliberately *not* later. Until the sink is installed the platform generates
//! no evidence at all, so every decision before this call is unrecorded — which
//! for a regulated deployment is the difference between an investigable
//! incident and an unfalsifiable story.
//!
//! Nothing here is async: a file sink and a file-backed ledger need no runtime,
//! so this can sit on the pre-tokio path alongside the policy gate.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use ainxt_audit::{Checkpoint, FileAuditSink};
use ainxt_session_risk::{Budgets, FileRiskStore};

use crate::Pep;

/// What happened to the existing audit chain at startup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuditStart {
    /// No prior chain; a new one begins at genesis.
    Fresh,
    /// An intact chain was found and extended.
    Resumed { records: usize },
    /// A chain was found but does not verify.
    ///
    /// Treated as an incident to report, not as a reason to refuse to start:
    /// the log lives in a user-writable directory, so failing closed here would
    /// hand any local user a trivial denial of service. A new chain is started
    /// and the break is surfaced loudly so the gateway can raise it.
    Broken { detail: String, records: usize },
}

/// Where the decision log lives.
pub fn audit_path(ainxt_home: &Path) -> PathBuf {
    ainxt_home.join("audit").join("decisions.jsonl")
}

/// Where the risk ledger lives. Under `policy/` alongside the anti-rollback
/// counter, since both are policy state rather than user data.
pub fn risk_dir(ainxt_home: &Path) -> PathBuf {
    ainxt_home.join("policy").join("risk")
}

/// Install the audit sink, resuming the existing hash chain when it verifies.
pub fn install_audit(ainxt_home: &Path) -> AuditStart {
    let path = audit_path(ainxt_home);
    if let Some(parent) = path.parent()
        && let Err(err) = std::fs::create_dir_all(parent)
    {
        return AuditStart::Broken {
            detail: format!("could not create the audit directory: {err}"),
            records: 0,
        };
    }

    let sink = FileAuditSink::new(&path);
    let records = match sink.load() {
        Ok(records) => records,
        Err(err) => {
            ainxt_audit::global::install(Box::new(FileAuditSink::new(&path)));
            return AuditStart::Broken {
                detail: format!("existing audit log could not be read: {err}"),
                records: 0,
            };
        }
    };

    if records.is_empty() {
        ainxt_audit::global::install(Box::new(sink));
        return AuditStart::Fresh;
    }

    // Verify before extending. Appending to a tampered chain would launder the
    // tampering: every later record would verify against a forged predecessor.
    if let Err(err) = ainxt_audit::verify_chain(&records) {
        ainxt_audit::global::install(Box::new(FileAuditSink::new(&path)));
        return AuditStart::Broken {
            detail: format!("audit chain does not verify: {err}"),
            records: records.len(),
        };
    }

    let Some(last) = records.last() else {
        ainxt_audit::global::install(Box::new(sink));
        return AuditStart::Fresh;
    };
    let checkpoint = Checkpoint {
        last_seq: last.seq,
        last_hash: last.this_hash.clone(),
    };
    ainxt_audit::global::install_resumed(Box::new(sink), &checkpoint);
    AuditStart::Resumed {
        records: records.len(),
    }
}

/// Install the process enforcement point over a file-backed risk ledger.
///
/// The file store rather than the in-process one is the whole point: budgets
/// have to be shared with any other client running as this user, or splitting
/// an attack across two clients resets them.
pub fn install_pep(ainxt_home: &Path, budgets: Budgets) -> Result<(), String> {
    let dir = risk_dir(ainxt_home);
    let store = FileRiskStore::new(&dir, budgets)
        .map_err(|e| format!("could not open the risk ledger at {}: {e}", dir.display()))?;
    crate::global::install(Pep::new(Arc::new(store), endpoint_id()));
    Ok(())
}

/// The `(bundle, overlay)` versions this host has accepted.
///
/// Recorded once at startup from the `StartupOutcome` rather than re-read from
/// disk: attestation runs on every outbound request, and two file reads per
/// HTTP call would be a needless cost on the hot path. `(0, 0)` means no signed
/// policy at all, which is what an unmanaged build reports.
static BUNDLE_VERSION: AtomicU64 = AtomicU64::new(0);
static OVERLAY_VERSION: AtomicU64 = AtomicU64::new(0);

pub fn record_accepted_versions(bundle: Option<u64>, overlay: Option<u64>) {
    BUNDLE_VERSION.store(bundle.unwrap_or(0), Ordering::Relaxed);
    OVERLAY_VERSION.store(overlay.unwrap_or(0), Ordering::Relaxed);
}

pub fn accepted_versions() -> (u64, u64) {
    (
        BUNDLE_VERSION.load(Ordering::Relaxed),
        OVERLAY_VERSION.load(Ordering::Relaxed),
    )
}

/// Stable per-machine identifier for the risk ledger key.
///
/// Only needs to be stable across processes on one host; it is not an identity
/// claim and nothing trusts it.
pub fn endpoint_id() -> String {
    gethostname::gethostname()
        .into_string()
        .unwrap_or_else(|_| "unknown-endpoint".to_owned())
}

#[cfg(test)]
mod tests {
    use ainxt_audit::AuditEntry;

    use super::*;

    fn record(n: u64) {
        ainxt_audit::global::record(AuditEntry {
            actor: "subject".to_owned(),
            action: "tool:bash".to_owned(),
            target: format!("cmd-{n}"),
            tier: "operator".to_owned(),
            decision: "permit".to_owned(),
            rule: None,
        });
    }

    #[test]
    fn a_missing_log_starts_a_fresh_chain() {
        let _guard = crate::tests::lock();
        ainxt_audit::global::reset_for_test();
        let home = tempfile::tempdir().expect("tempdir");
        assert_eq!(install_audit(home.path()), AuditStart::Fresh);
    }

    /// **Gate for step 5.** The hash chain is only evidence if it is continuous
    /// across restarts. A sink that silently began a new chain on every launch
    /// would still produce records, and they would still individually verify —
    /// while losing exactly the property that makes deletion detectable.
    #[test]
    fn the_chain_verifies_across_a_restart() {
        let _guard = crate::tests::lock();
        ainxt_audit::global::reset_for_test();
        let home = tempfile::tempdir().expect("tempdir");

        assert_eq!(install_audit(home.path()), AuditStart::Fresh);
        for n in 0..3 {
            record(n);
        }

        // Simulate a process restart against the same home.
        ainxt_audit::global::reset_for_test();
        match install_audit(home.path()) {
            AuditStart::Resumed { records } => assert_eq!(records, 3),
            other => panic!("expected to resume the existing chain, got {other:?}"),
        }
        for n in 3..6 {
            record(n);
        }

        let all = FileAuditSink::new(audit_path(home.path()))
            .load()
            .expect("load");
        assert_eq!(all.len(), 6, "records were lost across the restart");
        ainxt_audit::verify_chain(&all)
            .expect("the chain must verify as one sequence across the restart");
        assert_eq!(all.last().expect("last").seq, 6, "sequence did not continue");
    }

    #[test]
    fn a_tampered_chain_is_reported_rather_than_extended() {
        let _guard = crate::tests::lock();
        ainxt_audit::global::reset_for_test();
        let home = tempfile::tempdir().expect("tempdir");
        let path = audit_path(home.path());
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        // Well-formed JSON, wrong hashes: the shape parses, the chain does not
        // verify. Silently appending here is the failure this guards against.
        std::fs::write(
            &path,
            r#"{"seq":1,"timestamp":"2026-01-01T00:00:00Z","actor":"a","action":"exec","target":"t","tier":"operator","decision":"permit","prev_hash":"0","this_hash":"deadbeef"}
"#,
        )
        .expect("write");

        match install_audit(home.path()) {
            AuditStart::Broken { records, .. } => assert_eq!(records, 1),
            other => panic!("expected a broken chain, got {other:?}"),
        }
    }
}
