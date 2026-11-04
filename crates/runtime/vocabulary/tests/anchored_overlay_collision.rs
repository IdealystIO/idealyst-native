//! How an anchored overlay's collision insets reach the backend: its own
//! setter, or the inherited default a screen provides so every Select and Menu
//! in it avoids a sticky bar without a prop at each one. The geometry is
//! `runtime_shared::primitives::portal::placement_tests`.

use host_mock::Harness;
use runtime_scene::realize;
use runtime_shared::primitives::portal::AnchorTarget;
use runtime_shared::{EdgeInsets, Ref, ViewHandle};
use runtime_vocabulary::builders::{anchored_overlay, provide_overlay_insets, text};
use runtime_world::{collect_owned, signal};

fn bar(bottom: f32) -> EdgeInsets {
    EdgeInsets { bottom, ..EdgeInsets::ZERO }
}

fn dropdown() -> runtime_vocabulary::builders::AnchoredOverlayBuilder {
    anchored_overlay(AnchorTarget::from(Ref::<ViewHandle>::new())).child(text().content("panel"))
}

/// The insets the mock's `create_portal` saw, for the portals mounted by `f`.
fn mounted_insets(h: &Harness, f: impl FnOnce() -> runtime_scene::Element) -> String {
    let _ = host_mock::take_handle_log();
    let realized = h.world.enter(|| {
        let tree = f();
        realize(&h.backend, &h.registry, tree)
    });
    h.flush();
    drop(realized);
    let log = host_mock::take_handle_log();
    let line = log.iter().find(|l| l.starts_with("anchor")).expect("an anchored portal mounted");
    line.split_once(" insets ").map_or("none".into(), |(_, insets)| insets.to_string())
}

#[test]
fn collision_insets_setter_reaches_the_backend() {
    let h = Harness::new();
    let got = mounted_insets(&h, || dropdown().collision_insets(bar(57.0)).build());
    assert!(got.contains("bottom: 57.0"), "{got}");
}

#[test]
fn an_overlay_with_none_inherits_nothing_by_default() {
    let h = Harness::new();
    assert_eq!(mounted_insets(&h, || dropdown().build()), "none");
}

#[test]
fn provide_overlay_insets_reaches_an_overlay_with_no_prop() {
    let h = Harness::new();
    let got = mounted_insets(&h, || {
        provide_overlay_insets(|| bar(57.0));
        dropdown().build()
    });
    assert!(got.contains("bottom: 57.0"), "{got}");
}

#[test]
fn an_overlays_own_insets_win_over_the_provided_ones() {
    let h = Harness::new();
    let got = mounted_insets(&h, || {
        provide_overlay_insets(|| bar(57.0));
        dropdown().collision_insets(bar(10.0)).build()
    });
    assert!(got.contains("bottom: 10.0"), "{got}");
    let h = Harness::new();
    let opted_out = mounted_insets(&h, || {
        provide_overlay_insets(|| bar(57.0));
        dropdown().collision_insets(EdgeInsets::ZERO).build()
    });
    assert_eq!(opted_out, "none", "ZERO opts out");
}

/// The bar's height is a signal in practice (measured with `on_layout`): each
/// overlay mounts with its current value.
#[test]
fn provided_insets_are_read_when_the_overlay_mounts() {
    let h = Harness::new();
    let height = h.world.enter(|| signal(40.0_f32));
    h.world.enter(|| provide_overlay_insets(move || bar(height.get())));
    h.world.enter(|| height.set(57.0));
    h.flush();
    let got = mounted_insets(&h, || dropdown().build());
    assert!(got.contains("bottom: 57.0"), "{got}");
}

/// A screen's provision dies with the screen: the next screen's overlays are
/// not held clear of a bar that is gone.
#[test]
fn provided_insets_are_retracted_with_their_scope() {
    let h = Harness::new();
    let scope = h.world.enter(|| collect_owned(|| provide_overlay_insets(|| bar(57.0))).1);
    let inside = mounted_insets(&h, || dropdown().build());
    assert!(inside.contains("bottom: 57.0"), "{inside}");
    drop(scope);
    assert_eq!(mounted_insets(&h, || dropdown().build()), "none");
}
