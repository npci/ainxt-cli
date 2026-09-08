//! The build manifest and the startup gate (INV-7: no silent degradation).
//!
//! The build manifest is compiled into the binary. Managed builds set
//! `require_policy = true` and embed an `policy_authority` public key; the OSS
//! build sets `require_policy = false` and embeds none. This single field is the
//! entire open-source switch (`AINXT-SEC-001` §12.1).

use ainxt_policy_types::policy::SecurityPolicy;

use crate::bundle::{PolicyBundle, VerifiedBundle};
use crate::error::PolicyError;

/// Compile-time security posture of this build.
#[derive(Debug, Clone)]
pub struct BuildManifest {
    /// If true, the CLI refuses to start without a valid signed bundle.
    pub require_policy: bool,
    /// The Ed25519 authority public key (32 bytes), if this build trusts one.
    pub policy_authority: Option<Vec<u8>>,
}

impl BuildManifest {
    /// The open-source posture: no required policy, no trusted authority.
    pub fn oss() -> Self {
        BuildManifest { require_policy: false, policy_authority: None }
    }

    /// A managed posture with a trusted authority key.
    pub fn managed(authority_pubkey: Vec<u8>) -> Self {
        BuildManifest { require_policy: true, policy_authority: Some(authority_pubkey) }
    }
}

/// Evaluates whether the process is permitted to start, and with what policy.
pub struct StartupGate;

impl StartupGate {
    /// Decide the effective base policy at startup.
    ///
    /// - OSS build, no bundle → permissive [`SecurityPolicy::oss_default`].
    /// - Managed build, valid bundle → the bundle's floor.
    /// - Managed build (`require_policy`), missing/invalid/rolled-back bundle →
    ///   [`PolicyError`], and the caller **must refuse to start** (INV-7).
    ///
    /// `bundle` is the raw envelope bytes read from
    /// `/etc/ainxt/policy.d/…` (or the platform equivalent), or `None` if no
    /// bundle file was found. `last_version` is the highest bundle version this
    /// host has previously accepted, for anti-rollback.
    pub fn evaluate(
        manifest: &BuildManifest,
        bundle: Option<&[u8]>,
        last_version: Option<u64>,
    ) -> Result<StartupOutcome, PolicyError> {
        match (manifest.require_policy, bundle) {
            // OSS: no bundle needed. If one is present and a key is configured,
            // honour it; otherwise run permissively.
            (false, None) => Ok(StartupOutcome::permissive()),
            (false, Some(bytes)) => match &manifest.policy_authority {
                Some(key) => {
                    let verified = verify_bytes(bytes, key, last_version)?;
                    Ok(StartupOutcome::from_bundle(verified))
                }
                None => Ok(StartupOutcome::permissive()),
            },

            // Managed with require_policy: a valid bundle is mandatory.
            (true, None) => Err(PolicyError::PolicyRequiredButMissing(
                "no policy bundle found and require_policy is set".to_string(),
            )),
            (true, Some(bytes)) => {
                let key = manifest.policy_authority.as_deref().ok_or_else(|| {
                    PolicyError::BadAuthorityKey(
                        "require_policy set but build has no authority key".to_string(),
                    )
                })?;
                let verified = verify_bytes(bytes, key, last_version)?;
                Ok(StartupOutcome::from_bundle(verified))
            }
        }
    }
}

fn verify_bytes(
    bytes: &[u8],
    key: &[u8],
    last_version: Option<u64>,
) -> Result<VerifiedBundle, PolicyError> {
    let envelope = PolicyBundle::from_slice(bytes)?;
    envelope.verify(key, last_version)
}

/// The result of a successful startup evaluation.
#[derive(Debug, Clone)]
pub struct StartupOutcome {
    /// The base policy every settings layer will further narrow.
    pub base_policy: SecurityPolicy,
    /// The accepted bundle version, if a bundle was loaded (for persisting the
    /// anti-rollback high-water mark).
    pub bundle_version: Option<u64>,
    /// The accepted gateway-overlay version, if one narrowed the base.
    ///
    /// Tracked separately from `bundle_version`: the two counters advance
    /// independently, and sharing one would let a gateway overlay bump lock the
    /// host out of a legitimate machine-bundle update, or the reverse.
    pub overlay_version: Option<u64>,
}

impl StartupOutcome {
    fn permissive() -> Self {
        StartupOutcome {
            base_policy: SecurityPolicy::oss_default(),
            bundle_version: None,
            overlay_version: None,
        }
    }

    fn from_bundle(verified: VerifiedBundle) -> Self {
        let p = verified.payload();
        StartupOutcome {
            base_policy: SecurityPolicy {
                enforcement: p.enforcement,
                capabilities: p.capabilities.clone(),
            },
            bundle_version: Some(verified.version()),
            overlay_version: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ainxt_policy_types::verdict::Enforcement;

    #[test]
    fn oss_no_bundle_starts_permissive() {
        let outcome = StartupGate::evaluate(&BuildManifest::oss(), None, None).unwrap();
        assert_eq!(outcome.base_policy.enforcement, Enforcement::Off);
        assert_eq!(outcome.bundle_version, None);
    }

    #[test]
    fn managed_missing_bundle_refuses_to_start() {
        // INV-7: require_policy with no bundle is a hard error, not a degrade.
        let key = vec![0u8; 32];
        let err = StartupGate::evaluate(&BuildManifest::managed(key), None, None).unwrap_err();
        assert!(matches!(err, PolicyError::PolicyRequiredButMissing(_)));
    }

    #[test]
    fn managed_invalid_bundle_refuses_to_start() {
        let key = vec![0u8; 32];
        let garbage = b"{not a bundle}";
        let err = StartupGate::evaluate(&BuildManifest::managed(key), Some(garbage), None)
            .unwrap_err();
        // Parse failure of the envelope surfaces as a start-blocking error.
        assert!(matches!(err, PolicyError::Parse(_)));
    }
}
