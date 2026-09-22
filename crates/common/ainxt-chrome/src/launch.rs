//! Locating, seeding and launching a Chrome instance that speaks CDP.
//!
//! Chrome cannot be attached to after the fact: the DevTools port only exists
//! if `--remote-debugging-port` was passed at startup, and a second process
//! cannot share a running instance's `--user-data-dir` (Chrome aborts on the
//! profile's `SingletonLock` rather than risk corruption). Chrome 136+ also
//! refuses remote debugging outright when the profile *is* the default
//! user-data-dir.
//!
//! So ainxt drives its own instance against its own profile directory, seeded
//! once from the real one so the user's logins carry over. The everyday
//! browser keeps running, untouched.

use crate::error::{ChromeError, Result};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Files that carry a signed-in session. Copied from the real profile into
/// ainxt's dedicated one so the agent inherits the user's logins.
///
/// On macOS the cookie values are encrypted with a Keychain key scoped to the
/// user, not to the profile directory, so a copied `Cookies` file still
/// decrypts in the new location.
/// Deliberately only cookies. `Login Data` (saved passwords) and `Web Data`
/// (autofill, saved cards) would let the agent's browser autofill credentials
/// and payment details into forms it clicks — capability the stated goal,
/// "stay logged in", does not need. Copying them widens the blast radius of
/// every later mistake for no benefit.
const CREDENTIAL_FILES: &[&str] = &["Cookies"];

/// Where Chrome keeps the real profile, per platform.
fn default_user_data_dir() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    #[cfg(target_os = "macos")]
    return Some(home.join("Library/Application Support/Google/Chrome"));
    #[cfg(target_os = "windows")]
    return Some(home.join("AppData/Local/Google/Chrome/User Data"));
    #[cfg(all(unix, not(target_os = "macos")))]
    return Some(home.join(".config/google-chrome"));
}

/// Standard install locations, checked in order. `AINXT_CHROME_BINARY`
/// overrides all of them.
fn find_chrome_binary() -> Result<PathBuf> {
    resolve_chrome_binary(std::env::var("AINXT_CHROME_BINARY").ok().as_deref())
}

/// Binary resolution with the override passed in, so it is testable without
/// mutating the process environment.
fn resolve_chrome_binary(override_path: Option<&str>) -> Result<PathBuf> {
    if let Some(explicit) = override_path {
        let path = PathBuf::from(explicit);
        if path.is_file() {
            return Ok(path);
        }
        // An explicit override that doesn't exist is a mistake worth naming,
        // not something to silently fall back from.
        return Err(ChromeError::BinaryNotFound);
    }

    const CANDIDATES: &[&str] = &[
        #[cfg(target_os = "macos")]
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        #[cfg(target_os = "macos")]
        "/Applications/Chromium.app/Contents/MacOS/Chromium",
        #[cfg(all(unix, not(target_os = "macos")))]
        "/usr/bin/google-chrome",
        #[cfg(all(unix, not(target_os = "macos")))]
        "/usr/bin/chromium",
        #[cfg(all(unix, not(target_os = "macos")))]
        "/usr/bin/chromium-browser",
        #[cfg(target_os = "windows")]
        r"C:\Program Files\Google\Chrome\Application\chrome.exe",
        #[cfg(target_os = "windows")]
        r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
    ];

    CANDIDATES
        .iter()
        .map(PathBuf::from)
        .find(|p| p.is_file())
        .ok_or(ChromeError::BinaryNotFound)
}

/// How ainxt's dedicated profile gets its logins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProfileSeed {
    /// Copy the credential files from the real profile on first use.
    #[default]
    FromDefaultProfile,
    /// Start clean; the user logs in themselves.
    Empty,
}

/// Configuration for the Chrome instance ainxt drives.
#[derive(Debug, Clone)]
pub struct LaunchConfig {
    /// DevTools port. 0 lets the OS choose (read back from `DevToolsActivePort`).
    pub port: u16,
    /// Directory for ainxt's dedicated profile.
    pub user_data_dir: PathBuf,
    /// Whether to seed that profile from the user's real one.
    pub seed: ProfileSeed,
    /// Run without a visible window.
    pub headless: bool,
    /// How long to wait for the DevTools endpoint to come up.
    pub startup_timeout: Duration,
}

