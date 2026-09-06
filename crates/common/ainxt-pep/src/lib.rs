//! The Policy Enforcement Point: the single place a tool action is authorized.
//!
//! Everything else in the security stack describes or records. This crate is
//! the one that says no.
//!
//! # Shape of a decision
//!
//! [`Pep::authorize`] computes a [`Judgement`] — *ground truth*, meaning "what
//! would happen under `Block`" — with no knowledge of the current enforcement
//! posture. A single projection in [`mode`] then turns that into an
//! [`Obligation`], which is what the caller must actually do. The split is what
//! makes observe mode provably behaviour-free rather than aspirationally so.
//!
//! # Client neutrality
//!
//! This crate must never depend on `ainxt-workspace`. That is not tidiness: the
//! permission crate is CLI-shaped (it knows about ACP, `AccessKind`, prompts),
//! and depending on it would bake the CLI into the enforcement point. Today the
//! client is the terminal; tomorrow it is an IDE host or a CI runner reaching
//! the same binary over ACP, and they must all hit the same authority.
//! Consequently [`Principal::client`] is a plain string and every wire type
//! derives serde, so the eventual out-of-process boundary is a serialization
//! change and not a redesign.
//!
//! # Ordering
//!
//! Risk is charged *before* the verdict is computed, so the action that trips a
//! budget is itself refused rather than being the last one allowed through.

pub mod attest;
pub mod bootstrap;
pub mod context;
pub mod mode;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use ainxt_audit::AuditEntry;
use ainxt_intent::{Capability, Confidence, Derivation, ShellKind};
use ainxt_policy::engine::ExecTarget;
use ainxt_policy_types::{RuleId, SovereignAction, TrustTier, Verdict};
use ainxt_session_risk::{
    Artifact, ArtifactTrust, Charge, FreezeReason, LedgerKey, RiskStore,
};
use serde::{Deserialize, Serialize};

/// How long a minted exec ticket stays redeemable. Long enough to cover the
/// gap between authorization and spawn, short enough that a stolen ticket is
/// not a durable capability.
const TICKET_TTL: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ClientId(pub String);

impl From<&str> for ClientId {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(pub String);

impl From<&str> for SessionId {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

/// Who is acting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Principal {
    /// Gateway JWT `sub` when authenticated, else `local:<uid>`. This is the
    /// risk-ledger key, so it must be stable across processes on a host.
    pub subject: String,
    pub client: ClientId,
    pub session: SessionId,
    /// Set for subagents. Risk is charged to the parent, so spawning children
    /// is not a way to get a fresh budget.
    pub parent_session: Option<SessionId>,
}

/// What is being attempted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Intent {
    Shell {
        command: String,
        shell: Shell,
    },
    FileRead {
        path: String,
    },
    FileWrite {
        path: String,
    },
    Egress {
        url: String,
    },
    Mcp {
        server: String,
        tool: String,
    },
    /// Anything not otherwise modelled. Deny-by-default lands here, which is
    /// where a novel capability shows up before anyone has written a rule.
    ToolCall {
        tool: String,
    },
}

/// Serializable mirror of [`ainxt_intent::ShellKind`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Shell {
    Posix,
    PowerShell,
    Cmd,
}

