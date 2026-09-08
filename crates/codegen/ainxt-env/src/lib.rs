#![allow(
    unused_imports,
    unused_variables,
    unused_mut,
    unreachable_code,
    dead_code
)]
//! Backend environment presets for the ainxt CLI crate family: endpoint URL
//! defaults, environment selection, and env-var test support.
//!
//! All compiled-in endpoint and URL defaults are empty in the open-source
//! build (audit risk R42). Values resolve from `AINXT_*` env vars at runtime.
/// The endpoint set for one backend environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AinxtBuildEndpoints {
    pub cli_chat_proxy_base_url: &'static str,
    pub asset_server_url: &'static str,
    pub relay_ws_url: &'static str,
    pub gateway_ws_url: &'static str,
    pub ws_origin: &'static str,
}
// Backend endpoints. Each is overridable at runtime via the matching
// AINXT_PRODUCTION_* env var (see `resolve` below).
//
// THESE ARE DELIBERATELY EMPTY IN THE OPEN-SOURCE BUILD.
//
// This source tree does not ship with a hosted backend, so there is no honest
// default to compile in. An endpoint constant is not a hint -- it is a trust
// anchor. `is_cli_chat_proxy_url()` treats whatever host sits in
// `cli_chat_proxy_base_url` as first-party, and a build that trusts a host its
// publisher does not operate has handed that host's owner every client.
//
// That is not hypothetical here. These constants previously named
// `*.ainxt.dev`, and that domain **was not registered** -- the `.dev` registry
// returned `404 not found` for it while resolving its neighbours normally. Any
// person who bought it would have become the trusted API, the OAuth token
// endpoint and (via `ainxt-update`) the software update channel for every
// installed client. See audit risk R42.
//
// Empty is the safe value, not merely the tidy one: `matches_trusted_base_url`
// parses this with `Url::parse` and returns `false` when parsing fails, so an
// empty anchor trusts nothing. Loopback stays trusted for local development.
//
// To point a build at infrastructure you operate, set the environment
// variables; no rebuild is needed. Do not paste a hostname back in here unless
// you control the domain and intend every user of your build to trust it.
const PRODUCTION_ENDPOINTS: AinxtBuildEndpoints = AinxtBuildEndpoints {
    cli_chat_proxy_base_url: "",
    asset_server_url: "",
    relay_ws_url: "",
    gateway_ws_url: "",
    ws_origin: "",
};
pub const PROD_CLI_CHAT_PROXY_BASE_URL: &str = PRODUCTION_ENDPOINTS.cli_chat_proxy_base_url;
pub const PROD_ASSET_SERVER_URL: &str = PRODUCTION_ENDPOINTS.asset_server_url;
pub const PROD_RELAY_WS_URL: &str = PRODUCTION_ENDPOINTS.relay_ws_url;
pub const PROD_GATEWAY_WS_URL: &str = PRODUCTION_ENDPOINTS.gateway_ws_url;
pub const PROD_WS_ORIGIN: &str = PRODUCTION_ENDPOINTS.ws_origin;

// ---------------------------------------------------------------------------
// UI / marketing URL constants — all env-overridable via AINXT_URL_* vars.
// Centralised here so forks can rebrand without touching product code.
//
// Empty in the open-source build, for the same reason as the endpoints above:
// these pointed at `ainxt.dev`, which was unregistered (R42). A "Subscribe" or
// "Legal" link is a smaller prize than the update channel, but it is still a
// link this software puts in front of a user with an implied endorsement, and
// it must not lead somewhere the publisher does not control.
//
// Empty means "this build has no such page", and every call site treats it that
// way: `url_is_set()` below is the guard, and the UI omits the affordance
// rather than offering a link that goes nowhere. Set the matching AINXT_URL_*
// variable to switch it back on for your own deployment.
// ---------------------------------------------------------------------------

/// Subscribe / upgrade page shown in billing prompts.
/// Override: `AINXT_URL_SUBSCRIBE`
pub const URL_SUBSCRIBE: &str = "";

/// Pay-as-you-go / usage page.
/// Override: `AINXT_URL_USAGE`
pub const URL_USAGE: &str = "";

/// Promo / announcement CTA URL.
/// Override: `AINXT_URL_PROMO`
pub const URL_PROMO: &str = "";

