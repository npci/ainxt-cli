//! Exec resolution + decision primitive (P2 → INV-3).
//!
//! Turns a program invocation (`argv[0]` plus `$PATH`/cwd) into a resolved,
//! hashed [`ExecTarget`] and returns a [`Decision`] from the [`PolicyEngine`].
//! Resolution is what makes the exec allowlist meaningful:
//!
//! - **PATH lookup** finds the file the OS would actually run.
//! - **Canonicalisation** follows symlinks to the real target, so a symlink
//!   named `git` pointing at `/usr/bin/pwsh` is judged as pwsh (TM-07/TM-15).
//! - **blake3 hashing** records what actually ran, for audit and future
//!   hash-pinning.
//!
//! The decision itself (allowlist/denylist/tier) lives in
//! [`PolicyEngine::exec_decision`]; this module only supplies a faithful target.
//!
//! Fail-closed by construction: if a program cannot be resolved to a file, we
//! still build a best-effort target from the raw invocation and let the engine
//! decide. Under a restrictive allowlist an unresolved/unknown basename is not
//! permitted (deny by omission); under the permissive OSS default (`Any`) it is
//! allowed, so "command not found" still surfaces from the shell as normal.

use std::path::{Path, PathBuf};

use ainxt_policy_types::tier::TrustTier;
use ainxt_policy_types::verdict::Decision;

use crate::engine::{ExecTarget, PolicyEngine};

/// A program invocation resolved to the file that will run.
#[derive(Debug, Clone)]
pub struct ResolvedExec {
    pub target: ExecTarget,
    /// True if resolution found and hashed a real file; false if this is a
    /// best-effort target built from the raw invocation.
    pub resolved: bool,
}

/// Resolve `program` against `path_dirs` (the `$PATH` entries) and `cwd`.
///
/// - If `program` contains a path separator it is treated as a path (relative
///   to `cwd` when not absolute).
/// - Otherwise each entry in `path_dirs` is searched for an executable file
///   named `program`.
///
/// On success the path is canonicalised (symlinks resolved) and hashed.
pub fn resolve_program(program: &str, cwd: &Path, path_dirs: &[PathBuf]) -> ResolvedExec {
    let basename = basename_of(program);

    let candidate = if has_separator(program) {
        let p = Path::new(program);
        Some(if p.is_absolute() { p.to_path_buf() } else { cwd.join(p) })
    } else {
        path_dirs.iter().map(|d| d.join(program)).find(|c| is_executable_file(c))
    };

    match candidate.and_then(|c| dunce::canonicalize(&c).ok()) {
        Some(canon) if is_executable_file(&canon) => {
            let content_hash = hash_file(&canon);
            ResolvedExec {
                target: ExecTarget {
                    resolved_path: canon.to_string_lossy().into_owned(),
                    // Basename of the *resolved* file — a symlink `git` → `pwsh`
                    // is judged as `pwsh`, not `git`.
                    basename: canon
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or(basename),
                    content_hash,
                },
                resolved: true,
            }
        }
        _ => ResolvedExec {
            // Best-effort: keep the raw invocation so the engine can still judge
            // it against a restrictive allowlist (deny by omission).
            target: ExecTarget { resolved_path: program.to_string(), basename, content_hash: None },
            resolved: false,
        },
    }
}

/// Resolve then decide, in one call.
pub fn check_exec(
    engine: &PolicyEngine,
    program: &str,
    cwd: &Path,
    path_dirs: &[PathBuf],
    tier: TrustTier,
) -> (Decision, ResolvedExec) {
    let resolved = resolve_program(program, cwd, path_dirs);
    let decision = engine.exec_decision(&resolved.target, tier);
    (decision, resolved)
}

/// Parse `$PATH` into directory entries.
pub fn path_dirs_from_env(path_var: &str) -> Vec<PathBuf> {
    std::env::split_paths(path_var).filter(|p| !p.as_os_str().is_empty()).collect()
}

fn has_separator(s: &str) -> bool {
    s.contains('/') || (cfg!(windows) && s.contains('\\'))
}

fn basename_of(program: &str) -> String {
    Path::new(program)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| program.to_string())
}

