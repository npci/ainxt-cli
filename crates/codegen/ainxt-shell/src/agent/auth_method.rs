use agent_client_protocol as acp;

use crate::agent::config::ModelEntry;
use crate::auth::PreferredAuthMethod;

/// Shared, live handle to the agent's current ACP auth method id.
///
/// `Arc` so a clone can cross the per-session-thread boundary at spawn; the
/// `ArcSwapOption` interior lets the agent's `authenticate` handler publish a
/// new method that every running session's per-turn auth gate observes on its
/// next turn -- no re-spawn. `None` until the first `authenticate`. Auth is
/// process-global (one user, one `AuthManager`), so all sessions sharing one
/// cell is correct.
pub(crate) type SharedAuthMethodId = std::sync::Arc<arc_swap::ArcSwapOption<acp::AuthMethodId>>;

/// Construct a [`SharedAuthMethodId`]. `None` is the pre-`authenticate` state.
pub(crate) fn new_shared_auth_method_id(initial: Option<acp::AuthMethodId>) -> SharedAuthMethodId {
    std::sync::Arc::new(arc_swap::ArcSwapOption::new(
        initial.map(std::sync::Arc::new),
    ))
}

/// Env var that, when set, advertises `ainxt.api_key` as a viable auth method.
///
/// Kept as a constant so test code and the production check stay in sync.
pub const AINXT_API_KEY_ENV_VAR: &str = "AINXT_API_KEY";

/// Legacy env var name. Checked as a fallback when `AINXT_API_KEY` is not set,
/// so existing deployments that use the old name keep working.
pub const LEGACY_AINXT_API_KEY_ENV_VAR: &str = "AINXT_CODE_AINXT_API_KEY";

/// Primary bearer-token env var for the ainxt backend. `ainxt` authenticates
/// against your backend with this token (sent as `Authorization: Bearer`).
/// Preferred over `AINXT_API_KEY`; both are accepted.
pub const AINXT_TOKEN_ENV_VAR: &str = "AINXT_TOKEN";

/// Read the bearer token / API key from the environment, falling back to the
/// token stored by `ainxt login` at `~/.ainxt/credentials.json`.
///
/// Resolution order:
///   1. `AINXT_TOKEN` (the ainxt backend token)
///   2. `AINXT_API_KEY`
///   3. legacy `AINXT_CODE_AINXT_API_KEY`
///   4. the stored `credentials.json` access token (from `ainxt login`)
pub fn read_ainxt_api_key_env() -> Result<String, std::env::VarError> {
    std::env::var(AINXT_TOKEN_ENV_VAR)
        .or_else(|_| std::env::var(AINXT_API_KEY_ENV_VAR))
        .or_else(|_| std::env::var(LEGACY_AINXT_API_KEY_ENV_VAR))
        .or_else(|_| {
            crate::auth::gateway_auth::load_credentials()
                .map(|c| c.access_token)
                .filter(|t| !t.is_empty())
                .ok_or(std::env::VarError::NotPresent)
        })
}

/// Returns `true` if either `AINXT_API_KEY` or `AINXT_CODE_AINXT_API_KEY` is set.
pub fn has_ainxt_api_key_env() -> bool {
    read_ainxt_api_key_env().is_ok()
}

/// Whether `ainxt.api_key` should be advertised (and pushed FIRST) when building
/// the `auth_methods` list at `initialize()` time.
///
/// Regression: `ainxt.api_key` must stay first when only per-model credentials
/// exist (no global `AINXT_API_KEY`). Deferring it made BYOK users hit the login
/// screen because the pager uses `auth_methods.first()` for startup metadata.
///
/// [`build_auth_methods`] consumes this predicate and pins the ordering;
/// its tests catch call-site and predicate regressions.
///
/// Probes `std::env` at call time and consults each `ModelEntry` for a
/// resolvable api_key/env_key -- both inputs can change between calls, so the
/// result is not cached.
///
/// `disable_api_key_auth` (`[ainxt_com_config] disable_api_key_auth` /
/// `AINXT_DISABLE_API_KEY_AUTH`) is the admin kill switch: when true the
/// method is never advertised, regardless of available credentials, so
/// `AINXT_API_KEY` can't bypass a deployment's forced IdP login.
pub fn should_advertise_ainxt_api_key<'a, I>(disable_api_key_auth: bool, models: I) -> bool
where
    I: IntoIterator<Item = &'a ModelEntry>,
{
    if disable_api_key_auth {
        return false;
    }
    has_ainxt_api_key_env() || models.into_iter().any(ModelEntry::has_own_credentials)
}

/// Inputs to [`build_auth_methods`].
///
/// Booleans are computed by the caller (`MvpAgent::initialize()`) because they
/// depend on async side effects (token refresh) and shared mutable state
/// (`AuthManager`). The list-construction logic itself is pure so it can be
/// unit-tested without any of that machinery.
pub struct AuthMethodsBuildInputs<'a> {
    /// True if `ainxt.api_key` should be advertised AT ALL. Caller computes via
    /// [`should_advertise_ainxt_api_key`]. When `preferred_method` is `Oidc`,
    /// this is ignored (API key is never advertised under that pin).
    pub has_external_api_key: bool,
    /// True if a cached session token is available (either present at startup
    /// or recovered via silent refresh).
    pub has_cached_token: bool,
    /// True if enterprise OIDC is configured. Mutually exclusive with the
    /// default `ainxt.dev` method.
    pub has_enterprise_oidc: bool,
    /// Required when `has_enterprise_oidc` is true; ignored otherwise.
    pub enterprise_oidc_issuer: Option<&'a str>,
    /// Optional display label for the login method (`ainxt.dev` or `oidc`).
    pub login_label: Option<&'a str>,
    /// True if `ainxt_com_config.auth_provider_command` is configured (sets
    /// `meta.external_provider = true` on the `ainxt.dev` method).
    pub has_auth_provider_command: bool,
    /// Config pin (`[auth] preferred_method`). `None` keeps multi-method
    /// fallthrough; `Some` is fail-closed (only that method family).
    pub preferred_method: Option<PreferredAuthMethod>,
}

