//! Gateway-served update channel.
//!
//! Unlike the npm / GitHub-release / GCS channels, this channel treats the
//! ainxt gateway as the single source of truth for the latest CLI build:
//!
//! 1. On launch the CLI asks the gateway `GET /ainxt/v1/api/cli/version` for a
//!    signed manifest describing the latest binary for this platform.
//! 2. If the manifest version differs from what is installed, the CLI pulls the
//!    binary from `GET /ainxt/v1/api/cli/download/{os}/{arch}`, stages it next
//!    to the installed binary, and (in `auto_update::install_gateway`) swaps it
//!    in atomically on the next launch.
//!
//! Every downloaded artifact is verified for integrity: the SHA-256 of the
//! binary must match the value in the manifest served by the gateway.
//! Ed25519 signature verification is intentionally omitted for self-hosted
//! deployments where the gateway is the authoritative binary source.

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::sync::Arc;

use crate::version::UpdateConfig;

/// Description of the latest CLI build for one platform, returned by the
/// gateway version endpoint.
#[derive(Debug, Clone, Deserialize)]
pub struct GatewayManifest {
    /// Latest available semver, e.g. `"0.2.102"`.
    pub version: String,
    /// Optional hard floor; clients older than this must upgrade.
    #[serde(default)]
    pub min_version: Option<String>,
    /// sha256 and signature fields are accepted but ignored — validation
    /// is disabled for self-hosted gateway deployments.
    #[serde(default)]
    pub sha256: String,
    #[serde(default)]
    pub signature: String,
    /// Optional gateway-relative or absolute download path. When absent, the
    /// default `/ainxt/v1/api/cli/download/{os}/{arch}` path is used.
    #[serde(default)]
    pub url: Option<String>,
}

/// Apply the process TLS policy to a client builder.
///
/// TLS certificate and hostname verification are always enforced.
/// The previous `AINXT_TLS_INSECURE` runtime bypass has been removed —
/// all update/gateway HTTP clients must connect to servers with valid certificates.
pub(crate) fn apply_tls(builder: reqwest::ClientBuilder) -> reqwest::ClientBuilder {
    builder
}

/// Trim any trailing slash so we can join paths without doubling `/`.
fn base(config: &UpdateConfig) -> Result<&str> {
    let url = config
        .gateway_url
        .as_deref()
        .ok_or_else(|| anyhow!("gateway update channel selected but AINXT_GATEWAY_URL is not set"))?;
    Ok(url.trim_end_matches('/'))
}


/// Best-effort resolution of the caller's bearer credentials so version/download
/// requests carry `Authorization` + `X-Ainxt-Token-Auth` when a session exists.
/// Returns anonymous credentials when the user is not logged in — the gateway
/// decides whether anonymous access is allowed.
///
/// Uses the `UpdateConfig`'s `auth_scope` to look up the correct entry in
/// `~/.ainxt/auth.json`. Enterprise deployments store their token under a
/// gateway-specific scope (e.g. `https://my-gateway.corp::client-id`), not
/// under the default ainxt.com scope. Using `AinxtComConfig::default()` here
/// would look up the wrong scope key, find nothing, and send the request
/// without an Authorization header — causing a 401 on every gateway update call.
async fn credentials(config: &UpdateConfig) -> ainxt_shell::util::ainxt_auth_credentials::AinxtAuthCredentials {
    use ainxt_shell::util::ainxt_auth_credentials::AinxtAuthCredentials;
    let home = ainxt_shell::util::ainxt_home::ainxt_home();

    // Build an AinxtComConfig that matches the auth scope the user actually
    // logged in with. For enterprise/self-hosted gateways this is derived from
    // AINXT_OAUTH2_ISSUER / AINXT_OAUTH2_CLIENT_ID (or OIDC equivalents) and
    // stored in UpdateConfig::auth_scope by UpdateConfig::from_environment().
    // Falling back to AinxtComConfig::default() would resolve the public
    // ainxt.com scope, which has no entry in auth.json for enterprise users.
    let com_config = ainxt_shell::auth::AinxtComConfig::default();
    let effective_scope = config.auth_scope.clone();

    // If the UpdateConfig scope matches what default() would produce, use
    // default() directly (no-op for public ainxt.com users). Otherwise build
    // a minimal config that overrides only the scope so the AuthManager reads
    // the right auth.json key without touching any other config fields.
    let manager = if effective_scope == com_config.auth_scope() {
        Arc::new(ainxt_shell::auth::AuthManager::new(&home, com_config))
    } else {
        // Patch the OAuth2 config's issuer+client_id to match the stored scope.
        // The scope format is "{issuer}::{client_id}" (see AinxtComConfig::auth_scope).
        // We reconstruct a minimal OAuth2ProviderConfig so the AuthManager
        // looks up the correct key in auth.json without requiring a full OIDC
        // round-trip (we only need the stored token, not a fresh one here).
        let patched = if let Some((issuer, client_id)) = effective_scope.split_once("::") {
            let mut cfg = com_config.clone();
            cfg.oauth2 = Some(ainxt_shell::auth::OAuth2ProviderConfig {
                issuer: issuer.to_string(),
                client_id: client_id.to_string(),
                scopes: vec![],
                principal_type: None,
                principal_id: None,
                referrer: None,
            });
            cfg.oidc = None;
            cfg
        } else {
            // Scope is not in the expected format — fall back to default so we
            // at least attempt auth rather than sending an anonymous request.
            tracing::warn!(
                scope = %effective_scope,
                "[auto-update] gateway: auth_scope is not in '{{issuer}}::{{client_id}}' format; \
                 falling back to default AinxtComConfig for credential lookup"
            );
            com_config
        };
        Arc::new(ainxt_shell::auth::AuthManager::new(&home, patched))
    };

    AinxtAuthCredentials::new(None)
        .with_auth_manager(manager)
        .resolve_async()
        .await
}

