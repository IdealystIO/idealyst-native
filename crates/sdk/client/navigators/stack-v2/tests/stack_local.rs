//! Local-mount coverage for the outlet-model stack handler: push shows the new
//! top in the outlet, pop reveals the screen below. The author layout wraps the
//! outlet, exactly like swap — the difference is push/pop depth vs Select.

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::OnceLock;

use mock_backend::MockBackend;
use runtime_core::primitives::navigator::Screen;
use runtime_core::scheduling::{install_scheduler, ScheduleHandle, Scheduler};
use runtime_core::{text, ui, view, Element, Ref, Route};
use stack_navigator_v2::{StackBuilder, StackHandle, StackHandler, StackNavigator, StackPresentation};

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
