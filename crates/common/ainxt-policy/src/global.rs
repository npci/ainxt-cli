//! Process-global policy engine.
//!
//! Policy is resolved once at startup (from the signed bundle + settings chain)
//! and is fixed for the session. Rather than thread a [`PolicyEngine`] through
//! every enforcement call site — the sampler HTTP path, the permission manager,
//! the tool layer — we publish it here and let each site read it. This mirrors
//! how `effective_api_backend()` already reads process-global config.
//!
//! Reads are lock-free ([`arc_swap`]). Until [`install`] is called, reads return
//! a permissive OSS-default engine, so a binary that never wires policy behaves
//! exactly as it does today (no enforcement) rather than panicking.
//!
//! Enforcement points must call [`active`] on every action; they must never
//! cache the engine across a call, so that a mid-session policy reload (future)
//! takes effect immediately.

use std::sync::Arc;

use arc_swap::ArcSwapOption;

use ainxt_policy_types::policy::SecurityPolicy;

use crate::engine::PolicyEngine;

static ENGINE: ArcSwapOption<PolicyEngine> = ArcSwapOption::const_empty();

/// Publish the resolved engine for the process. Called once, early in startup,
/// after [`crate::StartupGate`] has produced the base policy and the settings
/// chain has narrowed it.
pub fn install(engine: PolicyEngine) {
    ENGINE.store(Some(Arc::new(engine)));
}

/// The active engine. Returns a permissive OSS-default engine if none has been
/// installed, so unwired binaries and tests behave as before.
pub fn active() -> Arc<PolicyEngine> {
    match ENGINE.load_full() {
        Some(e) => e,
        None => Arc::new(PolicyEngine::new(SecurityPolicy::oss_default())),
    }
}

/// Whether an engine has been explicitly installed (vs the OSS-default fallback).
pub fn is_installed() -> bool {
    ENGINE.load().is_some()
}

/// Test-only: install a specific engine and return a guard that clears it on
/// drop, so tests do not leak global state into one another.
#[cfg(any(test, feature = "test-support"))]
pub fn install_scoped(engine: PolicyEngine) -> ScopedEngine {
    ENGINE.store(Some(Arc::new(engine)));
    ScopedEngine(())
}

#[cfg(any(test, feature = "test-support"))]
pub struct ScopedEngine(());

#[cfg(any(test, feature = "test-support"))]
impl Drop for ScopedEngine {
    fn drop(&mut self) {
        ENGINE.store(None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ainxt_policy_types::verdict::Enforcement;

    #[test]
    fn default_is_permissive_when_uninstalled() {
        // Not calling install(): active() must be the OSS default (Off).
        let e = active();
        assert_eq!(e.enforcement(), Enforcement::Off);
    }

    #[test]
    fn scoped_install_and_clear() {
        {
            let _g = install_scoped(PolicyEngine::new(SecurityPolicy {
                enforcement: Enforcement::Block,
                capabilities: Default::default(),
            }));
            assert!(is_installed());
            assert_eq!(active().enforcement(), Enforcement::Block);
        }
        // Guard dropped → back to uninstalled/default.
        assert!(!is_installed());
        assert_eq!(active().enforcement(), Enforcement::Off);
    }
}
