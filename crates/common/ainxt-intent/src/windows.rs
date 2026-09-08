//! PowerShell / cmd.exe capability derivation — deliberately fail-closed.
//!
//! There is no grammar here. Windows is the majority developer platform for
//! this deployment, but all of the AST machinery in `ainxt-shell-ast` is
//! tree-sitter-**bash**, so a pwsh command yields no parse and therefore no
//! trustworthy decomposition.
//!
//! Rather than pretend, this backend does two things:
//!
//! 1. Scans for constructs that exist primarily to defeat static inspection
//!    (`-EncodedCommand`, `Invoke-Expression`, backtick escapes, `$(...)`,
//!    `^` line-continuation, `%VAR%` late expansion). Any hit ⇒
//!    [`Confidence::Unknown`], which the policy layer must treat as
//!    "ask a human", never "permit".
//! 2. For the remaining literal command forms, tokenises and classifies each
//!    segment, reporting [`Confidence::Partial`] — a lower bound on what the
//!    command does, never a complete picture.
//!
//! It never returns [`Confidence::Exact`]. A real PowerShell AST backend is
//! tracked separately; until then the curated allowlist of ordinary dev command
//! forms in the policy bundle is what keeps the common path usable.

use crate::capability::{Capability, Confidence, Derivation};
use crate::programs::classify;

/// Constructs whose presence means we cannot claim to know what will run.
/// Matched case-insensitively against the whole command.
const EVASION_MARKERS: &[(&str, &str)] = &[
    ("-encodedcommand", "base64-encoded command payload"),
    ("-enc ", "base64-encoded command payload"),
    ("frombase64string", "base64 decode into execution"),
    ("invoke-expression", "interprets a string as code"),
    ("iex ", "interprets a string as code"),
    ("iex(", "interprets a string as code"),
    ("$(", "subexpression substitution"),
    ("&{", "script-block invocation"),
    (".{", "dot-sourced script block"),
    ("`", "backtick escape"),
    ("^", "caret escape"),
    ("[char]", "character-code string construction"),
    ("-join", "string reassembly"),
    ("downloadstring", "in-memory remote code fetch"),
    ("downloadfile", "in-memory remote file fetch"),
    ("start-process", "indirect process launch"),
    ("new-object", "reflective object construction"),
    ("add-type", "inline code compilation"),
    ("reflection.assembly", "assembly loading"),
];

/// PowerShell aliases that resolve to something we classify. Notably `curl` and
/// `wget` are aliases for `Invoke-WebRequest` on Windows PowerShell, so leaving
/// them unmapped would classify a network fetch as an unknown program.
const ALIASES: &[(&str, &str)] = &[
    ("iwr", "invoke-webrequest"),
    ("irm", "invoke-restmethod"),
    ("curl", "invoke-webrequest"),
    ("wget", "invoke-webrequest"),
    ("gc", "get-content"),
    ("cat", "get-content"),
    ("type", "get-content"),
    ("sc", "set-content"),
    ("ri", "remove-item"),
    ("del", "remove-item"),
    ("erase", "remove-item"),
    ("rd", "remove-item"),
    ("rmdir", "remove-item"),
    ("ni", "new-item"),
    ("ci", "copy-item"),
    ("cpi", "copy-item"),
    ("mi", "move-item"),
    ("spps", "stop-process"),
    ("kill", "stop-process"),
];

pub fn derive(command: &str) -> Derivation {
    let lower = command.to_ascii_lowercase();
    for (marker, why) in EVASION_MARKERS {
        if lower.contains(marker) {
            return Derivation::unknown(format!(
                "command uses {why} (`{}`), which defeats static decomposition",
                marker.trim()
            ));
        }
    }
    if lower.contains('%') && lower.matches('%').count() >= 2 {
        return Derivation::unknown("command uses `%VAR%` late expansion");
    }

    let Some(segments) = split_segments(command) else {
        return Derivation::unknown("command has unbalanced quoting");
    };

    let mut d = Derivation::new(Confidence::Partial);
    d.add(
        Capability::ShellInterpretation,
        "runs through a Windows shell with no available grammar",
    );

    for segment in segments {
        let Some(mut words) = tokenize(&segment) else {
            return Derivation::unknown("a command segment could not be tokenised");
        };
        if words.is_empty() {
            continue;
        }
        if let Some(first) = words.first_mut() {
            let key = first.to_ascii_lowercase();
            if let Some((_, canonical)) = ALIASES.iter().find(|(a, _)| *a == key) {
                *first = (*canonical).to_owned();
            }
        }
        classify(&words, &mut d);
    }

    d
}

/// Split on `;`, `|`, `&&`, `||`, `&` and newlines, respecting quotes.
/// Returns `None` if quoting is unbalanced, which is itself a fail-closed
/// signal rather than something to recover from.
fn split_segments(command: &str) -> Option<Vec<String>> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut chars = command.chars().peekable();

    while let Some(c) = chars.next() {
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                }
                current.push(c);
            }
            None => match c {
                '\'' | '"' => {
                    quote = Some(c);
                    current.push(c);
                }
                ';' | '\n' | '\r' => {
                    out.push(std::mem::take(&mut current));
                }
                '|' | '&' => {
                    // Collapse the doubled forms; both separate commands.
                    if chars.peek() == Some(&c) {
                        chars.next();
                    }
                    out.push(std::mem::take(&mut current));
                }
                _ => current.push(c),
            },
        }
    }

    if quote.is_some() {
        return None;
    }
    out.push(current);
    Some(
        out.into_iter()
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .collect(),
    )
}

/// Whitespace tokenisation with quote stripping. `None` on unbalanced quotes.
fn tokenize(segment: &str) -> Option<Vec<String>> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut started = false;
    let mut quote: Option<char> = None;

    for c in segment.chars() {
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                } else {
                    current.push(c);
                }
            }
            None => match c {
                '\'' | '"' => {
                    quote = Some(c);
                    started = true;
                }
                c if c.is_whitespace() => {
                    if started || !current.is_empty() {
                        out.push(std::mem::take(&mut current));
                        started = false;
                    }
                }
                _ => current.push(c),
            },
        }
    }

    if quote.is_some() {
        return None;
    }
    if started || !current.is_empty() {
        out.push(current);
    }
    Some(out)
}
