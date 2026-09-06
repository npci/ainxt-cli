//! ainxt gateway authentication.
//!
//! Two credential paths, both ending in a bearer token stored at
//! `~/.ainxt/credentials.json` (mode 600):
//!
//!   1. **Email / password → JWT.** `POST {gateway}/ainxt/v1/api/auth/login`
//!      returns an access token (+ optional refresh token / expiry). The token is
//!      refreshed transparently via `POST {gateway}/ainxt/v1/api/auth/refresh`,
//!      which the gateway performs using the *current* token (there is no separate
//!      refresh token) with a post-expiry grace window.
//!
//!   2. **IDE API key.** A pasted `{slug}-{uuid}` key is verified against
//!      `GET {gateway}/ainxt/v1/api/auth/me` and persisted as a permanent bearer
//!      (never refreshed).
//!
//! Security hardening:
//!   - [`validate_gateway_url`] gates every credential transmission so a token is
//!     never sent in cleartext to a network-reachable host (https always allowed;
//!     http only for loopback unless `AINXT_ALLOW_INSECURE=1`).
//!   - [`logout`] overwrites the credentials file with zeros and fsyncs before
//!     unlink so the plaintext token is not trivially recoverable.

use std::fs::{self, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};

use ainxt_config::ainxt_home;

/// Seconds of pre-expiry skew: refresh when the token is this close to expiring.
const REFRESH_SKEW_SECONDS: i64 = 60;
/// The gateway accepts a refresh of the current token for up to this long past
/// its `exp`. Past this window an expired token is no longer refreshable.
const REFRESH_GRACE_SECONDS: i64 = 3600;

/// Client identity header sent on every gateway auth call.
const CLIENT_HEADER: &str = "cli/2.0.0";

/// Stored credentials, serialized to `~/.ainxt/credentials.json`.
///
/// Serialized with the ainxt credential field names (`accessToken`,
/// `refreshToken`, `expiresAt`, …) so the on-disk format is stable.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StoredCredentials {
    #[serde(rename = "accessToken")]
    pub access_token: String,
    #[serde(rename = "refreshToken", skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    /// Epoch seconds when the access token expires (from the JWT `exp` claim).
    #[serde(rename = "expiresAt", skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
    #[serde(rename = "gatewayUrl", skip_serializing_if = "Option::is_none")]
    pub gateway_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// `"api_key"` for the IDE-key path; absent for JWT logins.
    #[serde(rename = "authMethod", skip_serializing_if = "Option::is_none")]
    pub auth_method: Option<String>,
}

fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// The ainxt config directory (`~/.ainxt`, overridable via `$AINXT_HOME`).
pub fn config_dir() -> PathBuf {
    ainxt_home()
}

/// The one file name the credential store is ever allowed to write.
const CREDENTIALS_FILE: &str = "credentials.json";

fn credentials_path() -> PathBuf {
    config_dir().join(CREDENTIALS_FILE)
}

// ── Gateway URL safety gate ─────────────────────────────────────────────────

const LOOPBACK_HOSTS: &[&str] = &["localhost", "127.0.0.1", "::1", "[::1]"];

/// Validate a gateway base URL before any credential is sent to it; returns it
/// unchanged on success.
///
/// - `https://…` is always allowed.
/// - `http://` loopback is allowed (local dev).
/// - `http://<other host>` is refused unless `AINXT_ALLOW_INSECURE=1`, in which
///   case it is allowed with a stderr warning.
/// - Any other scheme or a malformed URL is refused.
pub fn validate_gateway_url(raw: &str) -> Result<String> {
    let parsed = url::Url::parse(raw)
        .map_err(|_| anyhow!("Invalid gateway URL: '{raw}' is not a valid URL."))?;
    let scheme = parsed.scheme();
    if scheme != "http" && scheme != "https" {
        bail!("Invalid gateway URL scheme '{scheme}:'. Only http:// and https:// are allowed.");
    }
    if scheme == "http" {
        let host = parsed.host_str().unwrap_or("");
        let is_loopback = LOOPBACK_HOSTS.contains(&host);
        let allow_insecure = std::env::var("AINXT_ALLOW_INSECURE").as_deref() == Ok("1");
        if !is_loopback && !allow_insecure {
            bail!(
                "Refusing to use plaintext HTTP gateway '{raw}'.\n  \
                 Credentials and tokens would be sent in cleartext to a network-reachable host.\n  \
                 Use https://… instead, or for testing only set AINXT_ALLOW_INSECURE=1."
            );
        }
        if !is_loopback {
            eprintln!(
                "WARNING: gateway is using plaintext HTTP ({raw}); credentials are sent in cleartext. AINXT_ALLOW_INSECURE=1 acknowledged."
            );
        }
    }
    Ok(raw.to_string())
}

