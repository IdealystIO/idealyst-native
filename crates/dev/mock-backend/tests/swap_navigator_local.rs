//! Local-mount coverage for the backend-neutral swap-navigator handler.
//!
//! The swap navigator inverts chrome ownership: the author `.layout(|nav| …)`
//! closure owns the tree and splats `{nav.outlet}`; the handler swaps the
//! active screen INTO that outlet on `Select`. These tests run the real
//! `SwapHandler` directly on `MockBackend` (no wire / recording path) and
//! assert:
//!
//! 1. the author layout mounts with the initial screen inside the outlet,
//!    and the surrounding chrome (a "bar") persists across a `Select` while
//!    only the outlet's screen swaps — the co-equal, depth-less semantics
//!    that distinguish *swap* from *stack*;
//! 2. a `Link` inside a swap screen activates as `Select`, not the default
//!    `Push` — the exact regression that made the old tab navigator panic on
//!    web (its `install_select_link_activator` was dead code).
//!
//! The handler defers its author-layout build to a microtask (it re-borrows
//! the backend, so it can't run inside the `init` borrow). A buffering test
//! scheduler captures that microtask so the test can drain it deterministically
//! — mirroring how the web/macOS backends buffer + drain.

use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::OnceLock;

use mock_backend::MockBackend;
use runtime_core::primitives::navigator::{set_initial_path, NavCommand, RouteParams, Screen};
use runtime_core::scheduling::{install_scheduler, ScheduleHandle, Scheduler};
use runtime_core::{
    text, view, Element, IntoElement, Length, Ref, Route, StyleApplication, StyleRules, StyleSheet,
};
use swap_navigator::{MountPolicy, SwapBuilder, SwapHandle, SwapHandler, SwapPresentation};

const HOME: Route<()> = Route::<()>::new("home", "/");
const SETTINGS: Route<()> = Route::<()>::new("settings", "/settings");
const PROFILE: Route<()> = Route::<()>::new("profile", "/profile");

// ---------------------------------------------------------------------------
// Buffering test scheduler — captures microtasks so the deferred author-layout
// build runs only when the test drains, not synchronously inside `init`.
// ---------------------------------------------------------------------------

thread_local! {
    static MICROTASKS: RefCell<Vec<Box<dyn FnOnce() + 'static>>> = const { RefCell::new(Vec::new()) };
}

struct BufferingScheduler;
// SAFETY: storage is thread-local; the value never crosses threads (single
// test thread). The bounds exist for `OnceLock`/web storage. Same posture as
// the `TestScheduler` in `runtime-core/tests/scheduling_scoped.rs`.
unsafe impl Send for BufferingScheduler {}
unsafe impl Sync for BufferingScheduler {}

struct InertHandle;
impl ScheduleHandle for InertHandle {
    fn cancel(&mut self) {}
}

impl Scheduler for BufferingScheduler {
    fn schedule_microtask(&self, f: Box<dyn FnOnce() + 'static>) {
        MICROTASKS.with(|q| q.borrow_mut().push(f));
    }
    // Timer / animation-frame callbacks are DROPPED (never fired). The swap
    // handler only uses `schedule_microtask`; funneling self-rescheduling raf
    // loops (which the reactive/layout system installs) into the drain queue
    // would make `drain_buffered_microtasks` loop forever.
    fn after_animation_frame(&self, _f: Box<dyn FnOnce() + 'static>) -> Box<dyn ScheduleHandle> {
        Box::new(InertHandle)
    }
    fn after_ms(&self, _delay: i32, _f: Box<dyn FnOnce() + 'static>) -> Box<dyn ScheduleHandle> {
        Box::new(InertHandle)
    }
    fn raf_loop(&self, _f: Box<dyn FnMut() + 'static>) -> Box<dyn ScheduleHandle> {
        Box::new(InertHandle)
    }
    fn drain_buffered_microtasks(&self) {
        // Drain to empty, running tasks that themselves enqueue more. Bounded
        // so a runaway self-reschedule surfaces as a test failure, not a hang.
        for _ in 0..1000 {
            let batch: Vec<_> = MICROTASKS.with(|q| std::mem::take(&mut *q.borrow_mut()));
            if batch.is_empty() {
                return;
            }
            for f in batch {
                f();
            }
        }
        panic!("BufferingScheduler: microtask queue never drained (runaway reschedule?)");
    }
}

