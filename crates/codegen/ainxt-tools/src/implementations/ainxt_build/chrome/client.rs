//! The shared browser handle injected into `Resources`.
//!
//! Chrome is launched lazily on the first tool call rather than at session
//! start: most sessions never browse, and launching costs a process plus a
//! profile seed. Once up, the same instance serves every later call so tabs,
//! cookies and history persist across a conversation.

use ainxt_chrome::{Browser, LaunchConfig, ProfileSeed};
use std::sync::Arc;
use tokio::sync::Mutex;

/// Runtime knobs, surfaced through `config.toml`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(default)]
pub struct ChromeParams {
    /// DevTools port to open.
    pub port: u16,
    /// Seed the dedicated profile from the user's real Chrome profile so
    /// logged-in sessions carry over.
    pub seed_from_default_profile: bool,
    /// Run Chrome without a visible window.
    pub headless: bool,
    /// Ceiling on a single page read, in characters.
    pub max_read_chars: usize,
}

impl Default for ChromeParams {
    fn default() -> Self {
        Self {
            port: 9222,
            seed_from_default_profile: true,
            headless: false,
            max_read_chars: ainxt_chrome::DEFAULT_MAX_READ_CHARS,
        }
    }
}

/// Lazily-launched browser shared by every Chrome tool in a session.
#[derive(Clone)]
pub struct ChromeClient {
    params: ChromeParams,
    browser: Arc<Mutex<Option<Arc<Browser>>>>,
}

impl std::fmt::Debug for ChromeClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChromeClient")
            .field("params", &self.params)
            .finish_non_exhaustive()
    }
}

impl ChromeClient {
    /// Build a client. Launching is deferred to the first [`Self::browser`].
    pub fn new(params: ChromeParams) -> Self {
        Self {
            params,
            browser: Arc::new(Mutex::new(None)),
        }
    }

    /// Configured read ceiling.
    pub fn max_read_chars(&self) -> usize {
        self.params.max_read_chars
    }

    /// The running browser, launching it on first use.
    pub async fn browser(&self) -> ainxt_chrome::Result<Arc<Browser>> {
        let mut guard = self.browser.lock().await;
        if let Some(existing) = guard.as_ref() {
            return Ok(Arc::clone(existing));
        }
        let config = LaunchConfig {
            port: self.params.port,
            headless: self.params.headless,
            seed: if self.params.seed_from_default_profile {
                ProfileSeed::FromDefaultProfile
            } else {
                ProfileSeed::Empty
            },
            ..Default::default()
        };
        let browser = Arc::new(Browser::launch(config).await?);
        *guard = Some(Arc::clone(&browser));
        Ok(browser)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_seed_from_the_real_profile_and_show_a_window() {
        let p = ChromeParams::default();
        assert!(p.seed_from_default_profile);
        assert!(!p.headless, "a visible window is what makes this auditable");
        assert_eq!(p.port, 9222);
    }

    #[tokio::test]
    async fn client_construction_does_not_launch_chrome() {
        let client = ChromeClient::new(ChromeParams::default());
        // No process yet — the handle is empty until `browser()` is called.
        assert!(client.browser.lock().await.is_none());
    }
}
