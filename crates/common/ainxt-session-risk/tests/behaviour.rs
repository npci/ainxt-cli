//! Behavioural contract for the risk ledger, exercised through the public API
//! so both store implementations are held to the same rules.

use std::collections::BTreeSet;

use ainxt_intent::Capability;
use ainxt_session_risk::{
    Artifact, ArtifactTrust, BudgetBreach, Budgets, Charge, FileRiskStore, InProcessRiskStore,
    LedgerKey, RiskStore,
};

fn key() -> LedgerKey {
    LedgerKey::new("subject", "endpoint")
}

fn stores(budgets: Budgets) -> Vec<(&'static str, Box<dyn RiskStore>, tempfile::TempDir)> {
    let dir_a = tempfile::tempdir().expect("tempdir");
    let dir_b = tempfile::tempdir().expect("tempdir");
    vec![
        (
            "in-process",
            Box::new(InProcessRiskStore::new(budgets.clone())) as Box<dyn RiskStore>,
            dir_a,
        ),
        (
            "file",
            Box::new(FileRiskStore::new(dir_b.path(), budgets).expect("file store")),
            dir_b,
        ),
    ]
}

/// The PDF brute force from the incident report. Each call is legitimate; the
/// repetition-with-failure is the finding.
#[test]
fn a_failing_loop_freezes_the_subject() {
    let budgets = Budgets {
        max_consecutive_failures: 12,
        ..Budgets::default()
    };
    for (name, store, _dir) in stores(budgets) {
        let mut frozen_at = None;
        for attempt in 1..=20 {
            let state = store
                .note_outcome(&key(), "qpdf", false)
                .expect("note outcome");
            if state.is_frozen() && frozen_at.is_none() {
                frozen_at = Some(attempt);
            }
        }
        assert_eq!(frozen_at, Some(13), "{name}: froze at the wrong attempt");

        let state = store.peek(&key()).expect("peek");
        let reason = state.frozen.expect("frozen");
        assert!(
            matches!(reason.breach, BudgetBreach::ConsecutiveFailures { .. }),
            "{name}: wrong breach recorded: {:?}",
            reason.breach
        );
    }
}

/// The false-positive guard. A build runs one compiler hundreds of times and
/// succeeds; if repetition alone froze sessions the control would be turned off
/// within a day, so success must reset the counter.
#[test]
fn repeated_success_never_freezes() {
    let budgets = Budgets {
        max_consecutive_failures: 12,
        max_execs: 10_000,
        max_repeats_same_program: 10_000,
        ..Budgets::default()
    };
    for (name, store, _dir) in stores(budgets) {
        for _ in 0..500 {
            store.charge(&key(), &Charge::for_program("rustc")).expect("charge");
            store.note_outcome(&key(), "rustc", true).expect("outcome");
        }
        assert!(!store.peek(&key()).expect("peek").is_frozen(), "{name}");
    }
}

/// Intermittent failure in an otherwise-working loop is normal engineering, not
/// an attack: only an unbroken run of failures counts.
#[test]
fn an_intervening_success_resets_the_failure_counter() {
    let budgets = Budgets {
        max_consecutive_failures: 3,
        ..Budgets::default()
    };
    for (name, store, _dir) in stores(budgets) {
        for _ in 0..30 {
            store.note_outcome(&key(), "pytest", false).expect("fail");
            store.note_outcome(&key(), "pytest", false).expect("fail");
            store.note_outcome(&key(), "pytest", true).expect("pass");
        }
        assert!(!store.peek(&key()).expect("peek").is_frozen(), "{name}");
    }
}

#[test]
fn host_fan_out_is_budgeted() {
    let budgets = Budgets {
        max_distinct_hosts: 5,
        ..Budgets::default()
    };
    for (name, store, _dir) in stores(budgets) {
        for i in 0..10 {
            let charge = Charge {
                hosts: vec![format!("host-{i}.example.com")],
                ..Charge::for_program("curl")
            };
            store.charge(&key(), &charge).expect("charge");
        }
        let state = store.peek(&key()).expect("peek");
        assert!(state.is_frozen(), "{name}: fan-out did not freeze");
    }
}

