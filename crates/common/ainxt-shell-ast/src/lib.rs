//! Shell command parsing and file-access derivation.
//!
//! Extracted from `ainxt-workspace::permission` so that policy enforcement can
//! reason about *what a command does* without depending on the permission
//! crate — `ainxt-workspace` already depends on the policy layer, so deriving
//! capabilities there would be a dependency cycle.
//!
//! - [`bash`] — tree-sitter-bash parsing, `&&`/`||`/`;`/`|` splitting, wrapper
//!   peeling (`timeout`, `env`, `nice`, ...), heredoc and soft-break handling.
//! - [`access`] — which files a command reads or writes, derived from that AST:
//!   redirects, `tee`, `dd of=`, in-place `sed`, package-manager launchers.
//!
//! This crate makes no decisions and knows nothing about policy. It reports
//! what a command *is*; callers decide what to do about it.

pub mod access;
pub mod bash;
