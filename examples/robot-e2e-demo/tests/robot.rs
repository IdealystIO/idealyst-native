//! Host-side cross-platform E2E suite for `robot-e2e-demo`, authored in Rust.
//!
//! Each `#[robot_test]` is a real `#[test]` that drives the running app over the
//! Robot relay — the same suite runs on web, macOS, iOS, and Android:
//!
//! ```text
//! idealyst test --macos examples/robot-e2e-demo
//! idealyst test --web   examples/robot-e2e-demo
//! idealyst test --ios   examples/robot-e2e-demo
//! ```
//!
//! `idealyst test` launches the app, stands up the relay, and points these
//! tests at it. Under a bare `cargo test` they skip unless an app is already up;
//! to run them that way, start the app with the external-driver flag set so its
//! in-app demo suite stands down, then test:
//!
//! ```text
//! IDEALYST_TEST_DRIVER=1 idealyst dev --macos examples/robot-e2e-demo   # term 1
//! cargo test -p robot-e2e-demo --test robot -- --test-threads=1         # term 2
//! ```
//!
//! Each test is the sole mutator of the state it asserts, so they're independent
//! of run order (the harness also holds the one shared app for a whole test).

use robot_test::{robot_test, App};

/// find → act → assert a signal: clicking the buttons drives the `count` signal
/// (exposed via `watch_signal` in the app).
#[robot_test]
fn increment_and_decrement_drive_the_count_signal(app: &mut App) {
    app.signal("count").assert_eq(0);
    app.test_id("inc").click();
    app.test_id("inc").click();
    app.signal("count").assert_eq(2);
    app.test_id("dec").click();
    app.signal("count").assert_eq(1);
}

/// An element that only mounts conditionally: hidden until its toggle is pressed.
#[robot_test]
fn secret_panel_toggles_into_existence(app: &mut App) {
    app.test_id("secret").assert_hidden();
    app.test_id("toggle-secret").click();
    app.test_id("secret").assert_visible();
}

/// Typing into the name field updates the live greeting text.
#[robot_test]
fn the_greeting_follows_the_name_field(app: &mut App) {
    app.test_id("greeting").assert_text("Hello, stranger");
    app.test_id("name").type_text("Ada");
    app.test_id("greeting").assert_text("Hello, Ada");
}
