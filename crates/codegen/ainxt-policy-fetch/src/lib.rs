//! Background refresh of the signed gateway policy overlay.
//!
//! # Why this is a separate crate
//!
//! `ainxt-policy` has no `reqwest` dependency and must keep it that way. It runs
//! on the pre-tokio startup path, it is the crate the whole security posture is
//! decided by, and keeping it offline means its tests need no network feature
//! and its decision logic cannot be perturbed by an HTTP client. Transport
//! therefore lives out here.
//!
//! # What the gateway can and cannot do
//!
//! The overlay only ever **narrows**. The machine bundle establishes the floor;
//! the gateway may tighten it at runtime but can never widen it, and that is
//! enforced in `ainxt_policy::bootstrap` at load time rather than trusted here.
//! This crate is therefore not a security boundary — it writes a file, and the
//! load path decides whether the file is worth anything. A compromised gateway,
//! or a compromised network between here and it, can at worst deliver a policy
//! that is stricter than intended.
//!
//! # Cadence
//!
//! Slow on purpose. Policy changes are a human-scale event, and a fleet polling
//! a gateway every few seconds is a self-inflicted load problem. `If-None-Match`
//! makes the steady state a 304.

use std::path::{Path, PathBuf};
use std::time::Duration;

use ainxt_policy_types::merge::{is_narrower_or_equal, narrow_capabilities};

/// Time between refresh attempts once the first one has completed.
const REFRESH_INTERVAL: Duration = Duration::from_secs(15 * 60);

/// Delay before the first attempt, so startup is never blocked on the network.
const INITIAL_DELAY: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchOutcome {
    /// The gateway had nothing newer.
    Unchanged,
    /// A new overlay was verified, written and applied.
    Applied { version: u64 },
    /// Nothing usable was obtained. Never fatal — the base policy still applies.
    Skipped(String),
}

fn overlay_path(state_dir: &Path) -> PathBuf {
    state_dir.join("policy").join("overlay.json")
}

fn etag_path(state_dir: &Path) -> PathBuf {
    state_dir.join("policy").join("overlay.etag")
}

/// Renders a string with newline/carriage-return bytes stripped, for use at
/// `tracing::` log call sites that embed externally influenced data (e.g. a
/// gateway-supplied ETag, HTTP status text, or an error string derived from
/// the network response) in a log record. Values are filtered
/// character-by-character at format time via `Display::fmt`, rather than
/// materialized into a separate sanitized `String` ahead of the call, so a
/// log site can never accidentally interpolate the raw value instead of the
/// sanitized one.
struct LogSafe<'a>(&'a str);

impl std::fmt::Display for LogSafe<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for ch in self.0.chars() {
            if ch != '\n' && ch != '\r' {
                write!(f, "{ch}")?;
            }
        }
        Ok(())
    }
}

/// Spawn the background refresh loop.
///
/// Returns immediately. Failures are logged and retried on the next tick: a
/// gateway that is down must not degrade the client, because the policy already
/// on disk remains valid and enforcing.
pub fn spawn_sync(base_url: String, state_dir: PathBuf, auth_header: Option<String>) {
    tokio::spawn(async move {
        tokio::time::sleep(INITIAL_DELAY).await;
        loop {
            match refresh_once(&base_url, &state_dir, auth_header.as_deref()).await {
                Ok(FetchOutcome::Applied { version }) => {
                    tracing::info!(version, "policy overlay applied");
                }
                Ok(FetchOutcome::Unchanged) => {}
                Ok(FetchOutcome::Skipped(reason)) => {
                    tracing::warn!(reason = %LogSafe(&reason), "policy overlay not applied; base policy still in force");
                }
                Err(err) => {
                    tracing::warn!(error = %LogSafe(&err), "policy overlay refresh failed");
                }
            }
            tokio::time::sleep(REFRESH_INTERVAL).await;
        }
    });
}

