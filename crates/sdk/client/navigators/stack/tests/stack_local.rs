//! Local-mount coverage for the outlet-model stack handler: push shows the new
//! top in the outlet, pop reveals the screen below. The author layout wraps the
//! outlet, exactly like swap — the difference is push/pop depth vs Select.

// mock-backend is a native-only dev-dep (its dev-server/wire transitive
// deps don't build on bare wasm32); the wasm test target runs
// `hydration_web.rs` instead.
#![cfg(not(target_arch = "wasm32"))]

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::OnceLock;

use mock_backend::MockBackend;
use runtime_core::primitives::navigator::Screen;
use runtime_core::scheduling::{install_scheduler, ScheduleHandle, Scheduler};
use runtime_core::{text, ui, view, Element, Ref, Route};
use stack_navigator::{
    StackBuilder, StackHandle, StackHandler, StackNavigator, StackPresentation, StackRetention,
};

const HOME: Route<()> = Route::<()>::new("home", "/");
const DETAIL: Route<()> = Route::<()>::new("detail", "/detail");

// Buffering scheduler: the handler defers its author-layout build to a
// microtask; drain runs it deterministically (only genuine microtasks buffered).
thread_local! {
    static MICROTASKS: RefCell<Vec<Box<dyn FnOnce() + 'static>>> = const { RefCell::new(Vec::new()) };
}
struct Buffering;
unsafe impl Send for Buffering {}
unsafe impl Sync for Buffering {}
struct Inert;
impl ScheduleHandle for Inert {
    fn cancel(&mut self) {}
}
impl Scheduler for Buffering {
    fn schedule_microtask(&self, f: Box<dyn FnOnce() + 'static>) {
        MICROTASKS.with(|q| q.borrow_mut().push(f));
    }
    fn after_animation_frame(&self, _f: Box<dyn FnOnce() + 'static>) -> Box<dyn ScheduleHandle> {
        Box::new(Inert)
    }
    fn after_ms(&self, _d: i32, _f: Box<dyn FnOnce() + 'static>) -> Box<dyn ScheduleHandle> {
        Box::new(Inert)
    }
    fn raf_loop(&self, _f: Box<dyn FnMut() + 'static>) -> Box<dyn ScheduleHandle> {
        Box::new(Inert)
    }
    fn drain_buffered_microtasks(&self) {
        for _ in 0..1000 {
            let batch: Vec<_> = MICROTASKS.with(|q| std::mem::take(&mut *q.borrow_mut()));
            if batch.is_empty() {
                return;
            }
            for f in batch {
                f();
            }
        }
        panic!("microtask queue never drained");
    }
}
fn scheduler() {
    static I: OnceLock<()> = OnceLock::new();
    I.get_or_init(|| install_scheduler(Box::new(Buffering)));
}

#[test]
fn push_shows_top_then_pop_reveals_below() {
    scheduler();

    let mut mock = MockBackend::new();
    mock.register_navigator::<StackPresentation, _>(|| Box::new(StackHandler::<MockBackend>::new()));
    let backend = Rc::new(RefCell::new(mock));

    let nav: Ref<StackHandle> = Ref::new();
    let nav_for_app = nav.clone();
    let _owner: Box<dyn Any> = Box::new(runtime_core::mount(backend.clone(), move || {
        let nav = nav_for_app.clone();
        StackNavigator::new(&HOME)
            .screen(HOME, |_| Screen::new(view(vec![text("HOME SCREEN").into()])))
            .screen(DETAIL, |_| Screen::new(view(vec![text("DETAIL SCREEN").into()])))
            .layout(|nav| {
                ui! {
                    view {
                        { nav.outlet }
                    }
                }
            })
            .bind(nav)
            .into()
    }));
    runtime_core::drain_buffered_microtasks();

    {
        let b = backend.borrow();
        assert!(b.contains_text("HOME SCREEN"), "root screen shows:\n{}", b.dump());
        assert!(!b.contains_text("DETAIL SCREEN"), "detail not yet pushed:\n{}", b.dump());
    }

    // Push detail → it becomes the visible top.
    nav.get().expect("StackHandle filled").push(&DETAIL, ());
    runtime_core::drain_buffered_microtasks();
    {
        let b = backend.borrow();
        assert!(b.contains_text("DETAIL SCREEN"), "pushed screen is the top:\n{}", b.dump());
        assert!(!b.contains_text("HOME SCREEN"), "root hidden beneath the top:\n{}", b.dump());
    }

    // Pop → the root below is revealed again.
    nav.get().unwrap().pop();
    runtime_core::drain_buffered_microtasks();
    {
        let b = backend.borrow();
        assert!(b.contains_text("HOME SCREEN"), "pop reveals the screen below:\n{}", b.dump());
        assert!(!b.contains_text("DETAIL SCREEN"), "popped screen removed:\n{}", b.dump());
    }
}