impl Default for LaunchConfig {
    fn default() -> Self {
        let user_data_dir = dirs::home_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join(".ainxt/chrome-profile");
        Self {
            port: 9222,
            user_data_dir,
            seed: ProfileSeed::default(),
            headless: false,
            startup_timeout: Duration::from_secs(30),
        }
    }
}

/// Copy the credential files from the real profile into ainxt's, once.
///
/// Best-effort per file: a profile that has never stored passwords simply has
/// no `Login Data`, which is not an error. A missing *source profile* is, when
/// seeding was explicitly asked for.
pub fn seed_profile(target_user_data_dir: &Path) -> Result<()> {
    let source_root = default_user_data_dir().ok_or_else(|| ChromeError::ProfileSeed {
        source_dir: "<unknown>".to_owned(),
        detail: "could not determine the home directory".to_owned(),
    })?;
    let source = source_root.join("Default");
    if !source.is_dir() {
        return Err(ChromeError::ProfileSeed {
            source_dir: source.display().to_string(),
            detail: "no Default profile found at that path".to_owned(),
        });
    }

    let target = target_user_data_dir.join("Default");
    std::fs::create_dir_all(&target)?;

    // `Local State` lives at the user-data-dir root, not inside the profile,
    // and carries the encrypted-key material Chrome needs to read `Cookies`.
    let local_state = source_root.join("Local State");
    if local_state.is_file() {
        let _ = std::fs::copy(&local_state, target_user_data_dir.join("Local State"));
    }

    let mut copied = 0usize;
    for name in CREDENTIAL_FILES {
        let from = source.join(name);
        if !from.is_file() {
            continue;
        }
        match std::fs::copy(&from, target.join(name)) {
            Ok(_) => copied += 1,
            Err(e) => tracing::warn!("could not seed profile file {name}: {e}"),
        }
    }

    if copied == 0 {
        return Err(ChromeError::ProfileSeed {
            source_dir: source.display().to_string(),
            detail: "found the profile but none of its credential files were readable".to_owned(),
        });
    }
    tracing::info!("seeded {copied} credential file(s) into {}", target.display());
    Ok(())
}

/// A launched Chrome process and the WebSocket URL to drive it.
#[derive(Debug)]
pub struct LaunchedChrome {
    /// The child process, when this handle launched it. `None` when an
    /// already-running Chrome was reused — that one is not ours to kill.
    pub child: Option<tokio::process::Child>,
    /// Browser-level DevTools WebSocket endpoint.
    pub ws_url: String,
    /// The port actually in use.
    pub port: u16,
}

impl Drop for LaunchedChrome {
    fn drop(&mut self) {
        // The child holds a profile lock; leaving it running would block the
        // next launch. start_kill is non-blocking, which Drop requires.
        // A reused instance was not ours to start, so it is not ours to kill.
        if let Some(child) = self.child.as_mut() {
            let _ = child.start_kill();
        }
    }
}

/// Launch Chrome with a DevTools port open, seeding the profile if needed.
pub async fn launch(config: &LaunchConfig) -> Result<LaunchedChrome> {
    // Reuse a Chrome already serving DevTools on this port rather than
    // spawning a second one that would only abort on the profile lock.
    if let Some(ws_url) = existing_instance(config.port, &config.user_data_dir).await {
        tracing::info!("reusing Chrome already on port {}", config.port);
        return Ok(LaunchedChrome {
            child: None,
            ws_url,
            port: config.port,
        });
    }

    let binary = find_chrome_binary()?;

    let first_use = !config.user_data_dir.join("Default").is_dir();
    if first_use && config.seed == ProfileSeed::FromDefaultProfile {
        seed_profile(&config.user_data_dir)?;
    }
    std::fs::create_dir_all(&config.user_data_dir)?;

    let mut cmd = tokio::process::Command::new(&binary);
    cmd.arg(format!("--remote-debugging-port={}", config.port))
        .arg(format!("--user-data-dir={}", config.user_data_dir.display()))
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        // Chrome's own restore prompt would otherwise steal the first page.
        .arg("--restore-last-session=false")
        .arg("about:blank")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    if config.headless {
        cmd.arg("--headless=new");
    }

    let child = cmd.spawn().map_err(ChromeError::Launch)?;

    let ws_url = wait_for_devtools(config.port, config.startup_timeout).await?;
    Ok(LaunchedChrome {
        child: Some(child),
        ws_url,
        port: config.port,
    })
}

