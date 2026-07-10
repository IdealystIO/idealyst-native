//! `TabBar` rendered as a real swap-navigator layout.
//!
//! Mounts a swap navigator whose `.layout(|nav| …)` wraps the outlet with a
//! [`TabBar`] wired to the navigator's `SwapContext`, then asserts the themed
//! bar (both tab labels) and the initial screen both render — i.e. the chrome
//! is author layout around the outlet, exactly as the model intends.

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::OnceLock;

use idea_theme::theme::{install_idea_theme, light_theme};
use idea_ui_nav::{TabBar, TabItem};
use mock_backend::MockBackend;
use runtime_core::primitives::navigator::Screen;
use runtime_core::scheduling::{install_scheduler, ScheduleHandle, Scheduler};
use runtime_core::{text, ui, view, Element, Ref, Route};
use swap_navigator::{SwapBuilder, SwapHandle, SwapHandler, SwapNavigator, SwapPresentation};

const HOME: Route<()> = Route::<()>::new("home", "/");
const SETTINGS: Route<()> = Route::<()>::new("settings", "/settings");

// Buffering scheduler so the swap handler's deferred author-layout build runs
// only on an explicit drain (mirrors the swap-navigator local test).
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
        panic!("microtask queue never drained");
    }
}
fn setup() {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    INSTALLED.get_or_init(|| {
        install_scheduler(Box::new(BufferingScheduler));
        install_idea_theme(light_theme());
    });
}

#[test]
fn tab_bar_renders_as_swap_layout_chrome() {
    setup();

    let mut mock = MockBackend::new();
    mock.register_navigator::<SwapPresentation, _>(|| Box::new(SwapHandler::<MockBackend>::new()));
    let backend = Rc::new(RefCell::new(mock));

    let nav: Ref<SwapHandle> = Ref::new();
    let nav_for_app = nav.clone();
    let _owner: Box<dyn Any> = Box::new(runtime_core::mount(backend.clone(), move || {
        let nav = nav_for_app.clone();
        SwapNavigator::new(&HOME)
            .screen(HOME, |_| Screen::new(view(vec![text("HOME CONTENT").into()])))
            .screen(SETTINGS, |_| Screen::new(view(vec![text("SETTINGS CONTENT").into()])))
            .layout(|nav| {
                ui! {
                    view {
                        { nav.outlet }
                        TabBar(
                            items = vec![
                                TabItem::new("home", "Home"),
                                TabItem::new("settings", "Settings"),
                            ],
                            active_route = nav.active_route,
                            on_select = nav.on_select,
                        )
                    }
                }
            })
            .bind(nav)
            .into()
    }));
    runtime_core::drain_buffered_microtasks();

    let b = backend.borrow();
    // Themed bar chrome: both tab labels rendered by the idea-ui Tabs strip.
    assert!(b.contains_text("Home"), "TabBar renders the Home tab label:\n{}", b.dump());
    assert!(b.contains_text("Settings"), "TabBar renders the Settings tab label:\n{}", b.dump());
    // The active screen is in the outlet, wrapped by the bar chrome.
    assert!(b.contains_text("HOME CONTENT"), "initial screen in outlet:\n{}", b.dump());
    assert!(
        !b.contains_text("SETTINGS CONTENT"),
        "inactive screen not mounted (swap shows one):\n{}",
        b.dump()
    );
}
