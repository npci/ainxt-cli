//! Attaches the policy attestation headers to every outbound model request.
//!
//! Lives here rather than in `ainxt-pep` because it needs `ainxt-sampler`'s
//! `HeaderInjector` trait, and `ainxt-pep` depending on the sampler closes a
//! cycle through `ainxt-sampling-types` → `ainxt-tools` → `ainxt-pep`. The
//! values themselves come from `ainxt_pep::attest`; this is only the transport.

use ainxt_sampler::config::HeaderInjector;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};

/// Reports the live enforcement posture on every request.
#[derive(Debug, Clone, Copy, Default)]
pub struct PolicyAttestation;

impl HeaderInjector for PolicyAttestation {
    fn inject(&self, headers: &mut HeaderMap) {
        for (name, value) in ainxt_pep::attest::headers() {
            // A header we cannot encode is dropped rather than panicking:
            // failing an inference request because an attestation value was
            // malformed would turn a reporting problem into an outage.
            if let (Ok(name), Ok(value)) = (
                HeaderName::from_bytes(name.as_bytes()),
                HeaderValue::from_str(&value),
            ) {
                headers.insert(name, value);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_headers_are_attached() {
        let mut headers = HeaderMap::new();
        PolicyAttestation.inject(&mut headers);
        assert!(headers.contains_key(ainxt_pep::attest::POLICY_VERSION_HEADER));
        assert!(headers.contains_key(ainxt_pep::attest::ENFORCEMENT_HEADER));
    }
}