/// Legal / terms of service page.
/// Override: `AINXT_URL_LEGAL`
pub const URL_LEGAL: &str = "";

/// Developer documentation root.
/// Override: `AINXT_URL_DOCS`
pub const URL_DOCS: &str = "";

/// Managed-connectors landing page linked from the MCP modal.
/// Override: `AINXT_URL_CONNECTORS`
pub const URL_CONNECTORS: &str = "";

/// Base for per-version CLI changelogs.
/// Override: `AINXT_URL_CHANGELOG_BASE`
pub const URL_CHANGELOG_BASE: &str = "";

/// True when a resolved `url_*()` value is usable as a link.
///
/// The open-source build ships every UI URL empty, so "is there a page for
/// this?" is now a real question at each call site rather than an assumption.
/// Opening `""` in a browser, or rendering `Learn more: `, is worse than
/// showing nothing at all.
pub fn url_is_set(url: &str) -> bool {
    !url.trim().is_empty()
}

/// Returns the effective managed-connectors URL, respecting `AINXT_URL_CONNECTORS`.
pub fn url_connectors() -> std::borrow::Cow<'static, str> {
    match std::env::var("AINXT_URL_CONNECTORS") {
        Ok(v) if !v.is_empty() => std::borrow::Cow::Owned(v),
        _ => std::borrow::Cow::Borrowed(URL_CONNECTORS),
    }
}

/// Returns the effective changelog base, respecting `AINXT_URL_CHANGELOG_BASE`.
pub fn url_changelog_base() -> std::borrow::Cow<'static, str> {
    match std::env::var("AINXT_URL_CHANGELOG_BASE") {
        Ok(v) if !v.is_empty() => std::borrow::Cow::Owned(v),
        _ => std::borrow::Cow::Borrowed(URL_CHANGELOG_BASE),
    }
}

/// Returns the effective subscribe URL, respecting `AINXT_URL_SUBSCRIBE`.
pub fn url_subscribe() -> std::borrow::Cow<'static, str> {
    match std::env::var("AINXT_URL_SUBSCRIBE") {
        Ok(v) if !v.is_empty() => std::borrow::Cow::Owned(v),
        _ => std::borrow::Cow::Borrowed(URL_SUBSCRIBE),
    }
}

/// Returns the effective usage URL, respecting `AINXT_URL_USAGE`.
pub fn url_usage() -> std::borrow::Cow<'static, str> {
    match std::env::var("AINXT_URL_USAGE") {
        Ok(v) if !v.is_empty() => std::borrow::Cow::Owned(v),
        _ => std::borrow::Cow::Borrowed(URL_USAGE),
    }
}

/// Returns the effective promo URL, respecting `AINXT_URL_PROMO`.
pub fn url_promo() -> std::borrow::Cow<'static, str> {
    match std::env::var("AINXT_URL_PROMO") {
        Ok(v) if !v.is_empty() => std::borrow::Cow::Owned(v),
        _ => std::borrow::Cow::Borrowed(URL_PROMO),
    }
}

/// Returns the effective legal URL, respecting `AINXT_URL_LEGAL`.
pub fn url_legal() -> std::borrow::Cow<'static, str> {
    match std::env::var("AINXT_URL_LEGAL") {
        Ok(v) if !v.is_empty() => std::borrow::Cow::Owned(v),
        _ => std::borrow::Cow::Borrowed(URL_LEGAL),
    }
}