fn install_buffering_scheduler() {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    INSTALLED.get_or_init(|| install_scheduler(Box::new(BufferingScheduler)));
}

/// Mount a two-screen swap navigator whose author layout wraps `{nav.outlet}`
/// with a persistent "BAR". Returns the mount owner (drop = unmount) — hold it
/// for the test's duration.
fn mount_swap(backend: &Rc<RefCell<MockBackend>>, nav: Ref<SwapHandle>) -> Box<dyn Any> {
    let nav_for_app = nav;
    Box::new(runtime_core::mount(backend.clone(), move || {
        SwapNavigatorApp(nav_for_app.clone())
    }))
}

// A tiny wrapper so the closure returns something `IntoElement`.
#[allow(non_snake_case)]
fn SwapNavigatorApp(nav: Ref<SwapHandle>) -> Element {
    use swap_navigator::SwapNavigator;
    SwapNavigator::new(&HOME)
        .screen(HOME, |_| Screen::new(view(vec![text("HOME CONTENT").into()])))
        .screen(SETTINGS, |_| Screen::new(view(vec![text("SETTINGS CONTENT").into()])))
        // Author layout: the outlet plus a persistent bar. `{nav.outlet}` is
        // where the active screen mounts; "BAR" stays put across swaps.
        .layout(|nav| view(vec![nav.outlet, text("BAR").into_element()]).into_element())
        .bind(nav)
        .into()
}

#[test]
fn swap_shows_initial_then_swaps_and_bar_persists() {
    install_buffering_scheduler();

    let mut mock = MockBackend::new();
    mock.register_navigator::<SwapPresentation, _>(|| Box::new(SwapHandler::<MockBackend>::new()));
    let backend = Rc::new(RefCell::new(mock));

    let nav: Ref<SwapHandle> = Ref::new();
    let _owner = mount_swap(&backend, nav.clone());
    // Run the deferred author-layout build → outlet captured, initial screen shown.
    runtime_core::drain_buffered_microtasks();

    {
        let b = backend.borrow();
        assert!(b.contains_text("BAR"), "author bar chrome present:\n{}", b.dump());
        assert!(b.contains_text("HOME CONTENT"), "initial screen in outlet:\n{}", b.dump());
        assert!(
            !b.contains_text("SETTINGS CONTENT"),
            "inactive screen NOT mounted (swap shows one at a time):\n{}",
            b.dump()
        );
    }

    // Select the settings screen → it swaps INTO the outlet; the bar persists.
    nav.get().expect("SwapHandle filled after mount").select(&SETTINGS, ());
    runtime_core::drain_buffered_microtasks();
    {
        let b = backend.borrow();
        assert!(b.contains_text("SETTINGS CONTENT"), "selected screen in outlet:\n{}", b.dump());
        assert!(
            !b.contains_text("HOME CONTENT"),
            "prior screen removed from outlet on swap:\n{}",
            b.dump()
        );
        assert!(b.contains_text("BAR"), "author bar persists across swap:\n{}", b.dump());
    }

    // Co-equal, depth-less: selecting HOME again just shows it (no back-stack).
    nav.get().unwrap().select(&HOME, ());
    runtime_core::drain_buffered_microtasks();
    {
        let b = backend.borrow();
        assert!(b.contains_text("HOME CONTENT"), "re-selecting a sibling shows it:\n{}", b.dump());
        assert!(!b.contains_text("SETTINGS CONTENT"), "only one screen visible:\n{}", b.dump());
    }
}

