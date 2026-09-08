//! The risk state machine, shared by every [`RiskStore`](crate::RiskStore)
//! implementation so that in-process, file-backed and (later) daemon-backed
//! stores cannot drift in behaviour.

use std::collections::{BTreeMap, BTreeSet};

use ainxt_intent::Capability;
use serde::{Deserialize, Serialize};

use crate::now_secs;

/// Thresholds over a rolling window.
///
/// The defaults are chosen to sit well above ordinary development and well
/// below abuse. They are deliberately *not* tight: a budget that fires on
/// normal work gets disabled by the first team that trips it, which is worse
/// than a loose budget that survives. Tighten from observed data, not from
/// intuition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Budgets {
    /// Rolling window in seconds.
    pub window_secs: u64,
    /// Total actions in the window.
    pub max_execs: u32,
    /// Same program repeated in the window. A high backstop only — the
    /// failure-loop detector below is the precise instrument.
    pub max_repeats_same_program: u32,
    /// Consecutive *failing* invocations of one program.
    ///
    /// This is the brute-force signature. A build runs `rustc` hundreds of
    /// times and succeeds; a password attack runs one tool repeatedly and keeps
    /// failing. Keying on failure rather than repetition is what makes this
    /// usable at a low threshold.
    pub max_consecutive_failures: u32,
    /// Distinct network hosts contacted in the window — scanning and fan-out.
    pub max_distinct_hosts: u32,
    /// Bytes sent outbound in the window — bulk exfiltration.
    pub max_bytes_out: u64,
    /// Package installs in the window.
    pub max_installs: u32,
}

impl Default for Budgets {
    fn default() -> Self {
        Self {
            window_secs: 300,
            max_execs: 300,
            max_repeats_same_program: 400,
            max_consecutive_failures: 12,
            max_distinct_hosts: 30,
            max_bytes_out: 256 * 1024 * 1024,
            max_installs: 40,
        }
    }
}

/// One action being charged against the budget.
#[derive(Debug, Clone, Default)]
pub struct Charge {
    pub capabilities: BTreeSet<Capability>,
    /// Resolved program basename, when the action runs one.
    pub program: Option<String>,
    /// Hosts the action reaches.
    pub hosts: Vec<String>,
    pub bytes_out: u64,
}

impl Charge {
    pub fn for_program(program: impl Into<String>) -> Self {
        Self {
            program: Some(program.into()),
            ..Self::default()
        }
    }
}

/// Which budget was exceeded. Carried into the freeze and the audit record so a
/// denial can say precisely why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BudgetBreach {
    ExecRate {
        count: u32,
        limit: u32,
    },
    RepeatedProgram {
        program: String,
        count: u32,
        limit: u32,
    },
    ConsecutiveFailures {
        program: String,
        count: u32,
        limit: u32,
    },
    HostFanOut {
        count: u32,
        limit: u32,
    },
    BytesOut {
        bytes: u64,
        limit: u64,
    },
    InstallRate {
        count: u32,
        limit: u32,
    },
}

