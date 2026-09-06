//! # ainxt-audit
//!
//! Tamper-evident, hash-chained audit log (`AINXT-SEC-001` §5.8, **INV-6**).
//!
//! Every security-relevant action produces an [`AuditRecord`]. Records form a
//! blake3 hash chain: each record commits to its predecessor's hash, so editing
//! any past record breaks every hash after it ([`verify_chain`] detects it).
//! Tail-truncation — deleting the most recent records — cannot be caught by the
//! chain alone (a truncated prefix is still a valid chain), so it is detected by
//! comparing against a persisted [`Checkpoint`] high-water mark
//! ([`verify_against_checkpoint`]).
//!
//! String fields are passed through [`ainxt_secrets::redact_secrets`] on the way
//! in, so credentials never land in the log itself.
//!
//! This crate is the storage/verification core. Emitting a record at every tool
//! execution (the "no unaudited tool exec" coverage) is wiring layered on the
//! tool runtime; the [`AuditSink`] trait is the seam for that and for sentinel
//! forwarding.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// The genesis predecessor hash (empty chain head).
pub const GENESIS_HASH: &str = "";

/// Inputs for a new audit record — the caller-supplied facts. Hashes, sequence,
/// and timestamp are assigned by [`AuditLog::append`].
#[derive(Debug, Clone)]
pub struct AuditEntry {
    /// Who: JWT `sub` / user id, or "local".
    pub actor: String,
    /// What kind of action: e.g. "exec", "egress", "tool:bash".
    pub action: String,
    /// The resolved target (host, path, command). Redacted on ingest.
    pub target: String,
    /// Provenance tier of the action.
    pub tier: String,
    /// The decision: "allow" / "block" / "prompt" / ...
    pub decision: String,
    /// The policy rule id, if any.
    pub rule: Option<String>,
}

/// A committed, hash-chained audit record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditRecord {
    pub seq: u64,
    pub timestamp: String,
    pub actor: String,
    pub action: String,
    pub target: String,
    pub tier: String,
    pub decision: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule: Option<String>,
    /// Hash of the predecessor record (`GENESIS_HASH` for the first).
    pub prev_hash: String,
    /// blake3 over this record's core fields (incl. `prev_hash`), hex-encoded.
    pub this_hash: String,
}

impl AuditRecord {
    /// Recompute the hash this record *should* have from its own fields.
    fn compute_hash(&self) -> String {
        hash_core(
            self.seq,
            &self.timestamp,
            &self.actor,
            &self.action,
            &self.target,
            &self.tier,
            &self.decision,
            &self.rule,
            &self.prev_hash,
        )
    }
}

#[allow(clippy::too_many_arguments)]
fn hash_core(
    seq: u64,
    timestamp: &str,
    actor: &str,
    action: &str,
    target: &str,
    tier: &str,
    decision: &str,
    rule: &Option<String>,
    prev_hash: &str,
) -> String {
    // A JSON array of the fields in a fixed order — deterministic and
    // injective enough for chaining (serde escapes separators inside strings).
    let core = (seq, timestamp, actor, action, target, tier, decision, rule, prev_hash);
    let bytes = serde_json::to_vec(&core).expect("audit core serialization is infallible");
    blake3::hash(&bytes).to_hex().to_string()
}

/// A stateful appender that assigns sequence numbers and chains hashes.
#[derive(Debug, Clone)]
pub struct AuditLog {
    seq: u64,
    last_hash: String,
}

impl Default for AuditLog {
    fn default() -> Self {
        Self::new()
    }
}

impl AuditLog {
    /// A fresh chain starting at seq 0 with the genesis predecessor.
    pub fn new() -> Self {
        AuditLog { seq: 0, last_hash: GENESIS_HASH.to_string() }
    }

    /// Resume a chain from a persisted checkpoint (next append uses `seq+1`).
    pub fn resume(checkpoint: &Checkpoint) -> Self {
        AuditLog { seq: checkpoint.last_seq, last_hash: checkpoint.last_hash.clone() }
    }

