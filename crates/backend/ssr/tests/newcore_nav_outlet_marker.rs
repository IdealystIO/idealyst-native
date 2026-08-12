//! REGRESSION — the SSG hydration remount cascade (every screen subtree
//! of a navigator app warned `[hydrate] SSR/client diverge` and
//! remounted instead of adopting).
//!
//! The navigator handlers realize the initial SCREEN before the author
//! layout builds the outlet (screens must resolve the launch URL before
//! chrome), but the emitted document nests the screen INSIDE the
//! outlet. Hydration adopts in `create_*` order, so the client's
//! out-of-document-order screen build consumed the outlet's node and
//! shifted the whole subtree one level — every view adopted its
//! parent's node until the first `<span>`-vs-`<div>` tag break
//! remounted, repeating across the page.
//!
//! The fix stamps `data-iy-nav-outlet="<base>"` on every outlet
//! (`LifecycleOps::annotate_nav_outlet`, SSR side) so the hydrating
//! client can steer its adoption cursor into the outlet for the screen
//! build (`hydrate_nav_screen_begin`/`_end`, web side). These tests pin
//! the SSR half: the marker is present, carries the navigator's base,
//! and wraps the screen content.

use runtime_shared::primitives::navigator::{Route, NAV_OUTLET_HYDRATION_ATTR};
use runtime_vocabulary::builders::{stack_navigator, swap_navigator, text, view};

static HOME: Route<()> = Route::new("home", "/");

/// Stack navigator, bare-outlet layout: the outlet carries the marker
/// with the root base (`""`), and the screen renders inside it.
#[test]
fn regression_ssg_stack_outlet_carries_hydration_marker() {
    let page = backend_ssr::newcore::render_path("/", || {
        stack_navigator(&HOME)
            .screen(HOME.clone(), |()| {
                view().children(vec![text().content("screen-content").build()]).build()
            })
            .build()
    });

    let marker = format!("{NAV_OUTLET_HYDRATION_ATTR}=\"\"");
    let marker_at = page.html.find(&marker).unwrap_or_else(|| {
        panic!(
            "SSR must stamp the outlet with {marker} — without it the hydrating \
             client's out-of-order screen build consumes the outlet node and every \
             screen subtree takes the divergence-remount path. html: {}",
            page.html
        )
    });
    let content_at = page
        .html
        .find("screen-content")
        .expect("screen content rendered");
    assert!(
        marker_at < content_at,
        "the screen must render INSIDE the marked outlet (marker before content); \
         the cursor steering enters the marked outlet's first child. html: {}",
        page.html
    );
}

/// Swap navigator: same contract through the other navigator kind.
#[test]
fn regression_ssg_swap_outlet_carries_hydration_marker() {
    let page = backend_ssr::newcore::render_path("/", || {
        swap_navigator(&HOME)
            .screen(HOME.clone(), |()| {
                view().children(vec![text().content("swap-screen").build()]).build()
            })
            .build()
    });

    let marker = format!("{NAV_OUTLET_HYDRATION_ATTR}=\"\"");
    assert!(
        page.html.contains(&marker),
        "swap navigator outlets need the hydration marker too. html: {}",
        page.html
    );
    let marker_at = page.html.find(&marker).expect("checked above");
    let content_at = page.html.find("swap-screen").expect("screen rendered");
    assert!(marker_at < content_at, "screen nests inside the marked outlet");
}
