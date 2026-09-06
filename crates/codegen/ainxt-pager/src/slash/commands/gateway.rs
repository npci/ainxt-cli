//! `/gateway <url>` -- set the ainxt gateway base URL.
//!
//! The gateway URL is frozen at process startup (`EndpointsConfig` reads
//! `[endpoints]` once at launch), so this command does NOT hot-swap the
//! live connection. `run` dispatches `Action::SetGatewayUrl(<url>)` — the
//! dispatcher persists both derived `[endpoints]` keys to `config.toml`
//! and shows a "takes effect on restart" toast.

use crate::app::actions::Action;
use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand};

/// Set the ainxt gateway base URL (applies on restart).
pub struct GatewayCommand;

impl SlashCommand for GatewayCommand {
    fn name(&self) -> &str {
        "gateway"
    }

    fn description(&self) -> &str {
        "Set the ainxt gateway URL (applies on restart)"
    }

    fn usage(&self) -> &str {
        "/gateway <url>"
    }

    fn takes_args(&self) -> bool {
        true
    }

    fn args_required(&self) -> bool {
        true
    }

    fn arg_placeholder(&self) -> Option<&str> {
        Some("<url>")
    }

    fn run(&self, _ctx: &mut CommandExecCtx, args: &str) -> CommandResult {
        let trimmed = args.trim();
        if trimmed.is_empty() {
            return CommandResult::Error("usage: /gateway <url>".to_string());
        }

        // Validate it parses as an absolute URL with an http/https scheme.
        // Anything else (missing scheme, relative path, ftp://, …) is
        // rejected so we never persist an unusable endpoint.
        match url::Url::parse(trimmed) {
            Ok(url) if matches!(url.scheme(), "http" | "https") => {
                CommandResult::Action(Action::SetGatewayUrl(trimmed.to_string()))
            }
            _ => CommandResult::Error(format!(
                "Invalid gateway URL: {trimmed}. Expected an http(s) URL, e.g. \
                 https://gateway.example.com"
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::model_state::ModelState;

    fn run(args: &str) -> CommandResult {
        let models = ModelState::default();
        let bundle = crate::app::bundle::BundleState::default();
        let mut ctx = CommandExecCtx {
            models: &models,
            session_id: None,
            bundle_state: &bundle,
            screen_mode: crate::app::ScreenMode::Inline,
            pager_state: crate::settings::PagerLocalSnapshot {
                multiline_mode: false,
                yolo_mode: false,
                ..crate::settings::PagerLocalSnapshot::default()
            },
        };
        GatewayCommand.run(&mut ctx, args)
    }

    /// A valid http(s) URL dispatches `Action::SetGatewayUrl(<trimmed>)`.
    #[test]
    fn run_valid_url_dispatches_set_gateway_url_action() {
        let result = run("  https://gateway.example.com  ");
        match result {
            CommandResult::Action(Action::SetGatewayUrl(url)) => {
                assert_eq!(url, "https://gateway.example.com");
            }
            other => panic!("expected Action::SetGatewayUrl(...), got {other:?}"),
        }
    }

    /// Plain http is accepted too.
    #[test]
    fn run_http_url_dispatches_set_gateway_url_action() {
        let result = run("http://localhost:8000");
        match result {
            CommandResult::Action(Action::SetGatewayUrl(url)) => {
                assert_eq!(url, "http://localhost:8000");
            }
            other => panic!("expected Action::SetGatewayUrl(...), got {other:?}"),
        }
    }

    /// Empty args return the usage error.
    #[test]
    fn run_empty_returns_error() {
        match run("   ") {
            CommandResult::Error(msg) => {
                assert!(msg.contains("usage"), "expected usage error, got: {msg}");
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    /// A non-URL / wrong-scheme value is rejected.
    #[test]
    fn run_invalid_url_returns_error() {
        match run("not a url") {
            CommandResult::Error(msg) => {
                assert!(msg.contains("Invalid gateway URL"), "got: {msg}");
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    /// A URL without a scheme (bare host) is rejected.
    #[test]
    fn run_missing_scheme_returns_error() {
        match run("gateway.example.com") {
            CommandResult::Error(msg) => {
                assert!(msg.contains("Invalid gateway URL"), "got: {msg}");
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    /// A non-http scheme (ftp) is rejected.
    #[test]
    fn run_non_http_scheme_returns_error() {
        match run("ftp://gateway.example.com") {
            CommandResult::Error(msg) => {
                assert!(msg.contains("Invalid gateway URL"), "got: {msg}");
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }
}