/// Resolve the gateway base URL from the environment / provided default,
/// trimming any trailing slashes. `AINXT_GATEWAY_URL` overrides the passed-in
/// default (which is normally the resolved proxy URL).
pub fn resolve_gateway_url(default_url: &str) -> String {
    let url =
        std::env::var("AINXT_GATEWAY_URL").unwrap_or_else(|_| default_url.to_string());
    url.trim_end_matches('/').to_string()
}

// ── Response parsing ─────────────────────────────────────────────────────────

/// A gateway auth response, accepting the common field spellings.
#[derive(Debug, Deserialize, Default)]
struct AuthResponse {
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default, rename = "accessToken")]
    access_token_camel: Option<String>,
    #[serde(default)]
    token: Option<String>,
    #[serde(default)]
    jwt: Option<String>,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default, rename = "refreshToken")]
    refresh_token_camel: Option<String>,
    #[serde(default)]
    expires_in: Option<i64>,
}

fn parse_tokens(body: &AuthResponse) -> StoredCredentials {
    let access = body
        .access_token
        .clone()
        .or_else(|| body.access_token_camel.clone())
        .or_else(|| body.token.clone())
        .or_else(|| body.jwt.clone())
        .unwrap_or_default();
    let refresh = body
        .refresh_token
        .clone()
        .or_else(|| body.refresh_token_camel.clone());
    let mut creds = StoredCredentials {
        access_token: access.clone(),
        refresh_token: refresh,
        ..Default::default()
    };
    if let Some(expires_in) = body.expires_in {
        creds.expires_at = Some(now_seconds() + expires_in);
    } else if !access.is_empty() {
        creds.expires_at = decode_jwt_exp(&access);
    }
    creds
}

/// Extract the `exp` claim (epoch seconds) from a JWT without verifying it.
pub fn decode_jwt_exp(token: &str) -> Option<i64> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() < 2 {
        return None;
    }
    let payload = base64_url_decode(parts[1])?;
    let json: serde_json::Value = serde_json::from_slice(&payload).ok()?;
    json.get("exp").and_then(|v| v.as_i64())
}

fn base64_url_decode(input: &str) -> Option<Vec<u8>> {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(input)
        .ok()
        .or_else(|| base64::engine::general_purpose::URL_SAFE.decode(input).ok())
}

/// True when the stored token is an IDE API key (`{slug}-{uuid}`) rather than a
/// JWT. API keys have hyphens but fewer than the three dot-separated JWT
/// segments; they never expire, so refresh must skip them.
pub fn is_api_key(token: &str) -> bool {
    token.split('.').count() < 3 && token.contains('-')
}

// ── Login / refresh / verify ───────────────────────────────────────────────

fn http_client() -> Result<reqwest::Client> {
    ainxt_http::apply_tls_policy(reqwest::Client::builder())
        .user_agent("ainxt-cli/2.0.0")
        .build()
        .context("failed to build HTTP client")
}

/// `POST {gateway}/ainxt/v1/api/auth/login` → store and return credentials.
pub async fn login_with_email_password(
    gateway: &str,
    email: &str,
    password: &str,
) -> Result<StoredCredentials> {
    let base = validate_gateway_url(gateway)?;
    let client = http_client()?;
    let res = client
        .post(format!("{base}/ainxt/v1/api/auth/login"))
        .header("Content-Type", "application/json")
        .header("X-AiNxt-Client", CLIENT_HEADER)
        .json(&serde_json::json!({ "email": email, "password": password }))
        .send()
        .await
        .context("Cannot reach the ainxt gateway")?;

    let status = res.status();
    if status.as_u16() == 401 || status.as_u16() == 403 {
        bail!("Login failed: invalid email or password");
    }
    if !status.is_success() {
        bail!("Login failed: gateway returned {}", status.as_u16());
    }
    let body: AuthResponse = res.json().await.unwrap_or_default();
    let mut creds = parse_tokens(&body);
    if creds.access_token.is_empty() {
        bail!("Login response did not include an access token");
    }
    creds.gateway_url = Some(base);
    creds.email = Some(email.to_string());
    store_credentials(&creds)?;
    Ok(creds)
}

