//! `/changes` -- open the source-control view (changed files in the tree).
//!
//! Phase A: dispatches [`Action::OpenSourceControl`], which surfaces a
//! "coming soon" toast. Phase B wires this to the real changed-files panel.

use crate::app::actions::Action;
use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand};

/// Open the source-control view listing files changed in the working tree.
pub struct ChangesCommand;

impl SlashCommand for ChangesCommand {
    fn name(&self) -> &str {
        "changes"
    }

    fn description(&self) -> &str {
        "View files changed in the working tree"
    }

    fn session_scoped(&self) -> bool {
        true
    }

    fn usage(&self) -> &str {
        "/changes"
    }

    fn run(&self, ctx: &mut CommandExecCtx, _args: &str) -> CommandResult {
        if ctx.session_id.is_none() {
            return CommandResult::Error("No active session".to_string());
        }

        CommandResult::Action(Action::OpenSourceControl)
    }
}
