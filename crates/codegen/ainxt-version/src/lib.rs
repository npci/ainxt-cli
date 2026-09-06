//! Installed ainxt CLI version, lockstepped with shipping binaries.

use semver::Version;

pub const TEST_VERSION_ENV: &str = "AINXT_TEST_VERSION";

pub const VERSION: &str = match option_env!("AINXT_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

/// Strip surrounding whitespace and any wrapping quote characters.
///
/// `AINXT_VERSION` is injected at build time. Depending on how the build
/// environment is populated (a `.env` file read literally, a CI variable that
/// keeps its quoting, `cargo:rustc-env` passthrough, …) the value can arrive
/// as `"0.2.107"` — quote characters included — which is baked into the
/// [`VERSION`] string literal. `semver::Version::parse` then fails with
/// "Failed to parse versions", breaking every update comparison while the
/// gateway's clean `0.2.107` parses fine.
///
/// Strips repeatedly so `'"0.2.107"'` normalises too, and only removes a
/// quote when it is matched on both ends (a bare leading quote is left alone
/// so genuinely malformed input still surfaces as a parse error).
fn normalize(raw: &str) -> String {
    let mut s = raw.trim();
    loop {
        let stripped = s
            .strip_prefix('"')
            .and_then(|r| r.strip_suffix('"'))
            .or_else(|| s.strip_prefix('\'').and_then(|r| r.strip_suffix('\'')));
        match stripped {
            Some(inner) => s = inner.trim(),
            None => break,
        }
    }
    s.to_string()
}

/// [`TEST_VERSION_ENV`] override first, then [`VERSION`]. Trimmed and
/// unquoted so non-semver-aware callers can pass the result straight into
/// parsing.
pub fn installed() -> String {
    std::env::var(TEST_VERSION_ENV)
        .map(|v| normalize(&v))
        .unwrap_or_else(|_| normalize(VERSION))
}

pub fn installed_semver() -> Result<Version, semver::Error> {
    Version::parse(&installed())
}

/// Format the compiled version with a channel label for user-facing display.
///
/// `channel_label` is a pre-formatted suffix such as `" [alpha]"`, `" [stable]"`,
/// or `""` (empty when no cached pointer is available). Obtain it from
/// `ainxt_update::channel_label()`.
///
/// Example: `"0.2.5 [stable]"` or `"0.2.5 [alpha]"`.
pub fn display_version(channel_label: &str) -> String {
    format!("{}{}", VERSION, channel_label)
}

/// Format a version-with-commit string with a channel label.
///
/// Same semantics as [`display_version`] but for the full
/// `"0.2.5 (abc1234)"` string.
pub fn display_version_with_commit(version_with_commit: &str, channel_label: &str) -> String {
    format!("{}{}", version_with_commit, channel_label)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Display formatting invariant matrix — verifies label appending
    /// works correctly across all label states (alpha, stable, empty).
    #[test]
    fn test_display_version_formatting_matrix() {
        let cases: &[(&str, &str, &str)] = &[
            // (version_with_commit,    label,        expected_suffix)
            ("0.2.5 (abc1234)", " [alpha]", "0.2.5 (abc1234) [alpha]"),
            ("0.2.5 (abc1234)", " [stable]", "0.2.5 (abc1234) [stable]"),
            ("0.2.5 (abc1234)", "", "0.2.5 (abc1234)"),
            (
                "0.1.220-alpha.2 (def0)",
                " [alpha]",
                "0.1.220-alpha.2 (def0) [alpha]",
            ),
        ];
        for (vwc, label, expected) in cases {
            assert_eq!(
                display_version_with_commit(vwc, label),
                *expected,
                "display_version_with_commit({:?}, {:?})",
                vwc,
                label,
            );
        }
        // display_version uses compiled VERSION — just verify the label appends
        assert_eq!(display_version(""), VERSION);
        assert!(display_version(" [stable]").ends_with("[stable]"));
    }

    /// A build that bakes `AINXT_VERSION="0.2.107"` (quotes included) into
    /// the binary must still yield a parseable semver. Regression guard for
    /// `Failed to parse versions (current="0.2.107", target=0.2.107)`.
    #[test]
    fn normalize_strips_wrapping_quotes_and_whitespace() {
        let cases: &[(&str, &str)] = &[
            ("0.2.107", "0.2.107"),
            ("\"0.2.107\"", "0.2.107"),
            ("'0.2.107'", "0.2.107"),
            ("  0.2.107  ", "0.2.107"),
            ("  \"0.2.107\"  ", "0.2.107"),
            ("\" 0.2.107 \"", "0.2.107"),
            // Nested / doubled quoting normalises too.
            ("'\"0.2.107\"'", "0.2.107"),
            ("\"\"0.2.107\"\"", "0.2.107"),
            // Pre-release strings survive intact.
            ("\"0.1.220-alpha.4\"", "0.1.220-alpha.4"),
            // Unmatched quotes are left alone so malformed input still errors.
            ("\"0.2.107", "\"0.2.107"),
            ("0.2.107\"", "0.2.107\""),
        ];
        for (raw, expected) in cases {
            assert_eq!(normalize(raw), *expected, "normalize({raw:?})");
        }
    }

    /// Every normalised form must parse as semver.
    #[test]
    fn normalized_quoted_version_parses_as_semver() {
        for raw in ["0.2.107", "\"0.2.107\"", "'0.2.107'", "  \"0.2.107\"  "] {
            let normalized = normalize(raw);
            assert!(
                Version::parse(&normalized).is_ok(),
                "normalize({raw:?}) = {normalized:?} must parse as semver"
            );
        }
    }

    /// `installed()` must always return something parseable.
    #[test]
    fn installed_returns_parseable_semver() {
        let v = installed();
        assert!(
            Version::parse(&v).is_ok(),
            "installed() returned unparseable version: {v:?}"
        );
    }
}
