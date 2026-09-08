//! Small diagnostics-only helpers shared across this crate's modules.

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
