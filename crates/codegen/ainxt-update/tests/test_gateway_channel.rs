//! Integration tests for the gateway update channel.
//!
//! Covers:
//! - Two-step `fetch_gateway_manifest` (channel pointer → manifest JSON)
//! - `verify_artifact` — checksum match and mismatch
//! - `download_url` construction
//! - `install_gateway_with_result` end-to-end (unix only)
//! - `min_version` propagation from manifest
//! - Error paths: non-2xx, bad JSON, version mismatch, missing platform

mod common;

use std::collections::HashMap;

use serial_test::serial;
use wiremock::matchers::{method, path, path_regex, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use ainxt_update::gateway::{
    GatewayVersionManifest, PlatformEntry, fetch_gateway_manifest, platform_key, verify_artifact,
};
use ainxt_update::UpdateConfig;

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

fn make_manifest_json(version: &str, platform: &str, checksum: &str) -> serde_json::Value {
    serde_json::json!({
        "version": version,
        "platforms": {
            platform: {
                "checksum": checksum,
                "filename": format!("ainxt-{version}-{platform}")
            }
        }
    })
}

fn make_manifest_json_with_min(
    version: &str,
    platform: &str,
    checksum: &str,
    min_version: &str,
) -> serde_json::Value {
    let mut m = make_manifest_json(version, platform, checksum);
    m["min_version"] = serde_json::Value::String(min_version.to_string());
    m
}

// ---------------------------------------------------------------------------
// fetch_gateway_manifest — happy path
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn fetch_manifest_two_step_success() {
    let _ = common::test_home();
    common::reset_home();

    let server = MockServer::start().await;
    let platform = common::host_platform();
    let version = "1.2.3";
    let checksum = "a".repeat(64); // 64 hex chars = valid sha256 placeholder

    // Step 1: channel pointer
    Mock::given(method("GET"))
        .and(path("/ainxt/v1/api/updates/latest"))
        .respond_with(ResponseTemplate::new(200).set_body_string(version))
        .mount(&server)
        .await;

    // Step 2: manifest
    Mock::given(method("GET"))
        .and(path(format!("/ainxt/v1/api/updates/{version}/manifest.json")))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(make_manifest_json(version, &platform, &checksum)),
        )
        .mount(&server)
        .await;

    let config = make_gateway_config(&server.uri());
    let manifest = fetch_gateway_manifest(&config).await.unwrap();

    assert_eq!(manifest.version, version);
    assert!(manifest.platforms.contains_key(&platform));
    assert_eq!(manifest.platforms[&platform].checksum, checksum);
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
    let platform = common::host_platform();
    let version = "1.2.3";
    let min_version = "1.1.0";
    let checksum = "b".repeat(64);

    Mock::given(method("GET"))
        .and(path("/ainxt/v1/api/updates/latest"))
        .respond_with(ResponseTemplate::new(200).set_body_string(version))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path(format!("/ainxt/v1/api/updates/{version}/manifest.json")))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(make_manifest_json_with_min(
                version, &platform, &checksum, min_version,
            )),
        )
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
async fn fetch_manifest_channel_404_is_error() {
    let _ = common::test_home();
    common::reset_home();

    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/ainxt/v1/api/updates/latest"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;

    let config = make_gateway_config(&server.uri());
    let result = fetch_gateway_manifest(&config).await;
    assert!(result.is_err(), "expected error on 404 channel response");
}

#[tokio::test]
#[serial]
async fn fetch_manifest_non_semver_version_is_error() {
    let _ = common::test_home();
    common::reset_home();

    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/ainxt/v1/api/updates/latest"))
        .respond_with(ResponseTemplate::new(200).set_body_string("not-a-version"))
        .mount(&server)
        .await;

    let config = make_gateway_config(&server.uri());
    let result = fetch_gateway_manifest(&config).await;
    assert!(result.is_err(), "expected error on non-semver channel pointer");
}

#[tokio::test]
#[serial]
async fn fetch_manifest_version_mismatch_is_error() {
    let _ = common::test_home();
    common::reset_home();

    let server = MockServer::start().await;
    let platform = common::host_platform();

    // Channel says 1.2.3 but manifest says 1.2.4 — race / misconfiguration
    Mock::given(method("GET"))
        .and(path("/ainxt/v1/api/updates/latest"))
        .respond_with(ResponseTemplate::new(200).set_body_string("1.2.3"))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/ainxt/v1/api/updates/1.2.3/manifest.json"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(make_manifest_json("1.2.4", &platform, &"c".repeat(64))),
        )
        .mount(&server)
        .await;

    let config = make_gateway_config(&server.uri());
    let result = fetch_gateway_manifest(&config).await;
    assert!(result.is_err(), "expected error on version mismatch");
    let msg = format!("{:#}", result.unwrap_err());
    assert!(
        msg.contains("does not match"),
        "error should mention mismatch: {msg}"
    );
}

