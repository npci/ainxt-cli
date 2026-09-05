//! Startup bootstrap: resolve the build manifest + signed bundle, evaluate the
//! [`StartupGate`] (INV-7), and publish the engine into [`crate::global`].
//!
//! `main.rs` calls [`initialize`] once, early, and refuses to start on `Err`.
//! All the I/O and policy logic lives here (not in the fork's entrypoint) so it
//! stays testable and the entrypoint change is a single call.
//!
//! ## The open-source switch (compile-time, non-bypassable)
//!
//! [`resolve_manifest`] reads `AINXT_POLICY_AUTHORITY_HEX` via [`option_env!`] —
//! i.e. at **compile time**. A governed build sets it (→ `require_policy = true`
//! with an embedded Ed25519 authority key); the OSS build leaves it unset
//! (→ permissive, no bundle required). Because it is baked into the binary, a
//! runtime environment variable cannot flip enforcement on or off.

use std::path::{Path, PathBuf};

use crate::bundle::decode_hex;
use crate::engine::PolicyEngine;
use crate::error::PolicyError;
use crate::manifest::{BuildManifest, StartupGate, StartupOutcome};

/// Resolve the compile-time build manifest.
///
/// Returns a managed manifest (with the embedded authority key) when the build
/// was stamped with `AINXT_POLICY_AUTHORITY_HEX`, otherwise the permissive OSS
/// manifest.
pub fn resolve_manifest() -> Result<BuildManifest, PolicyError> {
    match option_env!("AINXT_POLICY_AUTHORITY_HEX") {
        Some(hex) if !hex.trim().is_empty() => {
            let key = decode_hex(hex).map_err(|_| {
                PolicyError::BadAuthorityKey(
                    "AINXT_POLICY_AUTHORITY_HEX embedded at build time is not valid hex"
                        .to_string(),
                )
            })?;
            Ok(BuildManifest::managed(key))
        }
        _ => Ok(BuildManifest::oss()),
    }
}

/// Platform search paths for the signed policy bundle, in priority order.
pub fn bundle_search_paths() -> Vec<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        vec![
            PathBuf::from("/etc/ainxt/policy.d/policy.json"),
            PathBuf::from("/etc/ainxt/policy.json"),
        ]
    }
    #[cfg(target_os = "macos")]
    {
        vec![PathBuf::from("/Library/Application Support/AiNxt/policy.json")]
    }
    #[cfg(target_os = "windows")]
    {
        let base = std::env::var("PROGRAMDATA").unwrap_or_else(|_| "C:\\ProgramData".to_string());
        vec![PathBuf::from(base).join("AiNxt").join("policy.json")]
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        Vec::new()
    }
}

/// Load the first readable bundle from the platform search paths.
fn load_bundle_bytes(paths: &[PathBuf]) -> Option<(PathBuf, Vec<u8>)> {
    for p in paths {
        if let Ok(bytes) = std::fs::read(p) {
            return Some((p.clone(), bytes));
        }
    }
    None
}

/// A bundle baked into the binary at build time, base64-encoded.
///
/// This exists for Windows, which has no managed-settings tier at all — no
/// `/etc`, no MDM path, no registry — and therefore no root-owned location to
/// hold a trusted bundle. Since Windows is the majority developer platform for
/// this deployment, "there is nowhere trustworthy to put the policy" would mean
/// the majority of the fleet runs unprotected. Embedding it moves control to
/// the build, which the deploying organisation already owns.
///
/// Harmless on the other platforms, where it simply is not set.
pub fn embedded_bundle_bytes() -> Option<Vec<u8>> {
    let encoded = option_env!("AINXT_POLICY_BUNDLE_B64")?.trim();
    if encoded.is_empty() {
        return None;
    }
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .ok()
}

/// Choose between an on-disk bundle and the embedded one.
///
/// Prefers the **highest verified version**, which is not the same as "prefer
/// the file". Anti-rollback protects a host that has already accepted a
/// version, but on first run there is no high-water mark, so a validly-signed
/// *older* bundle dropped into a user-writable directory would otherwise beat
/// the one compiled into the binary. On Windows that directory is
/// `%PROGRAMDATA%`, which is exactly the case this guards.
///
/// If nothing verifies, the first candidate is returned unchanged so the
/// startup gate reports a real signature or parse failure rather than the
/// misleading "no bundle found".
fn choose_bundle(candidates: Vec<Vec<u8>>, key: Option<&[u8]>) -> Option<Vec<u8>> {
    if candidates.len() < 2 {
        return candidates.into_iter().next();
    }
    let Some(key) = key else {
        return candidates.into_iter().next();
    };

    let best = candidates
        .iter()
        .enumerate()
        .filter_map(|(index, bytes)| {
            let version = crate::bundle::PolicyBundle::from_slice(bytes)
                .ok()?
                .verify(key, None)
                .ok()?
                .version();
            Some((version, index))
        })
        .max_by_key(|(version, _)| *version)
        .map(|(_, index)| index);

    match best {
        Some(index) => candidates.into_iter().nth(index),
        None => candidates.into_iter().next(),
    }
}

