//! Proves the property that justifies [`FileRiskStore`] existing at all: two
//! separate processes charging the same subject share one budget.
//!
//! Without this, an attacker splits a chain across a terminal session and an
//! IDE extension host and each half stays under the limit. "Use two clients"
//! is not a sophisticated bypass, so it has to be closed by construction rather
//! than by convention.
//!
//! The child process is this same test binary, re-executed with an env var and
//! a `--exact` filter. That avoids shipping a fixture binary in the crate just
//! to have something to spawn.

use std::process::Command;

use ainxt_session_risk::{Budgets, Charge, FileRiskStore, LedgerKey, RiskStore};

const PROBE_ENV: &str = "AINXT_RISK_PROBE_SPEC";
const CHARGES_PER_CHILD: u32 = 100;
const CHILDREN: u32 = 2;

/// Acts as the spawned child when [`PROBE_ENV`] is set; a no-op in a normal run.
#[test]
fn probe_child() {
    let Ok(spec) = std::env::var(PROBE_ENV) else {
        return;
    };
    let parts: Vec<&str> = spec.split('|').collect();
    let (dir, subject, endpoint) = match parts.as_slice() {
        [d, s, e] => (*d, *s, *e),
        _ => panic!("malformed probe spec: {spec}"),
    };

    let store = FileRiskStore::new(dir, Budgets::default()).expect("open store");
    let key = LedgerKey::new(subject, endpoint);
    for _ in 0..CHARGES_PER_CHILD {
        store
            .charge(&key, &Charge::for_program("probe"))
            .expect("charge");
    }
}

#[test]
fn two_processes_share_one_budget() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dir_str = dir.path().to_str().expect("utf-8 tempdir path").to_owned();
    let exe = std::env::current_exe().expect("current exe");

    let children: Vec<_> = (0..CHILDREN)
        .map(|_| {
            Command::new(&exe)
                .args(["--exact", "probe_child", "--nocapture"])
                .env(PROBE_ENV, format!("{dir_str}|subject-1|endpoint-1"))
                .spawn()
                .expect("spawn probe child")
        })
        .collect();

    for mut child in children {
        let status = child.wait().expect("wait for probe child");
        assert!(status.success(), "probe child failed: {status:?}");
    }

    let store = FileRiskStore::new(dir.path(), Budgets::default()).expect("open store");
    let state = store
        .peek(&LedgerKey::new("subject-1", "endpoint-1"))
        .expect("peek");

    // Exactly the sum, not "at least". A lost update under concurrency would
    // land below this, and undercounting is precisely the failure an attacker
    // wants — so the assertion is deliberately exact.
    assert_eq!(
        state.execs_in_window,
        CHARGES_PER_CHILD * CHILDREN,
        "budget lost updates across processes"
    );
}

#[test]
fn a_freeze_written_by_one_process_is_visible_to_another() {
    let dir = tempfile::tempdir().expect("tempdir");
    let key = LedgerKey::new("subject-2", "endpoint-1");

    let budgets = Budgets {
        max_consecutive_failures: 3,
        ..Budgets::default()
    };

    // Stand in for two clients by opening the same directory twice.
    let writer = FileRiskStore::new(dir.path(), budgets.clone()).expect("writer");
    let reader = FileRiskStore::new(dir.path(), budgets).expect("reader");

    for _ in 0..4 {
        writer.note_outcome(&key, "qpdf", false).expect("outcome");
    }

    let seen = reader.peek(&key).expect("peek");
    assert!(
        seen.is_frozen(),
        "freeze did not cross the process boundary: {seen:?}"
    );
}
