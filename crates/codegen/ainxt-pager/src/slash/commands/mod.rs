//! Concrete slash command implementations.
//!
//! Each command lives in its own submodule. This module re-exports
//! command structs and provides `builtin_commands()` for registry
//! construction.
pub mod always_approve;
pub mod announcements;
pub mod auto;
pub mod btw;
pub mod cd;
pub mod changes;
pub mod compact;
pub mod compact_mode;
pub mod config_agents;
pub mod context;
pub mod copy;
pub mod dashboard;
pub mod debug;
pub mod docs;
pub mod effort;
pub mod effort_levels;
pub mod exit;
pub mod expand;
pub mod export;
pub mod feedback;
pub mod find;
pub mod fork;
pub mod gateway;
pub mod policy;
pub mod help;
pub mod history;
pub mod home;
pub mod imagine;
pub mod imagine_video;
pub mod import_claude;
pub mod jump;
pub mod login;
pub mod logout;
pub mod loop_cmd;
pub mod mcps;
pub mod model;
pub mod multiline;
pub mod new;
pub mod personas;
pub mod plan;
pub mod plugin;
pub mod privacy;
pub mod queue;
pub mod recap;
pub mod release_notes;
pub mod remember;
pub mod rename;
pub mod resume;
pub mod rewind;
pub mod screen_mode_switch;
pub mod scroll_debug;
pub mod session_info;
pub mod settings_cmd;
pub mod share;
pub mod stats;
pub mod tasks;
pub mod terminal_setup;
pub mod theme;
pub mod timeline;
pub mod timestamps;
pub mod toggle_mouse_reporting;
pub mod transcript;
pub mod update;
pub mod usage;
pub mod view_plan;
pub mod vim_mode;
pub mod voice;
use super::command::SlashCommand;
use std::sync::Arc;
/// All pager-local builtin commands, in display order.
///
/// This is the single source of truth for the builtin command set.
/// The registry is constructed from this list.
pub fn builtin_commands() -> Vec<Arc<dyn SlashCommand>> {
    vec![
        Arc::new(exit::ExitCommand),
        Arc::new(help::HelpCommand),
        Arc::new(docs::DocsCommand),
        Arc::new(home::HomeCommand),
        Arc::new(new::NewCommand),
        
        Arc::new(fork::ForkCommand),
        Arc::new(compact::CompactCommand),
        Arc::new(copy::CopyCommand),
        Arc::new(find::FindCommand),
        Arc::new(history::HistoryCommand),
        Arc::new(export::ExportCommand),
        Arc::new(transcript::TranscriptCommand),
        Arc::new(expand::ExpandCommand),
        Arc::new(context::ContextCommand),
        Arc::new(changes::ChangesCommand),
        Arc::new(screen_mode_switch::ScreenModeSwitchCommand::minimal()),
        Arc::new(screen_mode_switch::ScreenModeSwitchCommand::fullscreen()),
        Arc::new(model::ModelCommand),
        Arc::new(effort::EffortCommand),
        Arc::new(always_approve::AlwaysApproveCommand),
        Arc::new(auto::AutoCommand),
        Arc::new(multiline::MultilineCommand),
        Arc::new(compact_mode::CompactModeCommand),
        Arc::new(vim_mode::VimModeCommand),
        Arc::new(plugin::HooksCommand),
        Arc::new(plugin::PluginsCommand),
        Arc::new(plugin::MarketplaceCommand),
        Arc::new(plugin::SkillsCommand),
        Arc::new(share::ShareCommand),
        Arc::new(session_info::SessionInfoCommand),
        Arc::new(stats::StatsCommand),
        Arc::new(rename::RenameCommand),
        Arc::new(dashboard::DashboardCommand),
        Arc::new(cd::CdCommand),
        Arc::new(theme::ThemeCommand),
        Arc::new(gateway::GatewayCommand),
        Arc::new(update::UpdateCommand),
        Arc::new(policy::PolicyCommand),
        Arc::new(feedback::FeedbackCommand),
        Arc::new(announcements::AnnouncementsCommand),
        Arc::new(remember::RememberCommand),
        Arc::new(plan::PlanCommand),
        Arc::new(view_plan::ViewPlanCommand),
        Arc::new(resume::ResumeCommand),
        Arc::new(mcps::McpsCommand),
        Arc::new(btw::BtwCommand),
        Arc::new(recap::RecapCommand),
        
        Arc::new(terminal_setup::TerminalSetupCommand),
        Arc::new(voice::VoiceCommand),
        Arc::new(loop_cmd::LoopCommand),
        Arc::new(imagine::ImagineCommand),
        Arc::new(imagine_video::ImagineVideoCommand),
        Arc::new(timestamps::TimestampsCommand),
        Arc::new(timeline::TimelineCommand),
        Arc::new(toggle_mouse_reporting::ToggleMouseReportingCommand),
        Arc::new(settings_cmd::SettingsCommand),
        Arc::new(privacy::PrivacyCommand),
        Arc::new(rewind::RewindCommand),
        Arc::new(jump::JumpCommand),
        Arc::new(login::LoginCommand),
        Arc::new(logout::LogoutCommand),
        Arc::new(import_claude::ImportClaudeCommand),
        Arc::new(usage::UsageCommand),
        Arc::new(queue::QueueCommand),
        Arc::new(tasks::TasksCommand),
        Arc::new(release_notes::ReleaseNotesCommand),
        Arc::new(config_agents::ConfigAgentsCommand),
        Arc::new(personas::PersonasCommand),
        Arc::new(scroll_debug::ScrollDebugCommand),
        Arc::new(debug::DebugCommand),
    ]
}
#[cfg(test)]
mod tests {
    /// Reserved-domain stand-in for a deployment-configured usage page.
    const USAGE_URL_FOR_TEST: &str = "https://usage.example.test/?_s=usage";