/// Output of [`build_auth_methods`].
pub struct BuiltAuthMethods {
    /// Auth methods in advertised order. ORDER IS THE CONTRACT: the pager's
    /// `startup_auth_metadata()` reads `methods.first()` to decide whether
    /// interactive login is needed.
    pub methods: Vec<acp::AuthMethod>,
    /// The default `auth_method_id` to install on the agent. When unpinned,
    /// `cached_token` wins over `ainxt.api_key` when both are present. When
    /// pinned, only the preferred method may appear; `None` means unavailable
    /// (fail auth — no cross-method fallthrough).
    pub default_auth_method_id: Option<acp::AuthMethodId>,
}

/// Build the `auth_methods` list and default `auth_method_id` from
/// pre-computed inputs.
///
/// REGRESSION GUARD: when unpinned and
/// `has_external_api_key` is true, the **first** entry MUST be `ainxt.api_key`.
/// A prior change deferred it to the END for per-model credentials, which made
/// the pager send per-model-key users to the login screen. Unit tests lock this.
///
/// Unpinned ordering (when each method is enabled):
/// 1. `ainxt.api_key`     (if `has_external_api_key`)
/// 2. `cached_token`    (if `has_cached_token`)
/// 3. exactly one of:
///    - `oidc`          (if `has_enterprise_oidc`)
///    - `ainxt.dev`      (otherwise)
///
/// Unpinned `default_auth_method_id`:
/// - `cached_token` if `has_cached_token`
/// - `ainxt.api_key`  else if `has_external_api_key`
/// - `None`         otherwise
///
/// Pinned (`preferred_method`):
/// - `ApiKey`: only `ainxt.api_key` if available; else empty list + `None` (fail).
/// - `Oidc`: `cached_token` (if any) + interactive login; never `ainxt.api_key`.
///   Default is `cached_token` when present, else `None` (interactive).
pub fn build_auth_methods(inputs: AuthMethodsBuildInputs<'_>) -> BuiltAuthMethods {
    let AuthMethodsBuildInputs {
        has_external_api_key,
        has_cached_token,
        has_enterprise_oidc,
        enterprise_oidc_issuer,
        login_label,
        has_auth_provider_command,
        preferred_method,
    } = inputs;

    match preferred_method {
        Some(PreferredAuthMethod::ApiKey) => build_pinned_api_key(has_external_api_key),
        Some(PreferredAuthMethod::Oidc) => build_pinned_oidc(
            has_cached_token,
            has_enterprise_oidc,
            enterprise_oidc_issuer,
            login_label,
            has_auth_provider_command,
        ),
        None => build_unpinned(
            has_external_api_key,
            has_cached_token,
            has_enterprise_oidc,
            enterprise_oidc_issuer,
            login_label,
            has_auth_provider_command,
        ),
    }
}

fn build_pinned_api_key(has_external_api_key: bool) -> BuiltAuthMethods {
    if !has_external_api_key {
        ainxt_telemetry::unified_log::warn(
            "auth: preferred_method=api_key but no API key credentials available",
            None,
            None,
        );
        return BuiltAuthMethods {
            methods: Vec::new(),
            default_auth_method_id: None,
        };
    }
    BuiltAuthMethods {
        methods: vec![ainxt_api_key_auth_method()],
        default_auth_method_id: Some(acp::AuthMethodId::new(AINXT_API_KEY_METHOD_ID)),
    }
}

fn build_pinned_oidc(
    has_cached_token: bool,
    has_enterprise_oidc: bool,
    enterprise_oidc_issuer: Option<&str>,
    login_label: Option<&str>,
    has_auth_provider_command: bool,
) -> BuiltAuthMethods {
    let mut methods: Vec<acp::AuthMethod> = Vec::new();
    let mut default_auth_method_id: Option<acp::AuthMethodId> = None;

    if has_cached_token {
        methods.push(cached_token_auth_method());
        default_auth_method_id = Some(acp::AuthMethodId::new(CACHED_TOKEN_AUTH_METHOD_ID));
    }

    push_interactive_login(
        &mut methods,
        has_enterprise_oidc,
        enterprise_oidc_issuer,
        login_label,
        has_auth_provider_command,
    );

    BuiltAuthMethods {
        methods,
        default_auth_method_id,
    }
}

fn build_unpinned(
    has_external_api_key: bool,
    has_cached_token: bool,
    has_enterprise_oidc: bool,
    enterprise_oidc_issuer: Option<&str>,
    login_label: Option<&str>,
    has_auth_provider_command: bool,
) -> BuiltAuthMethods {
    let mut methods: Vec<acp::AuthMethod> = Vec::new();
    let mut default_auth_method_id: Option<acp::AuthMethodId> = None;

    if has_external_api_key {
        methods.push(ainxt_api_key_auth_method());
        default_auth_method_id = Some(acp::AuthMethodId::new(AINXT_API_KEY_METHOD_ID));
    }

    if has_cached_token {
        methods.push(cached_token_auth_method());
        // cached_token wins over ainxt.api_key for default_auth_method_id so
        // is_session_based_auth() returns true and OIDC refresh stays alive.
        let overrode_api_key = default_auth_method_id.is_some();
        default_auth_method_id = Some(acp::AuthMethodId::new(CACHED_TOKEN_AUTH_METHOD_ID));
        if overrode_api_key {
            ainxt_telemetry::unified_log::info(
                "auth method priority: cached_token overrides ainxt.api_key for default_auth_method_id",
                None,
                Some(serde_json::json!({
                    "has_external_api_key": has_external_api_key,
                    "has_cached_token": has_cached_token,
                })),
            );
        }
    }

    push_interactive_login(
        &mut methods,
        has_enterprise_oidc,
        enterprise_oidc_issuer,
        login_label,
        has_auth_provider_command,
    );

    BuiltAuthMethods {
        methods,
        default_auth_method_id,
    }
}

