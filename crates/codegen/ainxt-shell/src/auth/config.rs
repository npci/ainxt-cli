use super::model::TEAM_PRINCIPAL_TYPE;
use crate::env::{PROD_RELAY_WS_URL, PROD_WS_ORIGIN};
use serde::{Deserialize, Serialize};
fn default_oidc_scopes() -> Vec<String> {
    vec![
        "openid".into(),
        "profile".into(),
        "email".into(),
        "offline_access".into(),
        "api:access".into(),
    ]
}
/// Default scopes for the ainxt OAuth2 provider. Includes `ainxt-cli:access`
/// which authorizes the token for API proxy requests.
fn default_oauth2_scopes() -> Vec<String> {
    vec![
        "openid".into(),
        "profile".into(),
        "email".into(),
        "offline_access".into(),
        "ainxt-cli:access".into(),
        "api:access".into(),
        "conversations:read".into(),
        "conversations:write".into(),
        "workspaces:read".into(),
        "workspaces:write".into(),
    ]
}
fn default_team_oauth2_scopes() -> Vec<String> {
    vec![
        "profile".into(),
        "offline_access".into(),
        "ainxt-cli:access".into(),
        "api:access".into(),
        "team:read".into(),
        "conversations:read".into(),
        "conversations:write".into(),
        "workspaces:read".into(),
        "workspaces:write".into(),
    ]
}
/// Pin automatic auth to one method (`[auth] preferred_method` in config.toml).
///
/// When set, only that method is used for automatic selection; if it is
/// unavailable, auth fails (no silent fallthrough to the other method).
/// Unset keeps today's multi-method fallthrough (session preferred when both
/// exist). Config-toml only — not remote settings, settings UI, or env.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreferredAuthMethod {
    /// `AINXT_API_KEY` / auth.json `ainxt::api_key` / per-model BYOK (`ainxt.api_key`).
    ApiKey,
    /// OIDC / OAuth2 session (`cached_token`, interactive `ainxt.dev` / `oidc`,
    /// including devbox-minted OIDC).
    Oidc,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AinxtComConfig {
    pub ainxt_ws_origin: String,
    pub ainxt_ws_url: String,
    pub token_header: String,
    /// OIDC config for customer-provided IdPs. See [`OidcAuthConfig`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oidc: Option<OidcAuthConfig>,
    /// OAuth2 provider config. When set, preferred over the legacy relay flow.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oauth2: Option<OAuth2ProviderConfig>,
    /// External auth provider command (stdout = token, stderr = user UX, exit 0 = success).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_provider_command: Option<String>,
    /// Login button label (env: `AINXT_AUTH_PROVIDER_LABEL`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_provider_label: Option<String>,
    /// Token TTL in seconds for external auth providers that output bare
    /// tokens without `expires_in`. Synthesizes `expires_at` so proactive
    /// refresh works. Env: `AINXT_AUTH_TOKEN_TTL`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_token_ttl: Option<u64>,
    /// Admin kill switch: when `Some(true)`, the `ainxt.api_key` auth method is
    /// neither advertised nor accepted, so `AINXT_API_KEY`/per-model credentials
    /// can't bypass the deployment's IdP login. Env: `AINXT_DISABLE_API_KEY_AUTH`.
    /// Equivalence with common force-login-method admin knobs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disable_api_key_auth: Option<bool>,
    /// Restrict login to a specific team — the login token's team principal must
    /// equal this. Put in `requirements.toml` to enforce as non-overridable policy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub force_login_team_uuid: Option<ForceLoginTeam>,
    /// Pin automatic auth to `api_key` or `oidc`. When set and the chosen
    /// method is unavailable, auth fails (no fallthrough). Unset keeps
    /// multi-method fallthrough. Config.toml only (`[auth] preferred_method`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_method: Option<PreferredAuthMethod>,
}
/// Team login restriction. TOML string or array; an empty array fails closed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ForceLoginTeam {
    /// The only allowed team.
    Single(String),
    /// Allowed teams; empty = fail closed.
    AnyOf(Vec<String>),
}
/// Customer OIDC Identity Provider configuration (`[ainxt_com_config.oidc]`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OidcAuthConfig {
    pub issuer: String,
    pub client_id: String,
    #[serde(default = "default_oidc_scopes")]
    pub scopes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audience: Option<String>,
}
/// OAuth2 provider configuration (`AINXT_OAUTH2_ISSUER` / `AINXT_OAUTH2_CLIENT_ID`).
///
/// Uses the standard OAuth 2.1 Auth Code + PKCE flow via [`OidcAuthConfig`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuth2ProviderConfig {
    pub issuer: String,
    pub client_id: String,
    #[serde(default = "default_oauth2_scopes")]
    pub scopes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub principal_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub principal_id: Option<String>,
    /// Client-supplied referrer for OAuth usage-attribution analytics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub referrer: Option<String>,
}
/// Environment variable naming this build's OAuth2 issuer.
///
/// The same name [`OAuth2ProviderConfig::from_env`] already reads, so an
/// operator sets one variable and both the provider config and the trust
/// predicate agree.
pub const OAUTH2_ISSUER_ENV: &str = "AINXT_OAUTH2_ISSUER";

