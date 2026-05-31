//! End-to-end stack-navigator round-trip over the wire.
//!
//! The stack analogue of `wire_navigator.rs`: a real `Navigator` (stack)
//! travels recorder → wire::codec → `WireBackend<MockBackend>`, and the
//! reconstructed tree shows the top-of-stack screen. Exercises
//! Push/Pop reconstruction (Phase 7) on top of the recording handler.

use mock_backend::WireHarness;
use runtime_core::primitives::navigator::Screen;
use runtime_core::{text, view, Element, Ref, Route};
use stack_navigator::{Navigator, StackBuilder, StackHandle, StackScreenExt};

const HOME: Route<()> = Route::<()>::new("home", "/");
const DETAIL: Route<()> = Route::<()>::new("detail", "/detail");

fn stack_app(nav: Ref<StackHandle>) -> Element {
    Navigator::new(&HOME)
        .screen(HOME, |_| {
            Screen::new(view(vec![text("HOME SCREEN").into()])).title("Home")
        })
        .screen(DETAIL, |_| {
            Screen::new(view(vec![text("DETAIL SCREEN").into()])).title("Detail")
        })
        .bind(nav)
        .into()
}

#[test]
fn stack_navigator_push_pop_round_trips_through_wire_to_mock() {
    let nav: Ref<StackHandle> = Ref::new();
    let nav_for_app = nav.clone();
    let mut h = WireHarness::mount_with(
        |rec| stack_navigator::recording::register(rec),
        move || stack_app(nav_for_app),
    );

    // Initial: home screen reconstructed, not detail; the navigator node
    // is a plain CreateNavigator (not the Phase-1 fallback text).
    {
        let scene = h.scene();
        assert!(scene.contains_text("HOME SCREEN"), "initial screen:\n{}", scene.dump());
        assert!(!scene.contains_text("DETAIL SCREEN"), "detail not yet pushed:\n{}", scene.dump());
        assert!(!scene.contains_text("not registered"), "no fallback placeholder:\n{}", scene.dump());
    }

    // Push detail → top of stack is now detail.
    let handle = nav.get().expect("StackHandle filled after render");
    handle.push(&DETAIL, ());
    h.tick_and_sync();
    {
        let scene = h.scene();
        assert!(scene.contains_text("DETAIL SCREEN"), "pushed screen visible:\n{}", scene.dump());
    }

    // Pop → back to home (home node was retained, re-shown).
    handle.pop();
    h.tick_and_sync();
    {
        let scene = h.scene();
        assert!(scene.contains_text("HOME SCREEN"), "popped back to home:\n{}", scene.dump());
    }
}
