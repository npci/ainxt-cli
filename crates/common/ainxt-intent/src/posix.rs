//! POSIX-shell capability derivation, over the tree-sitter-bash AST.
//!
//! Uses [`shell_command_invocations`] rather than top-level segment splitting so
//! that commands nested inside substitutions and subshells — `$(curl ...)`,
//! `` `...` ``, `( ... )` — are classified too. A capability hidden one level
//! down is still a capability.

use ainxt_shell_ast::access::{
    ShellFileMode, command_write_paths_in_tree, shell_command_invocations, shell_file_candidates,
    shell_file_mode, shell_redirect_targets,
};
use ainxt_shell_ast::bash::{try_parse_shell, unwrap_wrappers};

use crate::capability::{Capability, Confidence, Derivation};
use crate::programs::{classify, normalize_program, path_is_credential, path_is_system};

/// Bound on recursion into nested `-c` scripts. Legitimate commands never
/// approach it; the cap exists so a pathological nest cannot spin.
const MAX_NESTING: usize = 4;

pub fn derive(command: &str) -> Derivation {
    let mut d = derive_at(command, 0);

    // Composition check, run over the whole command including nested scripts.
    // Deliberately separate from per-program classification: this is a property
    // of how programs are *combined*, and an allowlist over program names is
    // structurally incapable of expressing it.
    if let Some((fetcher, interpreter)) = composes_remote_execution(command, 0) {
        d.add(
            Capability::RemoteCodeExecution,
            format!("`{fetcher}` retrieves content and `{interpreter}` executes it in the same command"),
        );
    }
    d
}

fn derive_at(command: &str, depth: usize) -> Derivation {
    let Some(tree) = try_parse_shell(command) else {
        return Derivation::unknown("shell command could not be parsed");
    };
    let root = tree.root_node();
    if root.has_error() {
        // A syntax error means the AST does not describe what the shell will
        // actually execute. That gap is the classic decomposition-evasion
        // vector, so refuse to guess rather than derive a set we cannot trust.
        return Derivation::unknown(
            "shell command has a syntax error; its decomposition is not trustworthy",
        );
    }

    let mut d = Derivation::new(Confidence::Exact);

    for (_offset, words, ambiguous) in shell_command_invocations(root, command) {
        if ambiguous {
            // An operand we could not resolve (expansion, variable) means the
            // derived set is a lower bound, not the whole truth.
            d.confidence = d.confidence.max(Confidence::Partial);
        }
        if words.is_empty() {
            continue;
        }

        let inner = unwrap_wrappers(&words);
        classify(inner, &mut d);

        // Operands of a reader program are files it reads.
        if let Some(raw) = inner.first() {
            let program = normalize_program(raw);
            if matches!(shell_file_mode(&program), Some(ShellFileMode::Read)) {
                for candidate in shell_file_candidates(inner) {
                    note_read(&mut d, candidate);
                }
            }

            // Recurse into `bash -c "…"`. tree-sitter sees the script as an
            // opaque string, so without this the inner command's capabilities
            // are invisible and `bash -c "curl https://evil/x"` would derive
            // nothing but "runs bash".
            if depth < MAX_NESTING
                && crate::programs::is_code_interpreter(&program)
                && let Some(script) = crate::programs::dash_c_script(&inner[1..])
            {
                d.merge(derive_at(script, depth + 1));
            }
        }
    }

    for path in command_write_paths_in_tree(root, command) {
        note_write(&mut d, &path);
    }

    for (_offset, target, mode, _ambiguous) in shell_redirect_targets(root, command) {
        match target {
            Some(path) => match mode {
                ShellFileMode::Read => note_read(&mut d, &path),
                ShellFileMode::Write => note_write(&mut d, &path),
            },
            None => {
                // A redirect whose destination we could not pin down.
                d.confidence = d.confidence.max(Confidence::Partial);
                d.add(
                    Capability::FsWrite,
                    "redirects to a destination that could not be resolved",
                );
            }
        }
    }

    d
}

/// Does this command fetch content and hand it to an interpreter?
///
/// Returns the pair responsible, for the explanation.
///
/// The interpreter must **not** carry `-c`: `bash -c "npm install"` is a
/// wrapper around ordinary work, not something reading fetched input, and
/// flagging it would make the rule fire on routine commands and get it
/// switched off. Nested `-c` scripts are searched separately so the check
/// cannot be evaded by one more layer of quoting.
fn composes_remote_execution(command: &str, depth: usize) -> Option<(String, String)> {
    if depth > MAX_NESTING {
        return None;
    }
    let tree = try_parse_shell(command)?;
    let root = tree.root_node();

    let mut fetcher: Option<String> = None;
    let mut interpreter: Option<String> = None;

    for (_offset, words, _ambiguous) in shell_command_invocations(root, command) {
        let inner = unwrap_wrappers(&words);
        let Some(raw) = inner.first() else { continue };
        let program = normalize_program(raw);
        let args = &inner[1..];

        if crate::programs::is_downloader(&program, args) {
            fetcher.get_or_insert(program.clone());
        }
        if crate::programs::is_code_interpreter(&program) {
            match crate::programs::dash_c_script(args) {
                // Wrapping a literal script: recurse rather than flag.
                Some(script) => {
                    if let Some(found) = composes_remote_execution(script, depth + 1) {
                        return Some(found);
                    }
                }
                // Reads stdin or a named file — the dangerous shape.
                None => {
                    interpreter.get_or_insert(program.clone());
                }
            }
        }
    }

    match (fetcher, interpreter) {
        (Some(f), Some(i)) => Some((f, i)),
        _ => None,
    }
}

pub(crate) fn note_read(d: &mut Derivation, path: &str) {
    d.targets.add_read(path);
    d.add(Capability::FsRead, format!("reads {path}"));
    flag_sensitive_path(d, path, "read");
}

pub(crate) fn note_write(d: &mut Derivation, path: &str) {
    d.targets.add_write(path);
    d.add(Capability::FsWrite, format!("writes {path}"));
    flag_sensitive_path(d, path, "write");
}

fn flag_sensitive_path(d: &mut Derivation, path: &str, verb: &str) {
    if path_is_credential(path) {
        d.add(
            Capability::CredentialAccess,
            format!("{verb}s credential material at {path}"),
        );
    }
    if path_is_system(path) {
        d.add(
            Capability::SystemPath,
            format!("{verb}s an OS-owned path at {path}"),
        );
    }
}
