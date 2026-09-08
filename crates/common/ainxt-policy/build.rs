//! Build-time stamping of the security posture.
//!
//! `bootstrap::resolve_manifest` and `bootstrap::embedded_bundle_bytes` read
//! these through `option_env!`, which resolves at *compile* time. Cargo does
//! not track `option_env!` inputs on its own, so without these directives a
//! rebuild after changing either variable would silently reuse the previous
//! value — including rebuilding a managed binary as an unmanaged one, or the
//! reverse. Both are security-relevant enough to be worth the explicit
//! invalidation.

fn main() {
    println!("cargo:rerun-if-env-changed=AINXT_POLICY_AUTHORITY_HEX");
    println!("cargo:rerun-if-env-changed=AINXT_POLICY_BUNDLE_B64");
}