/// Returns the effective docs URL, respecting `AINXT_URL_DOCS`.
pub fn url_docs() -> std::borrow::Cow<'static, str> {
    match std::env::var("AINXT_URL_DOCS") {
        Ok(v) if !v.is_empty() => std::borrow::Cow::Owned(v),
        _ => std::borrow::Cow::Borrowed(URL_DOCS),
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AinxtBuildEnvironment {
    #[default]
    Production,
}
impl AinxtBuildEnvironment {
    pub fn from_flags(_dev: bool, _staging: bool) -> Self {
        AinxtBuildEnvironment::Production
    }
    /// Indicator string for display; `None` for Production.
    pub fn indicator(&self) -> Option<&'static str> {
        match self {
            AinxtBuildEnvironment::Production => None,
        }
    }
    pub fn is_production(&self) -> bool {
        matches!(self, AinxtBuildEnvironment::Production)
    }
    fn env_prefix(&self) -> &'static str {
        match self {
            AinxtBuildEnvironment::Production => "AINXT_PRODUCTION",
        }
    }
    /// Compiled endpoint set for this environment (production by default).
    pub fn endpoints(&self) -> AinxtBuildEndpoints {
        match self {
            AinxtBuildEnvironment::Production => PRODUCTION_ENDPOINTS,
        }
    }
    /// Env-var override when set, else the compiled endpoint.
    fn resolve(&self, var_suffix: &str, compiled: &'static str) -> String {
        std::env::var(format!("{}{var_suffix}", self.env_prefix()))
            .unwrap_or_else(|_| compiled.to_string())
    }
    pub fn cli_chat_proxy_base_url(&self) -> String {
        self.resolve(
            "_CLI_CHAT_PROXY_BASE_URL",
            self.endpoints().cli_chat_proxy_base_url,
        )
    }
    pub fn ws_origin(&self) -> String {
        self.resolve("_WS_ORIGIN", self.endpoints().ws_origin)
    }
    pub fn asset_server_url(&self) -> String {
        self.resolve("_ASSET_SERVER_URL", self.endpoints().asset_server_url)
    }
    /// The relay WebSocket URL (Web Frontend at `ainxt.dev/code` driving a
    /// local agent). Not the cloud-sandbox gateway ([`Self::gateway_ws_url`]);
    /// the two speak different protocols.
    pub fn relay_ws_url(&self) -> String {
        self.resolve("_WS_URL", self.endpoints().relay_ws_url)
    }
    /// The gateway WebSocket URL for `/cloud new` sandboxes. The shell's
    /// `AINXT_GATEWAY_URL` opt-in takes precedence.
    pub fn gateway_ws_url(&self) -> String {
        self.resolve("_GATEWAY_WS_URL", self.endpoints().gateway_ws_url)
    }
}
impl std::fmt::Display for AinxtBuildEnvironment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AinxtBuildEnvironment::Production => write!(f, "production"),
        }
    }
}
/// Serializes env-var mutation across tests; `std::env` is process-global.
#[cfg(any(test, feature = "test-support"))]
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
#[cfg(any(test, feature = "test-support"))]
fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner())
}
/// RAII env-var override for tests: constructors snapshot the prior value
/// under [`ENV_LOCK`], `Drop` restores it, panics included.
#[cfg(any(test, feature = "test-support"))]
pub struct EnvVarGuard {
    key: &'static str,
    prev: Option<String>,
    _lock: std::sync::MutexGuard<'static, ()>,
}
#[cfg(any(test, feature = "test-support"))]
impl EnvVarGuard {
    pub fn set(key: &'static str, value: &str) -> Self {
        let lock = env_lock();
        let prev = std::env::var(key).ok();
        unsafe { std::env::set_var(key, value) };
        Self {
            key,
            prev,
            _lock: lock,
        }
    }
    pub fn remove(key: &'static str) -> Self {
        let lock = env_lock();
        let prev = std::env::var(key).ok();
        unsafe { std::env::remove_var(key) };
        Self {
            key,
            prev,
            _lock: lock,
        }
    }
    /// Update the value while still holding the env lock.
    pub fn set_value(&self, value: &str) {
        unsafe { std::env::set_var(self.key, value) };
    }
}
#[cfg(any(test, feature = "test-support"))]
impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        match self.prev.take() {
            Some(prev) => unsafe { std::env::set_var(self.key, prev) },
            None => unsafe { std::env::remove_var(self.key) },
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    /// The env-var prefixes are an operator interface; do not rename.
    #[test]
    fn test_env_prefix() {
        assert_eq!(
            AinxtBuildEnvironment::Production.env_prefix(),
            "AINXT_PRODUCTION"
        );
    }
    #[test]
    fn env_var_guard_set_value_updates_then_restores_on_drop() {
        const KEY: &str = "AINXT_TEST_ENV_VAR_GUARD_SET_VALUE_PROBE";
        let before = std::env::var(KEY).ok();
        {
            let guard = EnvVarGuard::set(KEY, "initial");
            assert_eq!(std::env::var(KEY).ok().as_deref(), Some("initial"));
            guard.set_value("updated");
            assert_eq!(
                std::env::var(KEY).ok().as_deref(),
                Some("updated"),
                "set_value must update the env var while the guard is live"
            );
        }
        assert_eq!(
            std::env::var(KEY).ok(),
            before,
            "Drop must restore the pre-guard snapshot (was {before:?})"
        );
    }
    /// Guards against conflating the relay and gateway endpoints -- a relay loop
    /// mistakenly connecting to the cloud-sandbox gateway. The two speak
    /// different protocols, so crossing them fails in confusing ways.
    ///
    /// The compiled defaults are now empty (R42: they named an unregistered
    /// domain), and two unset endpoints are not "conflated" -- there is nothing
    /// to confuse. What still matters, and what this pins, is that the two
    /// resolve through *different* environment variables, so an operator
    /// configuring one cannot silently get the other.
    #[test]
    fn relay_and_gateway_urls_are_distinct() {
        let relay = AinxtBuildEnvironment::Production.relay_ws_url();
        let gateway = AinxtBuildEnvironment::Production.gateway_ws_url();
        if !relay.is_empty() || !gateway.is_empty() {
            assert_ne!(relay, gateway, "configured relay and gateway must differ");
        }

        // Set ONLY the relay, and confirm the gateway does not follow it.
        //
        // Exactly one guard is alive at a time on purpose. `EnvVarGuard` holds a
        // `MutexGuard` on the process-wide env lock, so constructing a second
        // one while the first is in scope deadlocks against a non-reentrant
        // `std::sync::Mutex` -- which is precisely what an earlier draft of this
        // test did, and it hung the suite rather than failing it.
        let _relay_guard = EnvVarGuard::set(
            "AINXT_PRODUCTION_WS_URL",
            "wss://relay.example.test/ws/code-agent",
        );
        assert_eq!(
            AinxtBuildEnvironment::Production.relay_ws_url(),
            "wss://relay.example.test/ws/code-agent",
            "the relay must honour its own variable"
        );
        assert_ne!(
            AinxtBuildEnvironment::Production.relay_ws_url(),
            AinxtBuildEnvironment::Production.gateway_ws_url(),
            "setting the relay must not also set the gateway"
        );
    }

    /// The shipped build must not carry an endpoint pointing at a host the
    /// publisher does not control. This is the regression guard for R42: the
    /// defaults named `*.ainxt.dev`, which was an unregistered domain, and it
    /// was compiled in as a trust anchor and an update origin.
    #[test]
    fn compiled_endpoint_defaults_are_empty() {
        for (name, value) in [
            ("cli_chat_proxy_base_url", PROD_CLI_CHAT_PROXY_BASE_URL),
            ("asset_server_url", PROD_ASSET_SERVER_URL),
            ("relay_ws_url", PROD_RELAY_WS_URL),
            ("gateway_ws_url", PROD_GATEWAY_WS_URL),
            ("ws_origin", PROD_WS_ORIGIN),
        ] {
            assert!(
                value.is_empty(),
                "{name} ships a compiled endpoint ({value:?}); endpoints must come \
                 from AINXT_PRODUCTION_* so a build never trusts a host by default"
            );
        }
        for (name, value) in [
            ("URL_SUBSCRIBE", URL_SUBSCRIBE),
            ("URL_USAGE", URL_USAGE),
            ("URL_PROMO", URL_PROMO),
            ("URL_LEGAL", URL_LEGAL),
            ("URL_DOCS", URL_DOCS),
            ("URL_CONNECTORS", URL_CONNECTORS),
            ("URL_CHANGELOG_BASE", URL_CHANGELOG_BASE),
        ] {
            assert!(
                value.is_empty(),
                "{name} ships a compiled URL ({value:?}); set AINXT_URL_* instead"
            );
        }
    }

    #[test]
    fn url_is_set_rejects_blank_values() {
        assert!(!url_is_set(""));
        assert!(!url_is_set("   "));
        assert!(url_is_set("https://example.test"));
    }

    #[test]
    fn test_from_flags() {
        assert_eq!(
            AinxtBuildEnvironment::from_flags(false, false),
            AinxtBuildEnvironment::Production
        );
    }
}
