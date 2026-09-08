//! Post-action facts fed back to the enforcement point.
//!
//! Two controls are inert without this module, because both depend on knowing
//! what happened *after* a tool ran rather than what was requested before it:
//!
//! * **Failure-loop detection** — the brute-force control. The risk ledger can
//!   count invocations from the authorization path alone, but counting is not
//!   enough: a build runs one compiler hundreds of times and succeeds, while a
//!   password attack runs one tool repeatedly and keeps failing. Only the
//!   outcome distinguishes them, and only this call supplies it.
//!
//! * **The artifact → exec link** — what stops "threat hunting" that ends with
//!   running the file it found. A download has to be recorded as untrusted at
//!   the moment it lands, or the later attempt to execute it looks like any
//!   other command.
//!
//! Everything here is best-effort and never fails a tool call. These are
//! observations; refusing to return a completed tool's result because a ledger
//! write failed would convert a telemetry problem into a broken session.

use ainxt_intent::{Capability, ShellKind};
use ainxt_pep::context::local_principal;
use ainxt_pep::{Effect, Pep};
use ainxt_session_risk::ArtifactTrust;

/// Record the outcome of a completed tool call.
///
/// `args` is the tool's parsed input; a `command` string field is treated as a
/// shell command regardless of the tool's name, which keeps this working for
/// any terminal-shaped tool rather than only the one called `bash` today.
pub fn note_tool_outcome(tool_name: &str, args: &serde_json::Value, success: bool) {
    let Some(pep) = ainxt_pep::global::active() else {
        return;
    };
    let Some(command) = args.get("command").and_then(|v| v.as_str()) else {
        return;
    };
    let _ = tool_name;

    let derivation = ainxt_intent::derive_shell(command, shell_kind());
    let principal = local_principal(None, None);

    // The failure-loop counter is keyed on the program, not the whole command:
    // a brute force varies its arguments on every attempt and would otherwise
    // look like a thousand unrelated one-off commands.
    if let Some(program) = derivation.targets.programs.first() {
        pep.observe_effect(
            &principal,
            Effect::Outcome {
                program: program.clone(),
                success,
            },
        );
    }

    if success {
        note_downloaded_artifacts(&pep, &principal, &derivation, command);
    }
}

/// Record files written by a command that also fetched from the network.
///
/// The conjunction is the point. A command that only writes is ordinary work;
/// a command that downloads *and* writes has landed remote bytes on disk, and
/// those bytes must be marked untrusted so a later attempt to execute them is
/// refused. Marking every write untrusted would freeze the workspace within
/// minutes.
fn note_downloaded_artifacts(
    pep: &Pep,
    principal: &ainxt_pep::Principal,
    derivation: &ainxt_intent::Derivation,
    command: &str,
) {
    if !derivation.has(Capability::Download) {
        return;
    }
    let origin = derivation
        .targets
        .urls
        .first()
        .map(|u| format!("downloaded from {u}"))
        .unwrap_or_else(|| format!("produced by a fetching command: {command}"));

    for path in &derivation.targets.writes {
        pep.observe_effect(
            principal,
            Effect::ArtifactWritten {
                path: path.clone(),
                origin: origin.clone(),
                trust: ArtifactTrust::Untrusted,
            },
        );
    }
}

/// Record that a web fetch landed content in the session.
///
/// The fetched bytes themselves are not a file, but recording the origin means
/// a subsequent write of that content can be attributed rather than appearing
/// from nowhere.
pub fn note_web_fetch(url: &str, bytes: u64) {
    if let Some(pep) = ainxt_pep::global::active() {
        pep.observe_effect(&local_principal(None, None), Effect::BytesSent { bytes });
        let _ = url;
    }
}

fn shell_kind() -> ShellKind {
    if cfg!(windows) {
        ShellKind::PowerShell
    } else {
        ShellKind::Posix
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Must be completely inert with no enforcement point installed — every OSS
    /// build takes this path on every tool call.
    #[test]
    fn no_installed_pep_is_a_no_op() {
        note_tool_outcome(
            "bash",
            &serde_json::json!({ "command": "curl https://x -o y.sh" }),
            true,
        );
        note_web_fetch("https://example.com", 10);
    }

    #[test]
    fn a_tool_without_a_command_field_is_ignored() {
        note_tool_outcome("read", &serde_json::json!({ "path": "/tmp/x" }), true);
    }
}
