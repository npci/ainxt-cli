//! # ainxt-policy-types
//!
//! Pure schema and merge algebra for the ainxt security policy engine.
//!
//! This crate is deliberately **I/O-free and dependency-light**. It defines the
//! shape of a security policy, the capability lattice, verdicts, and — most
//! importantly — the *narrowing-only merge algebra* that satisfies invariant
//! **INV-2** from `docs/security/SECURITY_ARCHITECTURE_AND_ACCEPTANCE.md`:
//!
//! > No lower-precedence settings source can widen capability.
//!
//! Enforcement, signature verification, filesystem resolution, and the decision
//! runtime live in the sibling `ainxt-policy` crate. Keeping the schema and the
//! merge algebra here — with no I/O — is what lets the red-team corpus
//! (`AINXT-SEC-003`) exercise INV-2 as fast, deterministic unit tests.
//!
//! ## The one rule that matters
//!
//! Repo-level settings (`projectSettings`, `localSettings`) are
//! attacker-controllable (a cloned repo can ship `.ainxt/settings.json`).
//! Therefore merge is **not** override: a lower-precedence source may only ever
//! *narrow* capability. Concretely:
//!
//! - allowlists **intersect** (meet) — the result permits only what *both* permit
//! - denylists **union** — any source may add a denial
//! - [`Enforcement`] is **monotone up** — a source may raise, never lower it
//! - the [`SovereignAction`] set **unions** — any source may add, none may remove
//!
//! Every operation in [`merge`] is associative and commutative in its narrowing
//! effect, which is what the property tests assert.

pub mod capability;
pub mod error;
pub mod ids;
pub mod merge;
pub mod policy;
pub mod tier;
pub mod verdict;

pub use capability::{Allowlist, Denylist, SecurityCapabilities, SovereignAction};
pub use error::PolicyTypesError;
pub use ids::{Authority, Domain, RuleId};
pub use policy::{SecurityPolicy, SourceLayer, SourceOrigin};
pub use tier::TrustTier;
pub use verdict::{Decision, Enforcement, Verdict};