/// Serialises tests that need a configured OAuth2 issuer.
///
/// Rust runs a crate's tests in parallel threads of one process, so mutating a
/// process-global env var is a data race between test functions. Every test
/// that needs an issuer takes this guard, so they queue instead of racing, and
/// the previous value is restored on drop — panics included.
#[cfg(any(test, feature = "test-support"))]
pub struct OAuth2IssuerGuard {
    prev: Option<String>,
    _lock: std::sync::MutexGuard<'static, ()>,
}

#[cfg(any(test, feature = "test-support"))]
impl OAuth2IssuerGuard {
    /// The issuer used by tests. RFC 6761 reserves `.test` so this provably
    /// resolves nowhere, which is the point: a test must not name a host that
    /// could exist.
    pub const TEST_ISSUER: &'static str = "https://auth.example.test";

    /// Configure `issuer` for as long as the guard is alive.
    pub fn set(issuer: &str) -> Self {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let lock = LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let prev = std::env::var(OAUTH2_ISSUER_ENV).ok();
        unsafe { std::env::set_var(OAUTH2_ISSUER_ENV, issuer) };
        Self { prev, _lock: lock }
    }

    /// Configure [`Self::TEST_ISSUER`].
    pub fn set_test_issuer() -> Self {
        Self::set(Self::TEST_ISSUER)
    }
}

#[cfg(any(test, feature = "test-support"))]
impl Drop for OAuth2IssuerGuard {
    fn drop(&mut self) {
        match self.prev.take() {
            Some(prev) => unsafe { std::env::set_var(OAUTH2_ISSUER_ENV, prev) },
            None => unsafe { std::env::remove_var(OAUTH2_ISSUER_ENV) },
        }
    }
}
/// Environment variable naming the accounts-app origins this build accepts,
/// comma-separated.
pub const ACCOUNTS_APP_ORIGINS_ENV: &str = "AINXT_ACCOUNTS_APP_ORIGINS";

/// Accounts-app origin allowlist — **empty in the open-source build**.
///
/// This feeds [`accounts_app_cors_layer`], so it is a CORS allowlist: every
/// origin named here may make cross-origin requests to the local endpoints the
/// OIDC login flow stands up. It previously read `https://accounts.example.test`,
/// and that domain was never registered (audit risk R42) — so the
/// browser-facing allowlist of a shipped binary named a host that anyone could
/// have bought and then spoken to those endpoints from.
///
/// Empty is fail-closed: `AllowOrigin::list([])` matches no origin, so the
/// login endpoints refuse cross-origin callers outright until an operator names
/// their own accounts app via `AINXT_ACCOUNTS_APP_ORIGINS`.
const PROD_ACCOUNTS_APP_ORIGINS: &[&str] = &[];