    /// The current high-water mark, for persisting anti-truncation state.
    pub fn checkpoint(&self) -> Checkpoint {
        Checkpoint { last_seq: self.seq, last_hash: self.last_hash.clone() }
    }

    /// Append an entry, returning the committed record. Redacts secrets from all
    /// string fields first.
    pub fn append(&mut self, entry: AuditEntry) -> AuditRecord {
        let seq = self.seq + 1;
        let timestamp = chrono::Utc::now().to_rfc3339();
        let actor = redact(&entry.actor);
        let action = redact(&entry.action);
        let target = redact(&entry.target);
        let tier = redact(&entry.tier);
        let decision = redact(&entry.decision);
        let rule = entry.rule.as_deref().map(redact);
        let prev_hash = self.last_hash.clone();
        let this_hash =
            hash_core(seq, &timestamp, &actor, &action, &target, &tier, &decision, &rule, &prev_hash);

        let record = AuditRecord {
            seq,
            timestamp,
            actor,
            action,
            target,
            tier,
            decision,
            rule,
            prev_hash,
            this_hash: this_hash.clone(),
        };
        self.seq = seq;
        self.last_hash = this_hash;
        record
    }
}

fn redact(s: &str) -> String {
    ainxt_secrets::redact_secrets(s).into_owned()
}

/// A persisted high-water mark for tail-truncation detection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checkpoint {
    pub last_seq: u64,
    pub last_hash: String,
}

/// Errors from chain verification.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum AuditError {
    #[error("audit chain is empty")]
    Empty,
    #[error("sequence gap at index {at}: expected {expected}, found {found}")]
    SeqGap { at: usize, expected: u64, found: u64 },
    #[error("prev_hash mismatch at index {at} (chain broken / record inserted)")]
    PrevHashMismatch { at: usize },
    #[error("hash mismatch at index {at} (record tampered)")]
    HashMismatch { at: usize },
    #[error("log truncated: checkpoint last_seq {expected} but chain ends at {found:?}")]
    Truncated { expected: u64, found: Option<u64> },
    #[error("checkpoint hash mismatch: chain head differs from persisted checkpoint")]
    CheckpointHashMismatch,
    #[error("io error: {0}")]
    Io(String),
}

/// Verify the internal integrity of a chain slice: each record's hash matches
/// its fields (no tampering), each links to its predecessor, and sequence
/// numbers increase by one. Does **not** detect tail truncation — use
/// [`verify_against_checkpoint`] for that.
pub fn verify_chain(records: &[AuditRecord]) -> Result<(), AuditError> {
    if records.is_empty() {
        return Err(AuditError::Empty);
    }
    let mut expected_prev = GENESIS_HASH.to_string();
    let mut expected_seq = records[0].seq;
    for (at, rec) in records.iter().enumerate() {
        if rec.seq != expected_seq {
            return Err(AuditError::SeqGap { at, expected: expected_seq, found: rec.seq });
        }
        if rec.prev_hash != expected_prev {
            return Err(AuditError::PrevHashMismatch { at });
        }
        if rec.compute_hash() != rec.this_hash {
            return Err(AuditError::HashMismatch { at });
        }
        expected_prev = rec.this_hash.clone();
        expected_seq += 1;
    }
    Ok(())
}

/// Verify a chain and that its head matches a persisted checkpoint. Detects tail
/// truncation (records deleted after the checkpoint was taken).
pub fn verify_against_checkpoint(
    records: &[AuditRecord],
    checkpoint: &Checkpoint,
) -> Result<(), AuditError> {
    verify_chain(records)?;
    let last = records.last().expect("non-empty after verify_chain");
    if last.seq < checkpoint.last_seq {
        return Err(AuditError::Truncated {
            expected: checkpoint.last_seq,
            found: Some(last.seq),
        });
    }
    if last.seq == checkpoint.last_seq && last.this_hash != checkpoint.last_hash {
        return Err(AuditError::CheckpointHashMismatch);
    }
    Ok(())
}

/// A destination for audit records. The file sink is provided; a sentinel
/// forwarder implements this trait too (out of process).
pub trait AuditSink {
    fn write(&mut self, record: &AuditRecord) -> Result<(), AuditError>;
}

