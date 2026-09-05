//! # ainxt-policy
//!
//! Resolution, bundle verification, and the decision engine for the ainxt
//! security policy. The pure schema and the narrowing-only merge algebra live in
//! [`ainxt_policy_types`]; this crate adds the parts that touch the outside
//! world — signed bundles, the `require_policy` startup gate, and the runtime
//! [`engine::PolicyEngine`] that every capability check consults.
//!
//! Satisfies P1 of `docs/security/IMPLEMENTATION_PLAN.md`:
//! - INV-2 (merge algebra) via `ainxt-policy-types`
//! - INV-5 (Sovereign non-bypass) via [`engine::PolicyEngine::is_sovereign`]
//! - INV-7 (no silent degradation) via [`manifest::StartupGate`]
//!
//! **Enforcement must never be delegated to `ainxt-hooks`** — hooks are
//! repo-configurable and therefore attacker-configurable (E-2). All decisions
//! flow through [`engine::PolicyEngine`].

pub mod bootstrap;
pub mod bundle;
pub mod egress_guard;
pub mod engine;
pub mod error;
pub mod exec_guard;
pub mod global;
pub mod manifest;

pub use bundle::{PolicyBundle, VerifiedBundle};
pub use engine::{ExecTarget, PolicyEngine};
pub use error::PolicyError;
pub use exec_guard::{check_exec, path_dirs_from_env, resolve_program, ResolvedExec};
pub use manifest::{BuildManifest, StartupGate};

// Re-export the schema so downstream crates depend only on `ainxt-policy`.
pub use ainxt_policy_types as types;
