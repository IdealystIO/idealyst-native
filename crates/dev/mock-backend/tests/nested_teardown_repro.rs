//! Minimal repro: nested outlet-model stack inside a swap screen, NO url
//! provider installed. If this aborts at thread death, the teardown leak
//! pre-dates the URL-sync work.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::OnceLock;

use mock_backend::MockBackend;
use runtime_core::primitives::navigator::{reset_url_sync_for_tests, Screen};
use runtime_core::scheduling::{install_scheduler, ScheduleHandle, Scheduler};
use runtime_core::{text, view, Element, IntoElement, Ref, Route};
use stack_navigator::{StackBuilder, StackHandle, StackHandler, StackNavigator, StackPresentation};
use swap_navigator::{SwapBuilder, SwapHandle, SwapHandler, SwapNavigator, SwapPresentation};

const HOME: Route<()> = Route::<()>::new("home", "/");
const DOCS: Route<()> = Route::<()>::new("docs", "/docs");

thread_local! {
    static MICROTASKS: RefCell<Vec<Box<dyn FnOnce() + 'static>>> = const { RefCell::new(Vec::new()) };
}
struct BufferingScheduler;
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
        panic!("never drained");
    }
}
fn install_buffering_scheduler() {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    INSTALLED.get_or_init(|| install_scheduler(Box::new(BufferingScheduler)));
}

#[test]
fn nested_stack_in_swap_teardown_without_url_sync() {
    install_buffering_scheduler();
    reset_url_sync_for_tests(); // ensure NO provider — url sync fully inert

    let mut mock = MockBackend::new();
    mock.register_navigator::<SwapPresentation, _>(|| Box::new(SwapHandler::<MockBackend>::new()));
    mock.register_navigator::<StackPresentation, _>(|| {
        Box::new(StackHandler::<MockBackend>::new())
    });
    let backend = Rc::new(RefCell::new(mock));

    let nav: Ref<SwapHandle> = Ref::new();
    let stack_nav: Ref<StackHandle> = Ref::new();
    let owner = {
        let nav = nav.clone();
        let stack_nav = stack_nav.clone();
        runtime_core::mount(backend.clone(), move || {
            let stack_nav = stack_nav.clone();
            SwapNavigator::new(&HOME)
                .screen(HOME, |_| Screen::new(view(vec![text("HOME").into()])))
                .screen(DOCS, move |_| {
                    const INDEX: Route<()> = Route::<()>::new("index", "");
                    const NDETAIL: Route<()> = Route::<()>::new("ndetail", "/detail");
                    let el: runtime_core::Element = StackNavigator::new(&INDEX)
                        .screen(INDEX, |_| Screen::new(view(vec![text("IDX").into()])))
                        .screen(NDETAIL, |_| Screen::new(view(vec![text("DET").into()])))
                        .into();
                    Screen::new(el)
                })
                .layout(|nav| view(vec![nav.outlet]).into_element())
                .bind(nav.clone())
                .into()
        })
    };
    runtime_core::drain_buffered_microtasks();

    nav.get().unwrap().select(&DOCS, ());
    runtime_core::drain_buffered_microtasks();
    assert!(backend.borrow().contains_text("IDX"));

    drop(owner);
    runtime_core::drain_buffered_microtasks();
}

/// Regression: disposing a styled subtree while the backend RefCell is
/// ALREADY mutably borrowed must not panic.
///
/// The iOS crash (SIGABRT "already borrowed" on the 4th navigation
/// away from a docs page): `Backend::release_virtualizer` runs under
/// the walker cleanup's `borrow_mut`, and the iOS impl synchronously
/// shuts down its data source — dropping the per-row scopes, whose
/// `StyleHandle`s (and graphics/portal/external cleanup handles) then
/// re-entered `backend.borrow_mut()` from their `Drop`. The fix makes
/// those Drops try-borrow and defer the release notification a
/// microtask when the backend is busy.
///
/// The distilled invariant under test: drop a mounted tree while
/// holding the backend borrow — exactly what any backend that tears
/// down walker-owned scopes from inside a `release_*` hook does.
#[test]
fn regression_styled_subtree_disposal_survives_outer_backend_borrow() {
    install_buffering_scheduler();

    let backend = Rc::new(RefCell::new(MockBackend::new()));
    let owner = runtime_core::mount(backend.clone(), || {
        // A small styled tree: reactive styles produce StyleHandles
        // whose Drop notifies the backend.
        let styled = runtime_core::view(vec![text("row").into()])
            .with_style(move || {
                runtime_core::StyleApplication::new(Rc::new(
                    runtime_core::StyleSheet::r#static(runtime_core::StyleRules {
                        background: Some(runtime_core::Color("#fff".into()).into()),
                        ..Default::default()
                    }),
                ))
            })
            .into_element();
        view(vec![styled, text("peer").into()]).into_element()
    });
    runtime_core::drain_buffered_microtasks();

    {
        // Simulate the nested context: the backend is mutably borrowed
        // (as it is inside `release_virtualizer` / any `release_*`
        // hook) while the subtree's scopes drop. Panicked
        // "already borrowed" before the fix.
        let _outer_borrow = backend.borrow_mut();
        drop(owner);
    }
    // The deferred release notifications run once the borrow is gone.
    runtime_core::drain_buffered_microtasks();
}
