//! Signed policy bundles (P1-T5).
//!
//! A bundle is a JSON envelope:
//!
//! ```json
//! {
//!   "payload": "<exact JSON text that was signed>",
//!   "signature_hex": "<128 hex chars — Ed25519 over payload UTF-8 bytes>"
//! }
//! ```
//!
//! The signature covers the *exact bytes* of the `payload` string, so we verify
//! first and only then parse — no canonicalisation round-trip is trusted. The
//! parsed [`BundlePayload`] carries a monotone `version` used for anti-rollback.

use ainxt_policy_types::capability::SecurityCapabilities;
use ainxt_policy_types::verdict::Enforcement;
use serde::{Deserialize, Serialize};

use crate::error::PolicyError;

/// The on-disk envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyBundle {
    /// Exact JSON text of the [`BundlePayload`] that was signed.
    pub payload: String,
    /// Ed25519 signature over `payload.as_bytes()`, hex-encoded.
    pub signature_hex: String,
}

/// The signed content of a bundle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundlePayload {
    /// Monotone counter. A load is rejected if this is not greater than the
    /// last-accepted version (anti-rollback, TM-23).
    pub version: u64,
    /// The enforcement floor this bundle establishes.
    pub enforcement: Enforcement,
    /// The capability floor this bundle establishes.
    #[serde(default)]
    pub capabilities: SecurityCapabilities,
    /// Free-form issuer metadata (issuer, date); not security-relevant.
    #[serde(default)]
    pub issued_at: Option<String>,
}

/// A bundle whose signature has been verified against the authority key. The
/// only way to obtain one is [`PolicyBundle::verify`]; the `payload` field is
/// private so an unverified payload cannot masquerade as verified.
#[derive(Debug, Clone)]
pub struct VerifiedBundle {
    payload: BundlePayload,
}

impl VerifiedBundle {
    pub fn payload(&self) -> &BundlePayload {
        &self.payload
    }

    pub fn version(&self) -> u64 {
        self.payload.version
    }
}

impl PolicyBundle {
    /// Parse an envelope from bytes.
    pub fn from_slice(bytes: &[u8]) -> Result<Self, PolicyError> {
        serde_json::from_slice(bytes).map_err(|e| PolicyError::Parse(e.to_string()))
    }

    /// Verify the signature against a 32-byte Ed25519 public key, then parse the
    /// payload. Optionally enforce anti-rollback against `last_version`.
    ///
    /// Returns [`PolicyError::SignatureInvalid`] on any signature failure and
    /// [`PolicyError::RollbackRejected`] if `version <= last_version`.
    pub fn verify(
        &self,
        authority_pubkey: &[u8],
        last_version: Option<u64>,
    ) -> Result<VerifiedBundle, PolicyError> {
        let sig = decode_hex(&self.signature_hex)
            .map_err(|_| PolicyError::SignatureInvalid)?;

        let key = ring::signature::UnparsedPublicKey::new(
            &ring::signature::ED25519,
            authority_pubkey,
        );
        key.verify(self.payload.as_bytes(), &sig)
            .map_err(|_| PolicyError::SignatureInvalid)?;

        // Signature good — now (and only now) trust the bytes.
        let payload: BundlePayload = serde_json::from_str(&self.payload)
            .map_err(|e| PolicyError::Parse(e.to_string()))?;

        if let Some(last) = last_version
            && payload.version <= last
        {
            return Err(PolicyError::RollbackRejected { found: payload.version, last });
        }

        Ok(VerifiedBundle { payload })
    }
}

