//! Integration tests for the gateway update channel.
//!
//! Covers:
//! - `fetch_gateway_manifest` (single `GET .../cli/version` request)
//! - `verify_artifact` — checksum match and mismatch
//! - `download_url` construction (explicit relative/absolute `url`, and the
//!   default `{base}/cli/download/{os}/{arch}` path)
//! - `install_gateway_for_test` end-to-end (unix only)
//! - `min_version` propagation from manifest
//! - Error paths: non-2xx, bad JSON, missing sha256, non-semver version

mod common;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use serial_test::serial;
use sha2::{Digest, Sha256};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use ainxt_update::UpdateConfig;
use ainxt_update::gateway::{GatewayManifest, download_url, fetch_gateway_manifest, verify_artifact};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_gateway_config(base_url: &str) -> UpdateConfig {
    UpdateConfig {
        proxy_base_url: "http://test.invalid/v1".to_string(),
        auth_scope: "test".to_string(),
        deployment_key: None,
        alpha_test_key: None,
        channel: "latest".to_string(),
        npm_registry: None,
        gateway_url: Some(base_url.to_string()),
    }
}

/// Base64 (not hex) SHA-256 — `verify_artifact` compares against
/// `base64::engine::general_purpose::STANDARD`-encoded digests.
fn b64_sha256(content: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content);
    B64.encode(hasher.finalize())
}

fn make_manifest_json(version: &str, sha256: &str) -> serde_json::Value {
    serde_json::json!({
        "version": version,
        "sha256": sha256,
    })
}

fn make_manifest_json_with_min(version: &str, sha256: &str, min_version: &str) -> serde_json::Value {
    let mut m = make_manifest_json(version, sha256);
    m["min_version"] = serde_json::Value::String(min_version.to_string());
    m
}

// ---------------------------------------------------------------------------
// fetch_gateway_manifest — happy path
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn fetch_manifest_success() {
    let _ = common::test_home();
    common::reset_home();

    let server = MockServer::start().await;
    let version = "1.2.3";
    let sha256 = b64_sha256(b"fake binary contents");

    Mock::given(method("GET"))
        .and(path("/ainxt/v1/api/cli/version"))
        .respond_with(ResponseTemplate::new(200).set_body_json(make_manifest_json(version, &sha256)))
        .mount(&server)
        .await;

    let config = make_gateway_config(&server.uri());
    let manifest = fetch_gateway_manifest(&config).await.unwrap();

    assert_eq!(manifest.version, version);
    assert_eq!(manifest.sha256, sha256);
    assert!(manifest.min_version.is_none());
}

// ---------------------------------------------------------------------------
// fetch_gateway_manifest — min_version propagation
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn fetch_manifest_carries_min_version() {
    let _ = common::test_home();
    common::reset_home();

    let server = MockServer::start().await;
    let version = "1.2.3";
    let min_version = "1.1.0";
    let sha256 = b64_sha256(b"fake binary contents 2");

    Mock::given(method("GET"))
        .and(path("/ainxt/v1/api/cli/version"))
        .respond_with(ResponseTemplate::new(200).set_body_json(make_manifest_json_with_min(
            version, &sha256, min_version,
        )))
        .mount(&server)
        .await;

    let config = make_gateway_config(&server.uri());
    let manifest = fetch_gateway_manifest(&config).await.unwrap();

    assert_eq!(manifest.min_version.as_deref(), Some(min_version));
}

// ---------------------------------------------------------------------------
// fetch_gateway_manifest — error paths
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn fetch_manifest_non_success_status_is_error() {
    let _ = common::test_home();
    common::reset_home();

    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/ainxt/v1/api/cli/version"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let config = make_gateway_config(&server.uri());
    let result = fetch_gateway_manifest(&config).await;
    assert!(result.is_err(), "expected error on non-2xx version response");
}

#[tokio::test]
#[serial]
async fn fetch_manifest_bad_json_is_error() {
    let _ = common::test_home();
    common::reset_home();

    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/ainxt/v1/api/cli/version"))
        .respond_with(ResponseTemplate::new(200).set_body_string("not json at all"))
        .mount(&server)
        .await;

    let config = make_gateway_config(&server.uri());
    let result = fetch_gateway_manifest(&config).await;
    assert!(result.is_err(), "expected error on bad JSON manifest");
}

