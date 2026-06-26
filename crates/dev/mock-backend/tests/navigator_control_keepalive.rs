//! Regression: a navigator's reactive `nav_state` (`active_route` et al.)
//! must stay alive for the navigator's lifetime even when the SDK handler
//! doesn't retain the `NavigatorHost`.
//!
//! The bug (macOS drawer, "signal used after its scope was dropped"):
//! `active_route` lives in a scope owned by the navigator's `control`
//! (`retain_scope` → `control.owning_scope`). `NavigatorHandler::init`
//! takes the `host` (holding a `control` clone) BY VALUE, and handlers
//! don't store it — so once `init` returns, the only strong ref to
//! `control` can be a transient clone the handler captured. The macOS
//! drawer defers its sidebar build into a `schedule_microtask` whose
//! `control` clone is consumed when the builder closure returns, BEFORE
//! the walker builds the sidebar's reactive styles. A sidebar that reads
//! `active_route` reactively (a nav item's active-highlight) but doesn't
//! itself capture a `control`-bearing closure then dropped `control`'s
//! last ref mid-build, freeing `active_route` out from under the style
//! effect → SIGABRT.
//!
//! The fix anchors `control` in the framework's navigator keepalive
//! (`walker::navigator::build`), alongside the chrome/screen scopes, so
//! nav-state lifetime is deterministic regardless of what a handler keeps.
//!
//! This test reproduces the essential shape: a handler that captures the
//! reactive `active_route` and then drops the host (retaining no
//! `control`). After mount, `active_route` must still be readable. Before
//! the fix this panics ("signal used after its scope was dropped"); after
//! it, the read returns the initial route.

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use drawer_navigator::{
    DrawerBuilder, DrawerHandle, DrawerNavigator, DrawerPresentation, DrawerScreenExt,
};
use mock_backend::MockBackend;
use runtime_core::primitives::navigator::{NavigatorHandler, NavigatorHost, Screen};
use runtime_core::{text, view, Backend, Ref, Route, Signal};

const HOME: Route<()> = Route::<()>::new("home", "/");

/// A minimal handler that mirrors the real-world hazard: it captures the
/// framework's reactive `active_route` handle and then lets `host` (with
/// its `control` clone) drop, storing nothing that keeps `control` alive.
struct DropHostHandler {
    active_route_out: Rc<RefCell<Option<Signal<&'static str>>>>,
}

impl NavigatorHandler<MockBackend> for DropHostHandler {
    fn init(
        &mut self,
        backend: &mut MockBackend,
        host: NavigatorHost<u64>,
        _presentation: Rc<dyn Any>,
    ) -> u64 {
        // Grab the reactive nav-state handle the sidebar would read…
        *self.active_route_out.borrow_mut() = Some(host.nav_state.active_route);
        let node = backend.create_view(&Default::default());
        // …and drop `host` (and its `control` clone) on return, exactly
        // like every real handler. Nothing here retains `control`.
        node
    }

    fn attach_initial(
        &mut self,
        _backend: &mut MockBackend,
        _screen: u64,
        _scope_id: u64,
        _options: Box<dyn Any>,
    ) {
    }
}

#[test]
fn reactive_nav_state_survives_handler_dropping_host() {
    let active_route_out: Rc<RefCell<Option<Signal<&'static str>>>> =
        Rc::new(RefCell::new(None));

    let mut mock = MockBackend::new();
    {
        let cell = active_route_out.clone();
        mock.register_navigator::<DrawerPresentation, _>(move || {
            Box::new(DropHostHandler { active_route_out: cell.clone() })
        });
    }

    let nav: Ref<DrawerHandle> = Ref::new();
    let backend = Rc::new(RefCell::new(mock));
    // Hold the owner so the mounted root scope (and the navigator keepalive
    // effect anchored in it) stays alive while we read `active_route`.
    let _owner = {
        let nav = nav.clone();
        runtime_core::mount(backend, move || {
            DrawerNavigator::new(&HOME)
                .sidebar(view(vec![text("SIDEBAR").into()]).into())
                .screen(HOME, |_| {
                    Screen::new(view(vec![text("HOME").into()])).title("Home")
                })
                .drawer_width(280.0)
                .bind(nav.clone())
                .into()
        })
    };

    // The handler dropped the host; the framework keepalive must keep
    // `control` — and thus `active_route` — alive. Reading it before the
    // fix panics with "signal used after its scope was dropped".
    let active_route = active_route_out
        .borrow()
        .expect("handler captured active_route during init");
    assert_eq!(
        active_route.get(),
        "home",
        "active_route must still be alive (and hold the initial route) after \
         the navigator builds, even though the handler retained no control"
    );
}
