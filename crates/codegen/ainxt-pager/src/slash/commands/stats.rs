//! `/stats` -- show current session's model, token, and cost usage.
//!
//! Distinct from `/usage` (alias `/cost`), which fetches the platform's
//! billing/credit-limit summary. `/stats` never leaves the local process: it
//! reads the session's own usage ledger (the same data backing the
//! status-bar cost indicator), so it works identically for OAuth, API-key,
//! and self-hosted deployments.

use crate::app::actions::Action;
use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand};

/// Show session usage stats (model, tokens, cost).
pub struct StatsCommand;

impl SlashCommand for StatsCommand {
    fn name(&self) -> &str {
        "stats"
    }

    fn description(&self) -> &str {
        "Show this session's model, token, and cost usage"
    }

    fn session_scoped(&self) -> bool {
        true
    }

    fn usage(&self) -> &str {
        "/stats"
    }

    fn run(&self, ctx: &mut CommandExecCtx, _args: &str) -> CommandResult {
        if ctx.session_id.is_none() {
            return CommandResult::Error("No active session".to_string());
        }

        CommandResult::Action(Action::ShowSessionUsage)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_and_usage() {
        let cmd = StatsCommand;
        assert_eq!(cmd.name(), "stats");
        assert_eq!(cmd.usage(), "/stats");
        assert!(cmd.session_scoped());
    }
}
