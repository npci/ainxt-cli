//! Error type for schema parsing and rule-ID validation.

use thiserror::Error;

/// Errors produced while parsing or validating policy schema values.
///
/// Runtime/decision errors (signature failure, missing bundle, I/O) live in
/// `ainxt-policy`; this type covers only pure-schema concerns.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum PolicyTypesError {
    /// A rule ID did not match `<AUTHORITY>-<DOMAIN>-<NNN>`.
    #[error("invalid rule id {input:?}: {reason}")]
    InvalidRuleId { input: String, reason: String },

    /// An unknown policy domain slug was encountered.
    #[error("unknown policy domain {0:?}")]
    UnknownDomain(String),
}