/// Cold-start deep link: the walker resolves the launch path, mounts THAT
/// screen, and delegates back-stack reconstruction to the stack SDK (see the
/// non-deferred mount in `walker::navigator`). The handler used to seat the
/// attached screen as `route: initial_route / path: initial_path` at depth 1 —
/// the entry was mislabeled, `can_go_back` stayed false, and Back could never
/// return to the index.
#[test]
fn regression_cold_deep_link_reconstructs_back_stack() {
    scheduler();

    let mut mock = MockBackend::new();
    mock.register_navigator::<StackPresentation, _>(|| Box::new(StackHandler::<MockBackend>::new()));
    let backend = Rc::new(RefCell::new(mock));

    runtime_core::primitives::navigator::set_initial_path(Some("/detail".to_string()));
    let nav: Ref<StackHandle> = Ref::new();
    let nav_for_app = nav.clone();
    let _owner: Box<dyn Any> = Box::new(runtime_core::mount(backend.clone(), move || {
        let nav = nav_for_app.clone();
        StackNavigator::new(&HOME)
            .screen(HOME, |_| Screen::new(view(vec![text("HOME SCREEN").into()])))
            .screen(DETAIL, |_| Screen::new(view(vec![text("DETAIL SCREEN").into()])))
            .layout(|nav| ui! { view { { nav.outlet } } })
            .bind(nav)
            .into()
    }));
    runtime_core::drain_buffered_microtasks();

    let handle = nav.get().expect("handle filled");
    let control = handle.inner().control().expect("control plane");
    {
        let b = backend.borrow();
        assert!(b.contains_text("DETAIL SCREEN"), "deep link shows the resolved screen:\n{}", b.dump());
        assert!(!b.contains_text("HOME SCREEN"), "index seated BELOW, not visible:\n{}", b.dump());
    }
    let (route, path, depth, can_go_back) = control.nav_state_snapshot().expect("nav state");
    assert_eq!(route, "detail");
    assert_eq!(path, "/detail");
    assert_eq!(depth, 2, "configured initial reconstructed beneath the resolved screen");
    assert!(can_go_back, "back is possible after a cold deep link");

    // Back returns to the index — the whole point of the reconstruction.
    nav.get().unwrap().pop();
    runtime_core::drain_buffered_microtasks();
    {
        let b = backend.borrow();
        assert!(b.contains_text("HOME SCREEN"), "pop reveals the index:\n{}", b.dump());
        assert!(!b.contains_text("DETAIL SCREEN"), "deep-linked screen released:\n{}", b.dump());
    }
    let (route, path, depth, can_go_back) = control.nav_state_snapshot().expect("nav state");
    assert_eq!(route, "home", "active mirror updated to the revealed index");
    assert_eq!(path, "/");
    assert_eq!(depth, 1);
    assert!(!can_go_back);
}

/// Per-screen lifecycle counters: how many times the screen body was built,
/// and how many times a built instance was disposed.
#[derive(Clone, Default)]
struct Counters {
    builds: Rc<std::cell::Cell<u32>>,
    disposals: Rc<std::cell::Cell<u32>>,
}

/// Mount a HOME/DETAIL stack whose HOME screen reports into `c`.
fn mount_counted(
    backend: &Rc<RefCell<MockBackend>>,
    nav: Ref<StackHandle>,
    c: Counters,
    retention: StackRetention,
) -> Box<dyn Any> {
    let nav_for_app = nav;
    Box::new(runtime_core::mount(backend.clone(), move || {
        let nav = nav_for_app.clone();
        let c = c.clone();
        StackNavigator::new(&HOME)
            .screen(HOME, move |_| {
                c.builds.set(c.builds.get() + 1);
                let disposals = c.disposals.clone();
                runtime_core::on_cleanup(move || disposals.set(disposals.get() + 1));
                Screen::new(view(vec![text("HOME SCREEN").into()]))
            })
            .screen(DETAIL, |_| Screen::new(view(vec![text("DETAIL SCREEN").into()])))
            .layout(|nav| ui! { view { { nav.outlet } } })
            .retention(retention)
            .bind(nav)
            .into()
    }))
}