#[test]
fn install_rate_is_budgeted() {
    let budgets = Budgets {
        max_installs: 3,
        ..Budgets::default()
    };
    for (name, store, _dir) in stores(budgets) {
        let mut caps = BTreeSet::new();
        caps.insert(Capability::InstallPackage);
        for _ in 0..5 {
            let charge = Charge {
                capabilities: caps.clone(),
                ..Charge::for_program("pip")
            };
            store.charge(&key(), &charge).expect("charge");
        }
        assert!(store.peek(&key()).expect("peek").is_frozen(), "{name}");
    }
}

/// The first breach is the true cause; a later one must not overwrite it, or
/// the incident report names the wrong thing.
#[test]
fn freeze_is_idempotent_and_keeps_the_original_cause() {
    let budgets = Budgets {
        max_consecutive_failures: 2,
        max_distinct_hosts: 1,
        ..Budgets::default()
    };
    for (name, store, _dir) in stores(budgets) {
        for _ in 0..3 {
            store.note_outcome(&key(), "qpdf", false).expect("outcome");
        }
        for i in 0..5 {
            let charge = Charge {
                hosts: vec![format!("h{i}.example.com")],
                ..Charge::for_program("curl")
            };
            store.charge(&key(), &charge).expect("charge");
        }
        let reason = store.peek(&key()).expect("peek").frozen.expect("frozen");
        assert!(
            matches!(reason.breach, BudgetBreach::ConsecutiveFailures { .. }),
            "{name}: original cause was overwritten by {:?}",
            reason.breach
        );
    }
}

#[test]
fn clear_freeze_restores_service() {
    let budgets = Budgets {
        max_consecutive_failures: 1,
        ..Budgets::default()
    };
    for (name, store, _dir) in stores(budgets) {
        store.note_outcome(&key(), "x", false).expect("outcome");
        store.note_outcome(&key(), "x", false).expect("outcome");
        assert!(store.peek(&key()).expect("peek").is_frozen(), "{name}");

        store.clear_freeze(&key()).expect("clear");
        assert!(!store.peek(&key()).expect("peek").is_frozen(), "{name}");
    }
}

/// The link that stops "threat hunting" ending in executing what it found.
#[test]
fn artifacts_round_trip_with_their_provenance() {
    for (name, store, _dir) in stores(Budgets::default()) {
        store
            .note_artifact(
                &key(),
                Artifact {
                    path: "/tmp/setup.sh".to_owned(),
                    trust: ArtifactTrust::Untrusted,
                    origin: "downloaded from https://example.com/setup.sh".to_owned(),
                    at: 0,
                },
            )
            .expect("note artifact");

        let found = store
            .artifact(&key(), "/tmp/setup.sh")
            .expect("lookup")
            .expect("present");
        assert_eq!(found.trust, ArtifactTrust::Untrusted, "{name}");
        assert!(found.origin.contains("example.com"), "{name}");

        assert!(
            store.artifact(&key(), "/tmp/other.sh").expect("lookup").is_none(),
            "{name}"
        );
    }
}

/// Overwriting the ledger with garbage must not read as "budget spent: zero".
#[test]
fn a_corrupt_ledger_fails_closed_rather_than_resetting() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = FileRiskStore::new(dir.path(), Budgets::default()).expect("store");
    let k = key();

    store.charge(&k, &Charge::for_program("x")).expect("charge");

    let path = dir.path().join(format!("{}.json", k.storage_id()));
    std::fs::write(&path, b"{ not json").expect("corrupt the ledger");

    assert!(
        store.charge(&k, &Charge::for_program("x")).is_err(),
        "corrupt ledger silently reset the budget"
    );
}

#[test]
fn ledger_keys_are_isolated_from_each_other() {
    for (name, store, _dir) in stores(Budgets::default()) {
        let a = LedgerKey::new("alice", "host");
        let b = LedgerKey::new("bob", "host");
        for _ in 0..5 {
            store.charge(&a, &Charge::for_program("x")).expect("charge");
        }
        assert_eq!(store.peek(&a).expect("peek").execs_in_window, 5, "{name}");
        assert_eq!(store.peek(&b).expect("peek").execs_in_window, 0, "{name}");
    }
}