/// `POST {gateway}/ainxt/v1/api/auth/refresh` → store and return refreshed
/// credentials. The gateway refreshes using the *current* token.
pub async fn refresh_jwt(gateway: &str, current_token: &str) -> Result<StoredCredentials> {
    let base = validate_gateway_url(gateway)?;
    let client = http_client()?;
    let res = client
        .post(format!("{base}/ainxt/v1/api/auth/refresh"))
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {current_token}"))
        .json(&serde_json::json!({ "refresh_token": current_token }))
        .send()
        .await
        .context("Cannot reach the ainxt gateway")?;

    if !res.status().is_success() {
        bail!("Session expired — please run `ainxt login` again");
    }
    let body: AuthResponse = res.json().await.unwrap_or_default();
    let mut creds = parse_tokens(&body);
    if creds.access_token.is_empty() {
        bail!("Refresh response did not include an access token");
    }
    creds.gateway_url = Some(base);
    store_credentials(&creds)?;
    Ok(creds)
}

/// Account info returned by `GET /ainxt/v1/api/auth/me`.
#[derive(Debug, Deserialize, Default)]
pub struct WhoAmI {
    pub id: Option<String>,
    pub email: Option<String>,
    pub name: Option<String>,
    pub role: Option<String>,
    pub department: Option<String>,
}

/// Verify an IDE API key against `GET {gateway}/ainxt/v1/api/auth/me`.
pub async fn verify_api_key(gateway: &str, key: &str) -> Result<WhoAmI> {
    let base = validate_gateway_url(gateway)?;
    let client = http_client()?;
    let res = client
        .get(format!("{base}/ainxt/v1/api/auth/me"))
        .header("Authorization", format!("Bearer {key}"))
        .header("X-AiNxt-Client", CLIENT_HEADER)
        .send()
        .await
        .with_context(|| format!("Could not reach gateway at {base}"))?;
    if !res.status().is_success() {
        bail!(
            "Key rejected by gateway ({}). Check the key and try again.",
            res.status().as_u16()
        );
    }
    Ok(res.json().await.unwrap_or_default())
}

/// Verify + persist an IDE API key. Returns the resolved account info.
pub async fn sign_in_with_api_key(gateway: &str, key: &str) -> Result<WhoAmI> {
    let base = validate_gateway_url(gateway)?;
    let who = verify_api_key(&base, key).await?;
    let creds = StoredCredentials {
        access_token: key.to_string(),
        gateway_url: Some(base),
        email: who.email.clone(),
        auth_method: Some("api_key".to_string()),
        ..Default::default()
    };
    store_credentials(&creds)?;
    Ok(who)
}

/// Return a valid access token, refreshing if it is expired or near-expiry.
/// `force` refreshes unconditionally (used after a 401/403).
pub async fn get_jwt_with_refresh(gateway: &str, force: bool) -> Result<String> {
    let creds = load_credentials().ok_or_else(|| anyhow!("Not logged in — run `ainxt login`"))?;
    // IDE API keys are permanent bearer tokens — never refresh.
    if is_api_key(&creds.access_token) {
        return Ok(creds.access_token);
    }
    if !force && !is_expiring(&creds) {
        return Ok(creds.access_token);
    }
    let current = creds
        .refresh_token
        .clone()
        .unwrap_or_else(|| creds.access_token.clone());
    let refreshed = refresh_jwt(gateway, &current).await?;
    Ok(refreshed.access_token)
}

fn is_expiring(creds: &StoredCredentials) -> bool {
    let exp = creds
        .expires_at
        .or_else(|| decode_jwt_exp(&creds.access_token));
    match exp {
        None => false, // no expiry info → assume valid
        Some(exp) => exp - now_seconds() <= REFRESH_SKEW_SECONDS,
    }
}

