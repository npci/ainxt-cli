//! Runtime policy errors.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum PolicyError {
    /// `require_policy = true` but no valid bundle was available (INV-7).
    #[error("policy required but no valid bundle is present: {0}")]
    PolicyRequiredButMissing(String),

    /// The bundle signature did not verify against the build's authority key.
    #[error("bundle signature verification failed")]
    SignatureInvalid,

    /// The bundle version is not greater than the last-seen version (rollback).
    #[error("bundle version {found} is not newer than last-accepted {last} (rollback rejected)")]
    RollbackRejected { found: u64, last: u64 },

    /// The bundle payload could not be parsed.
    #[error("bundle parse error: {0}")]
    Parse(String),

    /// The configured authority public key was malformed.
    #[error("malformed authority public key: {0}")]
    BadAuthorityKey(String),

    #[error("io error reading {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
}