impl From<Shell> for ShellKind {
    fn from(value: Shell) -> Self {
        match value {
            Shell::Posix => ShellKind::Posix,
            Shell::PowerShell => ShellKind::PowerShell,
            Shell::Cmd => ShellKind::Cmd,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    pub principal: Principal,
    pub intent: Intent,
    /// Trust tier of the content that produced this request. Callers derive it
    /// from the session's provenance; a request influenced by web or tool
    /// output arrives here already demoted.
    pub influence: TrustTier,
}

/// Ground truth: what *would* happen under `Block`, independent of posture.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Judgement {
    Permit,
    RequireHuman {
        action: SovereignAction,
        reason: String,
    },
    Deny {
        rule: String,
        reason: String,
    },
}

/// What the caller must do. The only thing enforcement posture affects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Obligation {
    Proceed,
    Prompt { reason: String, sovereign: bool },
    Refuse { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionId(pub String);

#[derive(Debug, Clone)]
pub struct Authorization {
    pub judgement: Judgement,
    pub obligation: Obligation,
    /// The derived request — surfaced by `ainxt policy explain` and carried
    /// into the audit record, so a denial can name the capability and not just
    /// the outcome.
    pub derivation: Derivation,
    pub decision_id: DecisionId,
}

impl Authorization {
    pub fn is_refused(&self) -> bool {
        matches!(self.obligation, Obligation::Refuse { .. })
    }
}

/// A post-action fact the enforcement point needs to know about.
#[derive(Debug, Clone)]
pub enum Effect {
    /// An artifact entered the workspace. Feeds the provenance ledger so that
    /// later *executing* it can be refused.
    ArtifactWritten {
        path: String,
        origin: String,
        trust: ArtifactTrust,
    },
    /// Whether an invocation succeeded. Drives failure-loop detection — without
    /// this, brute force is invisible.
    Outcome { program: String, success: bool },
    BytesSent { bytes: u64 },
}

/// A single-use, short-lived permit to spawn one exact command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecTicket(pub String);

pub struct Pep {
    risk: Arc<dyn RiskStore>,
    endpoint: String,
    tickets: Mutex<HashMap<String, Instant>>,
}

impl std::fmt::Debug for Pep {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pep")
            .field("endpoint", &self.endpoint)
            .finish_non_exhaustive()
    }
}

impl Pep {
    /// An enforcement point over an in-process, non-persistent ledger.
    ///
    /// For tests and for the non-enforcing path. Production installs go through
    /// [`bootstrap::install_pep`], which uses the file-backed store — budgets
    /// held only in memory are not shared with a second client and so do not
    /// actually bound anything.
    pub fn in_memory(endpoint: impl Into<String>) -> Self {
        Self::new(
            Arc::new(ainxt_session_risk::InProcessRiskStore::new(
                ainxt_session_risk::Budgets::default(),
            )),
            endpoint,
        )
    }

    pub fn new(risk: Arc<dyn RiskStore>, endpoint: impl Into<String>) -> Self {
        Self {
            risk,
            endpoint: endpoint.into(),
            tickets: Mutex::new(HashMap::new()),
        }
    }

    fn key(&self, principal: &Principal) -> LedgerKey {
        LedgerKey::new(principal.subject.clone(), self.endpoint.clone())
    }

    /// Authorize one action.
    ///
    /// Sync on purpose. A blocking signature is what lets a future
    /// daemon-backed [`RiskStore`] do a round trip without recolouring every
    /// caller in the tool path to async.
    pub fn authorize(&self, req: &Request) -> Authorization {
        // Read the engine per call: `global` is an ArcSwap and a hot-swapped
        // bundle must take effect on the next action, so caching it across
        // calls would silently pin a stale policy.
        let engine = ainxt_policy::global::active();
        let posture = engine.enforcement();

        if !mode::evaluation_required(posture) {
            return Authorization {
                judgement: Judgement::Permit,
                obligation: Obligation::Proceed,
                derivation: Derivation::new(Confidence::Exact),
                decision_id: DecisionId(String::new()),
            };
        }

        let derivation = derive(&req.intent);

        // Charge before judging, so the action that trips a budget is the one
        // refused rather than the last one waved through.
        let state = match self.risk.charge(&self.key(&req.principal), &charge_for(&derivation)) {
            Ok(state) => state,
            Err(err) => {
                // A ledger we cannot read is a budget we cannot enforce.
                let judgement = Judgement::Deny {
                    rule: "pep.risk.unavailable".to_owned(),
                    reason: format!("risk ledger unavailable, refusing to proceed blind: {err}"),
                };
                let obligation = mode::obligate(&judgement, posture);
                let decision_id = self.record(req, &derivation, &judgement, &obligation);
                return Authorization {
                    judgement,
                    obligation,
                    derivation,
                    decision_id,
                };
            }
        };

        let judgement = self.judge(req, &derivation, &engine, &state);
        let obligation = mode::obligate(&judgement, posture);

        let decision_id = self.record(req, &derivation, &judgement, &obligation);

        if matches!(obligation, Obligation::Proceed)
            && let Intent::Shell { command, .. } = &req.intent
        {
            self.mint_ticket(command);
        }

        Authorization {
            judgement,
            obligation,
            derivation,
            decision_id,
        }
    }

