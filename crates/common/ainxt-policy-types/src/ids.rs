//! Stable rule identifiers: `<AUTHORITY>-<DOMAIN>-<NNN>` (`AINXT-SEC-001` §15).
//!
//! Every rule ID must appear in the bundle, in denial messages, in audit
//! records, and in at least one test name. Parsing is strict so a typo in a
//! bundle is caught at load time rather than silently ignored.
//!
//! Authority prefixes are intentionally generic (`DEFAULT`, `MANAGED`,
//! `ORG-<name>`) so that no organisation name appears in the public source.
//! The actual rule content lives only in the signed policy bundle, which is
//! not distributed with the OSS build.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::error::PolicyTypesError;

/// The authority that issued a rule.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Authority {
    /// Built-in default shipped with the OSS engine.
    Default,
    /// A managed deployment's private policy bundle (operator-supplied).
    Managed,
    /// Another organisation's bundle: `ORG-<name>`.
    Org(String),
}

impl fmt::Display for Authority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Authority::Default => write!(f, "DEFAULT"),
            Authority::Managed => write!(f, "MANAGED"),
            Authority::Org(name) => write!(f, "ORG-{name}"),
        }
    }
}

/// The capability domain a rule governs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Domain {
    Egress,
    Exec,
    Fs,
    Cred,
    Sov,
    Rate,
    Crack,
    Content,
    Mcp,
    Audit,
}

impl Domain {
    fn as_str(self) -> &'static str {
        match self {
            Domain::Egress => "EGRESS",
            Domain::Exec => "EXEC",
            Domain::Fs => "FS",
            Domain::Cred => "CRED",
            Domain::Sov => "SOV",
            Domain::Rate => "RATE",
            Domain::Crack => "CRACK",
            Domain::Content => "CONTENT",
            Domain::Mcp => "MCP",
            Domain::Audit => "AUDIT",
        }
    }

    fn from_slug(s: &str) -> Result<Self, PolicyTypesError> {
        Ok(match s {
            "EGRESS" => Domain::Egress,
            "EXEC" => Domain::Exec,
            "FS" => Domain::Fs,
            "CRED" => Domain::Cred,
            "SOV" => Domain::Sov,
            "RATE" => Domain::Rate,
            "CRACK" => Domain::Crack,
            "CONTENT" => Domain::Content,
            "MCP" => Domain::Mcp,
            "AUDIT" => Domain::Audit,
            other => return Err(PolicyTypesError::UnknownDomain(other.to_string())),
        })
    }
}

impl fmt::Display for Domain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A parsed, validated rule identifier such as `MANAGED-EGRESS-002`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RuleId {
    pub authority: Authority,
    pub domain: Domain,
    /// The numeric suffix, preserved as text so leading zeros survive round-trips.
    pub number: String,
}

impl RuleId {
    /// Build a `DEFAULT-<domain>-<nnn>` id.
    pub fn default_rule(domain: Domain, number: impl Into<String>) -> Self {
        RuleId { authority: Authority::Default, domain, number: number.into() }
    }
}

impl fmt::Display for RuleId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}-{}", self.authority, self.domain, self.number)
    }
}

impl FromStr for RuleId {
    type Err = PolicyTypesError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let invalid = |reason: &str| PolicyTypesError::InvalidRuleId {
            input: s.to_string(),
            reason: reason.to_string(),
        };

        // Split from the right so `ORG-<name>` authorities (which contain a
        // hyphen) parse correctly: the last two segments are always
        // domain and number.
        let (head, number) = s.rsplit_once('-').ok_or_else(|| invalid("expected AUTHORITY-DOMAIN-NNN"))?;
        let (authority_str, domain_str) =
            head.rsplit_once('-').ok_or_else(|| invalid("expected AUTHORITY-DOMAIN-NNN"))?;

        if number.is_empty() || !number.bytes().all(|b| b.is_ascii_digit()) {
            return Err(invalid("number segment must be non-empty ASCII digits"));
        }

        let authority = match authority_str {
            "DEFAULT" => Authority::Default,
            "MANAGED" => Authority::Managed,
            other => {
                let name = other
                    .strip_prefix("ORG-")
                    .ok_or_else(|| invalid("authority must be DEFAULT, MANAGED, or ORG-<name>"))?;
                if name.is_empty() {
                    return Err(invalid("ORG authority requires a name"));
                }
                Authority::Org(name.to_string())
            }
        };

        let domain = Domain::from_slug(domain_str)?;
        Ok(RuleId { authority, domain, number: number.to_string() })
    }
}

// Serialize/Deserialize as the display string so bundles read naturally.
impl Serialize for RuleId {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for RuleId {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_managed_rule() {
        let id: RuleId = "MANAGED-EGRESS-002".parse().unwrap();
        assert_eq!(id.authority, Authority::Managed);
        assert_eq!(id.domain, Domain::Egress);
        assert_eq!(id.number, "002");
        assert_eq!(id.to_string(), "MANAGED-EGRESS-002");
    }

    #[test]
    fn parses_org_authority_with_hyphen() {
        let id: RuleId = "ORG-payments-CRACK-001".parse().unwrap();
        assert_eq!(id.authority, Authority::Org("payments".to_string()));
        assert_eq!(id.domain, Domain::Crack);
        assert_eq!(id.number, "001");
        assert_eq!(id.to_string(), "ORG-payments-CRACK-001");
    }

    #[test]
    fn rejects_unknown_domain() {
        let err = "MANAGED-BOGUS-001".parse::<RuleId>().unwrap_err();
        assert!(matches!(err, PolicyTypesError::UnknownDomain(d) if d == "BOGUS"));
    }

    #[test]
    fn rejects_non_numeric_suffix() {
        assert!("MANAGED-EGRESS-0x2".parse::<RuleId>().is_err());
        assert!("MANAGED-EGRESS-".parse::<RuleId>().is_err());
    }

    #[test]
    fn preserves_leading_zeros_through_serde() {
        let id: RuleId = "DEFAULT-SOV-001".parse().unwrap();
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"DEFAULT-SOV-001\"");
        let back: RuleId = serde_json::from_str(&json).unwrap();
        assert_eq!(back, id);
    }
}