fn is_executable_file(p: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(p) else { return false };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn hash_file(p: &Path) -> Option<String> {
    let mut file = std::fs::File::open(p).ok()?;
    let mut hasher = blake3::Hasher::new();
    std::io::copy(&mut file, &mut hasher).ok()?;
    // Explicitly release the file handle once hashing is complete, rather
    // than relying solely on end-of-scope Drop.
    drop(file);
    Some(hasher.finalize().to_hex().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ainxt_policy_types::capability::{Allowlist, SecurityCapabilities};
    use ainxt_policy_types::policy::SecurityPolicy;
    use ainxt_policy_types::verdict::{Enforcement, Verdict};
    use std::io::Write;

    #[cfg(unix)]
    fn write_exec(dir: &Path, name: &str, body: &[u8]) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join(name);
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(body).unwrap();
        // Explicitly release the file handle before chmod'ing it, rather
        // than relying solely on end-of-scope Drop.
        drop(f);
        let mut perm = std::fs::metadata(&p).unwrap().permissions();
        perm.set_mode(0o755);
        std::fs::set_permissions(&p, perm).unwrap();
        p
    }

    fn engine_allowing(names: &[&str]) -> PolicyEngine {
        PolicyEngine::new(SecurityPolicy {
            enforcement: Enforcement::Block,
            capabilities: SecurityCapabilities {
                exec_allow: Allowlist::only(names.iter().copied()),
                ..SecurityCapabilities::default()
            },
        })
    }

    #[cfg(unix)]
    #[test]
    fn resolves_via_path_and_allows_listed_binary() {
        let dir = tempfile::tempdir().unwrap();
        write_exec(dir.path(), "python3", b"#!/bin/sh\n");
        let engine = engine_allowing(&["python3"]);
        let (d, r) = check_exec(
            &engine,
            "python3",
            dir.path(),
            &[dir.path().to_path_buf()],
            TrustTier::Workspace,
        );
        assert!(r.resolved);
        assert!(r.target.content_hash.is_some());
        assert_eq!(d.verdict, Verdict::Allow);
    }

    #[cfg(unix)]
    #[test]
    fn renamed_binary_in_writable_dir_denied_by_omission() {
        // Attacker drops a binary named "notepad" (renamed pwsh) in a temp dir.
        // Only bash/python3/git are allowlisted → denied on the resolved
        // basename, regardless of what the file actually is.
        let dir = tempfile::tempdir().unwrap();
        write_exec(dir.path(), "notepad", b"binary");
        let engine = engine_allowing(&["bash", "python3", "git"]);
        let (d, r) = check_exec(
            &engine,
            "notepad",
            dir.path(),
            &[dir.path().to_path_buf()],
            TrustTier::Operator,
        );
        assert!(r.resolved);
        assert_eq!(d.verdict, Verdict::Block);
        assert_eq!(d.rule.unwrap().to_string(), "DEFAULT-EXEC-002");
    }

    #[cfg(unix)]
    #[test]
    fn symlink_is_judged_by_its_canonical_target() {
        // `git` is a symlink to `pwsh`; only `git` is on the allowlist. After
        // canonicalisation the resolved basename is `pwsh` → denied. This is the
        // symlink-swap defence (TM-15).
        let dir = tempfile::tempdir().unwrap();
        let real = write_exec(dir.path(), "pwsh", b"the real pwsh");
        let link = dir.path().join("git");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let engine = engine_allowing(&["git"]);
        let (d, r) = check_exec(
            &engine,
            "git",
            dir.path(),
            &[dir.path().to_path_buf()],
            TrustTier::Operator,
        );
        assert!(r.resolved);
        assert_eq!(r.target.basename, "pwsh");
        assert_eq!(d.verdict, Verdict::Block);
    }

    #[cfg(unix)]
    #[test]
    fn hash_is_deterministic_and_content_addressed() {
        let dir = tempfile::tempdir().unwrap();
        write_exec(dir.path(), "a", b"same bytes");
        write_exec(dir.path(), "b", b"same bytes");
        let ra = resolve_program("a", dir.path(), &[dir.path().to_path_buf()]);
        let rb = resolve_program("b", dir.path(), &[dir.path().to_path_buf()]);
        assert_eq!(ra.target.content_hash, rb.target.content_hash);
    }

    #[test]
    fn unresolved_program_under_any_allowlist_is_allowed() {
        // OSS default: no exec restriction. An unresolved command still yields
        // Allow, so the shell reports "not found" naturally.
        let engine = PolicyEngine::new(SecurityPolicy::oss_default());
        let dir = std::env::temp_dir();
        let (d, r) = check_exec(&engine, "definitely-not-a-real-binary-xyz", &dir, &[], TrustTier::Operator);
        assert!(!r.resolved);
        assert_eq!(d.verdict, Verdict::Allow);
    }

    #[test]
    fn unresolved_program_under_restrictive_allowlist_is_denied() {
        // Fail-closed: can't resolve, basename not on a restrictive allowlist.
        let engine = engine_allowing(&["bash"]);
        let dir = std::env::temp_dir();
        let (d, r) = check_exec(&engine, "mystery-binary", &dir, &[], TrustTier::Operator);
        assert!(!r.resolved);
        assert_eq!(d.verdict, Verdict::Block);
    }
}
