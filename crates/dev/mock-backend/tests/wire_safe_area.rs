//! Safe-area insets over the wire.
//!
//! `.safe_area(sides)` opts a view into platform safe-area padding. The
//! inset value is a CLIENT (device) concern — the dev side / recorder is
//! headless and has no insets. Before the fix, the framework's
//! `attach_safe_area` Effect ran on the dev side (against a stable ZERO
//! signal) and the recorder's `apply_safe_area_padding` was a no-op that
//! emitted nothing, so the opt-in never reached the client and the inset
//! was never applied (the drawer sidebar's top inset, etc.).
//!
//! Now the recorder emits `Command::ApplySafeAreaPadding { node, sides }`
//! (just the opt-in), the SceneModel persists it for late joiners, and
//! `dev-client` resolves it against the CLIENT backend's own device insets
//! — re-applying via a client-side effect whenever those insets change.

use mock_backend::WireHarness;
use runtime_core::{set_safe_area_insets, text, view, EdgeInsets, SafeAreaSides};

#[test]
fn safe_area_opt_in_crosses_wire_and_reapplies_on_inset_change() {
    let mut h = WireHarness::mount(|| {
        view(vec![text("SAFE CONTENT").into()])
            .safe_area(SafeAreaSides::TOP)
            .into()
    });

    // 1. The opt-in crossed the wire and the client backend applied it.
    //    Pre-fix this was `None` — the recorder dropped it on the floor.
    let (sides, count0) = h.scene().safe_area_applied().expect(
        "client must apply the safe-area opt-in received over the wire \
         (pre-fix it never crossed)",
    );
    assert_eq!(
        sides,
        SafeAreaSides::TOP,
        "the opted-in sides must cross the wire verbatim"
    );
    assert!(count0 >= 1, "applied at least once on mount");

    // 2. A device-insets change (rotation, sheet adaptation, dynamic
    //    island) must re-apply. The dev side has no insets, so this is
    //    driven by the CLIENT-side effect subscribing to the device
    //    `safe_area_insets()` signal — the wire analogue of the
    //    framework's per-node `attach_safe_area`.
    set_safe_area_insets(EdgeInsets {
        top: 44.0,
        right: 0.0,
        bottom: 34.0,
        left: 0.0,
    });
    h.tick_and_sync();

    let (_, count1) = h
        .scene()
        .safe_area_applied()
        .expect("safe-area still applied after insets change");
    assert!(
        count1 > count0,
        "client must re-apply safe-area when device insets change \
         (apply count {count0} → {count1})"
    );
}
