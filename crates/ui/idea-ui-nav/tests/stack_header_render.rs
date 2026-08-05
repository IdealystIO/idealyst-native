//! `StackHeader` render coverage: it draws the active screen's title/slots from
//! `StackHeaderState`, shows the back arrow when wired + enabled, and
//! self-suppresses when the state is `native` (a native bar owns the header) or
//! `hidden`.
//!
//! Mounts on `host_mock::Harness` — the recording scene `Host` + capability
//! mock — and reduces the recorded op log to a "rendered text dump" string the
//! assertions grep.

use idea_theme::theme::{install_idea_theme, light_theme};
use idea_ui_nav::{StackHeader, StackHeaderProps};
use runtime_core::primitives::navigator::{HeaderButton, StackHeaderState};

use runtime_core::Reactive;

fn header(title: &str) -> StackHeaderState {
    StackHeaderState { title: title.to_string(), ..Default::default() }
}

/// Mount the header and return a text dump of everything rendered.
fn rendered(state: StackHeaderState, show_back: bool) -> String {
    use std::rc::Rc;

    let h = host_mock::Harness::new();
    let on_back: Rc<dyn Fn()> = Rc::new(|| {});
    // The theme installs per WORLD, so it runs inside each mount's `enter`
    // (idempotent — same tokens each time).
    let root = h.world.enter(|| {
        install_idea_theme(light_theme());
        StackHeader(StackHeaderProps {
            state: Reactive::Static(Some(state.clone())),
            show_back: Reactive::Static(show_back),
            on_back: Some(on_back.clone()),
        })
    });
    let _realized = h.mount(root);
    h.flush();
    h.take_log().join("\n")
}

fn contains(dump: &str, needle: &str) -> bool {
    dump.contains(needle)
}

#[test]
fn draws_title_and_back_when_enabled() {
    let d = rendered(header("Detail"), true);
    assert!(contains(&d, "Detail"), "title renders:\n{d}");
    assert!(contains(&d, "‹"), "back chevron shows when wired + enabled:\n{d}");
}

#[test]
fn hides_back_at_root() {
    let d = rendered(header("Home"), false);
    assert!(contains(&d, "Home"), "title renders:\n{d}");
    assert!(!contains(&d, "‹"), "no back chevron at root (show_back=false):\n{d}");
}

#[test]
fn renders_left_and_right_slots() {
    let state = StackHeaderState {
        title: "Item".to_string(),
        left: Some(HeaderButton::text("Cancel")),
        right: Some(HeaderButton::text("Save")),
        ..Default::default()
    };
    let d = rendered(state, false);
    assert!(contains(&d, "Cancel"), "left slot renders:\n{d}");
    assert!(contains(&d, "Item"), "title renders:\n{d}");
    assert!(contains(&d, "Save"), "right slot renders:\n{d}");
}

#[test]
fn self_suppresses_when_native() {
    // native = true → a native bar owns the header, so the drawn header is empty.
    let state = StackHeaderState { title: "Native".to_string(), native: true, ..Default::default() };
    let d = rendered(state, true);
    assert!(!contains(&d, "Native"), "native header self-suppresses:\n{d}");
    assert!(!contains(&d, "‹"), "no back arrow when native:\n{d}");
}

#[test]
fn suppresses_when_hidden() {
    let state = StackHeaderState { title: "Hidden".to_string(), hidden: true, ..Default::default() };
    let d = rendered(state, false);
    assert!(!contains(&d, "Hidden"), "hidden header renders nothing:\n{d}");
}