fn push_interactive_login(
    methods: &mut Vec<acp::AuthMethod>,
    has_enterprise_oidc: bool,
    enterprise_oidc_issuer: Option<&str>,
    login_label: Option<&str>,
    has_auth_provider_command: bool,
) {
    if has_enterprise_oidc {
        // Caller invariant: `enterprise_oidc_issuer` MUST be `Some(...)` when
        // `has_enterprise_oidc` is true. Production callers derive both from
        // the same `cfg.ainxt_com_config.oidc` Option, so the inconsistent
        // `(true, None)` combination is a programmer error -- panic loudly
        // (matches the original `cfg.ainxt_com_config.oidc.as_ref().unwrap()`
        // call in `MvpAgent::initialize()` before this refactor).
        let issuer = enterprise_oidc_issuer
            .expect("enterprise_oidc_issuer is required when has_enterprise_oidc is true");
        methods.push(oidc_auth_method(issuer, login_label));
    } else {
        methods.push(ainxt_com_auth_method(login_label, has_auth_provider_command));
    }
}

/// ACP session auth method. Use `is_session_based_method` for classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMethodKind {
    AinxtApiKey,
    CachedToken,
    AinxtCom,
    Oidc,
    Unknown,
}

impl AuthMethodKind {
    pub fn from_id(id: &acp::AuthMethodId) -> Self {
        match id.0.as_ref() {
            AINXT_API_KEY_METHOD_ID => Self::AinxtApiKey,
            CACHED_TOKEN_AUTH_METHOD_ID => Self::CachedToken,
            AINXT_COM_METHOD_ID => Self::AinxtCom,
            OIDC_METHOD_ID => Self::Oidc,
            _ => Self::Unknown,
        }
    }

    /// API key auth: no auth.json, no refresh, no user interaction.
    pub fn is_api_key(self) -> bool {
        matches!(self, Self::AinxtApiKey)
    }

    /// `true` for session-based methods (cached_token, ainxt.dev, oidc).
    pub fn is_session_based(self) -> bool {
        matches!(self, Self::CachedToken | Self::AinxtCom | Self::Oidc)
    }

    /// Requires user interaction (browser, OIDC redirect, or external auth command).
    pub fn needs_interactive_login(self) -> bool {
        matches!(self, Self::AinxtCom | Self::Oidc)
    }

    pub fn auth_error_message(self) -> &'static str {
        if self.is_session_based() {
            AUTH_ERROR_SESSION_EXPIRED
        } else {
            AUTH_ERROR_API_KEY
        }
    }
}

/// `true` for session-based ACP methods (cached_token, ainxt.dev, oidc).
pub fn is_session_based_method(method_id: &acp::AuthMethodId) -> bool {
    AuthMethodKind::from_id(method_id).is_session_based()
}

/// Per-model BYOK status: whether the selected model carries its own
/// `[model.*]` `api_key`/`env_key`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelByok {
    /// Model has its own per-model key (not refreshable).
    Byok,
    /// Model has no per-model key (session auth governs).
    NotByok,
    /// Config couldn't be loaded/parsed — BYOK status indeterminate.
    Unknown,
}

impl ModelByok {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Byok => "byok",
            Self::NotByok => "not_byok",
            Self::Unknown => "unknown",
        }
    }
}

/// Whether this session+model uses a refreshable session token.
///
/// Gates on stable inputs, not `Credentials.auth_type`: that field collapses
/// to `ApiKey` when the session-token cache is momentarily empty and
/// `AINXT_API_KEY` is set, which demoted live OIDC sessions to non-refreshable
/// api-key mode and 401'd every prompt until restart. `model_byok` still
/// excludes genuine per-model BYOK, whose keys are not refreshable.
///
/// `Unknown` (BYOK status indeterminate — config currently unparseable, no
/// sampling config yet, or the per-model memo was cleared) must **not** demote
/// a live session to non-refreshable api-key mode: that re-sends the stale
/// buffered token on every turn and 401s with `bad-credentials` until restart
/// (the stale-token regression this gate addresses; fall back rather than
/// demote on `Unknown`). It refreshes when `endpoint_is_first_party` — the
/// request targets a first-party host (cli-chat-proxy / first-party API),
/// where sending the session token cannot leak to a third-party BYOK
/// endpoint. A definite `NotByok` always refreshes (it only ever routes to
/// the session endpoint); a definite `Byok` never does.
pub fn session_token_auth_gate(
    is_session_based_method: bool,
    model_byok: ModelByok,
    endpoint_is_first_party: bool,
) -> bool {
    is_session_based_method
        && match model_byok {
            ModelByok::NotByok => true,
            ModelByok::Byok => false,
            ModelByok::Unknown => endpoint_is_first_party,
        }
}

pub const AUTH_ERROR_SESSION_EXPIRED: &str =
    "Session expired. Run `ainxt login` to re-authenticate.";

pub const AUTH_ERROR_API_KEY: &str = "Authentication failed. Run `ainxt login`, set AINXT_API_KEY, or add api_key to ~/.ainxt/config.toml.";