/// Probe for a Chrome already serving DevTools on this port.
///
/// A Chrome left running from an earlier session still owns the profile
/// directory, so spawning a second one aborts on the profile lock. Reusing
/// the running instance is both correct and what the user expects — their
/// tabs are still there.
async fn existing_instance(port: u16, user_data_dir: &Path) -> Option<String> {
    // Chrome writes the live debugging port into its own profile directory.
    // If that file does not name this port, whatever is listening is not the
    // Chrome we own — it could be another automation setup driving the user's
    // real profile, or a local process impersonating DevTools to capture
    // every command we send and feed us fabricated page content.
    let active = std::fs::read_to_string(user_data_dir.join("DevToolsActivePort")).ok()?;
    let claimed: u16 = active.lines().next()?.trim().parse().ok()?;
    if claimed != port {
        tracing::debug!("port {port} is in use by a Chrome that is not ours; not reusing it");
        return None;
    }

    let client = reqwest::Client::new();
    let body: serde_json::Value = client
        .get(format!("http://127.0.0.1:{port}/json/version"))
        .timeout(Duration::from_millis(500))
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;
    let ws = body.get("webSocketDebuggerUrl")?.as_str()?;
    // Never follow an endpoint off this machine, whatever the reply says.
    if !is_loopback_ws(ws) {
        tracing::warn!("DevTools endpoint pointed off-host ({ws}); refusing to attach");
        return None;
    }
    Some(ws.to_owned())
}

/// True when a DevTools WebSocket URL points at this machine.
fn is_loopback_ws(ws: &str) -> bool {
    let Some(rest) = ws.strip_prefix("ws://") else {
        return false;
    };
    let host = rest.split('/').next().unwrap_or_default();
    let host = host.rsplit_once(':').map(|(h, _)| h).unwrap_or(host);
    matches!(host, "127.0.0.1" | "localhost" | "[::1]" | "::1")
}

/// Poll the DevTools HTTP endpoint until it serves a browser WebSocket URL.
async fn wait_for_devtools(port: u16, timeout: Duration) -> Result<String> {
    let client = reqwest::Client::new();
    let url = format!("http://127.0.0.1:{port}/json/version");
    let deadline = Instant::now() + timeout;

    while Instant::now() < deadline {
        if let Ok(resp) = client
            .get(&url)
            .timeout(Duration::from_secs(2))
            .send()
            .await
            && let Ok(body) = resp.json::<serde_json::Value>().await
            && let Some(ws) = body.get("webSocketDebuggerUrl").and_then(|v| v.as_str())
        {
            return Ok(ws.to_owned());
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    Err(ChromeError::DevToolsTimeout {
        port,
        secs: timeout.as_secs(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_uses_a_dedicated_profile_dir() {
        let cfg = LaunchConfig::default();
        // Never the real profile — that is the whole point of the copy.
        assert!(cfg.user_data_dir.ends_with(".ainxt/chrome-profile"));
        assert_ne!(Some(cfg.user_data_dir.clone()), default_user_data_dir());
    }

    #[test]
    fn only_loopback_devtools_endpoints_are_adopted() {
        assert!(is_loopback_ws("ws://127.0.0.1:9222/devtools/browser/abc"));
        assert!(is_loopback_ws("ws://localhost:9222/devtools/browser/abc"));
        assert!(!is_loopback_ws("ws://attacker.example/x"));
        assert!(!is_loopback_ws("ws://10.0.0.5:9222/devtools/browser/abc"));
        assert!(!is_loopback_ws("wss://attacker.example/x"));
    }

    #[test]
    fn the_seed_copies_cookies_only_not_passwords_or_cards() {
        assert_eq!(CREDENTIAL_FILES, &["Cookies"]);
    }

    #[test]
    fn explicit_missing_binary_override_is_an_error_not_a_fallback() {
        let result = resolve_chrome_binary(Some("/nonexistent/chrome"));
        assert!(matches!(result, Err(ChromeError::BinaryNotFound)));
    }

    #[test]
    fn seeding_a_missing_profile_names_the_path() {
        let tmp = std::env::temp_dir().join("ainxt-chrome-seed-test");
        // Only meaningful when a real Chrome profile is absent; when one
        // exists the seed succeeds and there is nothing to assert here.
        if default_user_data_dir().is_some_and(|p| p.join("Default").is_dir()) {
            return;
        }
        let err = seed_profile(&tmp).unwrap_err();
        assert!(matches!(err, ChromeError::ProfileSeed { .. }));
    }
}
