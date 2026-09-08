//! AI-suggested commit message generation.
//!
//! A one-shot model completion — modeled on
//! [`crate::session::helpers::session_summary::generate_session_summary`] —
//! that turns a *staged* git diff into a concise conventional-commit-style
//! message (subject `<= 72` chars, optional body).
//!
//! This does NOT create a visible turn; it is a standalone
//! [`OaiCompatClient::conversation_collect`] call using the session's existing
//! sampling client + model (never a hardcoded model id). The caller decides how
//! to source the client/model; see `ainxt.dev/git/suggest_commit_message` in
//! [`crate::extensions::git`].

use crate::sampling::{Client as OaiCompatClient, ConversationItem, ConversationRequest};
use crate::session::helpers::chat::floor_char_boundary;

/// Upper bound on the diff text fed to the model. A commit subject/body only
/// needs the shape of the change, and this keeps the request well under the
/// prompt limit for very large staged diffs.
const DIFF_SOURCE_MAX_BYTES: usize = 24_000;

/// Longest subject line we advertise / trim to (conventional-commit convention).
const SUBJECT_MAX_CHARS: usize = 72;

/// Cap the diff to a UTF-8-safe byte budget so oversized staged diffs don't
/// blow the prompt limit (mirrors `session_summary::title_source_text`).
fn diff_source_text(diff: &str) -> String {
    let mut out = diff.to_string();
    out.truncate(floor_char_boundary(&out, DIFF_SOURCE_MAX_BYTES));
    out
}

/// Trim a raw model reply into a usable commit message: drop surrounding
/// whitespace/quotes/backticks and clamp the subject line to `SUBJECT_MAX_CHARS`.
/// Any body (lines after the first blank line) is preserved verbatim.
pub(crate) fn sanitize_commit_message(raw: &str) -> String {
    let trimmed = raw.trim().trim_matches('`').trim();
    let mut lines = trimmed.splitn(2, '\n');
    let subject_raw = lines.next().unwrap_or("").trim();
    // Strip a wrapping pair of quotes some models add around the subject.
    let subject_unquoted = subject_raw
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(subject_raw)
        .trim();
    let subject = clamp_subject(subject_unquoted);
    match lines.next() {
        Some(body) if !body.trim().is_empty() => format!("{subject}\n\n{}", body.trim()),
        _ => subject,
    }
}

/// Clamp a subject to at most [`SUBJECT_MAX_CHARS`] chars on a char boundary.
fn clamp_subject(subject: &str) -> String {
    if subject.chars().count() <= SUBJECT_MAX_CHARS {
        return subject.to_string();
    }
    subject.chars().take(SUBJECT_MAX_CHARS).collect()
}

/// Errors from [`suggest_commit_message`]. Surfaced to the UI as a
/// "couldn't suggest" message; the user can still type the message manually.
#[derive(Debug, thiserror::Error)]
pub enum SuggestError {
    /// The staged diff was empty — nothing to describe.
    #[error("no staged changes to describe")]
    EmptyDiff,
    /// The sampling call failed or returned no usable content.
    #[error("model completion failed: {0}")]
    Sampling(String),
}

/// Generate a conventional-commit-style message from a staged `diff`.
///
/// Uses the caller-provided `client` + `model` (the session's active sampler);
/// no model id is hardcoded. Returns [`SuggestError`] on empty diff or a failed
/// completion so the UI can show "couldn't suggest".
pub async fn suggest_commit_message(
    diff: String,
    client: OaiCompatClient,
    model: &str,
) -> Result<String, SuggestError> {
    let diff = diff_source_text(&diff);
    if diff.trim().is_empty() {
        return Err(SuggestError::EmptyDiff);
    }

    let request = ConversationRequest::from_items(vec![
        ConversationItem::system(
            r#"You write git commit messages for software engineers.

Given a staged git diff, write ONE concise commit message in the Conventional
Commits style. Rules:
- Subject line: an imperative summary, at most 72 characters, no trailing period.
  Prefer a conventional prefix when it fits (feat:, fix:, refactor:, docs:, test:, chore:, perf:).
- Optionally add a short body after a blank line explaining the "why" only if it
  adds value; skip the body for small/obvious changes.
- Describe only what the diff actually changes. Do not invent scope.
- Output ONLY the commit message text — no code fences, no quotes, no preamble."#,
        ),
        ConversationItem::user(format!(
            r#"Write a commit message for this staged diff:

```diff
{diff}
```"#
        )),
    ])
    .with_model(model)
    .with_max_output_tokens(300)
    .with_temperature(0.3);

    match client.conversation_collect(request).await {
        Ok(response) => {
            let text = response
                .assistant()
                .map(|a| a.content.as_ref().to_owned())
                .unwrap_or_default();
            let message = sanitize_commit_message(&text);
            if message.is_empty() {
                tracing::debug!(model = %model, "commit message suggestion: empty content");
                return Err(SuggestError::Sampling("model returned no content".to_string()));
            }
            Ok(message)
        }
        Err(e) => {
            tracing::warn!(model = %model, error = %e, "commit message suggestion failed");
            Err(SuggestError::Sampling(e.to_string()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_strips_fences_and_quotes() {
        assert_eq!(sanitize_commit_message("`fix: bug`"), "fix: bug");
        assert_eq!(sanitize_commit_message("\"fix: bug\""), "fix: bug");
        assert_eq!(sanitize_commit_message("  fix: bug  \n"), "fix: bug");
    }

    #[test]
    fn sanitize_clamps_long_subject() {
        let long = "feat: ".to_string() + &"x".repeat(200);
        let out = sanitize_commit_message(&long);
        assert_eq!(out.chars().count(), SUBJECT_MAX_CHARS);
    }

    #[test]
    fn sanitize_preserves_body() {
        let msg = "fix: correct off-by-one\n\nThe loop bound was wrong.";
        assert_eq!(sanitize_commit_message(msg), msg);
    }

    #[test]
    fn diff_source_text_is_utf8_safe_when_capped() {
        let big = "あ".repeat(20_000);
        let out = diff_source_text(&big);
        assert!(!out.is_empty() && out.len() <= DIFF_SOURCE_MAX_BYTES);
    }
}