/// Gather every bundle this host offers, in discovery order.
fn collect_bundles(paths: &[PathBuf], embedded: Option<Vec<u8>>) -> Vec<Vec<u8>> {
    let mut candidates = Vec::new();
    if let Some((_, bytes)) = load_bundle_bytes(paths) {
        candidates.push(bytes);
    }
    if let Some(bytes) = embedded {
        candidates.push(bytes);
    }
    candidates
}

fn last_version_path(state_dir: &Path) -> PathBuf {
    state_dir.join("policy").join("bundle.version")
}

/// Read the highest bundle version this host has accepted (anti-rollback).
fn read_last_version(state_dir: &Path) -> Option<u64> {
    std::fs::read_to_string(last_version_path(state_dir)).ok()?.trim().parse().ok()
}

fn write_last_version(state_dir: &Path, version: u64) -> std::io::Result<()> {
    let path = last_version_path(state_dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, version.to_string())
}

/// Resolve the startup outcome **without** touching global state. Does all the
/// reads and gate evaluation; installs nothing. Kept separate from [`initialize`]
/// so tests can assert the outcome without mutating the process-global engine.
pub fn resolve_outcome(state_dir: &Path) -> Result<StartupOutcome, PolicyError> {
    resolve_outcome_full(
        &resolve_manifest()?,
        &bundle_search_paths(),
        embedded_bundle_bytes(),
        state_dir,
    )
}

/// Testable core: evaluate the gate for an explicit manifest and bundle paths.
///
/// Deliberately excludes the compile-time embedded bundle. A test that says
/// "this manifest with these paths yields this outcome" must not have its
/// answer changed by how the test binary happened to be stamped, or the suite
/// would pass or fail depending on the build environment.
pub fn resolve_outcome_with(
    manifest: &BuildManifest,
    bundle_paths: &[PathBuf],
    state_dir: &Path,
) -> Result<StartupOutcome, PolicyError> {
    resolve_outcome_full(manifest, bundle_paths, None, state_dir)
}

fn resolve_outcome_full(
    manifest: &BuildManifest,
    bundle_paths: &[PathBuf],
    embedded: Option<Vec<u8>>,
    state_dir: &Path,
) -> Result<StartupOutcome, PolicyError> {
    let bundle = choose_bundle(
        collect_bundles(bundle_paths, embedded),
        manifest.policy_authority.as_deref(),
    );
    let last = read_last_version(state_dir);
    let mut outcome = StartupGate::evaluate(manifest, bundle.as_deref(), last)?;
    apply_overlay(manifest, state_dir, &mut outcome);
    Ok(outcome)
}

// ── Gateway overlay ─────────────────────────────────────────────────────────
//
// The machine bundle establishes a floor. The gateway may narrow it further at
// runtime by dropping a signed overlay into `$AINXT_HOME/policy/overlay.json`.
//
// `$AINXT_HOME` is user-writable, so an overlay cannot be trusted the way a
// root-owned bundle is. Three things make that safe:
//
//  1. It must carry a valid signature from the *same* embedded authority key.
//  2. Its version must exceed the last overlay this host accepted.
//  3. The merged result must be narrower than or equal to the base.
//
// (3) is the one that actually matters. Because the merge is a meet, the worst
// a hostile overlay can achieve — even if (1) and (2) were somehow defeated —
// is a policy stricter than intended, i.e. a self-inflicted denial of service.
// It can never widen capability. That property is asserted rather than assumed.

fn overlay_path(state_dir: &Path) -> PathBuf {
    state_dir.join("policy").join("overlay.json")
}

fn overlay_version_path(state_dir: &Path) -> PathBuf {
    state_dir.join("policy").join("overlay.version")
}