/// Stricter than [`is_logged_in`]: present AND (unexpired OR refreshable).
pub fn has_usable_token() -> bool {
    let Some(creds) = load_credentials() else {
        return false;
    };
    if creds.access_token.is_empty() {
        return false;
    }
    if is_api_key(&creds.access_token) {
        return true;
    }
    let exp = creds
        .expires_at
        .or_else(|| decode_jwt_exp(&creds.access_token));
    let Some(exp) = exp else {
        return true; // no expiry info → assume usable
    };
    let now = now_seconds();
    if exp - now > 0 {
        return true;
    }
    // Expired: usable only if refreshable.
    if creds
        .refresh_token
        .as_deref()
        .is_some_and(|r| r != creds.access_token)
    {
        return true;
    }
    now - exp <= REFRESH_GRACE_SECONDS
}

pub fn is_logged_in() -> bool {
    load_credentials().is_some_and(|c| !c.access_token.is_empty())
}

// ── Storage ────────────────────────────────────────────────────────────────

/// Reject any destination that is not literally `<config_dir>/credentials.json`.
///
/// The credential store has exactly one legal target, so this is an allow-list
/// rather than a filter: relative to [`config_dir`], the destination must be
/// the single component `credentials.json`. Anchoring to `config_dir()` is what
/// rules out both absolute and relative escapes — nothing outside that
/// directory can satisfy it, whatever form the path takes.
///
/// Only the part *below* `config_dir()` is inspected, never `config_dir()`
/// itself: `$AINXT_HOME` is chosen by the invoking user and may legitimately be
/// relative or contain `..`, and rejecting that would break their own config
/// directory rather than stop an attack.
///
/// The check is deliberately lexical. The file does not exist yet on first
/// login, so canonicalizing it would fail, and canonicalizing the directory
/// would break where the home directory is itself a symlink (macOS `/var` →
/// `/private/var`). Comparing against `config_dir()` rather than a hardcoded
/// `~/.ainxt` also keeps `$AINXT_HOME` overrides — including the ones the test
/// suite points at temporary directories — working unchanged.
fn validate_credentials_path(path: &Path) -> Result<()> {
    let relative = path.strip_prefix(config_dir()).map_err(|_| {
        anyhow!(
            "refusing to write credentials outside the ainxt config directory: {}",
            path.display()
        )
    })?;
    if relative.components().any(|c| c == Component::ParentDir) {
        bail!(
            "refusing to write credentials to a path containing '..': {}",
            path.display()
        );
    }
    if relative != Path::new(CREDENTIALS_FILE) {
        bail!(
            "refusing to write credentials to an unexpected file: {}",
            path.display()
        );
    }
    Ok(())
}

/// Create/truncate `path` with owner-only permissions and write `bytes`.
///
/// On Unix the mode is applied by `open(2)` itself rather than a follow-up
/// `chmod`, closing the window in which the token sat on disk under the ambient
/// umask, and `O_NOFOLLOW` makes the call fail outright when the destination is
/// a symlink instead of silently writing through it.
fn write_owner_only(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW).mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    let result = file.flush();
    // Explicitly release the file handle now that the write is flushed,
    // rather than relying solely on end-of-scope Drop.
    drop(file);
    result
}

/// Persist credentials to `~/.ainxt/credentials.json` (mode 600).
pub fn store_credentials(creds: &StoredCredentials) -> Result<()> {
    let path = credentials_path();
    validate_credentials_path(&path)?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).ok();
    }
    let json = serde_json::to_string(creds).context("serialize credentials")?;
    write_owner_only(&path, json.as_bytes()).with_context(|| format!("write {}", path.display()))?;
    set_mode_600(&path);
    Ok(())
}

/// Load credentials, or `None` when absent / corrupt.
pub fn load_credentials() -> Option<StoredCredentials> {
    use zeroize::Zeroize as _;
    let mut raw = fs::read_to_string(credentials_path()).ok()?;
    let creds: Option<StoredCredentials> = serde_json::from_str(&raw).ok();
    // The JSON text contains the plaintext bearer token; explicitly wipe the
    // intermediate buffer once it has been deserialized into `creds`, rather
    // than relying solely on end-of-scope Drop.
    raw.zeroize();
    let creds = creds?;
    if creds.access_token.is_empty() {
        return None;
    }
    Some(creds)
}