#[test]
fn swap_link_activates_as_select_not_push() {
    install_buffering_scheduler();

    let mut mock = MockBackend::new();
    mock.register_navigator::<SwapPresentation, _>(|| Box::new(SwapHandler::<MockBackend>::new()));
    let backend = Rc::new(RefCell::new(mock));

    let nav: Ref<SwapHandle> = Ref::new();
    let _owner = mount_swap(&backend, nav.clone());
    runtime_core::drain_buffered_microtasks();

    // A `Link` inside a swap screen builds its activation command through the
    // navigator's control plane. The swap handler installs a link activator
    // that rewrites activation to `Select`. Without it (the old tab bug) this
    // falls back to `Push`, which the handler has no stack for.
    let handle = nav.get().expect("SwapHandle filled after mount");
    let control = handle
        .inner()
        .control()
        .expect("live navigator exposes its control plane");
    let cmd = control.build_link_command("settings", "/settings".to_string(), Box::new(()));
    assert!(
        matches!(cmd, NavCommand::Select { name: "settings", .. }),
        "a Link in a swap screen must activate as Select, not Push"
    );
}

// ---------------------------------------------------------------------------
// Regressions
// ---------------------------------------------------------------------------

/// Route params requiring a path segment — NOT constructible from a bare
/// route name, so `SwapContext::on_select` must refuse (not panic on the
/// `ScreenBuilder` downcast).
#[derive(Clone)]
struct UserParams {
    id: String,
}
impl RouteParams for UserParams {
    fn to_path(&self, pattern: &str) -> String {
        pattern.replace(":id", &self.id)
    }
    fn from_segments(segs: &HashMap<String, String>) -> Option<Self> {
        segs.get("id").map(|id| UserParams { id: id.clone() })
    }
}

