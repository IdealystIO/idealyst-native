//! Cross-platform render-parity check: capture the platform-native render tree
//! from two running apps and diff them.
//!
//! This is the end-to-end shape of "prove web and macOS render the same". It's
//! a plain `#[test]` (not `#[robot_test]`) because it needs **two**
//! connections, one per platform — the single-app harness can't express that.
//!
//! # Running it
//!
//! The easy way is `idealyst test --parity web,macos`, which launches both
//! apps, waits for them, sets the per-platform `IDEALYST_<PLATFORM>_BRIDGE`
//! vars, and runs this test. To drive it by hand, start the same app on two
//! platforms and point the vars at their bridges:
//!
//! ```bash
//! IDEALYST_WEB_BRIDGE=127.0.0.1:7001 \
//! IDEALYST_MACOS_BRIDGE=127.0.0.1:7002 \
//!   cargo test -p robot-test --test parity_web_vs_macos -- --nocapture
//! ```
//!
//! With either variable unset the test **skips** (prints a note and returns),
//! so a bare `cargo test` stays green — same posture as `#[robot_test]`.

use robot_test::parity::{self, compare, report, DiffOptions};

#[test]
fn web_and_macos_render_with_parity() {
    let (Some(mut web), Some(mut mac)) = (parity::connect("web"), parity::connect("macos")) else {
        eprintln!(
            "SKIP web_and_macos_render_with_parity: set IDEALYST_WEB_BRIDGE and \
             IDEALYST_MACOS_BRIDGE (or run `idealyst test --parity web,macos`)."
        );
        return;
    };

    let (alignment, mismatches) =
        compare(&mut web, &mut mac, DiffOptions::default(), None).expect("parity compare");
    eprintln!(
        "{} aligned, {} structural, {} prop divergences",
        alignment.pairs.len(),
        alignment.unmatched.len(),
        mismatches.len(),
    );
    if !mismatches.is_empty() {
        panic!(
            "render parity broken between web and macOS:\n{}",
            report(&mismatches, "web", "macos")
        );
    }
}