/// The accounts-app origins this build accepts.
///
/// Reads [`ACCOUNTS_APP_ORIGINS_ENV`] (comma-separated), else the compiled
/// list, which ships empty. Blank entries are dropped so that a stray trailing
/// comma cannot add an empty-string origin to a CORS allowlist.
pub fn allowed_accounts_app_origins() -> Vec<String> {
    if let Ok(raw) = std::env::var(ACCOUNTS_APP_ORIGINS_ENV) {
        let configured: Vec<String> = raw
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect();
        if !configured.is_empty() {
            return configured;
        }
    }
    PROD_ACCOUNTS_APP_ORIGINS
        .iter()
        .map(|o| o.to_string())
        .collect()
}
/// Build a CORS layer that accepts requests from the accounts-app deployments
/// listed in [`allowed_accounts_app_origins`] for the given HTTP method.
///
/// Callers can chain additional configuration (e.g. `.allow_headers(...)` or
/// `.allow_private_network(true)`) onto the returned layer.
pub fn accounts_app_cors_layer(method: axum::http::Method) -> tower_http::cors::CorsLayer {
    tower_http::cors::CorsLayer::new()
        .allow_origin(tower_http::cors::AllowOrigin::list(
            allowed_accounts_app_origins()
                .iter()
                .filter_map(|origin| match origin.parse() {
                    Ok(value) => Some(value),
                    Err(_) => {
                        tracing::warn!(origin, "skipping malformed accounts-app CORS origin");
                        None
                    }
                }),
        ))
        .allow_methods([method])
}
/// Local-dev OAuth2 issuer (accounts-app running on localhost).
const AINXT_OAUTH2_LOCAL_ISSUER: &str = "http://localhost:22255";
const DEFAULT_OAUTH2_REFERRER: &str = "ainxt-build";
/// Returns `true` when `AINXT_LOCAL_AUTH=1` is set,
/// indicating the local accounts-app should be used as the OAuth2 issuer.
pub fn use_local_auth() -> bool {
    std::env::var("AINXT_LOCAL_AUTH")
        .map(|v| !v.is_empty() && v != "0")
        .unwrap_or(false)
}
/// The OAuth2 issuer this build treats as first-party.
///
/// Resolution order: the local-dev issuer when `AINXT_LOCAL_AUTH=1`, else
/// whatever [`OAUTH2_ISSUER_ENV`] names, else **empty**.
///
/// Empty is the shipped default and it is deliberate. This used to be a
/// compiled constant reading `https://auth.ainxt.dev` — a placeholder domain
/// that was never registered (audit risk R42). An OAuth2 issuer is a trust
/// root: [`is_ainxt_oauth2_issuer`] decides, from this value, whether a token
/// is treated as first-party auth. A build that names an issuer its publisher
/// does not control has delegated that decision to whoever registers the name.
///
/// A source-available build has no identity provider behind it, so there is no
/// honest issuer to compile in. Operators set the variable to their own.
pub fn ainxt_oauth2_issuer() -> String {
    if use_local_auth() {
        return AINXT_OAUTH2_LOCAL_ISSUER.to_owned();
    }
    std::env::var(OAUTH2_ISSUER_ENV)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_default()
}