    /// Dry run: what *would* happen, without spending budget or writing a
    /// record.
    ///
    /// Backs `ainxt policy explain`. It deliberately calls the same
    /// [`Self::judge`] as [`Self::authorize`] rather than reproducing the
    /// reasoning — an explain command that can disagree with the enforcement it
    /// describes is worse than no explain command, because people would trust
    /// it. The only differences are `peek` instead of `charge`, and no audit
    /// record: inspecting policy must not consume a budget or pollute the
    /// evidence chain.
    pub fn explain(&self, req: &Request) -> Authorization {
        let engine = ainxt_policy::global::active();
        let derivation = derive(&req.intent);

        let judgement = match self.risk.peek(&self.key(&req.principal)) {
            Ok(state) => self.judge(req, &derivation, &engine, &state),
            Err(err) => Judgement::Deny {
                rule: "pep.risk.unavailable".to_owned(),
                reason: format!("risk ledger unavailable: {err}"),
            },
        };
        let obligation = mode::obligate(&judgement, engine.enforcement());

        Authorization {
            judgement,
            obligation,
            derivation,
            decision_id: DecisionId(String::new()),
        }
    }

    /// Current budget/freeze state for a principal. Reporting only.
    pub fn risk_snapshot(
        &self,
        principal: &Principal,
    ) -> Result<ainxt_session_risk::RiskState, ainxt_session_risk::RiskError> {
        self.risk.peek(&self.key(principal))
    }