/// Remove stored credentials, rendering the plaintext token unrecoverable
/// before unlink (overwrite with zeros + fsync, then delete). Best-effort at
/// each step so logout never leaves a credential behind.
pub fn logout() {
    let path = credentials_path();
    if !path.exists() {
        return;
    }
    if let Ok(meta) = fs::metadata(&path) {
        let size = meta.len();
        if size > 0
            && let Ok(mut f) = OpenOptions::new().write(true).open(&path)
        {
            let _ = f.seek(SeekFrom::Start(0));
            let _ = f.write_all(&vec![0u8; size as usize]);
            let _ = f.sync_all();
            // Explicitly release the file handle now that the
            // best-effort zeroing is complete, rather than relying
            // solely on end-of-scope Drop.
            drop(f);
        }
    }
    let _ = fs::remove_file(&path);
}

#[cfg(unix)]
fn set_mode_600(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn set_mode_600(_path: &std::path::Path) {
    // Platforms without POSIX modes (e.g. Windows): best-effort no-op.
}

// ── Interactive login flows ─────────────────────────────────────────────────

use std::io::{BufRead, Write as _};

/// Read a line of plain (echoed) input from stdin with a prompt.
fn prompt_line(prompt: &str) -> Result<String> {
    print!("{prompt}");
    std::io::stdout().flush().ok();
    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .context("failed to read input")?;
    Ok(line.trim().to_string())
}

/// Read a secret (password / API key) without echoing it to the terminal.
/// Turns off terminal echo via termios on Unix; falls back to plain read when
/// stdin is not a TTY (piped input) or on unsupported platforms.
///
/// Returned as `Zeroizing<String>` so the plaintext secret's backing buffer
/// is wiped from memory as soon as the caller drops it, rather than lingering
/// in the heap for an indeterminate time (defense-in-depth against a
/// privileged memory-read attacker, e.g. a debugger attach or crash dump).
/// In addition to relying on `Zeroizing`'s `Drop` impl, the intermediate
/// `line` buffer is explicitly `.zeroize()`-d with `zeroize::Zeroize` once
/// its content has been copied into the returned value.
fn prompt_secret(prompt: &str) -> Result<zeroize::Zeroizing<String>> {
    use zeroize::Zeroize as _;
    print!("{prompt}");
    std::io::stdout().flush().ok();

    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        let fd = std::io::stdin().as_raw_fd();
        // Only toggle echo when stdin is an interactive terminal.
        let is_tty = unsafe { libc::isatty(fd) } == 1;
        if is_tty {
            // Save current termios, clear ECHO, restore afterward.
            let mut term: libc::termios = unsafe { std::mem::zeroed() };
            if unsafe { libc::tcgetattr(fd, &mut term) } == 0 {
                let original = term;
                term.c_lflag &= !libc::ECHO;
                unsafe { libc::tcsetattr(fd, libc::TCSANOW, &term) };
                let mut line = String::new();
                let read = std::io::stdin().lock().read_line(&mut line);
                unsafe { libc::tcsetattr(fd, libc::TCSANOW, &original) };
                println!(); // echo the newline the user's Enter didn't show
                read.context("failed to read secret")?;
                let result = zeroize::Zeroizing::new(
                    line.trim_end_matches(['\n', '\r']).to_string(),
                );
                line.zeroize();
                return Ok(result);
            }
        }
    }

    // Non-TTY / non-Unix fallback: plain read (no masking possible).
    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .context("failed to read secret")?;
    let result = zeroize::Zeroizing::new(line.trim_end_matches(['\n', '\r']).to_string());
    line.zeroize();
    Ok(result)
}

/// Interactive email/password sign-in: prompt (email optional pre-fill),
/// authenticate against the gateway, persist the JWT, print a confirmation.
pub async fn interactive_password_login(gateway: &str, email: Option<&str>) -> Result<()> {
    println!("ainxt — sign in");
    println!("Gateway: {}", validate_gateway_url(gateway)?);
    println!();
    let email = match email {
        Some(e) if !e.is_empty() => e.to_string(),
        _ => prompt_line("Email: ")?,
    };
    if email.is_empty() {
        bail!("No email entered.");
    }
    let password = prompt_secret("Password: ")?;
    if password.is_empty() {
        bail!("No password entered.");
    }
    let creds = login_with_email_password(gateway, &email, &password).await?;
    println!("\n\u{2713} Signed in as {email}");
    if creds.expires_at.is_some() {
        println!("  Session token stored at {}", credentials_path().display());
    }
    println!("Run \"ainxt\" to start.");
    Ok(())
}

