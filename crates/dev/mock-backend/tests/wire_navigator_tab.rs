//! End-to-end tab-navigator round-trip over the wire.
//!
//! A real `TabNavigator` travels recorder → wire::codec →
//! `WireBackend<MockBackend>`. The reconstructed tree shows the tab bar
//! (built from the registration labels, which ride on the wire as data)
//! plus the active tab's screen; selecting another tab swaps the screen.

use mock_backend::WireHarness;
use runtime_core::primitives::navigator::Screen;
use runtime_core::{text, view, Element, Ref, Route};
use tab_navigator::{TabNavigator, TabSpec, TabsBuilder, TabsHandle};

const HOME: Route<()> = Route::<()>::new("home", "/");
const SETTINGS: Route<()> = Route::<()>::new("settings", "/settings");

fn tab_app(nav: Ref<TabsHandle>) -> Element {
    TabNavigator::new(&HOME)
        .tab(HOME, TabSpec::new("HomeTab"), |_| {
            Screen::new(view(vec![text("HOME CONTENT").into()]))
        })
        .tab(SETTINGS, TabSpec::new("SettingsTab"), |_| {
            Screen::new(view(vec![text("SETTINGS CONTENT").into()]))
        })
        .bind(nav)
        .into()
}

#[test]
fn tab_navigator_select_round_trips_through_wire_to_mock() {
    let nav: Ref<TabsHandle> = Ref::new();
    let nav_for_app = nav.clone();
    let mut h = WireHarness::mount_with(
        |rec| tab_navigator::recording::register(rec),
        move || tab_app(nav_for_app),
    );

    {
        let scene = h.scene();
        // Tab bar reconstructed from registration labels.
        assert!(scene.contains_text("HomeTab"), "home tab label:\n{}", scene.dump());
        assert!(scene.contains_text("SettingsTab"), "settings tab label:\n{}", scene.dump());
        // Initial tab's screen content present; the other tab's isn't.
        assert!(scene.contains_text("HOME CONTENT"), "initial tab screen:\n{}", scene.dump());
        assert!(!scene.contains_text("SETTINGS CONTENT"), "inactive tab not mounted:\n{}", scene.dump());
        assert!(!scene.contains_text("not registered"), "no fallback placeholder:\n{}", scene.dump());
    }

    // Select the settings tab → its screen swaps into the outlet.
    let handle = nav.get().expect("TabsHandle filled after render");
    handle.select(&SETTINGS, ());
    h.tick_and_sync();
    {
        let scene = h.scene();
        assert!(scene.contains_text("SETTINGS CONTENT"), "selected tab screen:\n{}", scene.dump());
        // Tab bar still present after the swap.
        assert!(scene.contains_text("SettingsTab"), "tab bar persists:\n{}", scene.dump());
    }
}
