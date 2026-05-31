//! End-to-end navigator round-trip over the wire.
//!
//! The full chain a navigator app travels under `idealyst dev`'s
//! runtime-server mode:
//!
//! ```text
//! DrawerNavigator (real walker)
//!   → drawer_navigator::recording handler → WireRecordingBackend (emits
//!     CreateDrawerNavigator / DrawerAttachSidebar / NavigatorAttachInitial)
//!   → wire::codec (encode/decode)
//!   → dev_client::WireBackend (reconstructs a persistent sidebar+outlet)
//!   → MockBackend (queryable scene tree)
//! ```
//!
//! Asserts the sidebar + active screen survive the trip and land in the
//! reconstructed tree — i.e. a navigator app no longer renders blank over
//! the wire (Phases 2 + 4 together).

use drawer_navigator::{DrawerBuilder, DrawerNavigator, DrawerScreenExt};
use mock_backend::WireHarness;
use runtime_core::primitives::navigator::Screen;
use runtime_core::{text, view, Element, Route};

const HOME: Route<()> = Route::<()>::new("home", "/");
const ABOUT: Route<()> = Route::<()>::new("about", "/about");

fn drawer_app() -> Element {
    DrawerNavigator::new(&HOME)
        .sidebar(view(vec![text("SIDEBAR NAV").into()]).into())
        .screen(HOME, |_| {
            Screen::new(view(vec![text("HOME BODY").into()])).title("Home")
        })
        .screen(ABOUT, |_| {
            Screen::new(view(vec![text("ABOUT BODY").into()])).title("About")
        })
        .drawer_width(280.0)
        .into()
}

#[test]
fn drawer_navigator_round_trips_through_wire_to_mock() {
    let h = WireHarness::mount_with(
        |rec| drawer_navigator::recording::register(rec),
        drawer_app,
    );
    let scene = h.scene();

    // Sidebar chrome + the initial (home) screen both reconstructed.
    assert!(
        scene.contains_text("SIDEBAR NAV"),
        "sidebar should reconstruct on the client:\n{}",
        scene.dump()
    );
    assert!(
        scene.contains_text("HOME BODY"),
        "initial screen should reconstruct on the client:\n{}",
        scene.dump()
    );

    // The inactive (lazy) screen is not mounted.
    assert!(
        !scene.contains_text("ABOUT BODY"),
        "inactive screen must not mount eagerly:\n{}",
        scene.dump()
    );

    // And NOT the Phase-1 graceful-fallback placeholder — proving the
    // recorder dispatched to the recording handler, not the text stub.
    assert!(
        !scene.contains_text("not registered"),
        "navigator must not fall back to the placeholder text node:\n{}",
        scene.dump()
    );
}
