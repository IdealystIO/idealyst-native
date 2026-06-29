//! Regression: a portal (modal / popover / tooltip / click-away catcher)
//! opened on a navigator screen must HIDE when that screen stops being the
//! active route, and SHOW again on return — WITHOUT being torn down.
//!
//! Why: a portal escapes its screen's view tree to mount on the window, so the
//! navigator swapping screens doesn't detach it; and with a persistent
//! `MountPolicy` the screen's scope (hence the portal) stays alive. Before this
//! fix, an overlay opened on screen A kept floating over screen B after
//! navigation. The fix: `mount_screen` `provide`s a `ScreenNav` into each
//! screen's scope, and the portal build path installs an `Effect` that calls
//! `Backend::set_portal_hidden(node, active_route != my_route)`.
//!
//! This drives that mechanism directly: render INSIDE the root scope (standing
//! in for a screen scope) where we `provide(ScreenNav { active_route, route })`,
//! build a portal, then flip `active_route` and assert the backend was told to
//! hide / show.

#[path = "common/mock_backend.rs"]
mod mock_backend;
#[path = "common/runtime.rs"]
mod runtime;

use mock_backend::{Event, MockBackendConfig};
use runtime::TestRuntime;
use runtime_core::primitives::navigator::ScreenNav;
use runtime_core::{
    portal, provide, signal, text, IntoElement, PortalTarget, Signal, ViewportPlacement,
};

fn last_hidden(events: &[Event]) -> Option<bool> {
    events.iter().rev().find_map(|e| match e {
        Event::SetPortalHidden { hidden, .. } => Some(*hidden),
        _ => None,
    })
}

#[test]
fn portal_hides_when_its_screen_is_not_the_active_route() {
    let rt = TestRuntime::with_config(MockBackendConfig::default());
    let active: Signal<&'static str> = signal!("A");

    // Stand in for a navigator screen "A": provide its nav context, then build
    // a portal in the same scope (as a screen's overlay would).
    let _owner = rt.render_with(move || {
        provide(ScreenNav { active_route: active, route: "A" });
        portal(
            PortalTarget::Viewport(ViewportPlacement::Center),
            vec![text("overlay".to_string()).into_element()],
        )
        .into_element()
    });

    // On the active screen → the portal is shown (not hidden).
    assert_eq!(
        last_hidden(&rt.events()),
        Some(false),
        "portal on the active route must be visible"
    );

    // Navigate away (a different route becomes active) → the portal hides,
    // even though its scope stays alive (no ReleasePortal).
    rt.backend_mut().clear_events();
    active.set("B");
    assert_eq!(
        last_hidden(&rt.events()),
        Some(true),
        "portal must hide when its owning screen is no longer the active route"
    );
    assert!(
        !rt.events().iter().any(|e| matches!(e, Event::ReleasePortal { .. })),
        "the portal is hidden, NOT released — its screen scope is still alive"
    );

    // Navigate back → it shows again.
    rt.backend_mut().clear_events();
    active.set("A");
    assert_eq!(
        last_hidden(&rt.events()),
        Some(false),
        "portal must reappear when its screen becomes active again"
    );
}

#[test]
fn portal_outside_any_navigator_is_never_hidden() {
    // No `ScreenNav` provided (a portal at app root, not under a navigator):
    // there's no active-route to track, so the visibility effect must not run.
    let rt = TestRuntime::with_config(MockBackendConfig::default());
    let _owner = rt.render(
        portal(
            PortalTarget::Viewport(ViewportPlacement::Center),
            vec![text("overlay".to_string()).into_element()],
        )
        .into_element(),
    );
    assert!(
        !rt.events().iter().any(|e| matches!(e, Event::SetPortalHidden { .. })),
        "a portal with no ScreenNav context must never be toggled hidden"
    );
}
