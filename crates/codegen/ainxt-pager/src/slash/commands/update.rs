//! `/update` — trigger a CLI self-update from the gateway.
//!
//! Dispatches `Action::QuitForUpdate`, which:
//!   1. Quits the TUI cleanly.
//!   2. Lets `main.rs::finish_update_on_exit` run `ainxt update` (blocking).
//!   3. Downloads the latest binary from the gateway, verifies SHA-256,
//!      smoke-tests it, and activates it atomically.
//!   4. Replaces the binary that launched this session in place — keeping
//!      its original name (`ainxt.exe` on Windows, `ainxt` elsewhere) and
//!      saving the previous build beside it as
//!      `<name>-backup-<version>-<UTC timestamp>` (no dots, no file
//!      extension — never a double extension like `.exe.backup`) — so the
//!      next launch of that same path runs the new version. The three most
//!      recent backups are kept.
//!   5. Prints "Update installed. Run `ainxt` to start." on success.
//!
//! If the CLI is already on the latest version, `ainxt update` reports
//! "Already up to date" and exits cleanly.

use crate::app::actions::Action;
use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand};

/// Trigger a CLI self-update from the gateway and quit the TUI.
pub struct UpdateCommand;

impl SlashCommand for UpdateCommand {
    fn name(&self) -> &str {
        "update"
    }

    fn description(&self) -> &str {
        "Update the CLI to the latest version from the gateway"
    }

    fn usage(&self) -> &str {
        "/update"
    }

    fn run(&self, _ctx: &mut CommandExecCtx, _args: &str) -> CommandResult {
        CommandResult::Action(Action::QuitForUpdate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::model_state::ModelState;
    use crate::app::actions::Action;
    use crate::slash::command::{CommandExecCtx, CommandResult};
    use crate::app::bundle::BundleState;

    fn make_ctx(models: &ModelState) -> CommandExecCtx<'_> {
        static BUNDLE: BundleState = BundleState {
            has_cache: false,
            version: String::new(),
            personas: Vec::new(),
            roles: Vec::new(),
            agents: Vec::new(),
            skills: Vec::new(),
            persona_details: Vec::new(),
            role_details: Vec::new(),
        };
        CommandExecCtx {
            models,
            session_id: None,
            bundle_state: &BUNDLE,
            screen_mode: crate::app::ScreenMode::Inline,
            pager_state: crate::settings::PagerLocalSnapshot {
                multiline_mode: false,
                yolo_mode: false,
                ..crate::settings::PagerLocalSnapshot::default()
            },
        }
    }

    #[test]
    fn update_dispatches_quit_for_update() {
        let models = ModelState::default();
        let mut ctx = make_ctx(&models);
        let result = UpdateCommand.run(&mut ctx, "");
        assert!(
            matches!(result, CommandResult::Action(Action::QuitForUpdate)),
            "expected QuitForUpdate, got {result:?}"
        );
    }

    #[test]
    fn update_ignores_args() {
        let models = ModelState::default();
        let mut ctx = make_ctx(&models);
        // Any args (e.g. accidental text) are silently ignored.
        let result = UpdateCommand.run(&mut ctx, "--force");
        assert!(matches!(result, CommandResult::Action(Action::QuitForUpdate)));
    }

    #[test]
    fn update_metadata() {
        let cmd = UpdateCommand;
        assert_eq!(cmd.name(), "update");
        assert!(!cmd.description().is_empty());
        assert!(!cmd.usage().is_empty());
        assert!(!cmd.takes_args());
        assert!(!cmd.args_required());
    }
}
