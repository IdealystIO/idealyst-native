//! Regression fixture for **nested navigators with two simultaneous author
//! layouts** — a primary sidebar (outer swap) wrapping a secondary sidebar
//! (inner swap / inner stack), BOTH chromes rendered at once on a route
//! nested in both.
//!
//! Why this fixture exists: the outlet model composes per-navigator — each
//! navigator wraps its OWN outlet in its OWN chrome and only ever
//! clears/inserts its own outlet's child on a switch. So an inner navigator
//! mounted as an outer screen's body brings its whole `layout → outlet →
//! screen` subtree, and the two layouts nest: `outer sidebar → outer outlet →
//! inner sidebar → inner outlet → innermost screen`. Selecting at one level
//! only swaps that level's outlet child, leaving the other level's sidebar
//! untouched.
//!
//! The shipped nesting tests all give the INNER navigator a *bare* outlet
//! (`nested_teardown_repro`, the `navigator_url_sync` nested-stack
//! regressions). This is the first fixture that puts a non-trivial
//! `.layout(...)` on BOTH levels — the "two visible sidebars" shape — and
//! asserts they render, persist, and swap independently. url-sync is left
//! inert (`reset_url_sync_for_tests`) so this isolates the render/compose
//! mechanism from URL routing (the base-path partitioning has its own
//! coverage in `navigator_url_sync.rs`).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::OnceLock;

use mock_backend::MockBackend;
use runtime_core::primitives::navigator::{reset_url_sync_for_tests, Screen};
use runtime_core::scheduling::{install_scheduler, ScheduleHandle, Scheduler};
use runtime_core::{text, view, Element, IntoElement, Ref, Route};
use stack_navigator::{
    StackBuilder, StackHandle, StackHandler, StackNavigator, StackPresentation,
};
use swap_navigator::{SwapBuilder, SwapHandle, SwapHandler, SwapNavigator, SwapPresentation};

// ---------------------------------------------------------------------------
// Buffering test scheduler — captures the navigators' deferred author-layout
// microtasks so the test drains them deterministically (both a nested outer
// AND inner navigator schedule one). Mirrors `nested_teardown_repro` /
// `swap_navigator_local`.
// ---------------------------------------------------------------------------

thread_local! {
    static MICROTASKS: RefCell<Vec<Box<dyn FnOnce() + 'static>>> = const { RefCell::new(Vec::new()) };
}

struct BufferingScheduler;
// SAFETY: storage is thread-local; the value never crosses threads (single
// test thread). Bounds exist only for `OnceLock`. Same posture as the sibling
// navigator tests.
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
    fn after_animation_frame(&self, _f: Box<dyn FnOnce() + 'static>) -> Box<dyn ScheduleHandle> {
        Box::new(InertHandle)
    }
    fn after_ms(&self, _d: i32, _f: Box<dyn FnOnce() + 'static>) -> Box<dyn ScheduleHandle> {
        Box::new(InertHandle)
    }
    fn raf_loop(&self, _f: Box<dyn FnMut() + 'static>) -> Box<dyn ScheduleHandle> {
        Box::new(InertHandle)
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
        panic!("never drained (self-rescheduling microtask?)");
    }
}

fn install_buffering_scheduler() {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    INSTALLED.get_or_init(|| install_scheduler(Box::new(BufferingScheduler)));
}

fn new_backend() -> Rc<RefCell<MockBackend>> {
    let mut mock = MockBackend::new();
    // One factory per presentation type; the walker calls it once per
    // Element::Navigator instance, so the SAME type nested in itself
    // (swap-in-swap) gets a fresh handler at each level.
    mock.register_navigator::<SwapPresentation, _>(|| Box::new(SwapHandler::<MockBackend>::new()));
    mock.register_navigator::<StackPresentation, _>(|| {
        Box::new(StackHandler::<MockBackend>::new())
    });
    Rc::new(RefCell::new(mock))
}

// Outer (primary sidebar) routes. REPORTS' body is a nested navigator.
const HOME: Route<()> = Route::<()>::new("home", "/");
const REPORTS: Route<()> = Route::<()>::new("reports", "/reports");

// ---------------------------------------------------------------------------
// swap-in-swap: primary sidebar (left) wrapping a secondary tool sidebar
// (right). Both are `swap` — co-equal screens the user switches between.
// ---------------------------------------------------------------------------