    /// Compute ground truth. Deliberately takes no posture argument — that is
    /// what guarantees the judgement is identical in observe and block.
    fn judge(
        &self,
        req: &Request,
        derivation: &Derivation,
        engine: &ainxt_policy::engine::PolicyEngine,
        state: &ainxt_session_risk::RiskState,
    ) -> Judgement {
        let key = self.key(&req.principal);

        if let Some(freeze) = &state.frozen {
            return Judgement::Deny {
                rule: "pep.session.frozen".to_owned(),
                reason: format!(
                    "session frozen: {}. A human must clear the freeze before work continues.",
                    freeze.describe()
                ),
            };
        }

        // An action we could not decompose is not an action we can permit.
        if derivation.confidence == Confidence::Unknown {
            return Judgement::RequireHuman {
                action: SovereignAction::SecurityConfigChange,
                reason: format!(
                    "this action could not be decomposed, so its capabilities are unknown: {}",
                    derivation
                        .evidence
                        .first()
                        .map(|e| e.detail.as_str())
                        .unwrap_or("no detail")
                ),
            };
        }

        // Executing an artifact that arrived from an untrusted origin. This is
        // the multi-turn chain — clone, install, then run what was fetched —
        // and it is invisible to any single-call check.
        if derivation.has(Capability::ExecuteProcess) {
            for path in derivation
                .targets
                .programs
                .iter()
                .chain(derivation.targets.reads.iter())
            {
                if let Ok(Some(artifact)) = self.risk.artifact(&key, path)
                    && artifact.trust == ArtifactTrust::Untrusted
                {
                    return Judgement::Deny {
                        rule: "pep.artifact.untrusted_exec".to_owned(),
                        reason: format!(
                            "refusing to execute {path}, which entered this session from an \
                             untrusted origin ({})",
                            artifact.origin
                        ),
                    };
                }
            }
        }

        // Fetch-then-execute in a single command. Denied structurally rather
        // than through the bundle, for two reasons: it is a property of how
        // programs compose, so a per-program allowlist cannot express it; and
        // it has no legitimate form that the two-step alternative (fetch,
        // review, run) does not cover. Hard-coding it means no profile can
        // forget it, and it survives `curl` being legitimately allowlisted for
        // API work — which is precisely when the allowlist stops helping.
        if derivation.has(Capability::RemoteCodeExecution) {
            return Judgement::Deny {
                rule: "pep.compose.remote_code_execution".to_owned(),
                reason: derivation
                    .evidence
                    .iter()
                    .find(|e| e.capability == Capability::RemoteCodeExecution)
                    .map(|e| e.detail.clone())
                    .unwrap_or_else(|| {
                        "content is fetched and executed in the same command".to_owned()
                    }),
            };
        }

        // MCP has no derivation to fall back on — the arguments are opaque
        // JSON from a third-party server — so the allowlist on the
        // `(server, tool)` pair is the whole control.
        if let Intent::Mcp { server, tool } = &req.intent {
            let decision = engine.mcp_decision(server, tool, req.influence);
            if decision.verdict == Verdict::Block {
                return Judgement::Deny {
                    rule: rule_label(&decision.rule, "pep.mcp.denied"),
                    reason: decision.reason,
                };
            }
        }

        // Capability checks against the granted set.
        for program in &derivation.targets.programs {
            let target = ExecTarget {
                resolved_path: program.clone(),
                basename: program.clone(),
                content_hash: None,
            };
            let decision = engine.exec_decision(&target, req.influence);
            if decision.verdict == Verdict::Block {
                return Judgement::Deny {
                    rule: rule_label(&decision.rule, "pep.exec.denied"),
                    reason: decision.reason,
                };
            }
        }

        for url in &derivation.targets.urls {
            let Some(host) = host_of(url) else { continue };
            let decision = engine.egress_decision(&host, req.influence);
            if decision.verdict == Verdict::Block {
                return Judgement::Deny {
                    rule: rule_label(&decision.rule, "pep.egress.denied"),
                    reason: decision.reason,
                };
            }
        }

        for path in derivation
            .targets
            .reads
            .iter()
            .chain(derivation.targets.writes.iter())
        {
            if engine.is_credential_path(path) {
                return Judgement::RequireHuman {
                    action: SovereignAction::CredentialAccess,
                    reason: format!("{path} holds credential material"),
                };
            }
        }

        // Capabilities that are Sovereign by policy need a live human, and an
        // influenced session cannot supply consent on the model's behalf.
        for (capability, action) in sovereign_map() {
            if derivation.has(capability) && engine.is_sovereign(action) {
                return Judgement::RequireHuman {
                    action,
                    reason: format!(
                        "`{}` is a sovereign action under the active policy",
                        capability.as_str()
                    ),
                };
            }
        }

        // A session under untrusted influence may not take consequential
        // action at all, regardless of which capability it is.
        if !engine.tier_permits_consequential(req.influence)
            && derivation
                .capabilities
                .iter()
                .any(|c| c.is_high_risk() || *c == Capability::NetworkConnect)
        {
            return Judgement::RequireHuman {
                action: SovereignAction::NonAllowlistedEgress,
                reason: format!(
                    "this session has been influenced by {:?} content and cannot take a \
                     consequential action without a human",
                    req.influence
                ),
            };
        }

        Judgement::Permit
    }

    /// Record a post-action fact. Required for the artifact chain and for
    /// failure-loop detection; without it those two controls are inert.
    pub fn observe_effect(&self, principal: &Principal, effect: Effect) {
        let key = self.key(principal);
        let _ = match effect {
            Effect::ArtifactWritten { path, origin, trust } => self.risk.note_artifact(
                &key,
                Artifact {
                    path,
                    trust,
                    origin,
                    at: unix_now(),
                },
            ),
            Effect::Outcome { program, success } => {
                self.risk.note_outcome(&key, &program, success).map(|_| ())
            }
            Effect::BytesSent { bytes } => self
                .risk
                .charge(
                    &key,
                    &Charge {
                        bytes_out: bytes,
                        ..Charge::default()
                    },
                )
                .map(|_| ()),
        };
    }

