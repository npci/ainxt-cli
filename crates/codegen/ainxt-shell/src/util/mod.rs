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

/// Percent-encodes `segment` so it can be interpolated into a URL as exactly
/// one path segment (e.g. the `{id}` in `/sessions/{id}/data`).
///
/// Every byte that RFC 3986 allows in a path segment (`pchar`: unreserved,
/// sub-delims, `:` and `@`) is kept verbatim, so ordinary ids (UUIDs, slugs,
/// `prefix_123`) are returned unchanged. Anything else -- notably `/`, `?`,
/// `#`, `%`, `\` and non-ASCII -- becomes `%XX`, so an id can never escape
/// its segment and address a different endpoint or smuggle in a query string.
///
/// The output is built byte-by-byte rather than by slicing the input, so it
/// is always pure ASCII.
pub(crate) fn encode_path_segment(segment: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(segment.len());
    for byte in segment.bytes() {
        let keep = byte.is_ascii_alphanumeric()
            || matches!(
                byte,
                b'-' | b'.'
                    | b'_'
                    | b'~'
                    | b'!'
                    | b'$'
                    | b'&'
                    | b'\''
                    | b'('
                    | b')'
                    | b'*'
                    | b'+'
                    | b','
                    | b';'
                    | b'='
                    | b':'
                    | b'@'
            );
        if keep {
            out.push(char::from(byte));
        } else {
            out.push('%');
            out.push(char::from(HEX[usize::from(byte >> 4)]));
            out.push(char::from(HEX[usize::from(byte & 0x0F)]));
        }
    }
    out
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

#[cfg(test)]
mod tests {
    use super::encode_path_segment;

    #[test]
    fn encode_path_segment_keeps_ordinary_ids_unchanged() {
        for id in [
            "0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b",
            "test-session",
            "sess_ABC123",
            "a.b~c",
            "ns:id@host",
            "x!$&'()*+,;=y",
        ] {
            assert_eq!(encode_path_segment(id), id);
        }
    }

    #[test]
    fn encode_path_segment_escapes_structural_characters() {
        assert_eq!(encode_path_segment("a/b"), "a%2Fb");
        assert_eq!(encode_path_segment("../x"), "..%2Fx");
        assert_eq!(encode_path_segment("a?b=c"), "a%3Fb=c");
        assert_eq!(encode_path_segment("a#b"), "a%23b");
        assert_eq!(encode_path_segment("a%2Fb"), "a%252Fb");
        assert_eq!(encode_path_segment("a\\b"), "a%5Cb");
        assert_eq!(encode_path_segment("a b"), "a%20b");
        assert_eq!(encode_path_segment(""), "");
    }

    #[test]
    fn encode_path_segment_escapes_non_ascii_as_utf8_bytes() {
        assert_eq!(encode_path_segment("é"), "%C3%A9");
        assert!(encode_path_segment("日本").is_ascii());
    }
}
