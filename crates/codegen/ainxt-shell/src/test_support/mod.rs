pub(crate) mod lsp_runtime;

pub(crate) const TEST_MODEL: &str = "test-model";

/// Guard returned by [`isolated_ainxt_home`]. Keeps the backing temp
/// directory alive and restores the prior `AINXT_HOME` value on drop.
pub(crate) struct IsolatedAinxtHome {
    _dir: tempfile::TempDir,
    _env: ainxt_test_support::EnvGuard,
}

/// Point `AINXT_HOME` at a fresh, empty temp directory for the lifetime of
/// the returned guard. Used by tests that must not read/write the
/// developer's real `~/.ainxt` (e.g. cached credentials, config.toml).
pub(crate) fn isolated_ainxt_home() -> IsolatedAinxtHome {
    let dir = tempfile::tempdir().expect("create temp dir for isolated AINXT_HOME");
    let env = ainxt_test_support::EnvGuard::set("AINXT_HOME", dir.path());
    IsolatedAinxtHome { _dir: dir, _env: env }
}

/// Prepend the hermetic git binary (via `GIT_BIN_PATH`) to `PATH` so that
/// `Command::new("git")` in test helpers resolves to the Bazel-provided
/// static binary instead of relying on system-installed git.
///
/// Safe to call multiple times — only the first call mutates `PATH`.
pub(crate) fn ensure_hermetic_git_on_path() {
    use std::path::PathBuf;
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        if let Ok(git_bin) = std::env::var("GIT_BIN_PATH") {
            let p = PathBuf::from(&git_bin);
            let p = if p.is_relative() {
                std::env::current_dir().unwrap().join(&p)
            } else {
                p
            };
            if let Some(dir) = p.parent() {
                let cur = std::env::var("PATH").unwrap_or_default();
                unsafe {
                    std::env::set_var("PATH", format!("{}:{}", dir.display(), cur));
                }
            }
        }
    });
}