    pub fn freeze(&self, principal: &Principal, reason: FreezeReason) {
        let _ = self.risk.freeze(&self.key(principal), reason);
    }

    /// Clear a freeze. Only ever reachable behind a human approval at the TTY.
    pub fn clear_freeze(&self, principal: &Principal) {
        let _ = self.risk.clear_freeze(&self.key(principal));
    }

    fn mint_ticket(&self, command: &str) {
        let mut tickets = self
            .tickets
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        tickets.retain(|_, issued| issued.elapsed() < TICKET_TTL);
        tickets.insert(ticket_hash(command), Instant::now());
    }

    /// Redeem the permit for one exact command, consuming it.
    ///
    /// This is a token check, not a second decision. It exists so that a tool
    /// path which forgets to call [`authorize`](Pep::authorize) fails closed at
    /// the spawn site instead of running unchecked — the failure mode that any
    /// single-chokepoint design is exposed to as the codebase grows.
    pub fn redeem_exec_ticket(&self, command: &str) -> Option<ExecTicket> {
        let mut tickets = self
            .tickets
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let hash = ticket_hash(command);
        match tickets.remove(&hash) {
            Some(issued) if issued.elapsed() < TICKET_TTL => Some(ExecTicket(hash)),
            _ => None,
        }
    }

