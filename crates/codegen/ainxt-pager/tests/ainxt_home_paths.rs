//! `AINXT_HOME` override tests in an isolated binary so `ainxt_home()`'s
//! process-wide `OnceLock` initializes from the overridden env var.

use std::path::PathBuf;

#[test]
fn ainxt_home_override_path_helpers() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ainxt_home = tmp.path().to_path_buf();
    unsafe {
        std::env::set_var("AINXT_HOME", &ainxt_home);
    }

    assert_eq!(
        ainxt_pager::util::pager_toml_path(),
        ainxt_home.join("pager.toml")
    );
    assert_eq!(
        ainxt_pager::util::display_ainxt_home_prefix(),
        "$AINXT_HOME"
    );
    assert_eq!(
        ainxt_pager::util::display_user_ainxt_path("config.toml"),
        "$AINXT_HOME/config.toml"
    );

    let memory_path = ainxt_home.join("memory/MEMORY.md");
    assert_eq!(
        ainxt_pager::util::abbreviate_path(&memory_path.display().to_string()),
        "$AINXT_HOME/memory/MEMORY.md"
    );

    assert!(ainxt_pager::util::is_under_user_ainxt_home(&memory_path));
    assert!(!ainxt_pager::util::is_under_user_ainxt_home(
        PathBuf::from("/tmp/other").as_path()
    ));
}