/// One refresh attempt.
pub async fn refresh_once(
    base_url: &str,
    state_dir: &Path,
    auth_header: Option<&str>,
) -> Result<FetchOutcome, String> {
    // An unmanaged build trusts no authority key, so there is nothing an
    // overlay could be verified against and fetching one would be theatre.
    let manifest =
        ainxt_policy::bootstrap::resolve_manifest().map_err(|e| format!("manifest: {e}"))?;
    let Some(key) = manifest.policy_authority.clone() else {
        return Ok(FetchOutcome::Skipped(
            "build trusts no policy authority".to_owned(),
        ));
    };

    let url = format!("{}/policy/bundle", base_url.trim_end_matches('/'));
    let mut request = ainxt_http::shared_client().get(&url);
    if let Some(header) = auth_header {
        request = request.header(reqwest::header::AUTHORIZATION, header);
    }
    if let Ok(etag) = std::fs::read_to_string(etag_path(state_dir)) {
        let etag = etag.trim().to_owned();
        if !etag.is_empty() {
            request = request.header(reqwest::header::IF_NONE_MATCH, etag);
        }
    }

    let response = request.send().await.map_err(|e| e.to_string())?;
    if response.status() == reqwest::StatusCode::NOT_MODIFIED {
        return Ok(FetchOutcome::Unchanged);
    }
    if !response.status().is_success() {
        return Ok(FetchOutcome::Skipped(format!(
            "gateway returned {}",
            response.status()
        )));
    }

    let etag = response
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let bytes = response.bytes().await.map_err(|e| e.to_string())?;

    // Verify before writing. An overlay that would be rejected at load time
    // must never reach the disk — otherwise every subsequent startup pays to
    // parse and reject it, and the file looks authoritative to anyone reading
    // the directory.
    let verified = ainxt_policy::bundle::PolicyBundle::from_slice(&bytes)
        .and_then(|envelope| envelope.verify(&key, None))
        .map_err(|e| format!("overlay failed verification: {e}"))?;
    let payload = verified.payload();

    // Narrowness is re-checked at load time too. Checking here as well means a
    // widening overlay is reported as a gateway misconfiguration now, rather
    // than silently ignored on every future start.
    let active = ainxt_policy::global::active();
    let base = &active.policy().capabilities;
    let merged = narrow_capabilities(base, &payload.capabilities);
    if !is_narrower_or_equal(&merged, base) {
        return Ok(FetchOutcome::Skipped(
            "overlay would widen capability; refusing it".to_owned(),
        ));
    }

    write_atomically(&overlay_path(state_dir), &bytes)?;
    if let Some(etag) = etag {
        let _ = write_atomically(&etag_path(state_dir), etag.as_bytes());
    }

    // Re-resolve from disk rather than installing the merge computed above, so
    // the applied policy comes from exactly the path a restart would take.
    // Two code paths that both "apply the overlay" will eventually disagree.
    let outcome = ainxt_policy::bootstrap::initialize(state_dir)
        .map_err(|e| format!("reload after overlay write: {e}"))?;

    match outcome.overlay_version {
        Some(version) => Ok(FetchOutcome::Applied { version }),
        None => Ok(FetchOutcome::Skipped(format!(
            "overlay written but rejected on reload verification (payload version {})",
            payload.version
        ))),
    }
}

/// Write via a temporary file and rename, so a crash mid-write cannot leave a
/// truncated overlay that fails verification on the next start.
fn write_atomically(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes).map_err(|e| e.to_string())?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    }

    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn an_unmanaged_build_does_not_fetch() {
        let dir = tempfile::tempdir().expect("tempdir");
        // This workspace is not stamped, so no authority key exists and there
        // is nothing an overlay could be checked against.
        let outcome = refresh_once("http://127.0.0.1:1", dir.path(), None)
            .await
            .expect("skips rather than erroring");
        assert!(matches!(outcome, FetchOutcome::Skipped(_)));
        assert!(!overlay_path(dir.path()).exists(), "wrote an overlay anyway");
    }

    #[test]
    fn atomic_write_leaves_no_temp_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("policy").join("overlay.json");
        write_atomically(&target, b"{}").expect("write");
        assert_eq!(std::fs::read(&target).expect("read"), b"{}");
        assert!(!target.with_extension("tmp").exists());
    }
}