#[tokio::test]
#[serial]
async fn fetch_manifest_bad_json_is_error() {
    let _ = common::test_home();
    common::reset_home();

    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/ainxt/v1/api/updates/latest"))
        .respond_with(ResponseTemplate::new(200).set_body_string("1.2.3"))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/ainxt/v1/api/updates/1.2.3/manifest.json"))
        .respond_with(ResponseTemplate::new(200).set_body_string("not json at all"))
        .mount(&server)
        .await;

    let config = make_gateway_config(&server.uri());
    let result = fetch_gateway_manifest(&config).await;
    assert!(result.is_err(), "expected error on bad JSON manifest");
}

#[tokio::test]
#[serial]
async fn fetch_manifest_manifest_500_is_error() {
    let _ = common::test_home();
    common::reset_home();

    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/ainxt/v1/api/updates/latest"))
        .respond_with(ResponseTemplate::new(200).set_body_string("1.2.3"))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/ainxt/v1/api/updates/1.2.3/manifest.json"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let config = make_gateway_config(&server.uri());
    let result = fetch_gateway_manifest(&config).await;
    assert!(result.is_err(), "expected error on 500 manifest response");
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
    let checksum = {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(content);
        format!("{:x}", h.finalize())
    };

    let platform = common::host_platform();
    let (os, arch) = platform.split_once('-').unwrap();

    let mut platforms = HashMap::new();
    platforms.insert(
        platform.clone(),
        PlatformEntry {
            checksum: checksum.clone(),
            filename: None,
        },
    );
    let manifest = GatewayVersionManifest {
        version: "1.0.0".to_string(),
        platforms,
        min_version: None,
    };

    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(tmp.path(), content).unwrap();

    verify_artifact(tmp.path(), &manifest, os, arch)
        .await
        .expect("verify should pass with correct checksum");
}

#[tokio::test]
#[serial]
async fn verify_artifact_wrong_checksum_fails() {
    let _ = common::test_home();
    common::reset_home();

    let content = b"#!/bin/sh\nexit 0\n";
    let platform = common::host_platform();
    let (os, arch) = platform.split_once('-').unwrap();

    let mut platforms = HashMap::new();
    platforms.insert(
        platform.clone(),
        PlatformEntry {
            checksum: "0".repeat(64), // wrong checksum
            filename: None,
        },
    );
    let manifest = GatewayVersionManifest {
        version: "1.0.0".to_string(),
        platforms,
        min_version: None,
    };

    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(tmp.path(), content).unwrap();

    let result = verify_artifact(tmp.path(), &manifest, os, arch).await;
    assert!(result.is_err(), "verify should fail with wrong checksum");
    let msg = format!("{:#}", result.unwrap_err());
    assert!(
        msg.contains("SHA-256 mismatch"),
        "error should mention SHA-256 mismatch: {msg}"
    );
}

#[tokio::test]
#[serial]
async fn verify_artifact_missing_platform_fails() {
    let _ = common::test_home();
    common::reset_home();

    let manifest = GatewayVersionManifest {
        version: "1.0.0".to_string(),
        platforms: HashMap::new(), // empty — no platform entry
        min_version: None,
    };

    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(tmp.path(), b"data").unwrap();

    let result = verify_artifact(tmp.path(), &manifest, "macos", "aarch64").await;
    assert!(result.is_err(), "verify should fail when platform is absent");
}

// ---------------------------------------------------------------------------
// platform_key helper
// ---------------------------------------------------------------------------

#[test]
fn platform_key_format() {
    assert_eq!(platform_key("macos", "aarch64"), "macos-aarch64");
    assert_eq!(platform_key("linux", "x86_64"), "linux-x86_64");
    assert_eq!(platform_key("windows", "x86_64"), "windows-x86_64");
}

// ---------------------------------------------------------------------------
// download_url construction
// ---------------------------------------------------------------------------

#[test]
fn download_url_uses_manifest_filename() {
    let mut platforms = HashMap::new();
    platforms.insert(
        "macos-aarch64".to_string(),
        PlatformEntry {
            checksum: "a".repeat(64),
            filename: Some("ainxt-1.0.0-macos-aarch64".to_string()),
        },
    );
    let manifest = GatewayVersionManifest {
        version: "1.0.0".to_string(),
        platforms,
        min_version: None,
    };
    let config = make_gateway_config("https://gateway.example.com");
    let url = ainxt_update::gateway::download_url(&config, &manifest, "macos", "aarch64").unwrap();
    assert_eq!(
        url,
        "https://gateway.example.com/ainxt/v1/api/updates/1.0.0/macos-aarch64/ainxt-1.0.0-macos-aarch64"
    );
}

#[test]
fn download_url_default_filename_when_absent() {
    let mut platforms = HashMap::new();
    platforms.insert(
        "linux-x86_64".to_string(),
        PlatformEntry {
            checksum: "b".repeat(64),
            filename: None, // no explicit filename
        },
    );
    let manifest = GatewayVersionManifest {
        version: "2.0.0".to_string(),
        platforms,
        min_version: None,
    };
    let config = make_gateway_config("https://gw.example.com");
    let url = ainxt_update::gateway::download_url(&config, &manifest, "linux", "x86_64").unwrap();
    assert_eq!(
        url,
        "https://gw.example.com/ainxt/v1/api/updates/2.0.0/linux-x86_64/ainxt-2.0.0-linux-x86_64"
    );
}

