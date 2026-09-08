pub mod config;
pub mod ainxt_auth_credentials;
pub mod hooks;

// The foundation utilities live in `ainxt-shell-base` (upstream of this
// crate so they build in parallel). Re-exported at the original paths so
// existing `crate::util::…` / `ainxt_shell::util::…` users compile
// unchanged.
pub use ainxt_shell_base::util::*;

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

/// Aborts the wrapped tokio task when dropped.
///
/// Use to tie a spawned helper task's lifetime to an async scope so that
/// cancelling the parent future (e.g. a turn abort dropping the tool loop)
/// also tears down the helper instead of leaving it running detached.
/// Aborting an already-finished task is a no-op, so this is safe to hold
/// across normal scope exit too.
pub struct AbortOnDrop(pub tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}
