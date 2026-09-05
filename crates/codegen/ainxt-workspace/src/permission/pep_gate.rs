//! Adapter between this crate's permission vocabulary and the enforcement
//! point.
//!
//! The dependency runs one way only: `ainxt-workspace` → `ainxt-pep`, never the
//! reverse. `ainxt-pep` must stay free of `AccessKind`, ACP and prompts so that
//! a future non-CLI client reaches the same authority. Everything CLI-shaped
//! about a request is translated here.

use ainxt_pep::context::{default_shell, local_principal};
use ainxt_pep::{Intent, Obligation, Request};
use ainxt_policy_types::TrustTier;

use crate::permission::types::AccessKind;

/// What the permission layer must do about a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PepOutcome {
    /// No enforcement point installed, or the action is permitted. Existing
    /// behaviour continues unchanged.
    Proceed,
    /// A human must decide. Joined into the actor's forced-prompt gates so that
    /// YOLO, session grants, auto-mode and the sandbox auto-allow all fall
    /// through to the interactive prompt.
    ForcedPrompt(String),
    /// Refused outright.
    Deny(String),
}

/// Evaluate one access request.
///
/// Returns [`PepOutcome::Proceed`] when no enforcement point is installed —
/// which is every OSS build — so this is inert until a signed bundle is
/// present.
pub(crate) fn evaluate(
    access: &AccessKind,
    session_id: Option<&str>,
    subagent_type: Option<&str>,
) -> PepOutcome {
    let Some(pep) = ainxt_pep::global::active() else {
        return PepOutcome::Proceed;
    };

    let request = Request {
        principal: local_principal(session_id, subagent_type),
        intent: intent_for(access),
        influence: influence_tier(),
    };

    match pep.authorize(&request).obligation {
        Obligation::Proceed => PepOutcome::Proceed,
        Obligation::Prompt { reason, .. } => PepOutcome::ForcedPrompt(reason),
        Obligation::Refuse { reason } => PepOutcome::Deny(reason),
    }
}

/// Report whether a command succeeded, feeding failure-loop detection.
///
/// Without this the brute-force control is inert: the ledger can count
/// invocations but not distinguish a build loop from a password attack.
pub(crate) fn note_outcome(program: &str, success: bool) {
    if let Some(pep) = ainxt_pep::global::active() {
        pep.observe_effect(
            &local_principal(None, None),
            ainxt_pep::Effect::Outcome {
                program: program.to_owned(),
                success,
            },
        );
    }
}

fn intent_for(access: &AccessKind) -> Intent {
    match access {
        AccessKind::Bash(command) => Intent::Shell {
            command: command.clone(),
            shell: default_shell(),
        },
        AccessKind::Read(Some(path)) => Intent::FileRead { path: path.clone() },
        AccessKind::Read(None) => Intent::ToolCall {
            tool: "read".to_owned(),
        },
        AccessKind::Edit(path) => Intent::FileWrite { path: path.clone() },
        AccessKind::Grep { .. } => Intent::ToolCall {
            tool: "grep".to_owned(),
        },
        AccessKind::WebFetch(url) => Intent::Egress { url: url.clone() },
        AccessKind::WebSearch(_) => Intent::ToolCall {
            tool: "websearch".to_owned(),
        },
        // MCP arguments are opaque JSON that cannot be introspected, so the
        // only containment available is an allowlist on the (server, tool)
        // pair. Splitting the conventional `server__tool` name gives the
        // policy both halves to match on.
        AccessKind::MCPTool { name, .. } => {
            let (server, tool) = name
                .split_once("__")
                .map(|(s, t)| (s.to_owned(), t.to_owned()))
                .unwrap_or_else(|| ("unknown".to_owned(), name.clone()));
            Intent::Mcp { server, tool }
        }
    }
}


/// Trust tier of the content that produced this request.
///
/// **Known gap:** provenance span tagging is not yet wired into the agent loop,
/// so this is always `Operator` and the tier-based checks in the enforcement
/// point do not yet fire. Everything else — capabilities, budgets, artifacts —
/// is live. Wiring this is what activates the prompt-injection defence.
fn influence_tier() -> TrustTier {
    TrustTier::Operator
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn access_kinds_map_to_the_intended_intents() {
        assert!(matches!(
            intent_for(&AccessKind::Bash("ls".into())),
            Intent::Shell { .. }
        ));
        assert!(matches!(
            intent_for(&AccessKind::Read(Some("/tmp/x".into()))),
            Intent::FileRead { .. }
        ));
        assert!(matches!(
            intent_for(&AccessKind::Edit("/tmp/x".into())),
            Intent::FileWrite { .. }
        ));
        assert!(matches!(
            intent_for(&AccessKind::WebFetch("https://x/".into())),
            Intent::Egress { .. }
        ));
    }

    #[test]
    fn mcp_names_split_into_server_and_tool() {
        match intent_for(&AccessKind::MCPTool {
            name: "linear__create_issue".into(),
            input: serde_json::Value::Null,
        }) {
            Intent::Mcp { server, tool } => {
                assert_eq!(server, "linear");
                assert_eq!(tool, "create_issue");
            }
            other => panic!("expected an MCP intent, got {other:?}"),
        }
    }

    #[test]
    fn an_unqualified_mcp_name_still_yields_a_matchable_pair() {
        match intent_for(&AccessKind::MCPTool {
            name: "do_thing".into(),
            input: serde_json::Value::Null,
        }) {
            Intent::Mcp { server, tool } => {
                assert_eq!(server, "unknown");
                assert_eq!(tool, "do_thing");
            }
            other => panic!("expected an MCP intent, got {other:?}"),
        }
    }

    /// With no engine installed this must be completely inert, or every OSS
    /// build changes behaviour the moment the crate is linked.
    #[test]
    fn no_installed_pep_means_proceed() {
        assert_eq!(
            evaluate(&AccessKind::Bash("rm -rf /".into()), None, None),
            PepOutcome::Proceed
        );
    }
}
