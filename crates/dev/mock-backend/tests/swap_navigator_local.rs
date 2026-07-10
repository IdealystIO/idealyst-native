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
use std::rc::Rc;
use std::sync::OnceLock;

use mock_backend::MockBackend;
use runtime_core::primitives::navigator::{NavCommand, Screen};
use runtime_core::scheduling::{install_scheduler, ScheduleHandle, Scheduler};
use runtime_core::{text, view, Element, IntoElement, Ref, Route};
use swap_navigator::{SwapBuilder, SwapHandle, SwapHandler, SwapPresentation};

const HOME: Route<()> = Route::<()>::new("home", "/");
const SETTINGS: Route<()> = Route::<()>::new("settings", "/settings");

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