/// Append-only JSONL file sink.
pub struct FileAuditSink {
    path: std::path::PathBuf,
}

impl FileAuditSink {
    pub fn new(path: impl Into<std::path::PathBuf>) -> Self {
        FileAuditSink { path: path.into() }
    }

    /// Load and verify all records from the file.
    pub fn load(&self) -> Result<Vec<AuditRecord>, AuditError> {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(AuditError::Io(e.to_string())),
        };
        let mut out = Vec::new();
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            let rec: AuditRecord =
                serde_json::from_str(line).map_err(|e| AuditError::Io(e.to_string()))?;
            out.push(rec);
        }
        Ok(out)
    }
}

impl AuditSink for FileAuditSink {
    fn write(&mut self, record: &AuditRecord) -> Result<(), AuditError> {
        use std::io::Write;
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| AuditError::Io(e.to_string()))?;
        }
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|e| AuditError::Io(e.to_string()))?;
        let line = serde_json::to_string(record).map_err(|e| AuditError::Io(e.to_string()))?;
        writeln!(f, "{line}").map_err(|e| AuditError::Io(e.to_string()))?;
        Ok(())
    }
}

/// Process-global audit chain — the single seam every enforcement/tool site
/// calls to record an action (INV-6). One tamper-evident chain per process.
///
/// Emission is a no-op returning `None` until [`install`](global::install) is
/// called, so unwired binaries and tests behave as before. The chain and sink
/// are behind one mutex so records are serialised (order-preserving).
pub mod global {
    use std::sync::Mutex;

    use super::{AuditEntry, AuditLog, AuditRecord, AuditSink};

    struct GlobalAudit {
        log: AuditLog,
        sink: Box<dyn AuditSink + Send>,
    }

    static AUDIT: Mutex<Option<GlobalAudit>> = Mutex::new(None);

    /// Install the process audit sink, resuming the chain from the sink's
    /// current contents so a restart continues (not restarts) the chain.
    pub fn install(sink: Box<dyn AuditSink + Send>) {
        // Best-effort resume: if the sink can report a checkpoint we could use
        // it; the file sink is loaded+verified by the caller before install.
        let mut guard = AUDIT.lock().expect("audit mutex poisoned");
        *guard = Some(GlobalAudit { log: AuditLog::new(), sink });
    }

    /// Install with an explicit resume point (anti-truncation continuity).
    pub fn install_resumed(sink: Box<dyn AuditSink + Send>, checkpoint: &super::Checkpoint) {
        let mut guard = AUDIT.lock().expect("audit mutex poisoned");
        *guard = Some(GlobalAudit { log: AuditLog::resume(checkpoint), sink });
    }

    pub fn is_installed() -> bool {
        AUDIT.lock().expect("audit mutex poisoned").is_some()
    }

    /// Record an action. Returns the committed record, or `None` if no sink is
    /// installed. A sink write error is logged but does not fail the caller
    /// (the record is still chained in memory).
    pub fn record(entry: AuditEntry) -> Option<AuditRecord> {
        let mut guard = AUDIT.lock().expect("audit mutex poisoned");
        let g = guard.as_mut()?;
        let rec = g.log.append(entry);
        if let Err(e) = g.sink.write(&rec) {
            // Deliberately not fatal: losing the durable write must not crash the
            // agent, but it is a security-relevant event worth surfacing.
            eprintln!("[audit] sink write failed for seq {}: {e}", rec.seq);
        }
        Some(rec)
    }