impl BudgetBreach {
    pub fn describe(&self) -> String {
        match self {
            Self::ExecRate { count, limit } => {
                format!("{count} actions in the window exceeds the limit of {limit}")
            }
            Self::RepeatedProgram {
                program,
                count,
                limit,
            } => format!("`{program}` ran {count} times in the window, limit {limit}"),
            Self::ConsecutiveFailures {
                program,
                count,
                limit,
            } => format!(
                "`{program}` failed {count} times in a row, limit {limit} — this is the \
                 signature of a credential or password attack"
            ),
            Self::HostFanOut { count, limit } => {
                format!("contacted {count} distinct hosts in the window, limit {limit}")
            }
            Self::BytesOut { bytes, limit } => {
                format!("sent {bytes} bytes outbound in the window, limit {limit}")
            }
            Self::InstallRate { count, limit } => {
                format!("{count} package installs in the window, limit {limit}")
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FreezeReason {
    pub breach: BudgetBreach,
    pub at: u64,
}

impl FreezeReason {
    pub fn describe(&self) -> String {
        self.breach.describe()
    }
}

/// How much a downloaded or written artifact can be trusted.
///
/// Kept local rather than reusing `ainxt-provenance`'s lattice so this crate
/// stays a leaf; the enforcement point maps between the two.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactTrust {
    /// Came from the network, an untrusted repo, or tool output.
    Untrusted,
    /// Produced by the workspace itself.
    Workspace,
    /// Placed by a human.
    Operator,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artifact {
    pub path: String,
    pub trust: ArtifactTrust,
    /// Human-readable provenance, e.g. "downloaded from https://…".
    pub origin: String,
    pub at: u64,
}

/// Snapshot returned by every mutating call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RiskState {
    pub frozen: Option<FreezeReason>,
    pub execs_in_window: u32,
    pub installs_in_window: u32,
    pub distinct_hosts_in_window: u32,
    pub bytes_out_in_window: u64,
    pub max_consecutive_failures: u32,
    /// Breaches detected by *this* call. Empty on a healthy action.
    pub breaches: Vec<BudgetBreach>,
}

impl RiskState {
    pub fn is_frozen(&self) -> bool {
        self.frozen.is_some()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Event {
    at: u64,
    program: Option<String>,
    installed: bool,
    hosts: Vec<String>,
    bytes_out: u64,
}

/// Hard cap on retained events, independent of the window, so a runaway session
/// cannot grow the ledger without bound before the budget fires.
const MAX_EVENTS: usize = 4096;

/// Persisted risk state for one [`LedgerKey`](crate::LedgerKey).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct Ledger {
    events: Vec<Event>,
    /// program → consecutive failures. Reset to zero on success.
    failures: BTreeMap<String, u32>,
    artifacts: BTreeMap<String, Artifact>,
    frozen: Option<FreezeReason>,
}

impl Ledger {
    pub(crate) fn charge(&mut self, charge: &Charge, budgets: &Budgets) -> RiskState {
        let now = now_secs();
        self.prune(now, budgets);

        self.events.push(Event {
            at: now,
            program: charge.program.clone(),
            installed: charge.capabilities.contains(&Capability::InstallPackage),
            hosts: charge.hosts.clone(),
            bytes_out: charge.bytes_out,
        });
        if self.events.len() > MAX_EVENTS {
            let overflow = self.events.len() - MAX_EVENTS;
            self.events.drain(..overflow);
        }

        let mut breaches = Vec::new();

        let execs = self.events.len() as u32;
        if execs > budgets.max_execs {
            breaches.push(BudgetBreach::ExecRate {
                count: execs,
                limit: budgets.max_execs,
            });
        }

        if let Some(program) = &charge.program {
            let count = self
                .events
                .iter()
                .filter(|e| e.program.as_deref() == Some(program.as_str()))
                .count() as u32;
            if count > budgets.max_repeats_same_program {
                breaches.push(BudgetBreach::RepeatedProgram {
                    program: program.clone(),
                    count,
                    limit: budgets.max_repeats_same_program,
                });
            }
        }

        let hosts: BTreeSet<&str> = self
            .events
            .iter()
            .flat_map(|e| e.hosts.iter().map(String::as_str))
            .collect();
        if hosts.len() as u32 > budgets.max_distinct_hosts {
            breaches.push(BudgetBreach::HostFanOut {
                count: hosts.len() as u32,
                limit: budgets.max_distinct_hosts,
            });
        }

        let bytes: u64 = self.events.iter().map(|e| e.bytes_out).sum();
        if bytes > budgets.max_bytes_out {
            breaches.push(BudgetBreach::BytesOut {
                bytes,
                limit: budgets.max_bytes_out,
            });
        }

        let installs = self.events.iter().filter(|e| e.installed).count() as u32;
        if installs > budgets.max_installs {
            breaches.push(BudgetBreach::InstallRate {
                count: installs,
                limit: budgets.max_installs,
            });
        }

        self.apply_breaches(&breaches, now);
        self.snapshot(breaches)
    }

    pub(crate) fn note_outcome(
        &mut self,
        program: &str,
        success: bool,
        budgets: &Budgets,
    ) -> RiskState {
        let now = now_secs();
        let mut breaches = Vec::new();

        if success {
            self.failures.remove(program);
        } else {
            let counter = self.failures.entry(program.to_owned()).or_insert(0);
            *counter = counter.saturating_add(1);
            if *counter > budgets.max_consecutive_failures {
                breaches.push(BudgetBreach::ConsecutiveFailures {
                    program: program.to_owned(),
                    count: *counter,
                    limit: budgets.max_consecutive_failures,
                });
            }
        }

        self.apply_breaches(&breaches, now);
        self.snapshot(breaches)
    }

    pub(crate) fn note_artifact(&mut self, artifact: Artifact) {
        self.artifacts.insert(artifact.path.clone(), artifact);
    }

    pub(crate) fn artifact(&self, path: &str) -> Option<Artifact> {
        self.artifacts.get(path).cloned()
    }

    pub(crate) fn freeze(&mut self, reason: FreezeReason) {
        // Idempotent: the first reason is the true cause, so a later breach
        // must not overwrite it and lose the original explanation.
        if self.frozen.is_none() {
            self.frozen = Some(reason);
        }
    }

    pub(crate) fn clear_freeze(&mut self) {
        self.frozen = None;
        self.failures.clear();
    }

    pub(crate) fn peek(&self, budgets: &Budgets) -> RiskState {
        let mut view = self.clone();
        view.prune(now_secs(), budgets);
        view.snapshot(Vec::new())
    }

    fn apply_breaches(&mut self, breaches: &[BudgetBreach], now: u64) {
        if let Some(first) = breaches.first() {
            self.freeze(FreezeReason {
                breach: first.clone(),
                at: now,
            });
        }
    }

    fn prune(&mut self, now: u64, budgets: &Budgets) {
        let cutoff = now.saturating_sub(budgets.window_secs);
        self.events.retain(|e| e.at >= cutoff);
    }

    fn snapshot(&self, breaches: Vec<BudgetBreach>) -> RiskState {
        let hosts: BTreeSet<&str> = self
            .events
            .iter()
            .flat_map(|e| e.hosts.iter().map(String::as_str))
            .collect();
        RiskState {
            frozen: self.frozen.clone(),
            execs_in_window: self.events.len() as u32,
            installs_in_window: self.events.iter().filter(|e| e.installed).count() as u32,
            distinct_hosts_in_window: hosts.len() as u32,
            bytes_out_in_window: self.events.iter().map(|e| e.bytes_out).sum(),
            max_consecutive_failures: self.failures.values().copied().max().unwrap_or(0),
            breaches,
        }
    }
}
