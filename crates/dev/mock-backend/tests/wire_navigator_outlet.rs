//! End-to-end OUTLET-MODEL navigator round-trips over the wire.
//!
//! The outlet model needs no kind-specific wire commands: the author
//! layout is recorded like any subtree, and the handler's screen swaps
//! go through `insert_node`/`clear_children` — ordinary node ops the
//! dev-client replays. These tests prove that by driving the REAL
//! `SwapHandler`/`StackHandler` on the runtime-server recorder and
//! replaying into a `MockBackend`:
//!
//! - swap: author chrome + initial screen reconstruct; `Select` swaps
//!   the outlet's child on the far side;
//! - stack-v2: push shows the detail on the far side, pop reveals the
//!   screen below.

use mock_backend::WireHarness;
use runtime_core::primitives::navigator::Screen;
use runtime_core::{text, view, Element, IntoElement, Ref, Route};
use stack_navigator::{StackBuilder, StackHandle, StackNavigator, StackScreenExt};
use swap_navigator::{SwapBuilder, SwapHandle, SwapNavigator};

const HOME: Route<()> = Route::<()>::new("home", "/");
const SETTINGS: Route<()> = Route::<()>::new("settings", "/settings");
const DETAIL: Route<()> = Route::<()>::new("detail", "/detail");

fn swap_app(nav: Ref<SwapHandle>) -> Element {
    SwapNavigator::new(&HOME)
        .screen(HOME, |_| Screen::new(view(vec![text("HOME CONTENT").into()])))
        .screen(SETTINGS, |_| Screen::new(view(vec![text("SETTINGS CONTENT").into()])))
        .layout(|nav| view(vec![nav.outlet, text("TAB BAR").into_element()]).into_element())
        .bind(nav)
        .into()
}

#[test]
fn swap_select_round_trips_through_wire_to_mock() {
    let nav: Ref<SwapHandle> = Ref::new();
    let nav_for_app = nav.clone();
    let mut h = WireHarness::mount_with(
        |rec| swap_navigator::recording::register(rec),
        move || swap_app(nav_for_app),
    );

    {
        let scene = h.scene();
        assert!(scene.contains_text("TAB BAR"), "author chrome replayed:\n{}", scene.dump());
        assert!(scene.contains_text("HOME CONTENT"), "initial screen replayed:\n{}", scene.dump());
        assert!(
            !scene.contains_text("SETTINGS CONTENT"),
            "inactive sibling not mounted:\n{}",
            scene.dump()
        );
        assert!(!scene.contains_text("not registered"), "no fallback placeholder:\n{}", scene.dump());
    }

    // Select on the HOST side — the outlet swap ships as plain node ops.
    nav.get().expect("SwapHandle filled").select(&SETTINGS, ());
    h.tick_and_sync();
    {
        let scene = h.scene();
        assert!(scene.contains_text("SETTINGS CONTENT"), "selected screen replayed:\n{}", scene.dump());
        assert!(!scene.contains_text("HOME CONTENT"), "prior screen removed:\n{}", scene.dump());
        assert!(scene.contains_text("TAB BAR"), "chrome persists:\n{}", scene.dump());
    }
}

fn stack_app(nav: Ref<StackHandle>) -> Element {
    StackNavigator::new(&HOME)
        .screen(HOME, |_| Screen::new(view(vec![text("HOME CONTENT").into()])).title("Home"))
        .screen(DETAIL, |_| Screen::new(view(vec![text("DETAIL CONTENT").into()])).title("Detail"))
        .layout(|nav| {
            view(vec![text("HEADER CHROME").into_element(), nav.outlet]).into_element()
        })
        .bind(nav)
        .into()
}

#[test]
fn stack_v2_push_pop_round_trips_through_wire_to_mock() {
    let nav: Ref<StackHandle> = Ref::new();
    let nav_for_app = nav.clone();
    let mut h = WireHarness::mount_with(
        |rec| stack_navigator::recording::register(rec),
        move || stack_app(nav_for_app),
    );

    {
        let scene = h.scene();
        assert!(scene.contains_text("HEADER CHROME"), "author chrome replayed:\n{}", scene.dump());
        assert!(scene.contains_text("HOME CONTENT"), "initial screen replayed:\n{}", scene.dump());
    }

    nav.get().expect("StackHandle filled").push(&DETAIL, ());
    h.tick_and_sync();
    {
        let scene = h.scene();
        assert!(scene.contains_text("DETAIL CONTENT"), "pushed screen replayed:\n{}", scene.dump());
        assert!(!scene.contains_text("HOME CONTENT"), "covered screen detached:\n{}", scene.dump());
    }

    nav.get().unwrap().pop();
    h.tick_and_sync();
    {
        let scene = h.scene();
        assert!(scene.contains_text("HOME CONTENT"), "pop revealed home:\n{}", scene.dump());
        assert!(!scene.contains_text("DETAIL CONTENT"), "popped screen removed:\n{}", scene.dump());
    }
}