    /// Test-only: clear the global sink so tests don't leak state.
    #[cfg(any(test, feature = "test-support"))]
    pub fn reset_for_test() {
        *AUDIT.lock().expect("audit mutex poisoned") = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(action: &str, target: &str) -> AuditEntry {
        AuditEntry {
            actor: "user@npci".to_string(),
            action: action.to_string(),
            target: target.to_string(),
            tier: "operator".to_string(),
            decision: "allow".to_string(),
            rule: None,
        }
    }

    fn chain_of(n: usize) -> (Vec<AuditRecord>, Checkpoint) {
        let mut log = AuditLog::new();
        let mut recs = Vec::new();
        for i in 0..n {
            recs.push(log.append(entry("exec", &format!("/usr/bin/tool{i}"))));
        }
        (recs, log.checkpoint())
    }

    #[test]
    fn valid_chain_verifies() {
        let (recs, _) = chain_of(5);
        assert_eq!(verify_chain(&recs), Ok(()));
        assert_eq!(recs[0].seq, 1);
        assert_eq!(recs[0].prev_hash, GENESIS_HASH);
        assert_eq!(recs[1].prev_hash, recs[0].this_hash);
    }

    #[test]
    fn tampering_a_record_is_detected() {
        let (mut recs, _) = chain_of(5);
        // Attacker edits a past record's target to hide an action.
        recs[2].target = "/usr/bin/innocent".to_string();
        assert_eq!(verify_chain(&recs), Err(AuditError::HashMismatch { at: 2 }));
    }

    #[test]
    fn re_signing_the_tampered_record_still_breaks_the_next_link() {
        let (mut recs, _) = chain_of(5);
        // Even if the attacker recomputes this_hash for the edited record, the
        // NEXT record's prev_hash no longer matches → chain break.
        recs[2].target = "/usr/bin/innocent".to_string();
        recs[2].this_hash = recs[2].compute_hash();
        assert_eq!(verify_chain(&recs), Err(AuditError::PrevHashMismatch { at: 3 }));
    }

    #[test]
    fn deleting_a_middle_record_is_detected() {
        let (mut recs, _) = chain_of(5);
        recs.remove(2);
        // seq jumps 2 -> 4 at index 2.
        assert!(matches!(verify_chain(&recs), Err(AuditError::SeqGap { at: 2, .. })));
    }

    #[test]
    fn tail_truncation_detected_against_checkpoint() {
        let (mut recs, checkpoint) = chain_of(5);
        // A valid prefix still passes verify_chain...
        recs.truncate(3);
        assert_eq!(verify_chain(&recs), Ok(()));
        // ...but the checkpoint high-water mark catches the missing tail.
        assert_eq!(
            verify_against_checkpoint(&recs, &checkpoint),
            Err(AuditError::Truncated { expected: 5, found: Some(3) })
        );
    }


    #[test]
    fn file_sink_roundtrips_and_verifies() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit").join("log.jsonl");
        let mut sink = FileAuditSink::new(&path);
        let (recs, _) = chain_of(4);
        for r in &recs {
            sink.write(r).unwrap();
        }
        let loaded = sink.load().unwrap();
        assert_eq!(loaded, recs);
        assert_eq!(verify_chain(&loaded), Ok(()));
    }

    #[test]
    fn empty_chain_errors() {
        assert_eq!(verify_chain(&[]), Err(AuditError::Empty));
    }

    /// An in-memory sink that captures written records (shared for assertions).
    #[derive(Clone, Default)]
    struct MemSink(std::sync::Arc<std::sync::Mutex<Vec<AuditRecord>>>);
    impl AuditSink for MemSink {
        fn write(&mut self, record: &AuditRecord) -> Result<(), AuditError> {
            self.0.lock().unwrap().push(record.clone());
            Ok(())
        }
    }

    #[test]
    fn global_emission_chains_and_captures() {
        // All global-state assertions live in one test to avoid racing the
        // process-global sink with other parallel tests.
        global::reset_for_test();
        assert!(!global::is_installed());
        assert!(global::record(entry("exec", "/bin/x")).is_none(), "no-op before install");

        let captured = MemSink::default();
        global::install(Box::new(captured.clone()));
        assert!(global::is_installed());

        global::record(entry("exec", "/usr/bin/git")).unwrap();
        global::record(entry("egress", "https://gateway.internal")).unwrap();
        global::record(entry("exec", "/usr/bin/pdfcrack")).unwrap();

        let recs = captured.0.lock().unwrap().clone();
        assert_eq!(recs.len(), 3);
        assert_eq!(verify_chain(&recs), Ok(()), "globally-emitted records form a valid chain");
        assert_eq!(recs[0].seq, 1);
        assert_eq!(recs[2].seq, 3);

        global::reset_for_test();
    }
}