/// Narrow the resolved policy by a signed gateway overlay, if a usable one is
/// present. Never fatal: a bad overlay is ignored and the base still applies.
///
/// Asymmetric with the machine bundle on purpose. A rolled-back *bundle* is a
/// hard error because it is the floor; a rolled-back *overlay* is merely
/// ignored, because falling back to the base can only be stricter or equal and
/// so is always a safe outcome.
fn apply_overlay(manifest: &BuildManifest, state_dir: &Path, outcome: &mut StartupOutcome) {
    let Some(key) = manifest.policy_authority.as_deref() else {
        return;
    };
    let Ok(bytes) = std::fs::read(overlay_path(state_dir)) else {
        return;
    };

    let last = std::fs::read_to_string(overlay_version_path(state_dir))
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok());

    let verified = match crate::bundle::PolicyBundle::from_slice(&bytes)
        .and_then(|envelope| envelope.verify(key, last))
    {
        Ok(verified) => verified,
        Err(err) => {
            tracing::warn!(error = %err, "ignoring policy overlay; base policy still applies");
            return;
        }
    };

    let payload = verified.payload();
    let merged = ainxt_policy_types::merge::narrow_capabilities(
        &outcome.base_policy.capabilities,
        &payload.capabilities,
    );

    // Hard assertion, not a comment. If the meet ever produced something wider
    // than the base, the merge algebra itself would be broken and the overlay
    // would be a privilege-escalation path straight through the startup gate.
    if !ainxt_policy_types::merge::is_narrower_or_equal(&merged, &outcome.base_policy.capabilities)
    {
        // `payload` is attacker-influenced (parsed from a signed-but-untrusted
        // overlay file read off disk); render its version field through a
        // display wrapper that strips newline/carriage-return bytes before it
        // reaches the log, rather than logging the raw field value directly.
        struct LogSafe<'a>(&'a str);
        impl std::fmt::Display for LogSafe<'_> {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                for ch in self.0.chars() {
                    if ch != '\n' && ch != '\r' {
                        write!(f, "{ch}")?;
                    }
                }
                Ok(())
            }
        }
        let overlay_version_str = payload.version.to_string();
        tracing::error!(
            overlay_version = %LogSafe(&overlay_version_str),
            "policy overlay would widen capability; refusing it and reporting tampering"
        );
        return;
    }

    outcome.base_policy.capabilities = merged;
    outcome.base_policy.enforcement = outcome.base_policy.enforcement.narrow(payload.enforcement);
    outcome.overlay_version = Some(payload.version);

    if let Some(parent) = overlay_version_path(state_dir).parent()
        && std::fs::create_dir_all(parent).is_ok()
    {
        let _ = std::fs::write(overlay_version_path(state_dir), payload.version.to_string());
    }
}

