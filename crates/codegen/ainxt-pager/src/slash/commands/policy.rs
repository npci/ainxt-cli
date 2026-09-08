//! `/policy [command]` — show the live security posture inside a session.
//!
//! The same information as `ainxt policy status`, reachable without leaving the
//! TUI. That matters at exactly the moment it is needed: a tool has just been
//! refused, and the question is "why, and am I frozen?" — asking someone to
//! open a second terminal to find out is how a control gets a reputation for
//! being inscrutable.
//!
//! Read-only. Nothing here can change the posture; loosening policy from inside
//! a session the model can influence would defeat the point of having it.

use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand};

pub struct PolicyCommand;

impl SlashCommand for PolicyCommand {
    fn name(&self) -> &str {
        "policy"
    }

    fn description(&self) -> &str {
        "Show the security policy posture and session risk state"
    }

    fn usage(&self) -> &str {
        "/policy [explain <command>]"
    }

    fn takes_args(&self) -> bool {
        true
    }

    fn args_required(&self) -> bool {
        false
    }

    fn arg_placeholder(&self) -> Option<&str> {
        Some("[explain <command>]")
    }

    fn run(&self, _ctx: &mut CommandExecCtx, args: &str) -> CommandResult {
        let args = args.trim();
        if let Some(command) = args.strip_prefix("explain").map(str::trim)
            && !command.is_empty()
        {
            return CommandResult::Message(explain(command));
        }
        CommandResult::Message(status())
    }
}

fn status() -> String {
    let engine = ainxt_policy::global::active();
    let posture = match engine.enforcement() {
        ainxt_policy_types::Enforcement::Off => "off — nothing is enforced",
        ainxt_policy_types::Enforcement::Warn => "observe — evaluated and recorded, not enforced",
        ainxt_policy_types::Enforcement::Block => "block — enforced",
    };

    let mut out = format!("Security policy: {posture}\n");

    let Some(pep) = ainxt_pep::global::active() else {
        out.push_str("Enforcement point: not installed (no signed policy bundle).\n");
        return out;
    };

    let principal = ainxt_pep::context::local_principal(None, None);
    match pep.risk_snapshot(&principal) {
        Ok(state) => {
            out.push_str(&format!(
                "Session risk: {} actions, {} installs, {} hosts, {} consecutive failures\n",
                state.execs_in_window,
                state.installs_in_window,
                state.distinct_hosts_in_window,
                state.max_consecutive_failures,
            ));
            match &state.frozen {
                Some(freeze) => out.push_str(&format!(
                    "FROZEN: {}\nA human must clear the freeze before work continues.\n",
                    freeze.describe()
                )),
                None => out.push_str("Not frozen.\n"),
            }
        }
        Err(err) => out.push_str(&format!("Session risk unavailable: {err}\n")),
    }

    out.push_str("\nRun `ainxt policy show` for the full allow/deny list.");
    out
}

/// Dry-run a command against the live policy.
///
/// Goes through `Pep::explain`, which shares `judge()` with the enforcement
/// path and spends no budget — so asking "would this be allowed?" cannot itself
/// push a session closer to a freeze.
fn explain(command: &str) -> String {
    let Some(pep) = ainxt_pep::global::active() else {
        return "No enforcement point installed; nothing would be refused.".to_owned();
    };

    let request = ainxt_pep::Request {
        principal: ainxt_pep::context::local_principal(None, None),
        intent: ainxt_pep::Intent::Shell {
            command: command.to_owned(),
            shell: ainxt_pep::context::default_shell(),
        },
        influence: ainxt_policy_types::TrustTier::Operator,
    };

    let auth = pep.explain(&request);
    let capabilities = auth
        .derivation
        .sorted_capabilities()
        .iter()
        .map(|c| c.as_str())
        .collect::<Vec<_>>()
        .join(", ");

    let verdict = match &auth.judgement {
        ainxt_pep::Judgement::Permit => "PERMIT".to_owned(),
        ainxt_pep::Judgement::RequireHuman { reason, .. } => {
            format!("NEEDS HUMAN APPROVAL — {reason}")
        }
        ainxt_pep::Judgement::Deny { rule, reason } => format!("DENY [{rule}] — {reason}"),
    };

    format!("{command}\n  capabilities: {capabilities}\n  {verdict}")
}
