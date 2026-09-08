#![allow(
    unused_imports,
    unused_variables,
    unused_mut,
    unreachable_code,
    dead_code
)]
pub(crate) use ainxt_telemetry::unified_log;
pub use ainxt_tracing_macros::{teprintln, timed, tprintln};
pub mod active_sessions;
pub mod agent;
pub mod auth;
pub mod builtin;
pub mod bundle;
pub mod claude_import;
pub mod claude_import_state;
pub mod cli_models;
pub mod config;
pub use ainxt_shell_base::cpu_profile;
pub use ainxt_shell_base::env;
pub mod extensions;
pub use ainxt_workspace::foreign_sessions;
pub mod heap_profile;
pub use ainxt_http as http;
pub mod inspect;
pub mod instrumentation;
pub mod leader;
pub mod managed_config;
pub mod mcp_doctor;

/// Test-only model-slug fixtures.
///
/// Production no longer has any bundled default models — the catalog is sourced
/// only from the ainxt gateway (see `agent::config::resolve_model_list`). These
/// helpers exist solely so tests can build configs/catalogs with a stable,
/// arbitrary slug; they are NOT a runtime fallback.
#[cfg(test)]
pub(crate) mod models {
    /// Arbitrary slug used by tests as a stand-in model id.
    pub(crate) const TEST_MODEL: &str = "ainxt-build";
    pub(crate) fn default_model() -> &'static str {
        TEST_MODEL
    }
    pub(crate) fn default_web_search_model() -> &'static str {
        TEST_MODEL
    }
    pub(crate) fn default_image_description_model() -> &'static str {
        TEST_MODEL
    }
    pub(crate) fn default_session_summary_model() -> &'static str {
        TEST_MODEL
    }
}

pub mod plugin;
pub mod relay;
pub mod remote;
pub mod sampling;
pub mod session;
pub mod terminal;
#[cfg(test)]
pub(crate) mod test_support;
pub mod tier;
pub mod tools;
pub mod trace_classifier;
pub mod upload;
pub mod util;
