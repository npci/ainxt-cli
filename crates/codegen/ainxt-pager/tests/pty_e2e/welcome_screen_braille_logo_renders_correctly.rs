// Per-test-case module for the `pty_e2e` integration test crate.
#[allow(unused_imports)]
use super::common::*;

/// 1b. **Welcome screen renders the ainxt wordmark logo correctly.**
///
/// The logo uses Unicode block / box-drawing characters. A regression in
/// the writer thread (using `WriteFile` instead of `WriteConsoleW` on
/// Windows, or a missing `SetConsoleOutputCP(65001)`) causes these
/// multi-byte UTF-8 characters to be misinterpreted as individual legacy
/// code-page bytes, producing garbled output.
///
/// This test asserts that distinctive logo characters appear intact in the
/// PTY screen buffer.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn welcome_screen_braille_logo_renders_correctly() {
    let content = ContentController::start().await.expect("start content");

    let binary = pager_binary().expect("resolve pager binary");
    // Use a tall terminal so pick_logo() selects the full logo (≥26 rows).
    let mut harness =
        PtyHarness::spawn_with_content(&binary, DEFAULT_ROWS, DEFAULT_COLS, &content, &[])
            .expect("spawn pager");

    harness
        .wait_for_text(WELCOME_SCREEN_SENTINEL, WELCOME_TIMEOUT)
        .expect("welcome text");

    let screen = harness.screen_contents();

    // The logo is drawn with Unicode Braille pattern glyphs (U+2800..U+28FF).
    // The check below looks for one such glyph. If the writer
    // thread sends raw UTF-8 bytes through a code-page-dependent API, these
    // 3-byte characters would be mangled. Check for a block glyph (▀ ▄ █),
    // which only appears in the logo — not in any ASCII menu label.
    assert!(
        screen.chars().any(|c| matches!(c, '\u{2580}' | '\u{2584}' | '\u{2588}')),
        "No block logo glyph (▀/▄/█) found in screen — \
         logo may be garbled by code-page misinterpretation.\n\
         Screen contents:\n{screen}"
    );

    harness.quit().expect("clean quit");
}
