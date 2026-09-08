//! The capability vocabulary: what an action *requests*.
//!
//! Deliberately distinct from `ainxt_policy_types::capability::SecurityCapabilities`,
//! which describes what a principal is *granted*. Enforcement is the question
//! "is the requested set contained in the granted set", so the two must not
//! share a name or a type.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// A class of dangerous thing an action can do.
///
/// This list is intentionally coarse. Fine-grained distinctions belong in the
/// policy rules that consume it, not here — a bigger vocabulary would mean more
/// ways for a novel action to fall between two categories and be permitted by
/// omission, which inverts the deny-by-default property we want.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    FsRead,
    FsWrite,
    FsDelete,
    /// Runs a program. Every shell command has this.
    ExecuteProcess,
    /// Interprets a string as code (`bash -c`, `eval`, `Invoke-Expression`).
    /// The capability that turns a download into an execution.
    ShellInterpretation,
    NetworkConnect,
    Download,
    Upload,
    /// Fetches and installs dependencies. Routine — governed by the egress
    /// allowlist and the install-rate budget, deliberately *not* by a prompt.
    /// Every build does this, so demanding approval would train people to
    /// approve blindly.
    InstallPackage,
    /// Pushes artifacts *out* to a registry (`npm publish`, `cargo publish`,
    /// `mvn deploy`, `twine upload`). A supply-chain event, and nothing like
    /// installing — conflating the two is how a control ends up prompting on
    /// every build while waving the dangerous case through.
    PublishPackage,
    /// Reads credential material: SSH/AWS/gcloud config, keychains, netrc.
    CredentialAccess,
    /// Ordinary repository writes: commit, checkout, merge. Not dangerous.
    ModifyGit,
    /// Destroys or rewrites existing history: rebase, `filter-branch`,
    /// `reset --hard`, `commit --amend`, `gc --prune`.
    RewriteGitHistory,
    /// Overwrites a remote branch non-fast-forward.
    ForcePush,
    /// Fetches content and feeds it to an interpreter in the same command
    /// (`curl … | bash`, `wget -O x.sh … && sh x.sh`).
    ///
    /// A composition, not a program: neither half is damning alone, which is
    /// exactly why an allowlist over *programs* cannot express it. Once `curl`
    /// is legitimately allowlisted for API work, this is the only thing left
    /// standing between a permitted fetcher and arbitrary remote code.
    RemoteCodeExecution,
    /// Signals, service control, process termination.
    ProcessControl,
    /// Touches OS-owned paths (`/etc`, `C:\Windows`, registry hives).
    SystemPath,
    PrivilegeEscalation,
    McpInvoke,
    AgentSpawn,
}

impl Capability {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FsRead => "fs_read",
            Self::FsWrite => "fs_write",
            Self::FsDelete => "fs_delete",
            Self::ExecuteProcess => "execute_process",
            Self::ShellInterpretation => "shell_interpretation",
            Self::NetworkConnect => "network_connect",
            Self::Download => "download",
            Self::Upload => "upload",
            Self::InstallPackage => "install_package",
            Self::PublishPackage => "publish_package",
            Self::CredentialAccess => "credential_access",
            Self::ModifyGit => "modify_git",
            Self::RewriteGitHistory => "rewrite_git_history",
            Self::ForcePush => "force_push",
            Self::RemoteCodeExecution => "remote_code_execution",
            Self::ProcessControl => "process_control",
            Self::SystemPath => "system_path",
            Self::PrivilegeEscalation => "privilege_escalation",
            Self::McpInvoke => "mcp_invoke",
            Self::AgentSpawn => "agent_spawn",
        }
    }

    /// Capabilities that should never be granted implicitly. Used only to order
    /// explanations and prompts — the actual grant decision is the policy's.
    pub fn is_high_risk(self) -> bool {
        matches!(
            self,
            Self::CredentialAccess
                | Self::PrivilegeEscalation
                | Self::ShellInterpretation
                | Self::InstallPackage
                | Self::PublishPackage
                | Self::RewriteGitHistory
                | Self::ForcePush
                | Self::RemoteCodeExecution
                | Self::SystemPath
        )
    }
}