type CapturedSelect = Rc<RefCell<Option<Rc<dyn Fn(&'static str)>>>>;

/// Mount a swap navigator whose layout captures `nav.on_select` so tests can
/// drive layout-chrome selection (the TabBar path) directly.
fn mount_swap_capturing_on_select(
    backend: &Rc<RefCell<MockBackend>>,
    nav: Ref<SwapHandle>,
) -> (Box<dyn Any>, CapturedSelect) {
    use swap_navigator::SwapNavigator;
    let captured: CapturedSelect = Rc::new(RefCell::new(None));
    let cap = captured.clone();
    let owner: Box<dyn Any> = Box::new(runtime_core::mount(backend.clone(), move || {
        let cap = cap.clone();
        let nav = nav.clone();
        const USER: Route<UserParams> = Route::<UserParams>::new("user", "/user/:id");
        SwapNavigator::new(&HOME)
            .screen(HOME, |_| Screen::new(view(vec![text("HOME CONTENT").into()])))
            .screen(SETTINGS, |_| Screen::new(view(vec![text("SETTINGS CONTENT").into()])))
            .screen(USER, |p: UserParams| {
                Screen::new(view(vec![text(format!("USER {}", p.id)).into()]))
            })
            .layout(move |ctx| {
                *cap.borrow_mut() = Some(ctx.on_select.clone());
                view(vec![ctx.outlet, text("BAR").into_element()]).into_element()
            })
            .bind(nav)
            .into()
    }));
    (owner, captured)
}

/// `SwapContext::on_select` used to dispatch `Select` with `url: String::new()`,
/// so the substrate wrote the bare base ("/") into `nav_state.active_path`
/// instead of the selected route's registered path — layout-driven selection
/// and `SwapHandle::select` disagreed about the path mirror.
#[test]
fn regression_on_select_dispatches_route_url_not_empty() {
    install_buffering_scheduler();

    let mut mock = MockBackend::new();
    mock.register_navigator::<SwapPresentation, _>(|| Box::new(SwapHandler::<MockBackend>::new()));
    let backend = Rc::new(RefCell::new(mock));

    let nav: Ref<SwapHandle> = Ref::new();
    let (_owner, on_select) = mount_swap_capturing_on_select(&backend, nav.clone());
    runtime_core::drain_buffered_microtasks();

    let select = on_select.borrow().clone().expect("layout ran and captured on_select");
    select("settings");
    runtime_core::drain_buffered_microtasks();

    let handle = nav.get().unwrap();
    let control = handle.inner().control().expect("control plane");
    let (route, path, _, _) = control.nav_state_snapshot().expect("nav state attached");
    assert_eq!(route, "settings");
    assert_eq!(
        path, "/settings",
        "on_select must carry the route's registered path, not the bare base"
    );
    assert!(backend.borrow().contains_text("SETTINGS CONTENT"));
}

/// `on_select` used to box `()` as the params for EVERY route, so selecting a
/// typed-params route from layout chrome panicked in the `ScreenBuilder`
/// downcast ("route params type mismatch"). A bare name can't supply required
/// path params — the selection must be refused, not crash.
#[test]
fn regression_on_select_params_route_is_refused_not_panic() {
    install_buffering_scheduler();

    let mut mock = MockBackend::new();
    mock.register_navigator::<SwapPresentation, _>(|| Box::new(SwapHandler::<MockBackend>::new()));
    let backend = Rc::new(RefCell::new(mock));

    let nav: Ref<SwapHandle> = Ref::new();
    let (_owner, on_select) = mount_swap_capturing_on_select(&backend, nav.clone());
    runtime_core::drain_buffered_microtasks();

    let select = on_select.borrow().clone().expect("layout ran and captured on_select");
    // Must not panic, and must not corrupt the nav-state mirror.
    select("user");
    runtime_core::drain_buffered_microtasks();

    let handle = nav.get().unwrap();
    let control = handle.inner().control().expect("control plane");
    let (route, _, _, _) = control.nav_state_snapshot().expect("nav state attached");
    assert_eq!(route, "home", "refused selection leaves the active route untouched");
    assert!(backend.borrow().contains_text("HOME CONTENT"));
}

/// Cold-start deep link: the walker resolves the launch path and attaches THAT
/// screen, but the handler used to cache it under the configured initial's key
/// ("home") and mark "home" active — so selecting Home was a no-op that left
/// the deep-linked screen visible, and re-selecting it mounted a duplicate.
#[test]
fn regression_deep_link_initial_cached_under_resolved_route() {
    install_buffering_scheduler();

    let mut mock = MockBackend::new();
    mock.register_navigator::<SwapPresentation, _>(|| Box::new(SwapHandler::<MockBackend>::new()));
    let backend = Rc::new(RefCell::new(mock));

    set_initial_path(Some("/settings".to_string()));
    let nav: Ref<SwapHandle> = Ref::new();
    let _owner = mount_swap(&backend, nav.clone());
    runtime_core::drain_buffered_microtasks();

    {
        let b = backend.borrow();
        assert!(b.contains_text("SETTINGS CONTENT"), "deep link shows the resolved screen:\n{}", b.dump());
        assert!(!b.contains_text("HOME CONTENT"), "configured initial not mounted:\n{}", b.dump());
    }

    // Selecting the configured initial must actually navigate there — before
    // the fix, `active` was already (wrongly) "home", so this was a no-op and
    // the settings screen stayed visible under a Home selection.
    nav.get().expect("handle filled").select(&HOME, ());
    runtime_core::drain_buffered_microtasks();
    {
        let b = backend.borrow();
        assert!(b.contains_text("HOME CONTENT"), "selecting home shows home:\n{}", b.dump());
        assert!(!b.contains_text("SETTINGS CONTENT"), "deep-linked screen swapped out:\n{}", b.dump());
    }

    // And back: the deep-linked screen was cached under ITS OWN route key, so
    // this re-shows the cached instance instead of mounting a duplicate.
    nav.get().unwrap().select(&SETTINGS, ());
    runtime_core::drain_buffered_microtasks();
    {
        let b = backend.borrow();
        assert!(b.contains_text("SETTINGS CONTENT"), "cached deep-link screen re-shown:\n{}", b.dump());
        assert!(!b.contains_text("HOME CONTENT"));
    }
}

/// `LazyDisposing` used to hold `mounted.borrow_mut()` (and the `active`
/// borrow) across `release_screen`, which drops the screen scope and runs
/// author `on_cleanup` synchronously — a cleanup that navigates re-entered
/// the navigator and aborted with "RefCell already borrowed". Also covers the
/// walker-side half: `release_screen` itself used to drop the scope while the
/// scopes-map `borrow_mut` was still live (same abort, one layer down).
#[test]
fn regression_lazy_disposing_cleanup_navigation_does_not_double_borrow() {
    install_buffering_scheduler();

    let mut mock = MockBackend::new();
    mock.register_navigator::<SwapPresentation, _>(|| Box::new(SwapHandler::<MockBackend>::new()));
    let backend = Rc::new(RefCell::new(mock));

    let nav: Ref<SwapHandle> = Ref::new();
    let nav_for_app = nav.clone();
    let _owner: Box<dyn Any> = Box::new(runtime_core::mount(backend.clone(), move || {
        use swap_navigator::SwapNavigator;
        let nav = nav_for_app.clone();
        let nav_for_cleanup = nav.clone();
        SwapNavigator::new(&HOME)
            .screen(HOME, move |_| {
                // A route-guard shape: navigating away from Home redirects to
                // Profile from the screen's cleanup.
                let nav = nav_for_cleanup.clone();
                runtime_core::on_cleanup(move || {
                    if let Some(h) = nav.get() {
                        h.select(&PROFILE, ());
                    }
                });
                Screen::new(view(vec![text("HOME CONTENT").into()]))
            })
            .screen(SETTINGS, |_| Screen::new(view(vec![text("SETTINGS CONTENT").into()])))
            .screen(PROFILE, |_| Screen::new(view(vec![text("PROFILE CONTENT").into()])))
            .layout(|ctx| view(vec![ctx.outlet, text("BAR").into_element()]).into_element())
            .mount_policy(MountPolicy::LazyDisposing)
            .bind(nav)
            .into()
    }));
    runtime_core::drain_buffered_microtasks();
    assert!(backend.borrow().contains_text("HOME CONTENT"));

    // Switching away disposes Home; its cleanup re-enters the navigator.
    // Before the fix this aborted ("RefCell already borrowed"); now the
    // re-entrant redirect completes and the outer selection lands last.
    nav.get().expect("handle filled").select(&SETTINGS, ());
    runtime_core::drain_buffered_microtasks();
    {
        let b = backend.borrow();
        assert!(b.contains_text("SETTINGS CONTENT"), "outer selection wins:\n{}", b.dump());
        assert!(!b.contains_text("HOME CONTENT"), "disposed screen removed:\n{}", b.dump());
    }
}

fn is_fill_rules(r: &StyleRules) -> bool {
    r.width == Some(Length::Percent(100.0).into())
        && r.height == Some(Length::Percent(100.0).into())
        && r.flex_grow == Some(1.0.into())
}

fn is_outlet_fill_rules(r: &StyleRules) -> bool {
    r.flex_grow == Some(1.0.into())
        && r.flex_basis == Some(Length::Px(0.0).into())
        && r.min_height == Some(Length::Px(0.0).into())
}

/// A style-less `{nav.outlet}` used to build as a bare hug-content view, but
/// screens assume a bounded, fillable region (`flex: 1`, scroll views need a
/// bounded height) — the zero-config path was broken until authors manually
/// plumbed `flex: 1 1 0` + `min-height: 0` onto the outlet. The walker now
/// applies `outlet_fill_rules` when the outlet carries no author style.
#[test]
fn regression_outlet_fills_by_default() {
    install_buffering_scheduler();

    let mut mock = MockBackend::new();
    mock.register_navigator::<SwapPresentation, _>(|| Box::new(SwapHandler::<MockBackend>::new()));
    let backend = Rc::new(RefCell::new(mock));

    let nav: Ref<SwapHandle> = Ref::new();
    let _owner = mount_swap(&backend, nav.clone());
    runtime_core::drain_buffered_microtasks();

    assert!(
        backend.borrow().any_node_styled(is_outlet_fill_rules),
        "a style-less outlet defaults to a bounded, fillable flex region:\n{}",
        backend.borrow().dump()
    );
}

/// `ctx.outlet.with_style(...)` replaces the outlet's fill default entirely —
/// opting out is a real author control, not a merge.
#[test]
fn outlet_author_style_replaces_default_fill() {
    install_buffering_scheduler();

    let mut mock = MockBackend::new();
    mock.register_navigator::<SwapPresentation, _>(|| Box::new(SwapHandler::<MockBackend>::new()));
    let backend = Rc::new(RefCell::new(mock));

    fn outlet_style() -> StyleApplication {
        static KEY: u8 = 0;
        let sheet = runtime_core::cached_stylesheet(&KEY as *const u8 as usize, || {
            Rc::new(StyleSheet::r#static(StyleRules::default()))
        });
        StyleApplication::new(sheet).with_computed("test_outlet_fixed", || StyleRules {
            height: Some(Length::Px(222.0).into()),
            ..Default::default()
        })
    }

    let nav: Ref<SwapHandle> = Ref::new();
    let nav_for_app = nav.clone();
    let _owner: Box<dyn Any> = Box::new(runtime_core::mount(backend.clone(), move || {
        use swap_navigator::SwapNavigator;
        let nav = nav_for_app.clone();
        SwapNavigator::new(&HOME)
            .screen(HOME, |_| Screen::new(view(vec![text("HOME CONTENT").into()])))
            .layout(|ctx| {
                view(vec![ctx.outlet.with_style(outlet_style()), text("BAR").into_element()])
                    .into_element()
            })
            .bind(nav)
            .into()
    }));
    runtime_core::drain_buffered_microtasks();

    let b = backend.borrow();
    assert!(
        b.any_node_styled(|r| r.height == Some(Length::Px(222.0).into())),
        "author outlet style landed:\n{}",
        b.dump()
    );
    assert!(
        !b.any_node_styled(is_outlet_fill_rules),
        "author outlet style replaced the fill default:\n{}",
        b.dump()
    );
}

/// The navigator's bare root view carried NO sizing, so an app whose root is
/// a navigator collapsed to content height instead of filling the viewport
/// (web `#app` / desktop window roots size children that ask for space; they
/// don't force it). The handler now applies `navigator_fill_rules` to its
/// root at init.
#[test]
fn regression_navigator_root_fills_container_by_default() {
    install_buffering_scheduler();

    let mut mock = MockBackend::new();
    mock.register_navigator::<SwapPresentation, _>(|| Box::new(SwapHandler::<MockBackend>::new()));
    let backend = Rc::new(RefCell::new(mock));

    let nav: Ref<SwapHandle> = Ref::new();
    let _owner = mount_swap(&backend, nav.clone());
    runtime_core::drain_buffered_microtasks();

    assert!(
        backend.borrow().any_node_styled(is_fill_rules),
        "the navigator root defaults to fill-the-container sizing:\n{}",
        backend.borrow().dump()
    );
}

/// An author `.with_style(...)` on the navigator element is applied AFTER the
/// handler's default, onto the same root node — the author's sizing wins.
#[test]
fn navigator_root_author_style_overrides_default_fill() {
    install_buffering_scheduler();

    let mut mock = MockBackend::new();
    mock.register_navigator::<SwapPresentation, _>(|| Box::new(SwapHandler::<MockBackend>::new()));
    let backend = Rc::new(RefCell::new(mock));

    fn fixed_style() -> StyleApplication {
        static KEY: u8 = 0;
        let sheet = runtime_core::cached_stylesheet(&KEY as *const u8 as usize, || {
            Rc::new(StyleSheet::r#static(StyleRules::default()))
        });
        StyleApplication::new(sheet).with_computed("test_nav_fixed", || StyleRules {
            width: Some(Length::Px(321.0).into()),
            height: Some(Length::Px(123.0).into()),
            ..Default::default()
        })
    }

    let nav: Ref<SwapHandle> = Ref::new();
    let nav_for_app = nav.clone();
    let _owner: Box<dyn Any> = Box::new(runtime_core::mount(backend.clone(), move || {
        use swap_navigator::SwapNavigator;
        let nav = nav_for_app.clone();
        SwapNavigator::new(&HOME)
            .screen(HOME, |_| Screen::new(view(vec![text("HOME CONTENT").into()])))
            .layout(|ctx| view(vec![ctx.outlet]).into_element())
            .bind(nav)
            .with_style(fixed_style())
            .into()
    }));
    runtime_core::drain_buffered_microtasks();

    let b = backend.borrow();
    assert!(
        b.any_node_styled(|r| r.width == Some(Length::Px(321.0).into())
            && r.height == Some(Length::Px(123.0).into())),
        "author style landed on the navigator root:\n{}",
        b.dump()
    );
    assert!(
        !b.any_node_styled(is_fill_rules),
        "author style replaced the default fill sizing (applied after it):\n{}",
        b.dump()
    );
}
