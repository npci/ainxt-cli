//! File-backed risk store: one budget per `(subject, endpoint)`, shared across
//! every process on the machine.
//!
//! This is the store that closes the split-across-clients evasion. Two `ainxt`
//! processes — a terminal session and an IDE extension host — running as the
//! same user hold the same ledger, so an attack chain split between them still
//! spends one budget. It costs an advisory file lock, not a daemon.
//!
//! # What this does not defend against
//!
//! The ledger lives under the user's own state directory, so a determined local
//! user can delete it and start a fresh budget. That is a real limitation and
//! is stated rather than papered over: the fix is a daemon running as a
//! different uid, which is deliberately out of scope for now. Two things make
//! the gap narrower than it looks — a *corrupt* ledger fails closed rather than
//! resetting (so tampering is not a silent reset), and every decision is
//! written to the hash-chained audit log, so a reset is visible after the fact
//! even when it cannot be prevented.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use fs2::FileExt;

use crate::ledger::{Artifact, Budgets, Charge, FreezeReason, Ledger, RiskState};
use crate::{LedgerKey, RiskError, RiskStore};

#[derive(Debug)]
pub struct FileRiskStore {
    dir: PathBuf,
    budgets: Budgets,
}

impl FileRiskStore {
    /// `dir` is created if absent. Typically `$AINXT_HOME/policy/risk`.
    pub fn new(dir: impl AsRef<Path>, budgets: Budgets) -> Result<Self, RiskError> {
        let dir = dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&dir)?;
        Ok(Self { dir, budgets })
    }

    fn path_for(&self, key: &LedgerKey) -> PathBuf {
        self.dir.join(format!("{}.json", key.storage_id()))
    }

    /// Read-modify-write under an exclusive advisory lock, then fsync.
    ///
    /// The lock is held across the whole read → mutate → write cycle, which is
    /// what makes [`RiskStore::charge`] atomic between processes. Without it,
    /// two clients charging concurrently would each read the same count and
    /// write back the same increment, and the budget would undercount by
    /// exactly the amount an attacker wants it to.
    fn with_ledger<T>(
        &self,
        key: &LedgerKey,
        f: impl FnOnce(&mut Ledger) -> T,
    ) -> Result<T, RiskError> {
        let path = self.path_for(key);
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)?;
        file.lock_exclusive()?;
        let result = Self::locked_update(&file, f);
        // Best-effort: the lock is released on close regardless, so a failure
        // here must not mask the result of the update itself.
        let _ = FileExt::unlock(&file);
        result
    }

    fn locked_update<T>(
        mut file: &File,
        f: impl FnOnce(&mut Ledger) -> T,
    ) -> Result<T, RiskError> {
        let mut buf = String::new();
        file.read_to_string(&mut buf)?;

        let mut ledger: Ledger = if buf.trim().is_empty() {
            Ledger::default()
        } else {
            // Fail closed. Resetting a corrupt ledger to zero would make
            // "overwrite the file with garbage" a one-step budget reset.
            serde_json::from_str(&buf).map_err(|e| RiskError::Corrupt(e.to_string()))?
        };

        let out = f(&mut ledger);

        let encoded =
            serde_json::to_vec(&ledger).map_err(|e| RiskError::Corrupt(e.to_string()))?;
        file.set_len(0)?;
        file.seek(SeekFrom::Start(0))?;
        file.write_all(&encoded)?;
        // Durability matters: a freeze that evaporates on power loss is not a
        // freeze.
        file.sync_all()?;
        Ok(out)
    }
}

impl RiskStore for FileRiskStore {
    fn charge(&self, key: &LedgerKey, charge: &Charge) -> Result<RiskState, RiskError> {
        self.with_ledger(key, |l| l.charge(charge, &self.budgets))
    }

    fn peek(&self, key: &LedgerKey) -> Result<RiskState, RiskError> {
        self.with_ledger(key, |l| l.peek(&self.budgets))
    }

    fn note_outcome(
        &self,
        key: &LedgerKey,
        program: &str,
        success: bool,
    ) -> Result<RiskState, RiskError> {
        self.with_ledger(key, |l| l.note_outcome(program, success, &self.budgets))
    }

    fn note_artifact(&self, key: &LedgerKey, artifact: Artifact) -> Result<(), RiskError> {
        self.with_ledger(key, |l| l.note_artifact(artifact))
    }

    fn artifact(&self, key: &LedgerKey, path: &str) -> Result<Option<Artifact>, RiskError> {
        self.with_ledger(key, |l| l.artifact(path))
    }

    fn freeze(&self, key: &LedgerKey, reason: FreezeReason) -> Result<(), RiskError> {
        self.with_ledger(key, |l| l.freeze(reason))
    }

    fn clear_freeze(&self, key: &LedgerKey) -> Result<(), RiskError> {
        self.with_ledger(key, |l| l.clear_freeze())
    }
}