#[test]
fn download_url_missing_platform_is_error() {
    let manifest = GatewayVersionManifest {
        version: "1.0.0".to_string(),
        platforms: HashMap::new(),
        min_version: None,
    };
    let config = make_gateway_config("https://gw.example.com");
    let result = ainxt_update::gateway::download_url(&config, &manifest, "macos", "aarch64");
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// install_gateway_with_result — end-to-end (unix only, requires exec)
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
    let platform = common::host_platform();
    let (os, arch) = platform.split_once('-').unwrap();
    let version = "9.9.9";

    // Build a minimal executable binary for the smoke test
    let binary_content = b"#!/bin/sh\necho 'ainxt 9.9.9'\nexit 0\n";
    let checksum = {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(binary_content);
        format!("{:x}", h.finalize())
    };
    let filename = format!("ainxt-{version}-{platform}");

    // Channel pointer
    Mock::given(method("GET"))
        .and(path("/ainxt/v1/api/updates/latest"))
        .respond_with(ResponseTemplate::new(200).set_body_string(version))
        .mount(&server)
        .await;

    // Manifest with min_version
    Mock::given(method("GET"))
        .and(path(format!("/ainxt/v1/api/updates/{version}/manifest.json")))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(make_manifest_json_with_min(
                version, &platform, &checksum, "1.0.0",
            )),
        )
        .mount(&server)
        .await;

    // Binary download
    Mock::given(method("GET"))
        .and(path(format!(
            "/ainxt/v1/api/updates/{version}/{platform}/{filename}"
        )))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(binary_content.to_vec())
                .insert_header("content-type", "application/octet-stream"),
        )
        .mount(&server)
        .await;

    // Set up the managed bin dir so activate_verified_download can create symlinks
    let home = common::test_home();
    let bin_dir = home.join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();

    // Place a stub "current" binary so the symlink swap has something to replace
    let current_bin = bin_dir.join("ainxt");
    std::fs::write(&current_bin, b"#!/bin/sh\necho 'ainxt 0.0.1'\nexit 0\n").unwrap();
    std::fs::set_permissions(&current_bin, std::fs::Permissions::from_mode(0o755)).unwrap();

    unsafe {
        std::env::set_var("AINXT_GATEWAY_URL", server.uri());
        std::env::set_var("AINXT_INSTALLER", "gateway");
    }

    let config = make_gateway_config(&server.uri());
    let result = ainxt_update::auto_update::install_gateway_with_result(None, &config).await;

    // Clean up env
    unsafe {
        std::env::remove_var("AINXT_GATEWAY_URL");
        std::env::remove_var("AINXT_INSTALLER");
    }

    let result = result.expect("install_gateway_with_result should succeed");
    assert_eq!(result.installed_version, version);
    assert_eq!(result.min_version.as_deref(), Some("1.0.0"));

    // The versioned binary should be on disk
    let versioned = home.join("downloads").join(&filename);
    assert!(versioned.exists(), "versioned binary should be in downloads/");
}

#[cfg(unix)]
#[tokio::test]
#[serial]
async fn install_gateway_checksum_mismatch_aborts() {
    let _ = common::test_home();
    common::reset_home();

    let server = MockServer::start().await;
    let platform = common::host_platform();
    let version = "9.9.8";
    let filename = format!("ainxt-{version}-{platform}");

    // Channel pointer
    Mock::given(method("GET"))
        .and(path("/ainxt/v1/api/updates/latest"))
        .respond_with(ResponseTemplate::new(200).set_body_string(version))
        .mount(&server)
        .await;

    // Manifest with WRONG checksum
    Mock::given(method("GET"))
        .and(path(format!("/ainxt/v1/api/updates/{version}/manifest.json")))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(make_manifest_json(version, &platform, &"0".repeat(64))),
        )
        .mount(&server)
        .await;

    // Binary download (content won't match the all-zeros checksum)
    Mock::given(method("GET"))
        .and(path(format!(
            "/ainxt/v1/api/updates/{version}/{platform}/{filename}"
        )))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(b"#!/bin/sh\nexit 0\n".to_vec())
                .insert_header("content-type", "application/octet-stream"),
        )
        .mount(&server)
        .await;

    let config = make_gateway_config(&server.uri());
    let result = ainxt_update::auto_update::install_gateway_with_result(None, &config).await;

    assert!(result.is_err(), "install should fail on checksum mismatch");
    let msg = format!("{:#}", result.unwrap_err());
    assert!(
        msg.contains("SHA-256 mismatch") || msg.contains("integrity"),
        "error should mention integrity failure: {msg}"
    );

    // The temp file must have been cleaned up
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