/// How much of the action we were able to decompose.
///
/// Variant order is the ordering: `Exact < Partial < Unknown`, so merging two
/// derivations with `max` keeps the *worst* confidence. That is the whole point
/// — one unparseable segment in a pipeline taints the entire command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    /// Full grammar parse; the derived set is complete.
    Exact,
    /// Recognised command forms without a full grammar (the Windows tokenizer).
    /// The derived set is a lower bound: there may be more we did not see.
    Partial,
    /// Could not be decomposed. Callers MUST fail closed — an empty capability
    /// set here means "we do not know", never "it is safe".
    Unknown,
}

/// Why a capability was attributed. Surfaced verbatim by `ainxt policy explain`,
/// so it is written for a developer trying to understand a denial.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Evidence {
    pub capability: Capability,
    pub detail: String,
}

/// The concrete objects an action touches. Policy matches allowlists against
/// these; `Derivation::capabilities` only says which *kind* of thing happens.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Targets {
    /// Resolved program name per pipeline segment, in order.
    pub programs: Vec<String>,
    pub reads: Vec<String>,
    pub writes: Vec<String>,
    pub urls: Vec<String>,
}

impl Targets {
    fn push_unique(list: &mut Vec<String>, value: String) {
        if !list.contains(&value) {
            list.push(value);
        }
    }

    pub fn add_program(&mut self, value: impl Into<String>) {
        Self::push_unique(&mut self.programs, value.into());
    }

    pub fn add_read(&mut self, value: impl Into<String>) {
        Self::push_unique(&mut self.reads, value.into());
    }

    pub fn add_write(&mut self, value: impl Into<String>) {
        Self::push_unique(&mut self.writes, value.into());
    }

    pub fn add_url(&mut self, value: impl Into<String>) {
        Self::push_unique(&mut self.urls, value.into());
    }

    fn merge(&mut self, other: Targets) {
        for p in other.programs {
            self.add_program(p);
        }
        for r in other.reads {
            self.add_read(r);
        }
        for w in other.writes {
            self.add_write(w);
        }
        for u in other.urls {
            self.add_url(u);
        }
    }
}

/// What an action requests. Produced by the `derive_*` functions; consumed by
/// the policy engine. Contains no decision and no policy state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Derivation {
    pub capabilities: BTreeSet<Capability>,
    pub confidence: Confidence,
    pub targets: Targets,
    pub evidence: Vec<Evidence>,
}

impl Derivation {
    pub fn new(confidence: Confidence) -> Self {
        Self {
            capabilities: BTreeSet::new(),
            confidence,
            targets: Targets::default(),
            evidence: Vec::new(),
        }
    }

    /// Fail-closed constructor for something we could not decompose.
    ///
    /// Deliberately still claims `ExecuteProcess`: we may not know *what* runs,
    /// but we know something does. An empty set plus `Confidence::Unknown` would
    /// be indistinguishable from "harmless" to a careless caller.
    pub fn unknown(reason: impl Into<String>) -> Self {
        let mut d = Self::new(Confidence::Unknown);
        d.add(Capability::ExecuteProcess, reason);
        d
    }

    pub fn add(&mut self, capability: Capability, detail: impl Into<String>) {
        let detail = detail.into();
        if self.capabilities.insert(capability)
            || !self
                .evidence
                .iter()
                .any(|e| e.capability == capability && e.detail == detail)
        {
            self.evidence.push(Evidence { capability, detail });
        }
    }

    pub fn has(&self, capability: Capability) -> bool {
        self.capabilities.contains(&capability)
    }

    /// Union of capabilities and targets; confidence degrades to the worst of
    /// the two. Used to fold pipeline segments into a whole-command view.
    pub fn merge(&mut self, other: Derivation) {
        self.confidence = self.confidence.max(other.confidence);
        self.capabilities.extend(other.capabilities);
        self.targets.merge(other.targets);
        for e in other.evidence {
            if !self.evidence.contains(&e) {
                self.evidence.push(e);
            }
        }
    }

    /// High-risk capabilities first, then declaration order. Only affects how a
    /// denial is explained.
    pub fn sorted_capabilities(&self) -> Vec<Capability> {
        let mut out: Vec<Capability> = self.capabilities.iter().copied().collect();
        out.sort_by_key(|c| (!c.is_high_risk(), *c));
        out
    }
}
