//! Host-side cross-platform E2E suite for `robot-e2e-demo`, authored in Rust.
//!
//! Each `#[robot_test]` is a real `#[test]` that drives the running app over the
//! Robot relay — the same suite runs on web, macOS, iOS, and Android:
//!
//! ```text
//! idealyst test --macos crates/dev/robot-e2e/examples/robot-e2e-demo
//! idealyst test --web   crates/dev/robot-e2e/examples/robot-e2e-demo
//! idealyst test --ios   crates/dev/robot-e2e/examples/robot-e2e-demo
//! ```
//!
//! `idealyst test` launches the app, stands up the relay, and points these
//! tests at it. Under a bare `cargo test` they skip unless an app is already up;
//! to run them that way, start the app with the external-driver flag set so its
//! in-app demo suite stands down, then test:
//!
//! ```text
//! IDEALYST_TEST_DRIVER=1 idealyst dev --macos crates/dev/robot-e2e/examples/robot-e2e-demo   # term 1
//! cargo test -p robot-e2e-demo --test robot -- --test-threads=1         # term 2
//! ```
//!
//! Each test is the sole mutator of the state it asserts, so they're independent
//! of run order (the harness also holds the one shared app for a whole test).

use robot_test::{json, robot_test, App};

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

/// The app's robot functions set state a test would otherwise click its way
/// to: a sync one answers at once, an async one is polled until done.
#[robot_test]
fn robot_functions_set_the_counter(app: &mut App) {
    let names: Vec<String> = app.functions().into_iter().map(|f| f.name).collect();
    assert!(names.contains(&"demo.set_count".to_string()), "{names:?}");
    assert_eq!(app.call_function("demo.set_count", json!({ "value": 5 })), json!(5));
    app.test_id("counter").assert_text("Counter: 5");
    assert_eq!(app.call_function("demo.set_count_later", json!({ "value": 9, "delay_ms": 300 })), json!(9));
    app.signal("count").assert_eq(9);
    // Leave the counter where the other tests expect it.
    app.call_function("demo.set_count", json!({ "value": 0 }));
    app.signal("count").assert_eq(0);
}

/// A tap through the platform's own input path reaches the button: real
/// events where the backend synthesizes them, UIKit's hit test on iOS.
#[robot_test]
fn a_native_tap_presses_the_button(app: &mut App) {
    app.call_function("demo.set_count", json!({ "value": 0 }));
    let how = app.test_id("inc").tap_native();
    assert!(how == "native" || how == "hit_test", "{how}");
    app.signal("count").assert_eq(1);
    app.call_function("demo.set_count", json!({ "value": 0 }));
    app.signal("count").assert_eq(0);
}

/// Under an overlay's default backdrop, a native tap doesn't reach the
/// button: a backend with a hit test refuses and says what covers it; every
/// backend leaves the count alone.
#[robot_test]
fn an_overlay_stops_a_native_tap(app: &mut App) {
    app.call_function("demo.set_count", json!({ "value": 0 }));
    app.call_function("demo.set_cover", json!({ "on": true }));
    app.test_id("cover-label").assert_visible();
    let id = app.call("find_element", json!({ "test_id": "inc" }))["id"].clone();
    match app.try_call("tap_native", json!({ "element_id": id })) {
        Err(e) => {
            assert!(e.to_string().contains("covers"), "{e}");
            eprintln!("[native tap under the overlay] refused by the hit test: {e}");
        }
        Ok(how) => eprintln!("[native tap under the overlay] injected ({how}); the overlay took it"),
    }
    std::thread::sleep(std::time::Duration::from_millis(300));
    app.signal("count").assert_eq(0);
    app.call_function("demo.set_cover", json!({ "on": false }));
    app.test_id("cover-label").assert_hidden();
}

/// An overlay with no backdrop leaves the page usable: a native tap beside
/// its content reaches the button underneath.
#[robot_test]
fn an_overlay_without_a_backdrop_leaves_the_page_tappable(app: &mut App) {
    app.call_function("demo.set_count", json!({ "value": 0 }));
    app.call_function("demo.set_cover", json!({ "on": true, "backdrop": false }));
    app.test_id("cover-label").assert_visible();
    app.test_id("inc").tap_native();
    app.signal("count").assert_eq(1);
    app.call_function("demo.set_cover", json!({ "on": false }));
    app.test_id("cover-label").assert_hidden();
    app.call_function("demo.set_count", json!({ "value": 0 }));
}