#[test]
fn nested_swap_in_swap_renders_both_sidebars_simultaneously() {
    install_buffering_scheduler();
    reset_url_sync_for_tests();

    let backend = new_backend();
    let outer: Ref<SwapHandle> = Ref::new();
    let inner: Ref<SwapHandle> = Ref::new();

    // Inner tool-sidebar routes are RELATIVE to the REPORTS base: "" is the
    // index screen mounted at /reports. Hoisted so the test body can drive
    // the inner navigator (`inner.select(&TRENDS, ...)`).
    const SUMMARY: Route<()> = Route::<()>::new("summary", "");
    const TRENDS: Route<()> = Route::<()>::new("trends", "/trends");

    let owner = {
        let outer = outer.clone();
        let inner = inner.clone();
        runtime_core::mount(backend.clone(), move || {
            let inner = inner.clone();
            SwapNavigator::new(&HOME)
                .screen(HOME, |_| Screen::new(view(vec![text("HOME_BODY").into()])))
                .screen(REPORTS, move |_| {
                    // The whole secondary navigator (with its own right-hand
                    // sidebar) is this outer screen's body.
                    let el: Element = SwapNavigator::new(&SUMMARY)
                        .screen(SUMMARY, |_| {
                            Screen::new(view(vec![text("SUMMARY_BODY").into()]))
                        })
                        .screen(TRENDS, |_| {
                            Screen::new(view(vec![text("TRENDS_BODY").into()]))
                        })
                        // Secondary sidebar on the RIGHT: outlet first, chrome after.
                        .layout(|nav| {
                            view(vec![nav.outlet, text("SECONDARY_SIDEBAR").into_element()])
                                .into_element()
                        })
                        .bind(inner.clone())
                        .into();
                    Screen::new(el)
                })
                // Primary sidebar on the LEFT: chrome first, outlet after.
                .layout(|nav| {
                    view(vec![text("PRIMARY_SIDEBAR").into_element(), nav.outlet]).into_element()
                })
                .bind(outer.clone())
                .into()
        })
    };
    runtime_core::drain_buffered_microtasks();

    // Start on HOME: only the primary sidebar + HOME body. The secondary
    // navigator hasn't been activated, so its sidebar isn't in the tree.
    {
        let b = backend.borrow();
        assert!(b.contains_text("PRIMARY_SIDEBAR"), "primary sidebar missing on HOME:\n{}", b.dump());
        assert!(b.contains_text("HOME_BODY"), "HOME body missing:\n{}", b.dump());
        assert!(!b.contains_text("SECONDARY_SIDEBAR"), "secondary sidebar leaked on HOME:\n{}", b.dump());
        assert!(!b.contains_text("SUMMARY_BODY"), "nested screen leaked on HOME:\n{}", b.dump());
    }

    // Select the REPORTS section on the OUTER navigator. This is the crux:
    // on a route nested in both, BOTH sidebars must render at once, wrapping
    // the innermost (SUMMARY) screen.
    outer.get().expect("outer handle bound").select(&REPORTS, ());
    runtime_core::drain_buffered_microtasks();
    {
        let b = backend.borrow();
        assert!(
            b.contains_text("PRIMARY_SIDEBAR") && b.contains_text("SECONDARY_SIDEBAR"),
            "both sidebars must render simultaneously on the doubly-nested route:\n{}",
            b.dump()
        );
        assert!(b.contains_text("SUMMARY_BODY"), "innermost screen missing:\n{}", b.dump());
        assert!(!b.contains_text("HOME_BODY"), "outer HOME body should be swapped out:\n{}", b.dump());
    }

    // Switch the INNER (tool) sidebar to TRENDS. Only the inner outlet's
    // child swaps — BOTH sidebars stay mounted, outer selection untouched.
    inner.get().expect("inner handle bound").select(&TRENDS, ());
    runtime_core::drain_buffered_microtasks();
    {
        let b = backend.borrow();
        assert!(b.contains_text("PRIMARY_SIDEBAR"), "primary sidebar torn down by inner switch:\n{}", b.dump());
        assert!(b.contains_text("SECONDARY_SIDEBAR"), "secondary sidebar torn down by inner switch:\n{}", b.dump());
        assert!(b.contains_text("TRENDS_BODY"), "inner switch did not swap to TRENDS:\n{}", b.dump());
        assert!(!b.contains_text("SUMMARY_BODY"), "prior inner screen still shown after switch:\n{}", b.dump());
    }

    // Back to HOME on the OUTER navigator: the primary sidebar stays, the
    // secondary navigator is swapped out of the outer outlet entirely.
    outer.get().expect("outer handle bound").select(&HOME, ());
    runtime_core::drain_buffered_microtasks();
    {
        let b = backend.borrow();
        assert!(b.contains_text("PRIMARY_SIDEBAR"), "primary sidebar lost returning to HOME:\n{}", b.dump());
        assert!(b.contains_text("HOME_BODY"), "HOME body missing on return:\n{}", b.dump());
        assert!(!b.contains_text("SECONDARY_SIDEBAR"), "secondary sidebar should be swapped out on HOME:\n{}", b.dump());
    }

    // Return to REPORTS: under the outer swap's default LazyPersistent policy
    // the nested navigator instance was kept mounted (detached), so it comes
    // back on its OWN retained selection — TRENDS, not the SUMMARY index.
    outer.get().expect("outer handle bound").select(&REPORTS, ());
    runtime_core::drain_buffered_microtasks();
    {
        let b = backend.borrow();
        assert!(
            b.contains_text("PRIMARY_SIDEBAR") && b.contains_text("SECONDARY_SIDEBAR"),
            "both sidebars must return together:\n{}",
            b.dump()
        );
        assert!(
            b.contains_text("TRENDS_BODY") && !b.contains_text("SUMMARY_BODY"),
            "nested navigator state (TRENDS selection) not retained across an outer swap-away:\n{}",
            b.dump()
        );
    }

    drop(owner);
    runtime_core::drain_buffered_microtasks();
}

