//! In-process risk store: a single process, no persistence.
//!
//! Correct for tests, for the OSS build, and for any deployment that is not
//! enforcing. **Not** correct for enforcement, because its budget dies with the
//! process and is not shared with a second client on the same machine — see
//! [`FileRiskStore`](crate::FileRiskStore).

use std::collections::HashMap;
use std::sync::Mutex;

use crate::ledger::{Artifact, Budgets, Charge, FreezeReason, Ledger, RiskState};
use crate::{LedgerKey, RiskError, RiskStore};

#[derive(Debug, Default)]
pub struct InProcessRiskStore {
    budgets: Budgets,
    ledgers: Mutex<HashMap<LedgerKey, Ledger>>,
}

impl InProcessRiskStore {
    pub fn new(budgets: Budgets) -> Self {
        Self {
            budgets,
            ledgers: Mutex::new(HashMap::new()),
        }
    }

    fn with_ledger<T>(&self, key: &LedgerKey, f: impl FnOnce(&mut Ledger) -> T) -> T {
        // A poisoned lock means another thread panicked mid-update. The ledger
        // is still structurally valid, and refusing to serve would turn one
        // panic into a permanent denial, so recover rather than propagate.
        let mut guard = self
            .ledgers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        f(guard.entry(key.clone()).or_default())
    }
}

impl RiskStore for InProcessRiskStore {
    fn charge(&self, key: &LedgerKey, charge: &Charge) -> Result<RiskState, RiskError> {
        Ok(self.with_ledger(key, |l| l.charge(charge, &self.budgets)))
    }

    fn peek(&self, key: &LedgerKey) -> Result<RiskState, RiskError> {
        Ok(self.with_ledger(key, |l| l.peek(&self.budgets)))
    }

    fn note_outcome(
        &self,
        key: &LedgerKey,
        program: &str,
        success: bool,
    ) -> Result<RiskState, RiskError> {
        Ok(self.with_ledger(key, |l| l.note_outcome(program, success, &self.budgets)))
    }

    fn note_artifact(&self, key: &LedgerKey, artifact: Artifact) -> Result<(), RiskError> {
        self.with_ledger(key, |l| l.note_artifact(artifact));
        Ok(())
    }

    fn artifact(&self, key: &LedgerKey, path: &str) -> Result<Option<Artifact>, RiskError> {
        Ok(self.with_ledger(key, |l| l.artifact(path)))
    }

    fn freeze(&self, key: &LedgerKey, reason: FreezeReason) -> Result<(), RiskError> {
        self.with_ledger(key, |l| l.freeze(reason));
        Ok(())
    }

    fn clear_freeze(&self, key: &LedgerKey) -> Result<(), RiskError> {
        self.with_ledger(key, |l| l.clear_freeze());
        Ok(())
    }
}
