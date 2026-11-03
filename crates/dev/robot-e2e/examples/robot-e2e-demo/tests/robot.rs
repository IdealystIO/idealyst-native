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

use robot_test::{json, robot_test, App, Platform};

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

/// The last tab of a strip wider than the window: `click()` scrolls the
/// strip (and the page) until the tab is in view, then taps it.
#[robot_test]
fn click_scrolls_an_off_screen_tab_into_view(app: &mut App) {
    app.call_function("demo.set_count", json!({ "value": 0 }));
    let tab = app.call("find_element", json!({ "test_id": "tab-last" }));
    assert_eq!(tab["in_viewport"], json!(false), "the last tab starts off screen: {tab}");
    app.test_id("tab-last").click();
    app.signal("count").assert_eq(100);
    app.test_id("tab-last").assert_in_viewport();
    app.test_id("tabs").scroll_to(0.0, 0.0);
    app.call_function("demo.set_count", json!({ "value": 0 }));
    app.signal("count").assert_eq(0);
}

/// Declared for the platforms whose robot reports one: elsewhere it is
/// filtered out or ignored, never passed. `platform()` agrees with the
/// declaration wherever it runs.
#[robot_test(platforms = [web, macos])]
fn the_app_reports_the_platform_the_test_declared(app: &mut App) {
    let p = app.platform();
    assert!(p == Platform::Web || p == Platform::Macos, "{p}");
}

/// The mobile half of the declaration: run on iOS and Android, filtered
/// out (never passed) on web and the desktops.
#[robot_test(platforms = mobile)]
fn a_phone_app_reports_a_mobile_platform(app: &mut App) {
    assert!(app.platform().is_mobile(), "{}", app.platform());
}

/// Keys pressed into a focused field edit it as a keyboard does — a capital
/// carries Shift, Backspace deletes — and Enter reaches the field's own
/// `on_key_down`, which submits the text.
#[robot_test]
fn pressed_keys_edit_a_field_and_enter_submits_it(app: &mut App) {
    app.test_id("note").fill("");
    for key in ["H", "i", "x"] {
        app.test_id("note").press_key(key);
    }
    app.signal("note").assert_eq("\"Hix\"");
    app.press_key("Backspace");
    app.signal("note").assert_eq("\"Hi\"");
    app.test_id("note").press_key("Enter");
    app.signal("submitted").assert_eq("\"Hi\"");
    // The handler took the Enter: a single-line field's text is unchanged.
    app.signal("note").assert_eq("\"Hi\"");
}

/// Tab moves keyboard focus to the next field and Shift+Tab back, through
/// the platform's own focus traversal.
#[robot_test]
fn tab_moves_focus_between_fields(app: &mut App) {
    app.test_id("tab-first").focus();
    app.test_id("tab-first").assert_focused(true);
    app.press_key("Tab");
    app.test_id("tab-second").assert_focused(true);
    app.test_id("tab-first").assert_focused(false);
    app.press_key("Shift+Tab");
    app.test_id("tab-first").assert_focused(true);
    app.test_id("tab-second").assert_focused(false);
}

/// `view(on_focus = …)` hears keyboard focus reaching a field inside it
/// (through each platform's own Tab traversal) and leaving it.
#[robot_test]
fn tab_into_and_out_of_a_view_reports_focus_within(app: &mut App) {
    app.test_id("focus-outer").focus();
    app.signal("focus_within").assert_eq("false");
    app.press_key("Tab");
    app.test_id("focus-inner").assert_focused(true);
    app.signal("focus_within").assert_eq("true");
    // Out again by moving focus to the field before it.
    app.test_id("focus-outer").focus();
    app.test_id("focus-outer").assert_focused(true);
    app.signal("focus_within").assert_eq("false");
}

/// Focusing a field reports it focused from the first focus after load, and
/// blurring it reports the blur. (On web, a headless browser window that
/// is not the OS's focused window delivers no `focus` event for the first
/// `element.focus()`; the state must follow the document's active element.)
#[robot_test]
fn focusing_a_field_reports_it_focused(app: &mut App) {
    // The field's own `on_focus` fires too, once per change, and its
    // `on_blur` once per blur (it lets the blur through), on every
    // platform.
    app.signal("name_focused").assert_eq(false);
    let changes: i32 = app.signal("name_focus_changes").value().parse().expect("an i32 signal");
    let blurs: i32 = app.signal("name_blurs").value().parse().expect("an i32 signal");
    app.test_id("name").focus();
    app.test_id("name").assert_focused(true);
    app.signal("name_focused").assert_eq(true);
    app.signal("name_focus_changes").assert_eq(changes + 1);
    app.signal("name_blurs").assert_eq(blurs);
    app.test_id("name").blur();
    app.test_id("name").assert_focused(false);
    app.signal("name_focused").assert_eq(false);
    app.signal("name_focus_changes").assert_eq(changes + 2);
    app.signal("name_blurs").assert_eq(blurs + 1);
}

/// Escape closes an overlay that has an `on_dismiss`, as a user's Escape
/// does on every platform.
#[robot_test]
fn escape_closes_an_overlay_with_on_dismiss(app: &mut App) {
    app.call_function("demo.set_cover", json!({ "on": true }));
    app.test_id("cover-label").assert_visible();
    app.press_key("Escape");
    app.test_id("cover-label").assert_hidden();
}

/// Escape closes a modal while a field inside it holds focus — the key goes
/// to the field first and, unconsumed, closes the topmost overlay. (The
/// typed key proves the field holds keyboard focus.)
#[robot_test]
fn escape_closes_a_modal_while_its_field_has_focus(app: &mut App) {
    app.call_function("demo.set_dialog", json!({ "open": true }));
    app.test_id("dialog-field").press_key("a");
    app.test_id("dialog-field").assert_value("a");
    app.test_id("dialog-field").assert_focused(true);
    app.press_key("Escape");
    app.test_id("dialog-field").assert_hidden();
}