// ---------------------------------------------------------------------------
// stack-in-swap: primary sidebar (outer swap) wrapping a drill-down STACK
// with its own persistent chrome. Directly exercises the "combines swap and
// stack" shape — both layouts live at once, and the inner stack pushes/pops
// beneath the outer sidebar without disturbing it.
// ---------------------------------------------------------------------------

#[test]
fn nested_stack_in_swap_renders_both_chromes_and_pushes_independently() {
    install_buffering_scheduler();
    reset_url_sync_for_tests();

    let backend = new_backend();
    let outer: Ref<SwapHandle> = Ref::new();
    let inner: Ref<StackHandle> = Ref::new();

    // Inner stack routes, RELATIVE to the REPORTS base. Hoisted so the test
    // body can push DETAIL on the inner stack.
    const ROOT: Route<()> = Route::<()>::new("root", "");
    const DETAIL: Route<()> = Route::<()>::new("detail", "/detail");

    let owner = {
        let outer = outer.clone();
        let inner = inner.clone();
        runtime_core::mount(backend.clone(), move || {
            let inner = inner.clone();
            SwapNavigator::new(&HOME)
                .screen(HOME, |_| Screen::new(view(vec![text("HOME_BODY").into()])))
                .screen(REPORTS, move |_| {
                    let el: Element = StackNavigator::new(&ROOT)
                        .screen(ROOT, |_| Screen::new(view(vec![text("ROOT_BODY").into()])))
                        .screen(DETAIL, |_| Screen::new(view(vec![text("DETAIL_BODY").into()])))
                        // The inner stack's own persistent chrome (a header).
                        .layout(|nav| {
                            view(vec![text("STACK_CHROME").into_element(), nav.outlet])
                                .into_element()
                        })
                        .bind(inner.clone())
                        .into();
                    Screen::new(el)
                })
                .layout(|nav| {
                    view(vec![text("PRIMARY_SIDEBAR").into_element(), nav.outlet]).into_element()
                })
                .bind(outer.clone())
                .into()
        })
    };
    runtime_core::drain_buffered_microtasks();

    // Enter the section that hosts the nested stack: primary sidebar + stack
    // chrome + the stack root, all at once.
    outer.get().expect("outer bound").select(&REPORTS, ());
    runtime_core::drain_buffered_microtasks();
    {
        let b = backend.borrow();
        assert!(
            b.contains_text("PRIMARY_SIDEBAR") && b.contains_text("STACK_CHROME"),
            "outer sidebar and inner stack chrome must render together:\n{}",
            b.dump()
        );
        assert!(b.contains_text("ROOT_BODY"), "stack root missing:\n{}", b.dump());
    }

    // Push a detail screen on the INNER stack. Both chromes stay mounted; only
    // the inner outlet's top screen changes.
    inner.get().expect("inner stack bound").push(&DETAIL, ());
    runtime_core::drain_buffered_microtasks();
    {
        let b = backend.borrow();
        assert!(b.contains_text("PRIMARY_SIDEBAR"), "outer sidebar torn down by inner push:\n{}", b.dump());
        assert!(b.contains_text("STACK_CHROME"), "inner stack chrome torn down by its own push:\n{}", b.dump());
        assert!(b.contains_text("DETAIL_BODY"), "inner push did not surface DETAIL:\n{}", b.dump());
    }

    // Pop back to the stack root — chromes intact, root revealed.
    inner.get().expect("inner stack bound").pop();
    runtime_core::drain_buffered_microtasks();
    {
        let b = backend.borrow();
        assert!(b.contains_text("PRIMARY_SIDEBAR") && b.contains_text("STACK_CHROME"), "chromes lost on inner pop:\n{}", b.dump());
        assert!(b.contains_text("ROOT_BODY"), "inner pop did not reveal ROOT:\n{}", b.dump());
        assert!(!b.contains_text("DETAIL_BODY"), "DETAIL still shown after inner pop:\n{}", b.dump());
    }

    drop(owner);
    runtime_core::drain_buffered_microtasks();
}
