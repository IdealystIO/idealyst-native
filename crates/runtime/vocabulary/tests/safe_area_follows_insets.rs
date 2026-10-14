//! A `safe_area` container re-applies its padding when the platform's
//! safe-area insets change.
//!
//! The bug (CrewForge kiosk, iPad): the view handler applied the padding
//! ONCE at mount. A native host mounts before its root view is in a
//! window, when the platform reports zero insets; the real ones arrive
//! afterwards. So `view(safe_area = ALL)` and
//! `keyboard_avoiding_view(safe_area = ALL)` padded by nothing for the
//! life of the app and the header sat under the status bar. The backend's
//! apply reads the platform insets itself, so what these tests pin is the
//! re-apply: one more `apply_safe_area_padding` per inset change, none for
//! a repeated report, none after the node is gone. The live UIKit half
//! (insets landing after mount and the padding following them) is the
//! `crates/dev/newcore-ios-smoke` self-test's `check_safe_area`.

use host_mock::Harness;
use runtime_scene::realize;
use runtime_shared::{EdgeInsets, SafeAreaSides};
use runtime_vocabulary::builders::{keyboard_avoiding_view, scroll_view, text, view};
use runtime_vocabulary::safe_area;

fn harness() -> Harness {
    let h = Harness::new();
    h.record_all();
    h
}

fn applies(h: &Harness, op: &str) -> usize {
    h.take_log().iter().filter(|l| l.starts_with(op)).count()
}

fn top(t: f32) -> EdgeInsets {
    EdgeInsets { top: t, ..EdgeInsets::ZERO }
}

#[test]
fn regression_safe_area_padding_reapplies_when_insets_arrive_after_mount() {
    let h = harness();
    let realized = h.world.enter(|| {
        realize(
            &h.backend,
            &h.registry,
            view()
                .safe_area(SafeAreaSides::ALL)
                .child(
                    keyboard_avoiding_view()
                        .safe_area(SafeAreaSides::BOTTOM)
                        .child(text().content("form")),
                )
                .build(),
        )
    });
    h.world.flush();
    assert_eq!(
        applies(&h, "apply_safe_area_padding"),
        2,
        "both containers apply once at mount"
    );

    // The host view joined the window: the status bar's inset arrives.
    safe_area::push(top(20.0));
    h.world.flush();
    assert_eq!(
        applies(&h, "apply_safe_area_padding"),
        2,
        "both containers re-apply against the real insets (pre-fix: 0)"
    );

    safe_area::push(top(20.0));
    h.world.flush();
    assert_eq!(applies(&h, "apply_safe_area_padding"), 0, "a repeated report is silent");

    // Gone with its scope: the follow effect must not outlive the node.
    drop(realized);
    h.take_log();
    safe_area::push(top(24.0));
    h.world.flush();
    assert_eq!(
        applies(&h, "apply_safe_area_padding"),
        0,
        "a released container no longer re-applies"
    );
}

#[test]
fn scroll_view_safe_area_follows_insets_but_an_opt_out_does_not() {
    let h = harness();
    let _realized = h.world.enter(|| {
        realize(
            &h.backend,
            &h.registry,
            view()
                .child(scroll_view().safe_area(SafeAreaSides::TOP).child(text().content("a")))
                .child(scroll_view().safe_area(SafeAreaSides::NONE).child(text().content("b")))
                .build(),
        )
    });
    h.world.flush();
    // The mock keeps the trait default: the scroll inset falls back to
    // `apply_safe_area_padding`. Both apply at mount — the opt-out must
    // still reach the backend to clear a platform default.
    assert_eq!(applies(&h, "apply_safe_area_padding"), 2);

    safe_area::push(top(47.0));
    h.world.flush();
    assert_eq!(
        applies(&h, "apply_safe_area_padding"),
        1,
        "only the opted-in scroll view follows the insets"
    );
}
