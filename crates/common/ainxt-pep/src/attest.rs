//! Attestation values: what this client tells the gateway it is enforcing.
//!
//! Local enforcement is only non-optional if something the user needs can
//! refuse to serve a client that is not enforcing. The gateway holds that
//! leverage — it has the model access — so every request carries the client's
//! posture and policy version, and the gateway may reject anything below the
//! floor its profile requires.
//!
//! The two halves make each other real: the gateway's attestation check forces
//! local enforcement to stay on, and the CLI's egress control forces traffic
//! through the gateway in the first place. Either alone is bypassable.
//!
//! # Why this module has no HTTP types
//!
//! It returns strings. The `HeaderInjector` implementation that puts them on a
//! request lives in `ainxt-shell`, because depending on `ainxt-sampler` from
//! here closes a dependency cycle (`ainxt-tools` → `ainxt-pep` → `ainxt-sampler`
//! → `ainxt-sampling-types` → `ainxt-tools`) and, less mechanically, because a
//! policy crate has no business knowing how bytes reach the network.

/// Header carrying `<bundle>.<overlay>` — the accepted policy versions.
pub const POLICY_VERSION_HEADER: &str = "x-ainxt-policy-version";
/// Header carrying `off` | `observe` | `block`.
pub const ENFORCEMENT_HEADER: &str = "x-ainxt-enforcement";

/// `<bundle>.<overlay>`, e.g. `42.7`; `0.0` when no signed policy is installed.
///
/// A version rather than a hash so the gateway can compare orderings and spot a
/// fleet running behind, which a hash cannot express.
pub fn version_string() -> String {
    let (bundle, overlay) = crate::bootstrap::accepted_versions();
    format!("{bundle}.{overlay}")
}

/// The live enforcement posture.
///
/// Read fresh on every call rather than cached: the policy engine is
/// hot-swappable, and a client that kept asserting a stale posture after an
/// overlay narrowed it would be misreporting to the very control meant to catch
/// misreporting.
pub fn enforcement_string() -> &'static str {
    match ainxt_policy::global::active().enforcement() {
        ainxt_policy_types::Enforcement::Off => "off",
        ainxt_policy_types::Enforcement::Warn => "observe",
        ainxt_policy_types::Enforcement::Block => "block",
    }
}

/// Both headers as `(name, value)` pairs, ready to attach to a request.
pub fn headers() -> [(&'static str, String); 2] {
    [
        (POLICY_VERSION_HEADER, version_string()),
        (ENFORCEMENT_HEADER, enforcement_string().to_owned()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_version_is_always_a_bundle_overlay_pair() {
        assert!(version_string().contains('.'));
    }

    /// An unmanaged build must still attest. Silence is indistinguishable from
    /// a client that stripped the header, so the gateway needs an explicit
    /// "off" to act on.
    #[test]
    fn an_unenforcing_build_reports_off_rather_than_nothing() {
        assert_eq!(enforcement_string(), "off");
        assert_eq!(version_string(), "0.0");
        assert_eq!(headers().len(), 2);
    }
}