    use super::*;
    use crate::acp::model_state::ModelState;
    use crate::app::actions::Action;
    use crate::slash::command::{CommandExecCtx, CommandResult};
    use crate::slash::registry::CommandRegistry;
    use agent_client_protocol as acp;
    /// Build a ModelState with two models for testing.
    fn sample_models() -> ModelState {
        let mut models = ModelState::default();
        let id_fast = acp::ModelId::new(Arc::from("ainxt-4.5"));
        models.available.insert(
            id_fast.clone(),
            acp::ModelInfo::new(id_fast.clone(), "ainxt 4.5".to_string()),
        );
        let id_pro = acp::ModelId::new(Arc::from("ainxt-4.3"));
        models.available.insert(
            id_pro.clone(),
            acp::ModelInfo::new(id_pro.clone(), "ainxt 4.3".to_string()),
        );
        models.current = Some(id_fast);
        models
    }
    static DEFAULT_BUNDLE_STATE: crate::app::bundle::BundleState =
        crate::app::bundle::BundleState {
            has_cache: false,
            version: String::new(),
            personas: Vec::new(),
            roles: Vec::new(),
            agents: Vec::new(),
            skills: Vec::new(),
            persona_details: Vec::new(),
            role_details: Vec::new(),
        };
    pub(crate) fn make_ctx(models: &ModelState) -> CommandExecCtx<'_> {
        CommandExecCtx {
            models,
            session_id: None,
            bundle_state: &DEFAULT_BUNDLE_STATE,
            screen_mode: crate::app::ScreenMode::Inline,
            pager_state: crate::settings::PagerLocalSnapshot {
                multiline_mode: false,
                yolo_mode: false,
                ..crate::settings::PagerLocalSnapshot::default()
            },
        }
    }
    #[test]
    fn builtin_registry_lookup_by_canonical() {
        let mut reg = CommandRegistry::new(builtin_commands());
        assert!(reg.get("quit").is_some());
        assert!(reg.get("new").is_some());
        assert!(reg.get("compact").is_some());
        assert!(reg.get("model").is_some());
        assert!(reg.get("home").is_some());
        assert!(reg.get("view-plan").is_some());
        reg.set_available_tools(std::collections::HashSet::from([
            "scheduler_create".to_string()
        ]));
        assert!(reg.get("loop").is_some(), "/loop should be registered");
        assert!(
            reg.get("vim-mode").is_some(),
            "/vim-mode should be registered"
        );
        assert!(reg.get("find").is_some(), "/find should be registered");
    }
    #[test]
    fn loop_command_declares_scheduler_tool_requirement() {
        let loop_cmd = loop_cmd::LoopCommand;
        assert_eq!(loop_cmd.required_tools(), &["scheduler_create"]);
    }
    #[test]
    fn loop_command_hidden_when_scheduler_tools_absent() {
        let mut reg = CommandRegistry::new(builtin_commands());
        reg.set_available_tools(std::collections::HashSet::from([
            "read_file".to_string(),
            "grep".to_string(),
        ]));
        assert!(reg.get("loop").is_none(), "/loop should be hidden");
        assert!(reg.get("quit").is_some());
        assert!(reg.get("compact").is_some());
        reg.set_available_tools(std::collections::HashSet::from([
            "scheduler_create".to_string()
        ]));
        assert!(reg.get("loop").is_some());
    }
    #[test]
    fn builtin_registry_lookup_by_alias() {
        let reg = CommandRegistry::new(builtin_commands());
        assert!(reg.get("exit").is_some());
        assert!(reg.get("clear").is_some());
        assert!(reg.get("m").is_some());
        assert!(reg.get("welcome").is_some());
        assert!(reg.get("show-plan").is_some());
        assert!(reg.get("plan-view").is_some());
    }
    #[test]
    fn alias_resolves_to_same_command() {
        let reg = CommandRegistry::new(builtin_commands());
        let exit_cmd = reg.get("exit").unwrap();
        let quit_cmd = reg.get("quit").unwrap();
        assert_eq!(exit_cmd.name(), quit_cmd.name());
    }
    #[test]
    fn exit_returns_quit_action() {
        let models = ModelState::default();
        let mut ctx = make_ctx(&models);
        let cmd = exit::ExitCommand;
        let result = cmd.run(&mut ctx, "");
        assert!(matches!(result, CommandResult::Action(Action::Quit)));
    }
    #[test]
    fn new_returns_new_session_action() {
        let models = ModelState::default();
        let mut ctx = make_ctx(&models);
        let cmd = new::NewCommand;
        let result = cmd.run(&mut ctx, "");
        assert!(matches!(result, CommandResult::Action(Action::NewSession)));
    }
    #[test]
    fn home_returns_exit_session_action() {
        let models = ModelState::default();
        let mut ctx = make_ctx(&models);
        let cmd = home::HomeCommand;
        let result = cmd.run(&mut ctx, "");
        assert!(matches!(result, CommandResult::Action(Action::ExitSession)));
    }
    #[test]
    fn view_plan_returns_show_plan_action() {
        let models = ModelState::default();
        let mut ctx = make_ctx(&models);
        let cmd = view_plan::ViewPlanCommand;
        let result = cmd.run(&mut ctx, "");
        assert!(matches!(result, CommandResult::Action(Action::ShowPlan)));
    }
    #[test]
    fn compact_no_args_returns_queue_command() {
        let models = ModelState::default();
        let mut ctx = make_ctx(&models);
        let cmd = compact::CompactCommand;
        let result = cmd.run(&mut ctx, "");
        match result {
            CommandResult::QueueCommand(text) => assert_eq!(text, "/compact"),
            other => panic!("expected QueueCommand, got {other:?}"),
        }
    }
    #[test]
    fn compact_with_context_returns_queue_command_with_args() {
        let models = ModelState::default();
        let mut ctx = make_ctx(&models);
        let cmd = compact::CompactCommand;
        let result = cmd.run(&mut ctx, "focus on auth");
        match result {
            CommandResult::QueueCommand(text) => {
                assert_eq!(text, "/compact focus on auth")
            }
            other => panic!("expected QueueCommand, got {other:?}"),
        }
    }
    #[test]
    fn compact_whitespace_only_args_treated_as_no_args() {
        let models = ModelState::default();
        let mut ctx = make_ctx(&models);
        let cmd = compact::CompactCommand;
        let result = cmd.run(&mut ctx, "   ");
        match result {
            CommandResult::QueueCommand(text) => assert_eq!(text, "/compact"),
            other => panic!("expected QueueCommand, got {other:?}"),
        }
    }
    /// Bare `/model <name>` → `SetDefaultModel` (switch + persist).
    /// `/model <name> <effort>` → `SwitchModel` (session-scoped).
    #[test]
    fn model_resolves_by_display_name() {
        let models = sample_models();
        let mut ctx = make_ctx(&models);
        let cmd = model::ModelCommand;
        let result = cmd.run(&mut ctx, "ainxt 4.5");
        match result {
            CommandResult::Action(Action::SetDefaultModel(id)) => {
                assert_eq!(id.0.as_ref(), "ainxt-4.5");
            }
            other => panic!("expected Action(SetDefaultModel), got {other:?}"),
        }
    }
    #[test]
    fn model_resolves_by_model_id() {
        let models = sample_models();
        let mut ctx = make_ctx(&models);
        let cmd = model::ModelCommand;
        let result = cmd.run(&mut ctx, "ainxt-4.3");
        match result {
            CommandResult::Action(Action::SetDefaultModel(id)) => {
                assert_eq!(id.0.as_ref(), "ainxt-4.3");
            }
            other => panic!("expected Action(SetDefaultModel), got {other:?}"),
        }
    }
    #[test]
    fn model_resolves_case_insensitively() {
        let models = sample_models();
        let mut ctx = make_ctx(&models);
        let cmd = model::ModelCommand;
        let result = cmd.run(&mut ctx, "ainxt 4.5");
        match result {
            CommandResult::Action(Action::SetDefaultModel(id)) => {
                assert_eq!(id.0.as_ref(), "ainxt-4.5");
            }
            other => panic!("expected Action(SetDefaultModel), got {other:?}"),
        }
    }
    #[test]
    fn model_invalid_arg_returns_error() {
        let models = sample_models();
        let mut ctx = make_ctx(&models);
        let cmd = model::ModelCommand;
        let result = cmd.run(&mut ctx, "nonexistent-model");
        match result {
            CommandResult::Error(msg) => {
                assert!(
                    msg.contains("nonexistent-model"),
                    "error should contain the arg"
                );
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }
    #[test]
    fn model_empty_arg_returns_error() {
        let models = sample_models();
        let mut ctx = make_ctx(&models);
        let cmd = model::ModelCommand;
        let result = cmd.run(&mut ctx, "");
        assert!(matches!(result, CommandResult::Error(_)));
    }
    #[test]
    fn model_whitespace_only_arg_returns_error() {
        let models = sample_models();
        let mut ctx = make_ctx(&models);
        let cmd = model::ModelCommand;
        let result = cmd.run(&mut ctx, "   ");
        assert!(matches!(result, CommandResult::Error(_)));
    }
    #[test]
    fn model_suggest_args_returns_available_models() {
        let models = sample_models();
        let ctx = crate::slash::command::AppCtx {
            models: &models,
            cwd: std::path::Path::new("."),
            has_session_announcements: false,
            screen_mode: crate::app::ScreenMode::Fullscreen,
        };
        let cmd = model::ModelCommand;
        let items = cmd.suggest_args(&ctx, "").expect("should have suggestions");
        assert_eq!(items.len(), 2);
        assert!(
            items
                .iter()
                .any(|i| i.display.starts_with("ainxt 4.5") && i.insert_text == "ainxt 4.5")
        );
        assert!(
            items
                .iter()
                .any(|i| i.display == "ainxt 4.3" && i.insert_text == "ainxt 4.3")
        );
    }
    #[test]
    fn model_suggest_args_empty_models_returns_none() {
        let models = ModelState::default();
        let ctx = crate::slash::command::AppCtx {
            models: &models,
            cwd: std::path::Path::new("."),
            has_session_announcements: false,
            screen_mode: crate::app::ScreenMode::Fullscreen,
        };
        let cmd = model::ModelCommand;
        assert!(cmd.suggest_args(&ctx, "").is_none());
    }
    #[test]
    fn remember_no_args_enters_remember_mode() {
        let models = ModelState::default();
        let mut ctx = make_ctx(&models);
        let cmd = remember::RememberCommand;
        let result = cmd.run(&mut ctx, "");
        assert!(matches!(
            result,
            CommandResult::Action(Action::EnterRememberMode)
        ));
    }
    #[test]
    fn remember_with_args_sends_note() {
        let models = ModelState::default();
        let mut ctx = make_ctx(&models);
        let cmd = remember::RememberCommand;
        let result = cmd.run(&mut ctx, "important detail");
        match result {
            CommandResult::Action(Action::SendRememberNote(text)) => {
                assert_eq!(text, "important detail");
            }
            other => panic!("expected SendRememberNote, got {other:?}"),
        }
    }
    #[test]
    fn remember_whitespace_only_enters_remember_mode() {
        let models = ModelState::default();
        let mut ctx = make_ctx(&models);
        let cmd = remember::RememberCommand;
        let result = cmd.run(&mut ctx, "   ");
        assert!(matches!(
            result,
            CommandResult::Action(Action::EnterRememberMode)
        ));
    }
    fn run_usage(args: &str) -> CommandResult {
        let models = ModelState::default();
        let mut ctx = make_ctx(&models);
        usage::UsageCommand.run(&mut ctx, args)
    }
    #[test]
    fn usage_no_args_returns_show_usage() {
        assert!(matches!(
            run_usage(""),
            CommandResult::Action(Action::ShowUsage)
        ));
    }
    #[test]
    fn usage_show_returns_show_usage() {
        assert!(matches!(
            run_usage("show"),
            CommandResult::Action(Action::ShowUsage)
        ));
    }
    // `/usage manage` resolves through `ainxt_env::url_usage()`, and this build
    // ships `URL_USAGE` empty (audit risk R42) — so the URL has to come from the
    // documented `AINXT_URL_USAGE` override. These tests previously asserted a
    // hardcoded `ainxt.example.test` value that the constant no longer returns.
    // Serialised because the guard mutates process-wide env.
    #[test]
    #[serial_test::serial(AINXT_URL_USAGE)]
    fn usage_manage_returns_open_url() {
        let _env = crate::test_util::EnvVarGuard::set("AINXT_URL_USAGE", USAGE_URL_FOR_TEST);
        match run_usage("manage") {
            CommandResult::Action(Action::OpenUrl(url)) => {
                assert_eq!(url, USAGE_URL_FOR_TEST);
            }
            other => panic!("expected Action(OpenUrl), got {other:?}"),
        }
    }

    /// With no `AINXT_URL_USAGE` configured there is no page to open, so
    /// `/usage manage` must not hand the UI an empty URL to launch.
    #[test]
    #[serial_test::serial(AINXT_URL_USAGE)]
    fn usage_manage_with_no_url_configured_yields_empty_url() {
        let _env = crate::test_util::EnvVarGuard::set("AINXT_URL_USAGE", "");
        match run_usage("manage") {
            CommandResult::Action(Action::OpenUrl(url)) => {
                assert!(
                    url.is_empty(),
                    "unconfigured build must not produce a usage URL, got {url:?}"
                );
            }
            other => panic!("expected Action(OpenUrl), got {other:?}"),
        }
    }
    #[test]
    fn usage_invalid_arg_returns_error() {
        match run_usage("delete") {
            CommandResult::Error(msg) => {
                assert!(msg.contains("delete"), "got: {msg}");
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }
    #[test]
    fn usage_whitespace_only_treated_as_no_args() {
        assert!(matches!(
            run_usage("   "),
            CommandResult::Action(Action::ShowUsage)
        ));
    }
    #[test]
    fn usage_show_with_leading_whitespace() {
        assert!(matches!(
            run_usage("  show  "),
            CommandResult::Action(Action::ShowUsage)
        ));
    }
    #[test]
    #[serial_test::serial(AINXT_URL_USAGE)]
    fn usage_manage_with_leading_whitespace() {
        let _env = crate::test_util::EnvVarGuard::set("AINXT_URL_USAGE", USAGE_URL_FOR_TEST);
        match run_usage("  manage  ") {
            CommandResult::Action(Action::OpenUrl(url)) => {
                assert_eq!(url, USAGE_URL_FOR_TEST);
            }
            other => panic!("expected Action(OpenUrl), got {other:?}"),
        }
    }
    #[test]
    fn usage_suggest_args_returns_show_and_manage() {
        let models = ModelState::default();
        let ctx = crate::slash::command::AppCtx {
            models: &models,
            cwd: std::path::Path::new("."),
            has_session_announcements: false,
            screen_mode: crate::app::ScreenMode::Fullscreen,
        };
        let items = usage::UsageCommand
            .suggest_args(&ctx, "")
            .expect("should have suggestions");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].display, "show");
        assert_eq!(items[0].insert_text, "show");
        assert_eq!(items[1].display, "manage");
        assert_eq!(items[1].insert_text, "manage");
    }
    #[test]
    fn usage_metadata() {
        let cmd = usage::UsageCommand;
        assert_eq!(cmd.name(), "usage");
        assert!(cmd.takes_args());
        assert_eq!(cmd.arg_placeholder(), Some("show | manage"));
        assert!(!cmd.description().is_empty());
        assert!(!cmd.usage().is_empty());
    }
    #[test]
    fn usage_registered_in_builtin_commands() {
        let reg = CommandRegistry::new(builtin_commands());
        assert!(
            reg.get("usage").is_some(),
            "/usage should be registered in builtins"
        );
    }
    #[test]
    fn cd_registered_in_builtin_commands() {
        let reg = CommandRegistry::new(builtin_commands());
        assert!(
            reg.get("cd").is_some(),
            "/cd should be registered in builtins"
        );
    }
    #[test]
    fn queue_registered_in_builtin_commands() {
        let reg = CommandRegistry::new(builtin_commands());
        assert!(
            reg.get("queue").is_some(),
            "/queue should be registered in builtins"
        );
    }
    #[test]
    fn tasks_registered_in_builtin_commands() {
        let reg = CommandRegistry::new(builtin_commands());
        assert!(
            reg.get("tasks").is_some(),
            "/tasks should be registered in builtins"
        );
    }
    #[test]
    fn cost_aliases_usage() {
        let reg = CommandRegistry::new(builtin_commands());
        let cost = reg.get("cost").expect("/cost should resolve");
        assert_eq!(cost.name(), "usage", "/cost must alias /usage");
    }
    #[test]
    fn debug_is_registered_and_executable() {
        let reg = CommandRegistry::new(builtin_commands());
        assert!(reg.get("debug").is_some(), "/debug must be executable");
    }
    #[test]
    fn update_is_registered_and_dispatches_quit_for_update() {
        let reg = CommandRegistry::new(builtin_commands());
        assert!(reg.get("update").is_some(), "/update must be registered");
        let models = ModelState::default();
        let mut ctx = make_ctx(&models);
        let result = reg.get("update").unwrap().run(&mut ctx, "");
        assert!(
            matches!(result, CommandResult::Action(Action::QuitForUpdate)),
            "/update must dispatch QuitForUpdate, got {result:?}"
        );
    }
    #[test]
    fn minimal_and_fullscreen_registered_in_builtin_commands() {
        let reg = CommandRegistry::new(builtin_commands());
        assert!(reg.get("minimal").is_some());
        assert!(reg.get("fullscreen").is_some());
        assert!(reg.get("full").is_some());
        assert_eq!(
            reg.get("full").unwrap().name(),
            reg.get("fullscreen").unwrap().name()
        );
    }
    #[test]
    fn recap_registered_in_builtin_commands() {
        let mut reg = CommandRegistry::new(builtin_commands());
        reg.set_recap_visible(true);
        assert!(
            reg.get("recap").is_some(),
            "/recap should be registered in builtins"
        );
    }
    #[test]
    fn recap_returns_manual_send_recap_action() {
        let models = ModelState::default();
        let mut ctx = make_ctx(&models);
        let cmd = recap::RecapCommand;
        let result = cmd.run(&mut ctx, "");
        assert!(matches!(
            result,
            CommandResult::Action(Action::SendRecap { auto: false })
        ));
    }
    #[test]
    fn recap_hidden_by_default_in_registry_until_revealed() {
        let mut reg = CommandRegistry::new(builtin_commands());
        assert!(
            reg.get("recap").is_none(),
            "/recap must be fail-closed until shell advertises sessionRecap"
        );
        reg.set_recap_visible(true);
        assert!(reg.get("recap").is_some());
        reg.set_recap_visible(false);
        assert!(reg.get("recap").is_none());
    }
    #[test]
    fn voice_hidden_by_default_in_registry_until_revealed() {
        let mut reg = CommandRegistry::new(builtin_commands());
        assert!(
            reg.get("voice").is_none(),
            "/voice must be fail-closed until set_voice_visible(true)"
        );
        reg.set_voice_visible(true);
        assert!(reg.get("voice").is_some());
        reg.set_voice_visible(false);
        assert!(reg.get("voice").is_none());
    }
}