/// Interactive token sign-in (the default `ainxt login`): print guidance, read
/// the pasted access token without echo, verify + persist it, print the account
/// summary. Accepts either an ainxt access token or an API key — both are
/// bearer tokens the gateway validates via `/auth/me`.
pub async fn interactive_token_login(gateway: &str) -> Result<()> {
    let base = validate_gateway_url(gateway)?;
    println!("ainxt — sign in");
    println!("Gateway: {base}");
    println!();
    println!("Paste the access token issued for your ainxt account.");
    println!(
        "You can create one in the ainxt web console under Profile \u{2192} Access Tokens."
    );
    println!();
    let raw = prompt_secret("Paste your ainxt token: ")?;
    // Keep the trimmed token in a `Zeroizing` buffer too, matching
    // `prompt_secret`'s own hardening: it is a plaintext bearer credential
    // just like `raw`, so it must not linger in the heap past its use here.
    let token = zeroize::Zeroizing::new(raw.trim().trim_matches(['\'', '"']).to_string());
    if token.is_empty() {
        bail!("No token entered.");
    }
    let who = sign_in_with_api_key(&base, &token).await?;
    println!("\n\u{2713} Token verified");
    if let Some(name) = who.name.as_deref().filter(|s| !s.is_empty()) {
        let email = who.email.as_deref().unwrap_or("");
        println!("  User:    {name} ({email})");
    } else if let Some(email) = who.email.as_deref() {
        println!("  User:    {email}");
    }
    if let Some(role) = who.role.as_deref().filter(|s| !s.is_empty()) {
        println!("  Role:    {role}");
    }
    println!("  Gateway: {base}");
    println!("\nRun \"ainxt\" to start.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_path_guard_accepts_only_the_canonical_target() {
        // The real target always passes.
        assert!(validate_credentials_path(&credentials_path()).is_ok());

        let dir = config_dir();
        // Wrong file name in the right directory.
        assert!(validate_credentials_path(&dir.join("other.json")).is_err());
        // Right file name, but escaping the config directory.
        assert!(validate_credentials_path(&dir.join("..").join(CREDENTIALS_FILE)).is_err());
        // Right file name, wrong directory.
        assert!(validate_credentials_path(&dir.join("nested").join(CREDENTIALS_FILE)).is_err());
        // A bare relative path is not anchored under the config directory.
        assert!(validate_credentials_path(Path::new(CREDENTIALS_FILE)).is_err());
    }

    #[test]
    fn detects_api_key_vs_jwt() {
        assert!(is_api_key("alice-1234abcd-5678-90ab-cdef-1234567890ab"));
        // A 3-segment JWT is not an API key.
        assert!(!is_api_key("aaa.bbb.ccc"));
    }

    #[test]
    fn gateway_url_gate_allows_https_and_loopback() {
        assert!(validate_gateway_url("https://api.example.com").is_ok());
        assert!(validate_gateway_url("http://localhost:8000").is_ok());
        assert!(validate_gateway_url("http://127.0.0.1:8000").is_ok());
    }

    #[test]
    fn gateway_url_gate_refuses_plaintext_remote() {
        assert!(validate_gateway_url("http://gateway.example.com").is_err());
        assert!(validate_gateway_url("ftp://x").is_err());
        assert!(validate_gateway_url("not a url").is_err());
    }

    #[test]
    fn resolve_gateway_trims_trailing_slash() {
        assert_eq!(
            resolve_gateway_url("https://api.example.test/v1/"),
            "https://api.example.test/v1"
        );
    }

    #[test]
    fn parse_tokens_accepts_snake_and_camel() {
        let snake: AuthResponse =
            serde_json::from_str(r#"{"access_token":"a.b.c","refresh_token":"r"}"#).unwrap();
        let c = parse_tokens(&snake);
        assert_eq!(c.access_token, "a.b.c");
        assert_eq!(c.refresh_token.as_deref(), Some("r"));

        let camel: AuthResponse =
            serde_json::from_str(r#"{"accessToken":"x","expires_in":3600}"#).unwrap();
        let c2 = parse_tokens(&camel);
        assert_eq!(c2.access_token, "x");
        assert!(c2.expires_at.unwrap() > now_seconds());
    }
}