/// Browser semantics (`Rebuild`, the web default): pushing over a screen
/// DISPOSES it — nothing below the visible screen stays resident — and pop
/// re-mounts the revealed screen from its URL like a fresh navigation.
#[test]
fn regression_rebuild_disposes_covered_screen_and_pop_remounts() {
    scheduler();

    let mut mock = MockBackend::new();
    mock.register_navigator::<StackPresentation, _>(|| Box::new(StackHandler::<MockBackend>::new()));
    let backend = Rc::new(RefCell::new(mock));

    let c = Counters::default();
    let nav: Ref<StackHandle> = Ref::new();
    let _owner = mount_counted(&backend, nav.clone(), c.clone(), StackRetention::Rebuild);
    runtime_core::drain_buffered_microtasks();
    assert_eq!(c.builds.get(), 1, "index mounted once");

    // Push covers the index — its scope must be dropped, not parked.
    nav.get().expect("handle filled").push(&DETAIL, ());
    runtime_core::drain_buffered_microtasks();
    assert_eq!(c.disposals.get(), 1, "covered screen disposed on push (browser semantics)");
    assert!(!backend.borrow().contains_text("HOME SCREEN"));

    // Pop re-mounts it fresh from its URL.
    nav.get().unwrap().pop();
    runtime_core::drain_buffered_microtasks();
    assert_eq!(c.builds.get(), 2, "pop re-mounts the revealed screen fresh");
    assert!(backend.borrow().contains_text("HOME SCREEN"));
    assert!(!backend.borrow().contains_text("DETAIL SCREEN"));
}

/// Native semantics (`Retain`, the non-web default — what the mock's
/// platform resolves to): covered screens stay alive and pop reveals the
/// SAME instance, no rebuild.
#[test]
fn retain_keeps_covered_screen_alive_across_push_pop() {
    scheduler();

    let mut mock = MockBackend::new();
    mock.register_navigator::<StackPresentation, _>(|| Box::new(StackHandler::<MockBackend>::new()));
    let backend = Rc::new(RefCell::new(mock));

    let c = Counters::default();
    let nav: Ref<StackHandle> = Ref::new();
    let _owner = mount_counted(&backend, nav.clone(), c.clone(), StackRetention::Retain);
    runtime_core::drain_buffered_microtasks();

    nav.get().expect("handle filled").push(&DETAIL, ());
    runtime_core::drain_buffered_microtasks();
    assert_eq!(c.disposals.get(), 0, "covered screen stays alive (native semantics)");

    nav.get().unwrap().pop();
    runtime_core::drain_buffered_microtasks();
    assert_eq!(c.builds.get(), 1, "pop reveals the retained instance, no rebuild");
    assert!(backend.borrow().contains_text("HOME SCREEN"));
}

/// `Rebuild` + cold deep link: the synthesized parent entry is URL-only —
/// a page loaded at /detail must NEVER load the index (no build, no effects,
/// no fetches) until the user actually pops to it.
#[test]
fn regression_rebuild_cold_deep_link_never_mounts_parent_until_pop() {
    scheduler();

    let mut mock = MockBackend::new();
    mock.register_navigator::<StackPresentation, _>(|| Box::new(StackHandler::<MockBackend>::new()));
    let backend = Rc::new(RefCell::new(mock));

    runtime_core::primitives::navigator::set_initial_path(Some("/detail".to_string()));
    let c = Counters::default();
    let nav: Ref<StackHandle> = Ref::new();
    let _owner = mount_counted(&backend, nav.clone(), c.clone(), StackRetention::Rebuild);
    runtime_core::drain_buffered_microtasks();

    let handle = nav.get().expect("handle filled");
    let control = handle.inner().control().expect("control plane");
    assert_eq!(c.builds.get(), 0, "unvisited deep-link parent must not mount");
    assert!(backend.borrow().contains_text("DETAIL SCREEN"));
    let (route, _, depth, can_go_back) = control.nav_state_snapshot().expect("nav state");
    assert_eq!(route, "detail");
    assert_eq!(depth, 2, "cold parent still counts for depth/back");
    assert!(can_go_back);

    // Popping is the first time the parent actually loads.
    nav.get().unwrap().pop();
    runtime_core::drain_buffered_microtasks();
    assert_eq!(c.builds.get(), 1, "parent mounts on first reveal");
    assert!(backend.borrow().contains_text("HOME SCREEN"));
    let (route, path, depth, _) = control.nav_state_snapshot().expect("nav state");
    assert_eq!((route, path.as_str(), depth), ("home", "/", 1));
}

#[test]
fn pop_at_root_is_a_noop() {
    scheduler();

    let mut mock = MockBackend::new();
    mock.register_navigator::<StackPresentation, _>(|| Box::new(StackHandler::<MockBackend>::new()));
    let backend = Rc::new(RefCell::new(mock));

    let nav: Ref<StackHandle> = Ref::new();
    let nav_for_app = nav.clone();
    let _owner: Box<dyn Any> = Box::new(runtime_core::mount(backend.clone(), move || {
        let nav = nav_for_app.clone();
        StackNavigator::new(&HOME)
            .screen(HOME, |_| Screen::new(view(vec![text("HOME SCREEN").into()])))
            .layout(|nav| ui! { view { { nav.outlet } } })
            .bind(nav)
            .into()
    }));
    runtime_core::drain_buffered_microtasks();

    // Popping the root must not remove it.
    nav.get().unwrap().pop();
    runtime_core::drain_buffered_microtasks();
    let b = backend.borrow();
    assert!(b.contains_text("HOME SCREEN"), "root survives a pop:\n{}", b.dump());
}