#[tokio::test]
#[serial]
async fn fetch_manifest_non_semver_version_is_error() {
    let _ = common::test_home();
    common::reset_home();

    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/ainxt/v1/api/cli/version"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(make_manifest_json("not-a-version", &b64_sha256(b"x"))),
        )
        .mount(&server)
        .await;

    let config = make_gateway_config(&server.uri());
    let result = fetch_gateway_manifest(&config).await;
    assert!(result.is_err(), "expected error on non-semver manifest version");
}

#[tokio::test]
#[serial]
async fn fetch_manifest_missing_sha256_is_error() {
    let _ = common::test_home();
    common::reset_home();

    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/ainxt/v1/api/cli/version"))
        .respond_with(ResponseTemplate::new(200).set_body_json(make_manifest_json("1.2.3", "")))
        .mount(&server)
        .await;

    let config = make_gateway_config(&server.uri());
    let result = fetch_gateway_manifest(&config).await;
    assert!(result.is_err(), "expected error when manifest sha256 is empty");
}

// ---------------------------------------------------------------------------
// verify_artifact — checksum match and mismatch
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn verify_artifact_correct_checksum_passes() {
    let _ = common::test_home();
    common::reset_home();

    let content = b"#!/bin/sh\nexit 0\n";
    let sha256 = b64_sha256(content);

    let manifest = GatewayManifest {
        version: "1.0.0".to_string(),
        min_version: None,
        sha256,
        signature: String::new(),
        url: None,
    };

    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(tmp.path(), content).unwrap();

    verify_artifact(tmp.path(), &manifest, "macos", "aarch64")
        .await
        .expect("verify should pass with correct checksum");
}

#[tokio::test]
#[serial]
async fn verify_artifact_wrong_checksum_fails() {
    let _ = common::test_home();
    common::reset_home();

    let content = b"#!/bin/sh\nexit 0\n";

    let manifest = GatewayManifest {
        version: "1.0.0".to_string(),
        min_version: None,
        sha256: b64_sha256(b"totally different content"),
        signature: String::new(),
        url: None,
    };

    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(tmp.path(), content).unwrap();

    let result = verify_artifact(tmp.path(), &manifest, "macos", "aarch64").await;
    assert!(result.is_err(), "verify should fail with wrong checksum");
    let msg = format!("{:#}", result.unwrap_err());
    assert!(
        msg.contains("SHA-256 mismatch"),
        "error should mention SHA-256 mismatch: {msg}"
    );
}

// ---------------------------------------------------------------------------
// download_url construction
// ---------------------------------------------------------------------------

#[test]
fn download_url_default_path_when_no_manifest_url() {
    let manifest = GatewayManifest {
        version: "1.0.0".to_string(),
        min_version: None,
        sha256: b64_sha256(b"x"),
        signature: String::new(),
        url: None,
    };
    let config = make_gateway_config("https://gateway.example.com");
    let url = download_url(&config, &manifest, "macos", "aarch64").unwrap();
    assert_eq!(
        url,
        "https://gateway.example.com/ainxt/v1/api/cli/download/macos/aarch64"
    );
}

#[test]
fn download_url_absolute_manifest_url_used_as_is() {
    let manifest = GatewayManifest {
        version: "1.0.0".to_string(),
        min_version: None,
        sha256: b64_sha256(b"x"),
        signature: String::new(),
        url: Some("https://cdn.example.com/ainxt-1.0.0-macos-aarch64".to_string()),
    };
    let config = make_gateway_config("https://gateway.example.com");
    let url = download_url(&config, &manifest, "macos", "aarch64").unwrap();
    assert_eq!(url, "https://cdn.example.com/ainxt-1.0.0-macos-aarch64");
}

#[test]
fn download_url_relative_manifest_url_joined_to_base() {
    let manifest = GatewayManifest {
        version: "1.0.0".to_string(),
        min_version: None,
        sha256: b64_sha256(b"x"),
        signature: String::new(),
        url: Some("/artifacts/ainxt-1.0.0-linux-x86_64".to_string()),
    };
    let config = make_gateway_config("https://gw.example.com");
    let url = download_url(&config, &manifest, "linux", "x86_64").unwrap();
    assert_eq!(
        url,
        "https://gw.example.com/artifacts/ainxt-1.0.0-linux-x86_64"
    );
}

// ---------------------------------------------------------------------------
// install_gateway_for_test — end-to-end (unix only, requires exec)
// ---------------------------------------------------------------------------

