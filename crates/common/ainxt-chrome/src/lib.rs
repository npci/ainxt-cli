//! Chrome DevTools Protocol client for ainxt.
//!
//! Drives a Chrome instance ainxt owns, against a dedicated profile seeded
//! from the user's real one so logins carry over. See [`launch`] for why
//! attaching to an already-running Chrome is not possible.
//!
//! ```no_run
//! # async fn demo() -> ainxt_chrome::Result<()> {
//! let browser = ainxt_chrome::Browser::launch(Default::default()).await?;
//! let page = browser.open("https://example.com").await?;
//! println!("{}", page.read_accessibility_tree(20_000).await?);
//! # Ok(()) }
//! ```

#![forbid(unsafe_code)]

pub mod cdp;
pub mod error;
pub mod launch;
pub mod page;

pub use cdp::{CdpSession, PageTarget, list_pages, new_page};
pub use error::{ChromeError, Result};
pub use launch::{LaunchConfig, LaunchedChrome, ProfileSeed, seed_profile};
pub use page::{Page, PageInfo};

use std::time::Duration;

/// Default ceiling on a single page read, in characters.
pub const DEFAULT_MAX_READ_CHARS: usize = 40_000;

/// Default per-navigation timeout.
pub const DEFAULT_NAV_TIMEOUT: Duration = Duration::from_secs(30);

/// A launched browser ainxt controls.
#[derive(Debug)]
pub struct Browser {
    chrome: LaunchedChrome,
}

impl Browser {
    /// Launch Chrome with a DevTools port, seeding the profile on first use.
    pub async fn launch(config: LaunchConfig) -> Result<Self> {
        let chrome = launch::launch(&config).await?;
        Ok(Self { chrome })
    }

    /// The DevTools port in use.
    pub fn port(&self) -> u16 {
        self.chrome.port
    }

    /// Open `url` in a new tab and wait for it to load.
    pub async fn open(&self, url: &str) -> Result<Page> {
        let target = new_page(self.chrome.port, url).await?;
        let page = Page::attach(&target.ws_url, &target.id).await?;
        page.navigate(url, DEFAULT_NAV_TIMEOUT).await?;
        Ok(page)
    }

    /// Attach to the frontmost existing tab, if there is one.
    pub async fn current_page(&self) -> Result<Option<Page>> {
        let pages = list_pages(self.chrome.port).await?;
        let Some(target) = pages.into_iter().next() else {
            return Ok(None);
        };
        Ok(Some(Page::attach(&target.ws_url, &target.id).await?))
    }
}