    fn record(
        &self,
        req: &Request,
        derivation: &Derivation,
        judgement: &Judgement,
        obligation: &Obligation,
    ) -> DecisionId {
        let entry = AuditEntry {
            actor: req.principal.subject.clone(),
            action: action_label(&req.intent),
            target: target_label(&req.intent),
            tier: format!("{:?}", req.influence).to_lowercase(),
            decision: decision_label(judgement, obligation),
            rule: match judgement {
                Judgement::Deny { rule, .. } => Some(rule.clone()),
                Judgement::RequireHuman { action, .. } => Some(format!("sovereign:{action:?}")),
                Judgement::Permit => None,
            },
        };
        let _ = derivation;
        match ainxt_audit::global::record(entry) {
            // The chain hash doubles as the decision id: it is unique, and it
            // lets `ainxt policy why <id>` locate the exact audit record.
            Some(record) => DecisionId(record.this_hash.clone()),
            None => DecisionId(String::new()),
        }
    }
}

/// Posture is *derived*, never read: a non-permit judgement that nonetheless
/// proceeds can only be observe mode. Computing it this way keeps
/// [`mode`] the single reader of `Enforcement`.
fn decision_label(judgement: &Judgement, obligation: &Obligation) -> String {
    let base = match judgement {
        Judgement::Permit => "permit",
        Judgement::RequireHuman { .. } => "require_human",
        Judgement::Deny { .. } => "deny",
    };
    let observed =
        !matches!(judgement, Judgement::Permit) && matches!(obligation, Obligation::Proceed);
    if observed {
        format!("{base}:observed")
    } else {
        base.to_owned()
    }
}

/// What this action spends against the budget.
fn charge_for(derivation: &Derivation) -> Charge {
    Charge {
        capabilities: derivation.capabilities.clone(),
        program: derivation.targets.programs.first().cloned(),
        hosts: derivation
            .targets
            .urls
            .iter()
            .filter_map(|u| host_of(u))
            .collect(),
        bytes_out: 0,
    }
}

fn derive(intent: &Intent) -> Derivation {
    match intent {
        Intent::Shell { command, shell } => ainxt_intent::derive_shell(command, (*shell).into()),
        Intent::FileRead { path } => ainxt_intent::derive_read(path),
        Intent::FileWrite { path } => ainxt_intent::derive_write(path),
        Intent::Egress { url } => ainxt_intent::derive_egress(url),
        Intent::Mcp { server, tool } => ainxt_intent::derive_mcp(server, tool),
        Intent::ToolCall { tool } => ainxt_intent::derive_tool(tool),
    }
}

/// Which derived capabilities correspond to a Sovereign action.
///
/// Two mappings are deliberately **absent**, because having them was worse than
/// useless:
///
/// - `InstallPackage` is *not* `PackagePublish`. Installing dependencies is what
///   every build does; publishing pushes an artifact out to a registry. Mapping
///   install onto publish made `mvn verify` and `npm install` demand approval
///   while `npm publish` — the actual supply-chain event — sailed through,
///   because nothing classified publishing at all.
/// - `ModifyGit` is *not* `GitHistoryRewrite`. `git commit` is a repository
///   write; rebasing and `filter-branch` destroy history. Conflating them made
///   every commit prompt, which is the fastest way to teach people to approve
///   without reading.
///
/// `InstallPackage` remains governed by the egress allowlist (a registry host
/// must be permitted) and the install-rate budget, which is the proportionate
/// control for it.
fn sovereign_map() -> [(Capability, SovereignAction); 5] {
    [
        (Capability::CredentialAccess, SovereignAction::CredentialAccess),
        (
            Capability::PrivilegeEscalation,
            SovereignAction::PrivilegeEscalation,
        ),
        (Capability::PublishPackage, SovereignAction::PackagePublish),
        (
            Capability::RewriteGitHistory,
            SovereignAction::GitHistoryRewrite,
        ),
        (Capability::ForcePush, SovereignAction::ForcePush),
    ]
}

/// Canonical rule identifier, e.g. `MANAGED-EXEC-002`.
///
/// Uses `Display`, not `Debug`: this string is what a developer pastes into a
/// ticket and what an auditor greps the decision log for, so it has to be the
/// documented rule id rather than a struct dump.
fn rule_label(rule: &Option<RuleId>, fallback: &str) -> String {
    rule.as_ref()
        .map(RuleId::to_string)
        .unwrap_or_else(|| fallback.to_owned())
}

fn action_label(intent: &Intent) -> String {
    match intent {
        Intent::Shell { .. } => "tool:bash".to_owned(),
        Intent::FileRead { .. } => "tool:read".to_owned(),
        Intent::FileWrite { .. } => "tool:write".to_owned(),
        Intent::Egress { .. } => "egress".to_owned(),
        Intent::Mcp { server, .. } => format!("mcp:{server}"),
        Intent::ToolCall { tool } => format!("tool:{tool}"),
    }
}

fn target_label(intent: &Intent) -> String {
    match intent {
        Intent::Shell { command, .. } => command.clone(),
        Intent::FileRead { path } | Intent::FileWrite { path } => path.clone(),
        Intent::Egress { url } => url.clone(),
        Intent::Mcp { tool, .. } => tool.clone(),
        Intent::ToolCall { tool } => tool.clone(),
    }
}

/// Host portion of a URL, without pulling in a URL parser. Only the host is
/// needed, and the egress allowlist matches on host.
fn host_of(url: &str) -> Option<String> {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let host = rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or(rest)
        .rsplit('@')
        .next()
        .unwrap_or(rest);
    let host = host.split(':').next().unwrap_or(host);
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

fn ticket_hash(command: &str) -> String {
    blake3::hash(command.as_bytes()).to_hex()[..32].to_string()
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Process-wide enforcement point, mirroring `ainxt_policy::global` so both are
/// installed and read the same way.
pub mod global {
    use std::sync::Arc;

    use arc_swap::ArcSwapOption;

    use super::Pep;

    static ACTIVE: ArcSwapOption<Pep> = ArcSwapOption::const_empty();

    pub fn install(pep: Pep) {
        ACTIVE.store(Some(Arc::new(pep)));
    }

    pub fn is_installed() -> bool {
        ACTIVE.load().is_some()
    }

    /// The active enforcement point, if one is installed.
    ///
    /// Returns `None` in an OSS build with no bundle, where callers keep their
    /// existing behaviour unchanged.
    pub fn active() -> Option<Arc<Pep>> {
        ACTIVE.load_full()
    }

    #[doc(hidden)]
    pub fn reset_for_test() {
        ACTIVE.store(None);
    }
}