/// Decode a hex string to bytes. Small local implementation to avoid adding a
/// workspace dependency for a single use site.
pub(crate) fn decode_hex(s: &str) -> Result<Vec<u8>, ()> {
    let s = s.trim();
    if !s.len().is_multiple_of(2) {
        return Err(());
    }
    let mut out = Vec::with_capacity(s.len() / 2);
    let bytes = s.as_bytes();
    let nibble = |c: u8| -> Result<u8, ()> {
        match c {
            b'0'..=b'9' => Ok(c - b'0'),
            b'a'..=b'f' => Ok(c - b'a' + 10),
            b'A'..=b'F' => Ok(c - b'A' + 10),
            _ => Err(()),
        }
    };
    for pair in bytes.chunks_exact(2) {
        out.push((nibble(pair[0])? << 4) | nibble(pair[1])?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::rand::SystemRandom;
    use ring::signature::{Ed25519KeyPair, KeyPair};

    /// Produce a signed bundle for a given payload, returning the envelope and
    /// the public key that verifies it.
    fn sign_bundle(payload: &BundlePayload) -> (PolicyBundle, Vec<u8>) {
        let rng = SystemRandom::new();
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng).unwrap();
        let keypair = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap();
        let payload_text = serde_json::to_string(payload).unwrap();
        let sig = keypair.sign(payload_text.as_bytes());
        let bundle = PolicyBundle {
            payload: payload_text,
            signature_hex: encode_hex(sig.as_ref()),
        };
        (bundle, keypair.public_key().as_ref().to_vec())
    }

    fn encode_hex(bytes: &[u8]) -> String {
        let mut s = String::with_capacity(bytes.len() * 2);
        for b in bytes {
            s.push_str(&format!("{b:02x}"));
        }
        s
    }

    fn sample_payload(version: u64) -> BundlePayload {
        BundlePayload {
            version,
            enforcement: Enforcement::Block,
            capabilities: SecurityCapabilities::default(),
            issued_at: Some("2026-08-02".to_string()),
        }
    }

    #[test]
    fn valid_signature_verifies() {
        let (bundle, pubkey) = sign_bundle(&sample_payload(1));
        let verified = bundle.verify(&pubkey, None).unwrap();
        assert_eq!(verified.version(), 1);
        assert_eq!(verified.payload().enforcement, Enforcement::Block);
    }

    #[test]
    fn tampered_payload_fails() {
        let (mut bundle, pubkey) = sign_bundle(&sample_payload(1));
        // Flip enforcement to Off in the payload text without re-signing.
        bundle.payload = bundle.payload.replace("block", "off");
        assert!(matches!(bundle.verify(&pubkey, None), Err(PolicyError::SignatureInvalid)));
    }

    #[test]
    fn wrong_key_fails() {
        let (bundle, _) = sign_bundle(&sample_payload(1));
        let (_, other_key) = sign_bundle(&sample_payload(1));
        assert!(matches!(bundle.verify(&other_key, None), Err(PolicyError::SignatureInvalid)));
    }

    #[test]
    fn rollback_rejected() {
        let (bundle, pubkey) = sign_bundle(&sample_payload(3));
        // Last-accepted was version 5; a version-3 bundle must be rejected.
        let err = bundle.verify(&pubkey, Some(5)).unwrap_err();
        assert!(matches!(err, PolicyError::RollbackRejected { found: 3, last: 5 }));
    }

    #[test]
    fn same_version_rejected() {
        let (bundle, pubkey) = sign_bundle(&sample_payload(5));
        assert!(matches!(
            bundle.verify(&pubkey, Some(5)),
            Err(PolicyError::RollbackRejected { found: 5, last: 5 })
        ));
    }

    #[test]
    fn newer_version_accepted() {
        let (bundle, pubkey) = sign_bundle(&sample_payload(6));
        assert_eq!(bundle.verify(&pubkey, Some(5)).unwrap().version(), 6);
    }

    #[test]
    fn hex_decoder_roundtrip() {
        assert_eq!(decode_hex("00ff1a").unwrap(), vec![0x00, 0xff, 0x1a]);
        assert!(decode_hex("abc").is_err()); // odd length
        assert!(decode_hex("zz").is_err()); // non-hex
    }
}
