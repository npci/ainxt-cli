//! ainxt-pager — ainxt TUI.
//!
//! A clean-room implementation built on the v3 pager rendering engine.

pub mod acp;
pub mod actions;
pub mod app;
pub mod client_identity;
pub mod completions_cmd;
mod config_toml_edit;
pub mod diagnostics;
pub mod diff;
pub mod docs;
pub mod export_cmd;
pub mod git_info;
pub mod headless;
pub mod hyperlink_route;
pub mod inline_media_ffmpeg;
pub mod input;
pub mod input_log;
pub mod mcp_cmd;
pub mod policy_cmd;
pub mod memory_cmd;
pub mod memory_release;
pub mod memory_trace;
// ── Minimal (scrollback-native) mode seam ────────────────────────────────────
// The *only* minimal-specific surface in this (the "full pager") crate. Both
// modules are grouped under `src/minimal/` so a full-pager contributor sees one
// folder to ignore, not files scattered through the module list. All the actual
// minimal rendering lives in the sibling `ainxt-pager-minimal` crate; these
// are just the two narrow seams it connects through:
//   - `minimal_hook` — pager → minimal dispatch (fn-pointer IoC seam).
//   - `minimal_api`  — minimal → pager read surface (facade over `pub(crate)`s).
// Module names are kept flat (via `#[path]`) so existing references and
// every `crate::minimal_{api,hook}` call site stay valid.
#[path = "minimal/api.rs"]
pub mod minimal_api;
#[path = "minimal/hook.rs"]
pub mod minimal_hook;
pub mod models;
pub mod notifications;
#[allow(unused_imports, unused_macros)]
pub mod obf;
pub mod plugin_cmd;
pub mod project_picker;
pub mod pty_wrap;
pub mod scrollback;
pub mod search;
pub mod sessions_cmd;
pub mod settings;
pub mod share_cmd;
pub mod slash;
pub mod startup;
pub mod tips;
pub mod wrap_clipboard_image;
pub mod wrap_cmd;

pub mod tool_usage;

// Presentation-primitives layer extracted into the sibling crate
// `ainxt-pager-render`. Re-exported at the crate root so existing
// `crate::<module>::...` references throughout the pager keep resolving.
pub use ainxt_pager_render::{
    appearance, clipboard, glyphs, host, link_opener, modal_window_state, prompt_images,
    render, syntax, terminal, theme, util,
};
pub mod trace_cmd;
pub mod tracing;
pub mod unified_log;
pub mod views;
pub mod voice;
pub mod worktree_cmd;

#[cfg(test)]
pub mod test_util;

/// A short, deterministic, non-reversible tag derived from a session id, for
/// diagnostics only. The same session id always maps to the same tag (so log
/// lines for one session can still be grepped together), but the tag itself
/// contains none of the id's bytes -- it's a hash digest, not a substring.
/// Session ids are locally-generated correlation handles (UUIDs/slugs),
/// never credentials, so this exists purely to keep raw ids out of log
/// output, not as a security control.
pub(crate) fn session_log_tag(session_id: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new(); // fixed internal keys: deterministic, not RandomState
    session_id.hash(&mut hasher);
    format!("{:08x}", hasher.finish() as u32)
}