#[cfg(unix)]
#[tokio::test]
#[serial]
async fn install_gateway_downloads_verifies_and_activates() {
    use std::os::unix::fs::PermissionsExt;

    let _ = common::test_home();
    common::reset_home();

    if !common::can_exec_shell_scripts() {
        eprintln!("Skipping: cannot exec shell scripts in this environment");
        return;
    }

    let server = MockServer::start().await;
    let version = "9.9.9";

    // Real, runnable "binary" for the smoke test (`--version` must exit 0).
    let binary_content = b"#!/bin/sh\nexit 0\n";
    let sha256 = b64_sha256(binary_content);

    Mock::given(method("GET"))
        .and(path("/ainxt/v1/api/cli/version"))
        .respond_with(ResponseTemplate::new(200).set_body_json(make_manifest_json_with_min(
            version, &sha256, "1.0.0",
        )))
        .mount(&server)
        .await;

    let (os, arch) = (
        if cfg!(target_os = "macos") { "macos" } else { "linux" },
        if cfg!(target_arch = "aarch64") { "aarch64" } else { "x86_64" },
    );
    Mock::given(method("GET"))
        .and(path(format!("/ainxt/v1/api/cli/download/{os}/{arch}")))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(binary_content.to_vec())
                .insert_header("content-type", "application/octet-stream"),
        )
        .mount(&server)
        .await;

    // Set up the managed bin dir so activate_verified_download can create symlinks.
    let home = common::test_home();
    let bin_dir = home.join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();

    // Place a stub "current" binary so the symlink swap has something to
    // replace. The managed installer always leaves `ainxt` as a symlink to
    // a versioned binary (never a plain file), so simulate that here: the
    // rollback-state capture unconditionally read_link()s this path on
    // Unix, which fails with EINVAL on a real regular file.
    let prior_versioned_bin = bin_dir.join("ainxt-0.0.0-prior");
    std::fs::write(&prior_versioned_bin, b"#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(
        &prior_versioned_bin,
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    let current_bin = bin_dir.join("ainxt");
    std::os::unix::fs::symlink(&prior_versioned_bin, &current_bin).unwrap();

    unsafe {
        std::env::set_var("AINXT_GATEWAY_URL", server.uri());
        std::env::set_var("AINXT_INSTALLER", "gateway");
    }

    let config = make_gateway_config(&server.uri());
    let result = ainxt_update::auto_update::install_gateway_for_test(None, &config).await;

    // Clean up env.
    unsafe {
        std::env::remove_var("AINXT_GATEWAY_URL");
        std::env::remove_var("AINXT_INSTALLER");
    }

    result.expect("install_gateway_for_test should succeed");

    // The versioned binary should be on disk, and the managed `ainxt` link
    // should now point at it.
    let downloads = home.join("downloads");
    let versioned = downloads.join(format!("ainxt-{version}-{os}-{arch}"));
    assert!(versioned.exists(), "versioned binary should be in downloads/");
    assert!(current_bin.exists(), "managed ainxt link should still exist");
}

#[cfg(unix)]
#[tokio::test]
#[serial]
async fn install_gateway_checksum_mismatch_aborts() {
    let _ = common::test_home();
    common::reset_home();

    let server = MockServer::start().await;
    let version = "9.9.8";

    // Manifest with a checksum that won't match the downloaded content.
    Mock::given(method("GET"))
        .and(path("/ainxt/v1/api/cli/version"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(make_manifest_json(version, &b64_sha256(b"not the real content"))),
        )
        .mount(&server)
        .await;

    let (os, arch) = (
        if cfg!(target_os = "macos") { "macos" } else { "linux" },
        if cfg!(target_arch = "aarch64") { "aarch64" } else { "x86_64" },
    );
    Mock::given(method("GET"))
        .and(path(format!("/ainxt/v1/api/cli/download/{os}/{arch}")))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(b"#!/bin/sh\nexit 0\n".to_vec())
                .insert_header("content-type", "application/octet-stream"),
        )
        .mount(&server)
        .await;

    let config = make_gateway_config(&server.uri());
    let result = ainxt_update::auto_update::install_gateway_for_test(None, &config).await;

    assert!(result.is_err(), "install should fail on checksum mismatch");
    let msg = format!("{:#}", result.unwrap_err());
    assert!(
        msg.contains("SHA-256 mismatch") || msg.contains("integrity"),
        "error should mention integrity failure: {msg}"
    );

    // The temp download file must have been cleaned up.
    let home = common::test_home();
    let downloads = home.join("downloads");
    if downloads.exists() {
        let tmps: Vec<_> = std::fs::read_dir(&downloads)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp"))
            .collect();
        assert!(tmps.is_empty(), "temp file should be cleaned up after checksum failure");
    }
}