/// Next ACP method id when `cached_token` cannot proceed (missing / expired /
/// legacy WebLogin), or `None` when fallthrough is forbidden.
///
/// Unpinned: prefer non-interactive `ainxt.api_key` when advertiseable, else
/// interactive `ainxt.dev`.
///
/// Pinned `oidc`: **no** fallthrough to api_key — return `None` so the caller
/// fails auth. Pinned `api_key` should not reach this path (cached_token is
/// not advertised).
pub fn method_id_after_cached_token_unavailable(
    has_external_api_key: bool,
    preferred_method: Option<PreferredAuthMethod>,
) -> Option<&'static str> {
    match preferred_method {
        Some(PreferredAuthMethod::Oidc) | Some(PreferredAuthMethod::ApiKey) => None,
        None => Some(if has_external_api_key {
            AINXT_API_KEY_METHOD_ID
        } else {
            AINXT_COM_METHOD_ID
        }),
    }
}

/// Error when `preferred_method=api_key` but no key/BYOK credentials exist.
pub const PREFERRED_API_KEY_UNAVAILABLE: &str = "preferred_method=api_key but no API key is configured (set AINXT_API_KEY or model api_key/env_key in config.toml).";

/// Error when `preferred_method=oidc` but the session path cannot proceed.
pub const PREFERRED_OIDC_UNAVAILABLE: &str =
    "preferred_method=oidc but no session is available. Run `ainxt login` to authenticate.";

pub const AINXT_API_KEY_METHOD_ID: &str = "ainxt.api_key";
pub fn ainxt_api_key_auth_method() -> acp::AuthMethod {
    acp::AuthMethod::Agent(
        acp::AuthMethodAgent::new(
            acp::AuthMethodId::new(AINXT_API_KEY_METHOD_ID),
            "ainxt.api_key".to_string(),
        )
        .description(Some(format!(
            "{AINXT_API_KEY_ENV_VAR} or api_key/env_key in config.toml"
        ))),
    )
}

pub const CACHED_TOKEN_AUTH_METHOD_ID: &str = "cached_token";
pub fn cached_token_auth_method() -> acp::AuthMethod {
    acp::AuthMethod::Agent(
        acp::AuthMethodAgent::new(
            acp::AuthMethodId::new(CACHED_TOKEN_AUTH_METHOD_ID),
            "cached_token".to_string(),
        )
        .description(Some("Cached token from ~/.ainxt/auth.json".to_string())),
    )
}

pub const AINXT_COM_METHOD_ID: &str = "ainxt.dev";

/// ainxt OAuth2/OIDC auth. Method id `"ainxt.dev"` kept for ACP wire-compat.
pub fn ainxt_com_auth_method(
    label: Option<&str>,
    has_auth_provider_command: bool,
) -> acp::AuthMethod {
    let name = label.unwrap_or("ainxt");
    let meta = if has_auth_provider_command {
        let mut m = acp::Meta::new();
        m.insert("external_provider".to_owned(), serde_json::json!(true));
        Some(m)
    } else {
        None
    };
    acp::AuthMethod::Agent(
        acp::AuthMethodAgent::new(acp::AuthMethodId::new(AINXT_COM_METHOD_ID), name.to_string())
            .description(Some(format!("Sign in with {name}")))
            .meta(meta),
    )
}