/// Resolve, publish the engine into [`crate::global`], and persist the accepted
/// bundle version. Returns the outcome for logging. On `Err`, the caller **must
/// refuse to start** (INV-7) — nothing has been installed.
///
/// Note: this installs the *base* policy (the signed bundle floor, or the OSS
/// default). Narrowing by the user/project/local settings chain is layered on
/// separately (P1-T4) and only ever tightens this floor.
pub fn initialize(state_dir: &Path) -> Result<StartupOutcome, PolicyError> {
    let outcome = resolve_outcome(state_dir)?;
    crate::global::install(PolicyEngine::new(outcome.base_policy.clone()));
    if let Some(v) = outcome.bundle_version
        && let Err(e) = write_last_version(state_dir, v)
    {
        tracing::warn!(error = %e, "failed to persist accepted policy bundle version");
    }
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ainxt_policy_types::verdict::Enforcement;

    #[test]
    fn oss_no_bundle_resolves_permissive() {
        let dir = tempfile::tempdir().unwrap();
        let outcome =
            resolve_outcome_with(&BuildManifest::oss(), &[], dir.path()).unwrap();
        assert_eq!(outcome.base_policy.enforcement, Enforcement::Off);
        assert_eq!(outcome.bundle_version, None);
    }

    #[test]
    fn managed_missing_bundle_refuses() {
        let dir = tempfile::tempdir().unwrap();
        let err = resolve_outcome_with(&BuildManifest::managed(vec![0u8; 32]), &[], dir.path())
            .unwrap_err();
        assert!(matches!(err, PolicyError::PolicyRequiredButMissing(_)));
    }

    #[test]
    fn last_version_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_last_version(dir.path()), None);
        write_last_version(dir.path(), 7).unwrap();
        assert_eq!(read_last_version(dir.path()), Some(7));
    }

    #[test]
    fn compile_time_manifest_is_oss_in_this_build() {
        // This workspace is not built with AINXT_POLICY_AUTHORITY_HEX, so the
        // resolved manifest must be the OSS (non-required) one.
        assert!(!resolve_manifest().unwrap().require_policy);
    }

    fn keypair() -> ring::signature::Ed25519KeyPair {
        let rng = ring::rand::SystemRandom::new();
        let pkcs8 =
            ring::signature::Ed25519KeyPair::generate_pkcs8(&rng).expect("generate key");
        ring::signature::Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).expect("parse key")
    }

    fn signed(kp: &ring::signature::Ed25519KeyPair, version: u64) -> Vec<u8> {
        let payload = format!(
            r#"{{"version":{version},"enforcement":"block","capabilities":{{}},"issued_at":null}}"#
        );
        let sig = kp.sign(payload.as_bytes());
        let hex: String = sig.as_ref().iter().map(|b| format!("{b:02x}")).collect();
        format!(
            r#"{{"payload":{},"signature_hex":"{hex}"}}"#,
            serde_json::to_string(&payload).expect("encode payload string")
        )
        .into_bytes()
    }

    fn public_key(kp: &ring::signature::Ed25519KeyPair) -> Vec<u8> {
        use ring::signature::KeyPair;
        kp.public_key().as_ref().to_vec()
    }

    /// A validly-signed but older bundle must not beat a newer one just because
    /// it was discovered first. On a host with no accepted version yet, this is
    /// the only thing standing between `%PROGRAMDATA%` and a policy downgrade.
    #[test]
    fn the_highest_verified_version_wins_regardless_of_discovery_order() {
        let kp = keypair();
        let key = public_key(&kp);
        let older = signed(&kp, 3);
        let newer = signed(&kp, 9);

        for candidates in [
            vec![older.clone(), newer.clone()],
            vec![newer.clone(), older.clone()],
        ] {
            let chosen =
                choose_bundle(candidates, Some(&key)).expect("a bundle must be chosen");
            let version = crate::bundle::PolicyBundle::from_slice(&chosen)
                .expect("parse")
                .verify(&key, None)
                .expect("verify")
                .version();
            assert_eq!(version, 9, "an older bundle won the selection");
        }
    }

    /// When nothing verifies, the gate must still see a bundle so it can report
    /// the real signature failure instead of the misleading "none found".
    #[test]
    fn an_unverifiable_candidate_is_still_handed_to_the_gate() {
        let candidates = vec![b"{garbage".to_vec(), b"{also garbage".to_vec()];
        let chosen = choose_bundle(candidates, Some(&[0u8; 32]));
        assert_eq!(chosen.as_deref(), Some(&b"{garbage"[..]));
    }

    /// The Windows story end to end: a managed build with no file on disk still
    /// starts, because the embedded bundle satisfies the gate.
    #[test]
    fn an_embedded_bundle_satisfies_a_managed_build_with_no_file_on_disk() {
        let kp = keypair();
        let key = public_key(&kp);
        let manifest = BuildManifest::managed(key.clone());
        let dir = tempfile::tempdir().expect("tempdir");

        // No file candidates — exactly the Windows situation.
        assert!(matches!(
            resolve_outcome_with(&manifest, &[], dir.path()),
            Err(PolicyError::PolicyRequiredButMissing(_))
        ));

        // With the embedded bundle present, the same build starts and enforces.
        let bundle = choose_bundle(vec![signed(&kp, 1)], Some(&key)).expect("chosen");
        let outcome = StartupGate::evaluate(&manifest, Some(&bundle), None).expect("starts");
        assert_eq!(outcome.base_policy.enforcement, Enforcement::Block);
        assert_eq!(outcome.bundle_version, Some(1));
    }

    #[test]
    fn no_embedded_bundle_in_this_build() {
        // This workspace is not stamped, so the embedded slot must be empty —
        // the same property `compile_time_manifest_is_oss_in_this_build` asserts
        // for the authority key.
        assert!(embedded_bundle_bytes().is_none());
    }

    /// Write a signed overlay into the state dir the way the fetcher would.
    fn write_overlay(
        state_dir: &Path,
        kp: &ring::signature::Ed25519KeyPair,
        version: u64,
        capabilities_json: &str,
    ) {
        let payload = format!(
            r#"{{"version":{version},"enforcement":"block","capabilities":{capabilities_json},"issued_at":null}}"#
        );
        let sig = kp.sign(payload.as_bytes());
        let hex: String = sig.as_ref().iter().map(|b| format!("{b:02x}")).collect();
        let envelope = format!(
            r#"{{"payload":{},"signature_hex":"{hex}"}}"#,
            serde_json::to_string(&payload).expect("encode")
        );
        let path = overlay_path(state_dir);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(path, envelope).expect("write overlay");
    }

    /// An overlay may tighten the base. This is the intended use: the gateway
    /// pushes a stricter posture to a subject without reissuing the machine
    /// bundle.
    #[test]
    fn a_signed_overlay_narrows_the_base() {
        let kp = keypair();
        let key = public_key(&kp);
        let dir = tempfile::tempdir().expect("tempdir");
        let manifest = BuildManifest::managed(key.clone());

        write_overlay(dir.path(), &kp, 1, r#"{"exec_allow":{"only":["git"]}}"#);

        let mut outcome = StartupGate::evaluate(
            &manifest,
            Some(&signed(&kp, 1)),
            None,
        )
        .expect("base starts");
        apply_overlay(&manifest, dir.path(), &mut outcome);

        assert_eq!(outcome.overlay_version, Some(1));
        match &outcome.base_policy.capabilities.exec_allow {
            ainxt_policy_types::Allowlist::Only(entries) => {
                assert!(entries.contains("git"));
                assert_eq!(entries.len(), 1, "overlay did not narrow exec_allow");
            }
            other => panic!("expected a narrowed allowlist, got {other:?}"),
        }
    }

    /// The property the whole design rests on: `$AINXT_HOME` is user-writable,
    /// so an overlay must never be able to grant something the base withheld.
    #[test]
    fn an_overlay_cannot_widen_the_base() {
        let kp = keypair();
        let key = public_key(&kp);
        let dir = tempfile::tempdir().expect("tempdir");
        let manifest = BuildManifest::managed(key.clone());

        // Base permits only `git`; the overlay asks for the world.
        let mut outcome = StartupOutcome {
            base_policy: ainxt_policy_types::SecurityPolicy {
                enforcement: Enforcement::Block,
                capabilities: ainxt_policy_types::SecurityCapabilities {
                    exec_allow: ainxt_policy_types::Allowlist::only(["git"]),
                    ..Default::default()
                },
            },
            bundle_version: Some(1),
            overlay_version: None,
        };
        write_overlay(dir.path(), &kp, 1, r#"{"exec_allow":"any"}"#);
        apply_overlay(&manifest, dir.path(), &mut outcome);

        match &outcome.base_policy.capabilities.exec_allow {
            ainxt_policy_types::Allowlist::Only(entries) => {
                assert_eq!(entries.len(), 1, "an overlay widened the base");
            }
            other => panic!("exec_allow was widened to {other:?}"),
        }
    }

    #[test]
    fn an_unsigned_or_wrongly_signed_overlay_is_ignored() {
        let kp = keypair();
        let other = keypair();
        let dir = tempfile::tempdir().expect("tempdir");
        let manifest = BuildManifest::managed(public_key(&kp));

        // Signed by a key this build does not trust.
        write_overlay(dir.path(), &other, 1, r#"{"exec_allow":{"only":["git"]}}"#);

        let mut outcome = StartupOutcome {
            base_policy: ainxt_policy_types::SecurityPolicy::oss_default(),
            bundle_version: None,
            overlay_version: None,
        };
        apply_overlay(&manifest, dir.path(), &mut outcome);
        assert_eq!(outcome.overlay_version, None, "an untrusted overlay applied");
    }

    /// Rolled-back overlays are ignored rather than fatal — falling back to the
    /// base can only be stricter or equal, so refusing to start would be a
    /// self-inflicted outage with no security benefit.
    #[test]
    fn a_rolled_back_overlay_is_ignored_not_fatal() {
        let kp = keypair();
        let dir = tempfile::tempdir().expect("tempdir");
        let manifest = BuildManifest::managed(public_key(&kp));

        std::fs::create_dir_all(dir.path().join("policy")).expect("mkdir");
        std::fs::write(overlay_version_path(dir.path()), "9").expect("write version");
        write_overlay(dir.path(), &kp, 3, r#"{"exec_allow":{"only":["git"]}}"#);

        let mut outcome = StartupOutcome {
            base_policy: ainxt_policy_types::SecurityPolicy::oss_default(),
            bundle_version: None,
            overlay_version: None,
        };
        apply_overlay(&manifest, dir.path(), &mut outcome);
        assert_eq!(outcome.overlay_version, None, "a rolled-back overlay applied");
    }

    #[test]
    fn bundle_search_paths_present_on_supported_platforms() {
        // On the platforms we support, there must be at least one search path.
        #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
        assert!(!bundle_search_paths().is_empty());
    }
}
