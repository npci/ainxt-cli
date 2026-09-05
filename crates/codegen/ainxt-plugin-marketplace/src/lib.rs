//! Plugin marketplace browse and index crate.
//!
//! Provides marketplace source configuration, plugin discovery (indexed +
//! filesystem fallback), and install integration with the existing
//! `InstallRegistry` pipeline.

pub mod catalog;
pub mod config;
pub mod error;
pub mod git;
pub mod index;
pub mod install_resolve;
pub mod installer;
pub mod matcher;
pub mod scanner;
pub mod types;

pub use config::{
    load_extra_sources_from_settings, load_extra_sources_from_settings_in, load_sources,
};
pub use error::MarketplaceError;
pub use scanner::scan_marketplace;
pub use types::*;

/// Display name of the official ainxt marketplace source.
pub const OFFICIAL_SOURCE_NAME: &str = "ainxt Official";

/// Default Git URL of the official ainxt marketplace source.
///
/// Override at runtime with the `AINXT_MARKETPLACE_SOURCE_URL` environment variable.
/// Override the org/repo slug used for canonical matching with `AINXT_MARKETPLACE_ORG`
/// (format: `owner/repo`, e.g. `my-org/my-plugin-marketplace`).
pub const OFFICIAL_SOURCE_GIT_URL: &str = "https://github.com/ainxt-org/plugin-marketplace.git";

/// Default owner/repo slug for the official marketplace (used for canonical URL matching).
const OFFICIAL_SOURCE_ORG_REPO: &str = "ainxt-org/plugin-marketplace";

/// Returns the effective Git URL for the official marketplace source.
///
/// If `AINXT_MARKETPLACE_SOURCE_URL` is set and non-empty, that value is used.
/// Otherwise falls back to [`OFFICIAL_SOURCE_GIT_URL`].
pub fn official_source_git_url() -> std::borrow::Cow<'static, str> {
    match std::env::var("AINXT_MARKETPLACE_SOURCE_URL") {
        Ok(v) if !v.is_empty() => std::borrow::Cow::Owned(v),
        _ => std::borrow::Cow::Borrowed(OFFICIAL_SOURCE_GIT_URL),
    }
}

/// Returns the effective `owner/repo` slug for the official marketplace source.
///
/// If `AINXT_MARKETPLACE_ORG` is set and non-empty, that value is used.
/// Otherwise falls back to the built-in `ainxt-org/plugin-marketplace`.
pub fn official_source_org_repo() -> std::borrow::Cow<'static, str> {
    match std::env::var("AINXT_MARKETPLACE_ORG") {
        Ok(v) if !v.is_empty() => std::borrow::Cow::Owned(v),
        _ => std::borrow::Cow::Borrowed(OFFICIAL_SOURCE_ORG_REPO),
    }
}

/// Whether `url` is the official ainxt marketplace source, normalizing case, a
/// `www.` prefix, a trailing `/` or `.git`, and HTTPS/SSH forms before comparing.
///
/// Respects the `AINXT_MARKETPLACE_ORG` override (see [`official_source_org_repo`]).
pub fn is_official_source_url(url: &str) -> bool {
    canonical_github_owner_repo(url).as_deref() == Some(official_source_org_repo().as_ref())
}

/// Normalized lowercase `owner/repo` from a GitHub URL (HTTPS/http/ssh/scp,
/// `www.`, trailing `.git`/`/`), or `None` if not a GitHub URL.
pub(crate) fn canonical_github_owner_repo(url: &str) -> Option<String> {
    let s = url.trim();
    let s = s.strip_suffix('/').unwrap_or(s);
    let s = s.strip_suffix(".git").unwrap_or(s);
    let lower = s.to_ascii_lowercase();
    let rest = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"))
        .or_else(|| lower.strip_prefix("ssh://"))
        .unwrap_or(&lower);
    let rest = rest.strip_prefix("git@").unwrap_or(rest);
    let rest = rest.strip_prefix("www.").unwrap_or(rest);
    let owner_repo = rest
        .strip_prefix("github.com/")
        .or_else(|| rest.strip_prefix("github.com:"))?;
    if owner_repo.is_empty() {
        None
    } else {
        Some(owner_repo.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_official_matches_canonical_https() {
        assert!(is_official_source_url(OFFICIAL_SOURCE_GIT_URL));
        assert!(is_official_source_url(
            "https://github.com/ainxt-org/plugin-marketplace"
        ));
    }

    #[test]
    fn is_official_matches_ssh_form() {
        assert!(is_official_source_url(
            "git@github.com:ainxt-org/plugin-marketplace.git"
        ));
        assert!(is_official_source_url(
            "git@github.com:ainxt-org/plugin-marketplace"
        ));
        assert!(is_official_source_url(
            "ssh://git@github.com/ainxt-org/plugin-marketplace.git"
        ));
        assert!(is_official_source_url(
            "ssh://git@github.com/ainxt-org/plugin-marketplace"
        ));
    }

    #[test]
    fn is_official_rejects_unrelated_urls() {
        assert!(!is_official_source_url(
            "https://github.com/anthropics/claude-plugins-official.git"
        ));
        assert!(!is_official_source_url(
            "https://github.com/ainxt-org/some-other-repo.git"
        ));
        assert!(!is_official_source_url(""));
    }

    #[test]
    fn is_official_matches_noncanonical_forms() {
        assert!(is_official_source_url(
            "https://GitHub.com/ainxt-org/Plugin-Marketplace"
        ));
        assert!(is_official_source_url(
            "https://github.com/ainxt-org/plugin-marketplace/"
        ));
        assert!(is_official_source_url(
            "https://github.com/ainxt-org/plugin-marketplace.git/"
        ));
        assert!(is_official_source_url(
            "http://github.com/ainxt-org/plugin-marketplace"
        ));
        assert!(is_official_source_url(
            "https://www.github.com/ainxt-org/plugin-marketplace.git"
        ));
        assert!(is_official_source_url(
            "git@github.com:ainxt-org/plugin-marketplace.git"
        ));
    }
}
