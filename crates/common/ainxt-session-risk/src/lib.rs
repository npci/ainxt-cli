//! Session-scoped risk state: the part of enforcement that per-call permission
//! checks structurally cannot do.
//!
//! A credential brute force is ten thousand individually-legitimate commands.
//! Running `qpdf` to unlock a protected PDF is fine; approving it
//! is correct. Approving it ten thousand times is the incident. No
//! capability rule can see that, because every single call is benign — only
//! accumulated state can.
//!
//! Three mechanisms live here:
//!
//! - **Budgets** over a rolling window (exec rate, host fan-out, bytes out,
//!   install rate) — the generic backstop.
//! - **Failure-loop detection** — repeated invocations of the *same program*
//!   that keep *failing*. This is the brute-force signature specifically, and
//!   it is far more precise than raw repetition: a build runs `rustc` hundreds
//!   of times and succeeds, so raw repetition alone would be a false-positive
//!   machine.
//! - **Artifact provenance** — what was written or downloaded, so that later
//!   executing it can be refused. This is what stops "threat hunting" that ends
//!   with running the artifact it found.
//!
//! # Why the ledger key is not the process
//!
//! [`LedgerKey`] is `(subject, endpoint)` — a user on a machine — deliberately
//! **not** a pid or a session id. Budgets scoped to a process are evaded by
//! splitting an attack chain across two clients on the same laptop, and "run
//! half of it in the IDE extension" is not a sophisticated bypass. Any store
//! that cannot enforce the budget across processes is not doing its job; see
//! [`FileRiskStore`].

mod file;
mod ledger;
mod memory;

pub use file::FileRiskStore;
pub use ledger::{
    Artifact, ArtifactTrust, BudgetBreach, Budgets, Charge, FreezeReason, RiskState,
};
pub use memory::InProcessRiskStore;

use std::time::SystemTime;

/// Identifies whose budget is being spent.
///
/// `subject` is the gateway JWT `sub` when authenticated, else `local:<uid>`.
/// `endpoint` is a stable host identifier. Neither is a process identifier —
/// see the module docs for why that matters.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LedgerKey {
    pub subject: String,
    pub endpoint: String,
}

impl LedgerKey {
    pub fn new(subject: impl Into<String>, endpoint: impl Into<String>) -> Self {
        Self {
            subject: subject.into(),
            endpoint: endpoint.into(),
        }
    }

    /// Filesystem-safe, collision-resistant name for this key. Hashed rather
    /// than encoded so a subject containing a path separator or a very long
    /// identifier cannot escape the state directory.
    pub fn storage_id(&self) -> String {
        let mut hasher = blake3::Hasher::new();
        hasher.update(self.subject.as_bytes());
        hasher.update(b"\x00");
        hasher.update(self.endpoint.as_bytes());
        hasher.finalize().to_hex()[..32].to_string()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RiskError {
    #[error("risk ledger i/o failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("risk ledger state is corrupt: {0}")]
    Corrupt(String),
}

/// The storage boundary.
///
/// This trait is the single seam that makes a later extraction to a local
/// daemon mechanical: `RemoteRiskStore` becomes a third implementation and
/// nothing above it changes. If extracting the daemon ever requires editing a
/// caller of this trait, the boundary was drawn in the wrong place.
///
/// [`charge`](RiskStore::charge) **must be atomic** with respect to other
/// processes holding the same key.
pub trait RiskStore: Send + Sync {
    /// Record an action against the budget and return the resulting state.
    /// Atomic: read, apply, persist, return — no lost updates under concurrency.
    fn charge(&self, key: &LedgerKey, charge: &Charge) -> Result<RiskState, RiskError>;

    /// Read-only view, for `ainxt policy status`. Never an input to a decision:
    /// deciding on a peeked value would be a time-of-check/time-of-use race.
    fn peek(&self, key: &LedgerKey) -> Result<RiskState, RiskError>;

    /// Record whether an invocation succeeded. Drives failure-loop detection —
    /// a success resets the counter, a failure advances it.
    fn note_outcome(
        &self,
        key: &LedgerKey,
        program: &str,
        success: bool,
    ) -> Result<RiskState, RiskError>;

    /// Record that an artifact entered the workspace, with where it came from.
    fn note_artifact(&self, key: &LedgerKey, artifact: Artifact) -> Result<(), RiskError>;

    /// Look up an artifact by path, to decide whether executing it is safe.
    fn artifact(&self, key: &LedgerKey, path: &str) -> Result<Option<Artifact>, RiskError>;

    /// Freeze the subject. Idempotent, and survives process restart for any
    /// store that persists.
    fn freeze(&self, key: &LedgerKey, reason: FreezeReason) -> Result<(), RiskError>;

    /// Clear a freeze. Only ever called behind a human approval at the TTY —
    /// the model must never be able to reach this.
    fn clear_freeze(&self, key: &LedgerKey) -> Result<(), RiskError>;
}

pub(crate) fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