pub const OIDC_METHOD_ID: &str = "oidc";
pub fn oidc_auth_method(issuer: &str, label: Option<&str>) -> acp::AuthMethod {
    let name = label
        .map(|l| l.to_string())
        .unwrap_or_else(|| format!("Single sign-on ({})", issuer));
    acp::AuthMethod::Agent(
        acp::AuthMethodAgent::new(acp::AuthMethodId::new(OIDC_METHOD_ID), name.clone())
            .description(Some(format!("Sign in with {name}"))),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::config::{Config, resolve_model_list};
    use agent_client_protocol as acp;
    use serial_test::serial;

    /// When API-key credentials are advertiseable, fall through from a dead
    /// `cached_token` to non-interactive `ainxt.api_key` (not browser OAuth).
    /// Covers the both-advertised case (`has_cached_token` true at initialize
    /// but session later missing/expired/legacy): advertise order still puts
    /// `ainxt.api_key` first, while `default_auth_method_id` prefers session;
    /// after session fails, this helper must still pick `ainxt.api_key`.
    #[test]
    fn after_cached_token_unavailable_prefers_api_key_when_advertiseable() {
        assert_eq!(
            method_id_after_cached_token_unavailable(true, None),
            Some(AINXT_API_KEY_METHOD_ID),
        );
    }

    /// No advertiseable API-key credentials → interactive `ainxt.dev`.
    #[test]
    fn after_cached_token_unavailable_falls_to_ainxt_com_without_api_key() {
        assert_eq!(
            method_id_after_cached_token_unavailable(false, None),
            Some(AINXT_COM_METHOD_ID),
        );
    }

    /// Pinned methods never fall through across the api_key ↔ oidc boundary.
    #[test]
    fn after_cached_token_unavailable_fails_closed_when_pinned() {
        assert_eq!(
            method_id_after_cached_token_unavailable(true, Some(PreferredAuthMethod::Oidc)),
            None,
        );
        assert_eq!(
            method_id_after_cached_token_unavailable(true, Some(PreferredAuthMethod::ApiKey)),
            None,
        );
    }

    /// Classifier matrix for all auth method variants.
    #[test]
    fn auth_method_kind_classifier_matrix() {
        let session_methods = [
            CACHED_TOKEN_AUTH_METHOD_ID,
            AINXT_COM_METHOD_ID,
            OIDC_METHOD_ID,
        ];
        for method_id in session_methods {
            let id = acp::AuthMethodId::new(method_id);
            let kind = AuthMethodKind::from_id(&id);
            assert!(
                kind.is_session_based(),
                "{method_id}: kind must be session-based"
            );
            assert!(
                is_session_based_method(&id),
                "{method_id}: wrapper must agree"
            );
        }
        let api_id = acp::AuthMethodId::new(AINXT_API_KEY_METHOD_ID);
        let api_kind = AuthMethodKind::from_id(&api_id);
        assert!(!api_kind.is_session_based());
        assert!(api_kind.is_api_key());
        assert!(!is_session_based_method(&api_id));
        assert!(!is_session_based_method(&acp::AuthMethodId::new(
            "unknown-method"
        )));
    }

    use ainxt_test_support::EnvGuard;

    // ── Helpers ─────────────────────────────────────────────────────────

    /// Default inputs to `build_auth_methods` representing a session-only user
    /// with no API key anywhere. Tests override only the fields they care
    /// about.
    fn default_inputs() -> AuthMethodsBuildInputs<'static> {
        AuthMethodsBuildInputs {
            has_external_api_key: false,
            has_cached_token: false,
            has_enterprise_oidc: false,
            enterprise_oidc_issuer: None,
            login_label: None,
            has_auth_provider_command: false,
            preferred_method: None,
        }
    }

    fn method_ids(built: &BuiltAuthMethods) -> Vec<&str> {
        built.methods.iter().map(|m| m.id().0.as_ref()).collect()
    }

    fn default_id(built: &BuiltAuthMethods) -> Option<&str> {
        built
            .default_auth_method_id
            .as_ref()
            .map(|id| id.0.as_ref())
    }

    fn first_kind(methods: &[acp::AuthMethod]) -> Option<AuthMethodKind> {
        methods.first().map(|m| AuthMethodKind::from_id(m.id()))
    }

    // build_auth_methods regression: pin production call-site ordering.
    // Reordering so `ainxt.api_key` is after login methods must fail the tests below.

    /// BYOK with only per-model `env_key` must list `ainxt.api_key` first.
    #[test]
    fn enterprise_byok_first_method_is_ainxt_api_key() {
        let inputs = AuthMethodsBuildInputs {
            has_external_api_key: true, // enterprise user with resolved per-model env_key
            has_cached_token: false,
            ..default_inputs()
        };
        let built = build_auth_methods(inputs);

        assert_eq!(
            first_kind(&built.methods),
            Some(AuthMethodKind::AinxtApiKey),
            "BYOK enterprise-style: auth_methods.first() MUST be ainxt.api_key \
             (deferred-to-last ordering sends users to the login screen)",
        );
        assert_eq!(
            built
                .default_auth_method_id
                .as_ref()
                .map(|id| id.0.as_ref()),
            Some(AINXT_API_KEY_METHOD_ID),
        );
        // Cross-check with the pager-side predicate: the first method must
        // not require interactive login, which is the exact condition the
        // pager's `startup_auth_metadata()` uses.
        assert!(
            !AuthMethodKind::from_id(built.methods[0].id()).needs_interactive_login(),
            "first method MUST NOT need interactive login when ainxt.api_key is available",
        );
    }

    /// BYOK + cached session token: ainxt.api_key stays first in the methods
    /// list (skips login screen), but `default_auth_method_id` is
    /// `cached_token` (keeps OIDC refresh alive).
    #[test]
    fn byok_with_cached_token_keeps_ainxt_api_key_first() {
        let inputs = AuthMethodsBuildInputs {
            has_external_api_key: true,
            has_cached_token: true,
            ..default_inputs()
        };
        let built = build_auth_methods(inputs);

        assert_eq!(
            first_kind(&built.methods),
            Some(AuthMethodKind::AinxtApiKey),
            "ainxt.api_key MUST precede cached_token in advertised order",
        );
        // Sanity: cached_token still appears, just second.
        assert!(
            built
                .methods
                .iter()
                .any(|m| AuthMethodKind::from_id(m.id()) == AuthMethodKind::CachedToken),
            "cached_token must still be advertised when present",
        );
        // cached_token wins for default_auth_method_id (keeps OIDC refresh alive).
        assert_eq!(
            built
                .default_auth_method_id
                .as_ref()
                .map(|id| id.0.as_ref()),
            Some(CACHED_TOKEN_AUTH_METHOD_ID),
        );
    }

    /// Session-only user (no API key anywhere): cached_token first, then
    /// `ainxt.dev` — `auth_methods.first()` does NOT need interactive login,
    /// so this user also skips the login screen at startup.
    #[test]
    fn session_only_user_first_method_is_cached_token() {
        let inputs = AuthMethodsBuildInputs {
            has_external_api_key: false,
            has_cached_token: true,
            ..default_inputs()
        };
        let built = build_auth_methods(inputs);

        assert_eq!(
            first_kind(&built.methods),
            Some(AuthMethodKind::CachedToken)
        );
        assert_eq!(
            built
                .default_auth_method_id
                .as_ref()
                .map(|id| id.0.as_ref()),
            Some(CACHED_TOKEN_AUTH_METHOD_ID),
        );
    }

    /// Brand-new user (no API key, no cached token): only `ainxt.dev` is
    /// advertised, and the pager will (correctly) show the login screen.
    /// `default_auth_method_id` is None so the pager falls back to the
    /// advertised login method.
    #[test]
    fn fresh_user_only_advertises_ainxt_com_and_requires_login() {
        let built = build_auth_methods(default_inputs());

        assert_eq!(first_kind(&built.methods), Some(AuthMethodKind::AinxtCom));
        assert!(built.default_auth_method_id.is_none());
        assert_eq!(built.methods.len(), 1);
    }

    /// Enterprise OIDC replaces `ainxt.dev` (mutually exclusive). ainxt.api_key,
    /// when present, still leads.
    #[test]
    fn enterprise_oidc_replaces_ainxt_com_but_ainxt_api_key_still_first() {
        let inputs = AuthMethodsBuildInputs {
            has_external_api_key: true,
            has_cached_token: false,
            has_enterprise_oidc: true,
            enterprise_oidc_issuer: Some("https://sso.example.com"),
            ..default_inputs()
        };
        let built = build_auth_methods(inputs);

        assert_eq!(first_kind(&built.methods), Some(AuthMethodKind::AinxtApiKey));
        assert!(
            built
                .methods
                .iter()
                .any(|m| AuthMethodKind::from_id(m.id()) == AuthMethodKind::Oidc),
            "oidc must be advertised when has_enterprise_oidc",
        );
        assert!(
            !built
                .methods
                .iter()
                .any(|m| AuthMethodKind::from_id(m.id()) == AuthMethodKind::AinxtCom),
            "ainxt.dev and oidc are mutually exclusive",
        );
    }

    /// `has_auth_provider_command` is plumbed through to the `ainxt.dev` method
    /// as `meta.external_provider = true`. Pinning this here so the pager's
    /// `AuthStartMode::Command` path keeps working.
    #[test]
    fn auth_provider_command_sets_external_provider_meta() {
        let inputs = AuthMethodsBuildInputs {
            has_auth_provider_command: true,
            login_label: Some("Acme Corp"),
            ..default_inputs()
        };
        let built = build_auth_methods(inputs);

        let ainxt = built
            .methods
            .iter()
            .find(|m| AuthMethodKind::from_id(m.id()) == AuthMethodKind::AinxtCom)
            .expect("ainxt.dev must be advertised");
        assert_eq!(ainxt.name(), "Acme Corp");
        let meta = ainxt.meta().expect("meta should be set");
        assert_eq!(
            meta.get("external_provider").and_then(|v| v.as_bool()),
            Some(true),
        );
    }

    // ── End-to-end: enterprise TOML -> resolved models -> build_auth_methods ─

    /// END-TO-END REGRESSION TEST: parses the literal enterprise-style
    /// `~/.ainxt/config.toml` skeleton from the bug report, walks it through
    /// the same predicate (`should_advertise_ainxt_api_key`) and the same
    /// list-builder (`build_auth_methods`) that `MvpAgent::initialize()` uses
    /// in production, and asserts that `auth_methods.first()` is `ainxt.api_key`
    /// (which causes the pager to skip the login screen).
    ///
    /// This is the test that *would have caught* that regression -- if you mentally
    /// re-introduce that bug (push ainxt.api_key LAST when has_external_api_key
    /// && !global env var), this test fails because `first_kind` is no longer
    /// `AinxtApiKey`.
    #[test]
    #[serial]
    fn enterprise_byok_config_does_not_require_login() {
        const TEST_ENV_VAR: &str = "TEST_ENTERPRISE_REGRESSION_AUTH_TOKEN";

        // Make sure no global key is masking the per-model path we're trying
        // to exercise. Held until end-of-scope so we restore on panic too.
        let _global = EnvGuard::unset(AINXT_API_KEY_ENV_VAR);

        let dm = crate::models::default_model();
        let toml: toml::Value = toml::from_str(&format!(
            r#"
            [model."{dm}"]
            model = "{dm}"
            base_url = "https://inference.example.com/v1"
            context_window = 200000
            env_key = "{TEST_ENV_VAR}"
            "#,
        ))
        .unwrap();
        let cfg = Config::new_from_toml_cfg(&toml).expect("config should parse");
        let models = resolve_model_list(&cfg, None);
        let model = models.get(dm).expect("enterprise-style model should exist");
        assert_eq!(
            model.env_key.as_ref().map(|k| k.names()),
            Some(vec![TEST_ENV_VAR])
        );

        // Without the env var present, has_own_credentials() returns false,
        // the predicate returns false, and the builder advertises only the
        // login method. Confirms the predicate isn't trivially true.
        {
            let _unset = EnvGuard::unset(TEST_ENV_VAR);
            let has_external_api_key = should_advertise_ainxt_api_key(false, models.values());
            assert!(!has_external_api_key);
            let built = build_auth_methods(AuthMethodsBuildInputs {
                has_external_api_key,
                ..default_inputs()
            });
            assert_ne!(
                first_kind(&built.methods),
                Some(AuthMethodKind::AinxtApiKey),
                "without env_key resolved, ainxt.api_key must NOT be advertised first",
            );
        }

        // With the env var present (the actual enterprise scenario), the predicate
        // returns true and the builder MUST put `ainxt.api_key` first so the
        // pager's `startup_auth_metadata()` returns `needs_login = false`.
        {
            let _set = EnvGuard::set(TEST_ENV_VAR, "enterprise-secret-token");
            let has_external_api_key = should_advertise_ainxt_api_key(false, models.values());
            assert!(has_external_api_key);
            let built = build_auth_methods(AuthMethodsBuildInputs {
                has_external_api_key,
                // Realistic enterprise user: no cached session token, default
                // ainxt.dev login (no enterprise OIDC).
                has_cached_token: false,
                ..default_inputs()
            });
            assert_eq!(
                first_kind(&built.methods),
                Some(AuthMethodKind::AinxtApiKey),
                "BYOK: ainxt.api_key must be auth_methods.first(); deferred-to-last \
                 ordering sends enterprise users to the login screen",
            );
            assert!(
                !AuthMethodKind::from_id(built.methods[0].id()).needs_interactive_login(),
                "auth_methods.first() MUST NOT need interactive login -- this \
                 is the exact predicate the pager's startup_auth_metadata() \
                 uses to decide whether to show the login screen",
            );
        }
    }

    /// `AINXT_API_KEY` alone (no per-model creds) also triggers
    /// advertising `ainxt.api_key` as the first method. Historical "external
    /// key" path; covered here so the predicate keeps treating env-var-only
    /// users the same as per-model users.
    #[test]
    #[serial]
    fn global_external_api_key_advertises_ainxt_api_key_first() {
        let _set = EnvGuard::set(AINXT_API_KEY_ENV_VAR, "ainxt-external-key");
        let cfg = Config::default();
        let models = resolve_model_list(&cfg, None);
        let has_external_api_key = should_advertise_ainxt_api_key(false, models.values());
        assert!(has_external_api_key);
        let built = build_auth_methods(AuthMethodsBuildInputs {
            has_external_api_key,
            ..default_inputs()
        });
        assert_eq!(first_kind(&built.methods), Some(AuthMethodKind::AinxtApiKey));
    }

    /// Admin kill switch (`disable_api_key_auth`): the predicate must return
    /// false even when credentials are available everywhere (global env var
    /// AND per-model env_key), so the builder never advertises `ainxt.api_key`
    /// and the pager sends the user to the deployment's login method instead.
    #[test]
    #[serial]
    fn disable_api_key_auth_suppresses_ainxt_api_key_method() {
        let _set = EnvGuard::set(AINXT_API_KEY_ENV_VAR, "ainxt-external-key");
        let cfg = Config::default();
        let models = resolve_model_list(&cfg, None);

        // Flag off: today's behavior (advertised first).
        assert!(should_advertise_ainxt_api_key(false, models.values()));

        // Flag on: never advertised, regardless of credentials.
        let has_external_api_key = should_advertise_ainxt_api_key(true, models.values());
        assert!(!has_external_api_key);
        let built = build_auth_methods(AuthMethodsBuildInputs {
            has_external_api_key,
            ..default_inputs()
        });
        assert!(
            !built
                .methods
                .iter()
                .any(|m| AuthMethodKind::from_id(m.id()) == AuthMethodKind::AinxtApiKey),
            "ainxt.api_key must not be advertised when disable_api_key_auth is set",
        );
        assert_eq!(
            first_kind(&built.methods),
            Some(AuthMethodKind::AinxtCom),
            "with api-key auth disabled and no cached token, the login method \
             must lead so the pager requires interactive login",
        );
        assert!(built.default_auth_method_id.is_none());
    }

    /// Legacy `AINXT_CODE_AINXT_API_KEY` env var is accepted as a fallback
    /// when `AINXT_API_KEY` is not set, ensuring existing deployments keep working.
    #[test]
    #[serial]
    fn legacy_env_var_fallback_advertises_ainxt_api_key() {
        let _unset_new = EnvGuard::unset(AINXT_API_KEY_ENV_VAR);
        let _set_legacy = EnvGuard::set(LEGACY_AINXT_API_KEY_ENV_VAR, "ainxt-legacy-key");
        assert!(has_ainxt_api_key_env());
        assert_eq!(read_ainxt_api_key_env().unwrap(), "ainxt-legacy-key");

        let cfg = Config::default();
        let models = resolve_model_list(&cfg, None);
        let has_external_api_key = should_advertise_ainxt_api_key(false, models.values());
        assert!(has_external_api_key);
    }

    /// When both `AINXT_API_KEY` and `AINXT_CODE_AINXT_API_KEY` are set,
    /// the new name takes precedence.
    #[test]
    #[serial]
    fn new_env_var_takes_precedence_over_legacy() {
        let _new = EnvGuard::set(AINXT_API_KEY_ENV_VAR, "new-key");
        let _legacy = EnvGuard::set(LEGACY_AINXT_API_KEY_ENV_VAR, "old-key");
        assert_eq!(read_ainxt_api_key_env().unwrap(), "new-key");
    }

    // -- ainxt login --legacy regression coverage ------------------------
    //
    // `ainxt login --legacy` produces a AinxtAuth with `auth_mode: WebLogin`,
    // `oidc_issuer: None`, and no `expires_at` (30-day hardcoded TTL).
    // When this token is present via the `AINXT_AUTH` env var (or via legacy
    // scope fallback in auth.json), `AuthManager::new` returns it from
    // `current()`, feeding `has_cached_token = true` into `build_auth_methods`.
    // This puts `cached_token` first so `startup_auth_metadata()` returns
    // `needs_login = false` -- legacy users get frictionless auth, no login
    // screen.
    //
    // This test pins the env-var path (highest priority in AuthManager) end-
    // to-end. A regression in AINXT_AUTH JSON parsing or in auth method
    // ordering would send legacy-token users to the login screen.

    /// END-TO-END REGRESSION TEST: a legacy auth token (WebLogin, no
    /// expires_at) present in the `AINXT_AUTH` env var, with no other auth
    /// available, MUST be loaded by `AuthManager` and cause `build_auth_methods`
    /// to advertise `cached_token` first. The pager therefore skips the login
    /// screen (frictionless legacy auth). This behavior works; the test
    /// prevents regressions.
    #[test]
    #[serial]
    fn ainxt_login_legacy_token_does_not_require_login() {
        use crate::auth::{AuthManager, AuthMode, AinxtAuth, AinxtComConfig};

        // Ensure clean slate for "no other auth available".
        let _g1 = EnvGuard::unset("AINXT_AUTH_PATH");
        let _g2 = EnvGuard::unset(AINXT_API_KEY_ENV_VAR);

        // Construct a legacy-style token exactly as `ainxt login --legacy`
        // produces: WebLogin mode, no OIDC fields, no refresh_token, no
        // expires_at (is_expired falls back to 30-day age check).
        let legacy_token = AinxtAuth {
            key: "legacy-relay-token".into(),
            auth_mode: AuthMode::WebLogin,
            create_time: chrono::Utc::now(),
            user_id: "legacy-user".into(),
            email: Some("legacy@example.com".into()),
            oidc_issuer: None,
            oidc_client_id: None,
            refresh_token: None,
            expires_at: None,
            ..AinxtAuth::test_default()
        };

        // Provide it via AINXT_AUTH env var (highest priority code path in
        // AuthManager::new). This is the "legacy auth token exists in the env"
        // case with no other auth.
        let legacy_json = serde_json::to_string(&legacy_token).expect("serialize legacy token");
        let _g = EnvGuard::set("AINXT_AUTH", &legacy_json);

        // AuthManager picks it up from the env var directly (no file needed).
        let dir = tempfile::tempdir().unwrap();
        let cfg = AinxtComConfig::default();
        let mgr = AuthManager::new(dir.path(), cfg);
        let current = mgr.current();
        assert!(
            current.is_some(),
            "legacy token in AINXT_AUTH env MUST be loaded directly -- if this fails, \
             users with legacy auth in env would be sent to the login screen",
        );
        assert_eq!(
            current.as_ref().unwrap().key,
            "legacy-relay-token",
            "loaded token must match the one injected via env",
        );

        // derive has_cached_token exactly as initialize() does.
        let has_cached_token = mgr.current().is_some();
        assert!(has_cached_token);

        // With only this legacy token (no ainxt api key), first method must be
        // cached_token so pager skips login screen.
        let built = build_auth_methods(AuthMethodsBuildInputs {
            has_external_api_key: false,
            has_cached_token,
            ..default_inputs()
        });

        assert_eq!(
            first_kind(&built.methods),
            Some(AuthMethodKind::CachedToken),
            "legacy token in env: cached_token MUST be auth_methods.first() \
             (pager startup_auth_metadata returns needs_login=false)",
        );
        assert!(
            !AuthMethodKind::from_id(built.methods[0].id()).needs_interactive_login(),
            "auth_methods.first() MUST NOT need interactive login when legacy token \
             is in env -- prevents login screen regression",
        );
        assert_eq!(
            built
                .default_auth_method_id
                .as_ref()
                .map(|id| id.0.as_ref()),
            Some(CACHED_TOKEN_AUTH_METHOD_ID),
        );
    }

    /// Negative case for the legacy flow: when auth.json does NOT contain a
    /// legacy-scope entry, AuthManager::current() is None,
    /// has_cached_token is false, and build_auth_methods advertises only
    /// the login method. This pins the predicate's "no" answer so the test
    /// above isn't trivially passing.
    #[test]
    #[serial]
    fn no_legacy_token_means_no_cached_token_advertised() {
        use crate::auth::{AuthManager, AinxtComConfig};

        let _g1 = EnvGuard::unset("AINXT_AUTH");
        let _g2 = EnvGuard::unset("AINXT_AUTH_PATH");

        let dir = tempfile::tempdir().unwrap();
        // No auth.json in the tempdir.
        let cfg = AinxtComConfig::default();
        let mgr = AuthManager::new(dir.path(), cfg);
        assert!(mgr.current().is_none());

        let built = build_auth_methods(AuthMethodsBuildInputs {
            has_external_api_key: false,
            has_cached_token: mgr.current().is_some(),
            ..default_inputs()
        });
        assert_eq!(
            first_kind(&built.methods),
            Some(AuthMethodKind::AinxtCom),
            "no cached token AND no api key: pager must show login (ainxt.dev first)",
        );
    }

    // ── preferred_method pin (fail-closed) ──────────────────────────────

    #[test]
    fn pin_api_key_with_key_only_advertises_api_key() {
        let built = build_auth_methods(AuthMethodsBuildInputs {
            has_external_api_key: true,
            has_cached_token: true,
            preferred_method: Some(PreferredAuthMethod::ApiKey),
            ..default_inputs()
        });
        assert_eq!(method_ids(&built), vec![AINXT_API_KEY_METHOD_ID]);
        assert_eq!(default_id(&built), Some(AINXT_API_KEY_METHOD_ID));
    }

    #[test]
    fn pin_api_key_without_key_fails_closed_even_with_session() {
        let built = build_auth_methods(AuthMethodsBuildInputs {
            has_external_api_key: false,
            has_cached_token: true,
            preferred_method: Some(PreferredAuthMethod::ApiKey),
            ..default_inputs()
        });
        assert!(built.methods.is_empty());
        assert!(built.default_auth_method_id.is_none());
    }

    #[test]
    fn pin_oidc_with_session_hides_api_key() {
        let built = build_auth_methods(AuthMethodsBuildInputs {
            has_external_api_key: true,
            has_cached_token: true,
            preferred_method: Some(PreferredAuthMethod::Oidc),
            ..default_inputs()
        });
        assert_eq!(
            method_ids(&built),
            vec![CACHED_TOKEN_AUTH_METHOD_ID, AINXT_COM_METHOD_ID]
        );
        assert_eq!(default_id(&built), Some(CACHED_TOKEN_AUTH_METHOD_ID));
    }

    #[test]
    fn pin_oidc_without_session_is_interactive_only() {
        let built = build_auth_methods(AuthMethodsBuildInputs {
            has_external_api_key: true,
            has_cached_token: false,
            preferred_method: Some(PreferredAuthMethod::Oidc),
            ..default_inputs()
        });
        assert_eq!(method_ids(&built), vec![AINXT_COM_METHOD_ID]);
        assert!(built.default_auth_method_id.is_none());
    }
}
