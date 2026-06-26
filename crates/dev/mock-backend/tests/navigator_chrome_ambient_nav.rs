//! Regression: chrome built via `NavigatorHost::build_node_scoped` (a drawer
//! sidebar / header) must see THIS navigator as the ambient one, so
//! `link(route = …)` elements in a sidebar can dispatch navigation.
//!
//! The bug: `build_node_scoped` ran its builder inside a fresh scope +
//! identity but did NOT push `AmbientNavGuard`, unlike `mount_screen` for
//! screen content. A `link`'s `on_activate` snapshots `ambient_navigator()`
//! as the walker builds it; with no ambient navigator it captures `None` and
//! `make_on_activate` no-ops. The symptom (every native backend): tapping a
//! drawer-sidebar nav link registers — Android even plays the link view's
//! click sound — but no navigation happens.
//!
//! The fix pushes `AmbientNavGuard(control)` for the duration of the chrome
//! build. This test drives `build_node_scoped` and asserts the navigator is
//! ambient inside the builder (it was `None` before the fix).

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use drawer_navigator::{
    DrawerBuilder, DrawerHandle, DrawerNavigator, DrawerPresentation, DrawerScreenExt,
};
use mock_backend::MockBackend;
use runtime_core::primitives::navigator::{ambient_navigator, NavigatorHandler, NavigatorHost, Screen};
use runtime_core::{view, Backend, Element, IntoElement, Ref, Route};

const HOME: Route<()> = Route::<()>::new("home", "/");

type BuildNodeScoped = Rc<dyn Fn(Box<dyn FnOnce() -> Element>) -> u64>;

/// Captures `host.build_node_scoped` so the test can drive a chrome build
/// AFTER mount (outside the init backend borrow, like the real handlers'
/// deferred sidebar microtask).
struct CaptureHandler {
    out: Rc<RefCell<Option<BuildNodeScoped>>>,
}

impl NavigatorHandler<MockBackend> for CaptureHandler {
    fn init(
        &mut self,
        backend: &mut MockBackend,
        host: NavigatorHost<u64>,
        _presentation: Rc<dyn Any>,
    ) -> u64 {
        *self.out.borrow_mut() = Some(host.build_node_scoped.clone());
        backend.create_view(&Default::default())
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
fn chrome_build_node_scoped_publishes_ambient_navigator() {
    let out: Rc<RefCell<Option<BuildNodeScoped>>> = Rc::new(RefCell::new(None));

    let mut mock = MockBackend::new();
    {
        let out = out.clone();
        mock.register_navigator::<DrawerPresentation, _>(move || {
            Box::new(CaptureHandler { out: out.clone() })
        });
    }

    let nav: Ref<DrawerHandle> = Ref::new();
    let backend = Rc::new(RefCell::new(mock));
    let _owner = {
        let nav = nav.clone();
        runtime_core::mount(backend, move || {
            DrawerNavigator::new(&HOME)
                .sidebar(view(vec![]).into())
                .screen(HOME, |_| Screen::new(view(vec![])).title("Home"))
                .drawer_width(280.0)
                .bind(nav.clone())
                .into()
        })
    };

    // Drive a chrome build (mirrors a sidebar slot build) and record whether
    // the navigator is ambient inside it — exactly where a `link`'s
    // `on_activate` would snapshot it.
    let build_node_scoped = out.borrow().clone().expect("handler captured build_node_scoped");
    let saw_ambient: Rc<RefCell<Option<bool>>> = Rc::new(RefCell::new(None));
    {
        let saw = saw_ambient.clone();
        let _node = build_node_scoped(Box::new(move || {
            *saw.borrow_mut() = Some(ambient_navigator().is_some());
            view(vec![]).into_element()
        }));
    }

    assert_eq!(
        *saw_ambient.borrow(),
        Some(true),
        "the navigator must be ambient while chrome builds, so sidebar links \
         can dispatch (was None before the fix → links silently no-op)"
    );
}
