//! Integration test: the shell re-parks `exit_plan_mode` on
//! resume, so approval chrome reappears after quit/`--continue` and approving
//! leaves plan mode + starts the implement turn.
//!
//! CI stages the pager binary via `PAGER_BINARY`. Also runs under plain cargo
//! (which builds the pager on demand):
//!
//! ```bash
//! cargo test -p ainxt-pager-pty-harness --test plan_approval_resume -- --nocapture
//! ```

// ci.yml runs this crate in a separate, non-blocking (continue-on-error)
// step -- see the comment there for why. Left runnable (not #[ignore]'d)
// so its pass/fail is visible in that step's output rather than hidden.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn plan_approval_restored_after_resume() {
    ainxt_pager_pty_harness::scenarios::plan_approval_resume::assert_plan_approval_restored_after_resume()
        .await
        .expect("shell must re-park exit_plan_mode on resume so approval chrome returns");
}