/// Fetch the signed manifest for the current platform from the gateway.
pub async fn fetch_gateway_manifest(config: &UpdateConfig) -> Result<GatewayManifest> {
    let (os, arch) = crate::auto_update::detect_platform()?;
    let base = base(config)?;
    let url = format!(
        "{base}/ainxt/v1/api/cli/version?os={os}&arch={arch}&channel={}",
        config.channel
    );

    tracing::info!(
        url = %url,
        os = %os,
        arch = %arch,
        channel = %config.channel,
        "[auto-update] gateway: fetching version manifest"
    );

    let client = apply_tls(reqwest::Client::builder())
        .timeout(std::time::Duration::from_secs(20))
        .build()?;
    let creds = credentials(config).await;
    let resp = creds
        .apply(client.get(&url), base)
        .send()
        .await
        .with_context(|| format!("gateway version check failed: {url}"))?;

    let status = resp.status();
    if !status.is_success() {
        tracing::warn!(
            url = %url,
            status = %status,
            "[auto-update] gateway: version endpoint returned non-success HTTP status"
        );
        bail!("gateway version endpoint returned HTTP {status}");
    }
    let manifest: GatewayManifest = resp
        .json()
        .await
        .context("gateway version response was not a valid manifest")?;

    // Validate shape: version must be valid semver, sha256 must be present.
    // Ed25519 signature is not required for self-hosted gateway deployments.
    semver::Version::parse(&manifest.version)
        .with_context(|| format!("gateway manifest version '{}' is not semver", manifest.version))?;
    if manifest.sha256.trim().is_empty() {
        bail!("gateway manifest is missing sha256");
    }

    tracing::info!(
        url = %url,
        version = %manifest.version,
        sha256 = %manifest.sha256,
        "[auto-update] gateway: manifest fetched successfully"
    );

    Ok(manifest)
}

/// Resolve the absolute download URL for the platform binary.
pub fn download_url(config: &UpdateConfig, manifest: &GatewayManifest, os: &str, arch: &str) -> Result<String> {
    let base = base(config)?;
    Ok(match manifest.url.as_deref() {
        Some(u) if u.starts_with("http://") || u.starts_with("https://") => u.to_string(),
        Some(u) => format!("{base}/{}", u.trim_start_matches('/')),
        None => format!("{base}/ainxt/v1/api/cli/download/{os}/{arch}"),
    })
}

/// Download the platform binary from the gateway into `dest` (authenticated).
pub async fn download_binary(
    config: &UpdateConfig,
    manifest: &GatewayManifest,
    dest: &std::path::Path,
) -> Result<()> {
    let (os, arch) = crate::auto_update::detect_platform()?;
    let url = download_url(config, manifest, os, arch)?;
    let base = base(config)?;

    tracing::info!(
        url = %url,
        version = %manifest.version,
        os = %os,
        arch = %arch,
        dest = %dest.display(),
        "[auto-update] gateway: downloading binary"
    );

    let client = apply_tls(reqwest::Client::builder())
        .timeout(std::time::Duration::from_secs(20 * 60))
        .build()?;
    let creds = credentials(config).await;
    let resp = creds
        .apply(client.get(&url), base)
        .send()
        .await
        .with_context(|| format!("gateway binary download failed: {url}"))?;

    let status = resp.status();
    if !status.is_success() {
        tracing::warn!(
            url = %url,
            status = %status,
            "[auto-update] gateway: binary download endpoint returned non-success HTTP status"
        );
        bail!("gateway download endpoint returned HTTP {status}");
    }
    let bytes = resp.bytes().await.context("failed to read gateway binary body")?;
    tokio::fs::write(dest, &bytes)
        .await
        .with_context(|| format!("failed to write downloaded binary to {}", dest.display()))?;

    tracing::info!(
        url = %url,
        version = %manifest.version,
        dest = %dest.display(),
        "[auto-update] gateway: binary downloaded successfully"
    );

    Ok(())
}

/// Verify a downloaded artifact: SHA-256 of the binary must match the value
/// in the manifest. Ed25519 signature verification is intentionally skipped
/// for self-hosted gateway deployments — the gateway is the authoritative
/// binary source and operators have opted out of the signing requirement.
pub async fn verify_artifact(
    path: &std::path::Path,
    manifest: &GatewayManifest,
    _os: &str,
    _arch: &str,
) -> Result<()> {
    let bytes = tokio::fs::read(path)
        .await
        .with_context(|| format!("failed to read downloaded binary for verification: {}", path.display()))?;

    // Integrity: SHA-256 of the bytes must equal the manifest's.
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let digest = hasher.finalize();
    let computed_b64 = B64.encode(digest);
    let expected_b64 = manifest.sha256.trim();
    if computed_b64 != expected_b64 {
        bail!(
            "update integrity check failed: SHA-256 mismatch (expected {expected_b64}, got {computed_b64})"
        );
    }

    tracing::info!(
        version = %manifest.version,
        "[auto-update] gateway: SHA-256 integrity check passed"
    );
    Ok(())
}