/// Returns `true` if `issuer` is a recognised first-party OAuth2 issuer — the
/// one this build is configured with, or the local-dev accounts app.
///
/// **Fails closed.** An empty candidate is never first-party, and neither is
/// anything at all when no issuer is configured. Without that guard, emptying
/// the compiled default would have made `""` match `""` and quietly promoted
/// every token with a missing issuer claim to first-party auth — turning a
/// hardening change into a privilege escalation.
pub fn is_ainxt_oauth2_issuer(issuer: &str) -> bool {
    let issuer = issuer.trim();
    if issuer.is_empty() {
        return false;
    }
    if issuer == AINXT_OAUTH2_LOCAL_ISSUER {
        return true;
    }
    let configured = ainxt_oauth2_issuer();
    !configured.is_empty() && issuer == configured
}
/// auth.json scope key used by the pre-OIDC `ainxt login --legacy` flow.
/// Matches the key format produced by the original `accounts.ainxt.dev` relay auth.
pub const LEGACY_AUTH_SCOPE: &str = "https://accounts.example.test/sign-in";
impl AinxtComConfig {
    /// Whether `ainxt.api_key` auth is disabled. Pinning a team
    /// (`force_login_team_uuid`) implies this — team membership can't be verified
    /// from a bare API key, so it must go through IdP login. The
    /// `AINXT_DISABLE_API_KEY_AUTH` env lockdown is sticky: because the env value
    /// seeds `default()` (the merge base), a lower-trust user `config.toml` could
    /// otherwise set `disable_api_key_auth = false` and override it — so the env
    /// is OR-ed in here and cannot be turned back off by a user layer. Trusted
    /// `requirements.toml` already wins over `config.toml` via layer precedence.
    pub fn api_key_auth_disabled(&self) -> bool {
        self.disable_api_key_auth == Some(true)
            || self.force_login_team_uuid.is_some()
            || env_lockdown_forced()
    }
    /// When `preferred_method = api_key`, automatic OIDC paths (devbox mint,
    /// interactive browser login, external auth provider) must not run — the
    /// pin is fail-closed. Explicit `ainxt login --devbox` / `--api-key` bypass
    /// this by not consulting automatic flow helpers.
    pub fn blocks_automatic_oidc(&self) -> bool {
        matches!(self.preferred_method, Some(PreferredAuthMethod::ApiKey))
    }
    /// The auth.json scope key for this config.
    pub fn auth_scope(&self) -> String {
        if let Some(ref oidc) = self.oidc {
            format!("{}::{}", oidc.issuer.trim_end_matches('/'), oidc.client_id)
        } else if let Some(ref oauth2) = self.oauth2 {
            oauth2.auth_scope()
        } else {
            unreachable!("oauth2 config is always present (ainxt default or env override)")
        }
    }
}
impl OAuth2ProviderConfig {
    pub fn is_team_principal(&self) -> bool {
        self.principal_type.as_deref() == Some(TEAM_PRINCIPAL_TYPE)
    }
    pub fn from_env() -> Option<Self> {
        let issuer = std::env::var("AINXT_OAUTH2_ISSUER").ok()?;
        let client_id = std::env::var("AINXT_OAUTH2_CLIENT_ID").ok()?;
        let principal_type = std::env::var("AINXT_OAUTH2_PRINCIPAL_TYPE").ok();
        let principal_id = std::env::var("AINXT_OAUTH2_PRINCIPAL_ID").ok();
        let default_scopes = match principal_type.as_deref() {
            Some(TEAM_PRINCIPAL_TYPE) => default_team_oauth2_scopes(),
            _ => default_oauth2_scopes(),
        };
        Some(Self {
            issuer,
            client_id,
            scopes: std::env::var("AINXT_OAUTH2_SCOPES")
                .map(|s| s.split(',').map(|s| s.trim().to_owned()).collect())
                .unwrap_or(default_scopes),
            principal_type,
            principal_id,
            referrer: Some(
                std::env::var("AINXT_OAUTH2_REFERRER")
                    .unwrap_or_else(|_| DEFAULT_OAUTH2_REFERRER.to_owned()),
            ),
        })
    }
    /// Convert to [`OidcAuthConfig`] to reuse the OIDC login flow.
    pub fn as_oidc(&self) -> OidcAuthConfig {
        OidcAuthConfig {
            issuer: self.issuer.clone(),
            client_id: self.client_id.clone(),
            scopes: self.scopes.clone(),
            audience: None,
        }
    }
    pub fn base_auth_scope(&self) -> String {
        format!("{}::{}", self.issuer.trim_end_matches('/'), self.client_id)
    }
    pub fn auth_scope(&self) -> String {
        self.base_auth_scope()
    }
}
impl Default for AinxtComConfig {
    fn default() -> Self {
        let oidc = OidcAuthConfig::from_env();
        let oauth2 = if oidc.is_some() {
            None
        } else {
            Some(
                OAuth2ProviderConfig::from_env().unwrap_or_else(|| OAuth2ProviderConfig {
                    issuer: ainxt_oauth2_issuer(),
                    client_id: obfstr::obfstr!("b1a00492-073a-47ea-816f-4c329264a828").to_owned(),
                    scopes: default_oauth2_scopes(),
                    principal_type: None,
                    principal_id: None,
                    referrer: Some(DEFAULT_OAUTH2_REFERRER.to_owned()),
                }),
            )
        };
        Self {
            ainxt_ws_origin: std::env::var("AINXT_WS_ORIGIN")
                .unwrap_or_else(|_| PROD_WS_ORIGIN.to_owned()),
            ainxt_ws_url: std::env::var("AINXT_WS_URL")
                .unwrap_or_else(|_| PROD_RELAY_WS_URL.to_owned()),
            token_header: "ainxt-cli".to_owned(),
            oidc,
            oauth2,
            auth_provider_command: std::env::var("AINXT_AUTH_PROVIDER_COMMAND").ok(),
            auth_provider_label: std::env::var("AINXT_AUTH_PROVIDER_LABEL").ok(),
            auth_token_ttl: std::env::var("AINXT_AUTH_TOKEN_TTL")
                .ok()
                .and_then(|v| v.parse().ok()),
            disable_api_key_auth: std::env::var("AINXT_DISABLE_API_KEY_AUTH")
                .ok()
                .map(|v| env_flag_enabled(&v)),
            force_login_team_uuid: None,
            preferred_method: None,
        }
    }
}
/// Parse a boolean env-var value for ainxt's on/off flags. A bare presence
/// enables the flag, but the common falsy spellings (`0`, `false`, `off`,
/// `no`, empty) count as disabled — so e.g. `AINXT_DISABLE_API_KEY_AUTH=false`
/// does NOT turn the kill switch on.
fn env_flag_enabled(value: &str) -> bool {
    !matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "" | "0" | "false" | "off" | "no"
    )
}
/// True when the admin has set `AINXT_DISABLE_API_KEY_AUTH` to a truthy value in
/// the process environment. Read live (call-time) and OR-ed into
/// `api_key_auth_disabled()` so the env lockdown is non-overridable by a
/// user-layer `config.toml`.
fn env_lockdown_forced() -> bool {
    std::env::var("AINXT_DISABLE_API_KEY_AUTH")
        .ok()
        .is_some_and(|v| env_flag_enabled(&v))
}
impl OidcAuthConfig {
    pub fn from_env() -> Option<Self> {
        let issuer = std::env::var("AINXT_OIDC_ISSUER").ok()?;
        let client_id = std::env::var("AINXT_OIDC_CLIENT_ID").ok()?;
        Some(Self {
            issuer,
            client_id,
            scopes: std::env::var("AINXT_OIDC_SCOPES")
                .map(|s| s.split(',').map(|s| s.trim().to_owned()).collect())
                .unwrap_or_else(|_| default_oidc_scopes()),
            audience: std::env::var("AINXT_OIDC_AUDIENCE").ok(),
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn team_auth_scope_is_base_scope() {
        let cfg = OAuth2ProviderConfig {
            issuer: "https://auth.example.test".into(),
            client_id: "client-123".into(),
            scopes: default_team_oauth2_scopes(),
            principal_type: Some("Team".into()),
            principal_id: Some("team-abc".into()),
            referrer: Some("ainxt-build".into()),
        };
        assert_eq!(cfg.auth_scope(), "https://auth.example.test::client-123");
    }
    #[test]
    fn env_flag_enabled_treats_falsy_spellings_as_off() {
        for off in ["", " ", "0", "false", "FALSE", "off", "No", "  false  "] {
            assert!(!env_flag_enabled(off), "{off:?} should be off");
        }
        for on in ["1", "true", "yes", "on", "enabled"] {
            assert!(env_flag_enabled(on), "{on:?} should be on");
        }
    }
    #[test]
    fn personal_auth_scope_is_base_scope() {
        let cfg = OAuth2ProviderConfig {
            issuer: "https://auth.example.test".into(),
            client_id: "client-123".into(),
            scopes: default_oauth2_scopes(),
            principal_type: None,
            principal_id: None,
            referrer: Some("ainxt-build".into()),
        };
        assert_eq!(cfg.auth_scope(), "https://auth.example.test::client-123");
    }
    /// The accounts-app CORS allowlist: which origins may speak to the CLI's
    /// loopback callback server.
    ///
    /// This test used to freeze the list at `https://accounts.example.test`, on the
    /// stated grounds that removing an origin "breaks loopback delivery for
    /// already-installed CLIs". That reasoning is sound in general and did not
    /// apply here: the domain was never registered (audit risk R42), so no
    /// installed CLI was using it.
    ///
    /// The contract now pinned is the one worth keeping: **empty by default**,
    /// operator-configurable, and never silently widened.
    /// The OAuth2 issuer is a trust root, and this build ships without one.
    ///
    /// It used to be a compiled constant naming `https://auth.example.test`, a
    /// placeholder domain that was never registered (R42). Whoever bought it
    /// could have minted tokens this client accepted as first-party.
    #[test]
    fn oauth2_issuer_ships_unset_and_fails_closed() {
        let _guard = OAuth2IssuerGuard::set("");
        assert!(
            ainxt_oauth2_issuer().is_empty(),
            "no issuer should be configured by default"
        );

        // With nothing configured, NOTHING is first-party. In particular the
        // empty string must not match the empty default -- that would promote
        // every token with a missing issuer claim to first-party auth.
        for candidate in [
            "",
            "   ",
            "https://auth.example.test",
            "https://auth.example.test",
            "https://evil.example",
        ] {
            assert!(
                !is_ainxt_oauth2_issuer(candidate),
                "unconfigured build must not trust {candidate:?}"
            );
        }
    }

    #[test]
    fn oauth2_issuer_trusts_exactly_what_is_configured() {
        let _guard = OAuth2IssuerGuard::set_test_issuer();
        let trusted = OAuth2IssuerGuard::TEST_ISSUER;

        assert_eq!(ainxt_oauth2_issuer(), trusted);
        assert!(is_ainxt_oauth2_issuer(trusted));
        // Surrounding whitespace is tolerated on the candidate.
        assert!(is_ainxt_oauth2_issuer(&format!("  {trusted}  ")));

        // Everything else stays third-party, including near-misses.
        for candidate in [
            "",
            "https://auth.example.test.evil.example",
            "https://evil-auth.example.test",
            "https://auth.example.test/extra",
            "https://some-other-idp.example.test",
        ] {
            assert!(
                !is_ainxt_oauth2_issuer(candidate),
                "{candidate:?} must not be treated as first-party"
            );
        }
    }

    #[test]
    fn local_dev_issuer_is_always_recognised() {
        // Independent of configuration: `AINXT_LOCAL_AUTH` workflows depend on
        // the loopback accounts app being first-party.
        let _guard = OAuth2IssuerGuard::set("");
        assert!(is_ainxt_oauth2_issuer("http://localhost:22255"));
    }

    #[test]
    fn accounts_app_origins_ship_empty_and_are_configurable() {
        // Nothing compiled in. `AllowOrigin::list([])` matches no origin.
        assert!(
            PROD_ACCOUNTS_APP_ORIGINS.is_empty(),
            "a shipped CORS allowlist must not name a host this build does not control"
        );

        let _guard = EnvGuard::set(ACCOUNTS_APP_ORIGINS_ENV, "https://accounts.example.test");
        assert_eq!(
            allowed_accounts_app_origins(),
            vec!["https://accounts.example.test".to_string()],
            "an operator-declared origin must be honoured"
        );

        // Whitespace and stray commas must not widen the allowlist, and must
        // never introduce an empty-string origin.
        let _guard = EnvGuard::set(
            ACCOUNTS_APP_ORIGINS_ENV,
            " https://a.example.test , ,https://b.example.test ,",
        );
        assert_eq!(
            allowed_accounts_app_origins(),
            vec![
                "https://a.example.test".to_string(),
                "https://b.example.test".to_string()
            ]
        );

        // An all-blank value is not a configuration; fall back to empty.
        let _guard = EnvGuard::set(ACCOUNTS_APP_ORIGINS_ENV, " , , ");
        assert!(allowed_accounts_app_origins().is_empty());
    }

    /// Sets an env var for the duration of a test and restores it after.
    struct EnvGuard {
        key: &'static str,
        prev: Option<String>,
    }
    impl EnvGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let prev = std::env::var(key).ok();
            unsafe { std::env::set_var(key, value) };
            Self { key, prev }
        }
    }
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match self.prev.take() {
                Some(prev) => unsafe { std::env::set_var(self.key, prev) },
                None => unsafe { std::env::remove_var(self.key) },
            }
        }
    }
    /// FROZEN client contract: the 10 scopes the ainxt OAuth2 client requests.
    /// The server must keep accepting all of them; existing tokens carry
    /// exactly this set. Frozen OAuth client scope contract.
    #[test]
    fn default_oauth2_scopes_are_frozen() {
        let scopes = default_oauth2_scopes();
        let scopes: Vec<&str> = scopes.iter().map(String::as_str).collect();
        assert_eq!(
            scopes,
            [
                "openid",
                "profile",
                "email",
                "offline_access",
                "ainxt-cli:access",
                "api:access",
                "conversations:read",
                "conversations:write",
                "workspaces:read",
                "workspaces:write",
            ]
        );
    }
    #[test]
    fn preferred_method_deserializes_from_toml() {
        let cfg: AinxtComConfig = toml::from_str(
            r#"
            preferred_method = "api_key"
            "#,
        )
        .expect("parse");
        assert_eq!(cfg.preferred_method, Some(PreferredAuthMethod::ApiKey));
        let cfg: AinxtComConfig = toml::from_str(
            r#"
            preferred_method = "oidc"
            "#,
        )
        .expect("parse");
        assert_eq!(cfg.preferred_method, Some(PreferredAuthMethod::Oidc));
        let cfg: AinxtComConfig = toml::from_str("").expect("parse empty");
        assert_eq!(cfg.preferred_method, None);
    }
}
