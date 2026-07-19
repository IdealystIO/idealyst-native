//! Substrate URL-sync coverage for the outlet-model navigators.
//!
//! The legacy web navigators owned their URL work per-SDK; the outlet
//! model delegates it to `runtime_core::primitives::navigator::url_sync`
//! — one shared implementation driven by the commands flowing through
//! `NavigatorControl::dispatch`. These tests install a FAKE platform URL
//! provider (a simulated browser history) and drive the real
//! `SwapHandler` / `StackHandler` on `MockBackend`, asserting:
//!
//! 1. `Select`/`Push` write `pushState`; `Pop` moves the browser back
//!    and the echoed popstate is swallowed (no double pop);
//! 2. a browser-initiated popstate reconciles into ordinary commands
//!    (Pop for a stack, Select for a swap);
//! 3. a popstate whose change belongs to a NESTED navigator's URL slice
//!    does NOT remount the parent's screen (the owned-slice teardown
//!    race the legacy helpers documented);
//! 4. scroll offsets are snapshotted on push and restored on back;
//! 5. a cold start at a deep-link path mounts that screen (the walker's
//!    initial-path resolution, seeded on web by the backend).
//!
//! The buffering scheduler mirrors `swap_navigator_local.rs` — the
//! handlers defer their layout build to a microtask the test drains.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::OnceLock;

use mock_backend::MockBackend;
use runtime_core::primitives::navigator::{
    handle_popstate, install_url_provider, reset_url_sync_for_tests, set_initial_path, Screen,
    UrlProvider,
};
use runtime_core::scheduling::{install_scheduler, ScheduleHandle, Scheduler};
use runtime_core::{text, view, Backend, Element, IntoElement, Ref, Route};
use stack_navigator::{StackBuilder, StackHandle, StackHandler, StackNavigator, StackPresentation};
use swap_navigator::{SwapBuilder, SwapHandle, SwapHandler, SwapNavigator, SwapPresentation};

const HOME: Route<()> = Route::<()>::new("home", "/");
const SETTINGS: Route<()> = Route::<()>::new("settings", "/settings");
const DETAIL: Route<()> = Route::<()>::new("detail", "/detail");
const DOCS: Route<()> = Route::<()>::new("docs", "/docs");

// ---------------------------------------------------------------------------
// Buffering scheduler (same shape as swap_navigator_local.rs)
// ---------------------------------------------------------------------------

thread_local! {
    static MICROTASKS: RefCell<Vec<Box<dyn FnOnce() + 'static>>> = const { RefCell::new(Vec::new()) };
}

struct BufferingScheduler;
// SAFETY: storage is thread-local; the value never crosses threads.
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
    fn after_ms(&self, _delay: i32, _f: Box<dyn FnOnce() + 'static>) -> Box<dyn ScheduleHandle> {
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
        panic!("BufferingScheduler: microtask queue never drained");
    }
}

fn install_buffering_scheduler() {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    INSTALLED.get_or_init(|| install_scheduler(Box::new(BufferingScheduler)));
}

// ---------------------------------------------------------------------------
// Simulated browser history
// ---------------------------------------------------------------------------

/// A fake History API: entry list + index + an op log. `history_back`
/// only moves the index and logs — the popstate the real browser would
/// fire is delivered by the TEST calling [`handle_popstate`], mirroring
/// the async delivery order.
#[derive(Default)]
struct SimHistory {
    entries: Vec<String>,
    index: usize,
    log: Vec<String>,
}

impl SimHistory {
    fn current(&self) -> String {
        self.entries
            .get(self.index)
            .cloned()
            .unwrap_or_else(|| "/".to_string())
    }
}

/// Install the fake provider seeded at `initial`; returns the shared
/// history for assertions. Resets any prior sync state on this thread.
fn install_sim_history(initial: &str) -> Rc<RefCell<SimHistory>> {
    reset_url_sync_for_tests();
    let sim = Rc::new(RefCell::new(SimHistory {
        entries: vec![initial.to_string()],
        index: 0,
        log: Vec::new(),
    }));
    let s1 = sim.clone();
    let s2 = sim.clone();
    let s3 = sim.clone();
    let s4 = sim.clone();
    install_url_provider(UrlProvider {
        current_path: Box::new(move || s1.borrow().current()),
        push_state: Box::new(move |url| {
            let mut h = s2.borrow_mut();
            let idx = h.index;
            h.entries.truncate(idx + 1);
            h.entries.push(url.to_string());
            h.index += 1;
            h.log.push(format!("push:{url}"));
        }),
        replace_state: Box::new(move |url| {
            let mut h = s3.borrow_mut();
            let idx = h.index;
            h.entries[idx] = url.to_string();
            h.log.push(format!("replace:{url}"));
        }),
        history_back: Box::new(move || {
            let mut h = s4.borrow_mut();
            if h.index > 0 {
                h.index -= 1;
            }
            h.log.push("back".to_string());
        }),
    });
    sim
}

/// Simulate the user pressing the browser Back button: move the sim
/// index and deliver the popstate like the browser would.
fn browser_back(sim: &Rc<RefCell<SimHistory>>) {
    {
        let mut h = sim.borrow_mut();
        assert!(h.index > 0, "browser_back below the first entry");
        h.index -= 1;
    }
    let path = sim.borrow().current();
    handle_popstate(&path);
}

/// Simulate browser Forward.
fn browser_forward(sim: &Rc<RefCell<SimHistory>>) {
    {
        let mut h = sim.borrow_mut();
        assert!(h.index + 1 < h.entries.len(), "browser_forward past the last entry");
        h.index += 1;
    }
    let path = sim.borrow().current();
    handle_popstate(&path);
}

// ---------------------------------------------------------------------------
// App builders
// ---------------------------------------------------------------------------

fn swap_backend() -> Rc<RefCell<MockBackend>> {
    let mut mock = MockBackend::new();
    mock.register_navigator::<SwapPresentation, _>(|| Box::new(SwapHandler::<MockBackend>::new()));
    mock.register_navigator::<StackPresentation, _>(|| {
        Box::new(StackHandler::<MockBackend>::new())
    });
    Rc::new(RefCell::new(mock))
}

fn swap_app(nav: Ref<SwapHandle>) -> Element {
    SwapNavigator::new(&HOME)
        .screen(HOME, |_| Screen::new(view(vec![text("HOME CONTENT").into()])))
        .screen(SETTINGS, |_| Screen::new(view(vec![text("SETTINGS CONTENT").into()])))
        .layout(|nav| view(vec![nav.outlet, text("BAR").into_element()]).into_element())
        .bind(nav)
        .into()
}

fn stack_app(nav: Ref<StackHandle>) -> Element {
    StackNavigator::new(&HOME)
        .screen(HOME, |_| Screen::new(view(vec![text("HOME CONTENT").into()])))
        .screen(DETAIL, |_| Screen::new(view(vec![text("DETAIL CONTENT").into()])))
        .bind(nav)
        .into()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// `Select` mirrors into pushState; browser Back reconciles into a
/// `Select` of the previous route (no extra history writes).
#[test]
fn swap_select_pushes_url_and_browser_back_selects_previous() {
    install_buffering_scheduler();
    let sim = install_sim_history("/");
    let backend = swap_backend();
    let nav: Ref<SwapHandle> = Ref::new();
    let _owner = {
        let nav = nav.clone();
        runtime_core::mount(backend.clone(), move || swap_app(nav.clone()))
    };
    runtime_core::drain_buffered_microtasks();

    nav.get().unwrap().select(&SETTINGS, ());
    runtime_core::drain_buffered_microtasks();
    assert!(backend.borrow().contains_text("SETTINGS CONTENT"));
    assert!(
        sim.borrow().log.contains(&"push:/settings".to_string()),
        "Select pushed the URL, log: {:?}",
        sim.borrow().log
    );
    assert_eq!(sim.borrow().current(), "/settings");

    let log_len_before = sim.borrow().log.len();
    browser_back(&sim);
    runtime_core::drain_buffered_microtasks();
    let b = backend.borrow();
    assert!(b.contains_text("HOME CONTENT"), "back re-selects home:\n{}", b.dump());
    assert!(!b.contains_text("SETTINGS CONTENT"), "settings swapped out:\n{}", b.dump());
    assert_eq!(
        sim.borrow().log.len(),
        log_len_before,
        "reconciling a popstate must not write history again, log: {:?}",
        sim.borrow().log
    );
}

/// Browser Forward after Back re-selects the forward route through the
/// navigator's link activator (Select for a swap).
#[test]
fn swap_browser_forward_reselects() {
    install_buffering_scheduler();
    let sim = install_sim_history("/");
    let backend = swap_backend();
    let nav: Ref<SwapHandle> = Ref::new();
    let _owner = {
        let nav = nav.clone();
        runtime_core::mount(backend.clone(), move || swap_app(nav.clone()))
    };
    runtime_core::drain_buffered_microtasks();

    nav.get().unwrap().select(&SETTINGS, ());
    runtime_core::drain_buffered_microtasks();
    browser_back(&sim);
    runtime_core::drain_buffered_microtasks();
    assert!(backend.borrow().contains_text("HOME CONTENT"));

    browser_forward(&sim);
    runtime_core::drain_buffered_microtasks();
    assert!(
        backend.borrow().contains_text("SETTINGS CONTENT"),
        "forward re-selects settings:\n{}",
        backend.borrow().dump()
    );
}

/// `Push` mirrors into pushState; browser Back pops the stack.
#[test]
fn stack_push_pushes_url_and_browser_back_pops() {
    install_buffering_scheduler();
    let sim = install_sim_history("/");
    let backend = swap_backend();
    let nav: Ref<StackHandle> = Ref::new();
    let _owner = {
        let nav = nav.clone();
        runtime_core::mount(backend.clone(), move || stack_app(nav.clone()))
    };
    runtime_core::drain_buffered_microtasks();

    nav.get().unwrap().push(&DETAIL, ());
    runtime_core::drain_buffered_microtasks();
    assert!(backend.borrow().contains_text("DETAIL CONTENT"));
    assert!(
        sim.borrow().log.contains(&"push:/detail".to_string()),
        "Push pushed the URL, log: {:?}",
        sim.borrow().log
    );

    browser_back(&sim);
    runtime_core::drain_buffered_microtasks();
    let b = backend.borrow();
    assert!(b.contains_text("HOME CONTENT"), "back popped to home:\n{}", b.dump());
    assert!(!b.contains_text("DETAIL CONTENT"), "detail released:\n{}", b.dump());
}

/// Regression: a programmatic `pop()` moves the browser back exactly
/// once, and the echoed popstate must NOT pop again (pending-self-pop
/// swallow). Double-popping past the root was the failure mode.
#[test]
fn regression_programmatic_pop_swallows_its_popstate_echo() {
    install_buffering_scheduler();
    let sim = install_sim_history("/");
    let backend = swap_backend();
    let nav: Ref<StackHandle> = Ref::new();
    let _owner = {
        let nav = nav.clone();
        runtime_core::mount(backend.clone(), move || stack_app(nav.clone()))
    };
    runtime_core::drain_buffered_microtasks();

    nav.get().unwrap().push(&DETAIL, ());
    runtime_core::drain_buffered_microtasks();

    nav.get().unwrap().pop();
    runtime_core::drain_buffered_microtasks();
    assert!(backend.borrow().contains_text("HOME CONTENT"), "pop committed synchronously");
    assert_eq!(
        sim.borrow().log.iter().filter(|l| *l == "back").count(),
        1,
        "one history.back(), log: {:?}",
        sim.borrow().log
    );

    // The browser delivers the popstate for OUR back — must be inert.
    let path = sim.borrow().current();
    handle_popstate(&path);
    runtime_core::drain_buffered_microtasks();
    let b = backend.borrow();
    assert!(b.contains_text("HOME CONTENT"), "echo did not pop again:\n{}", b.dump());

    // A root pop is a handler no-op and must not move the browser.
    drop(b);
    let backs_before = sim.borrow().log.iter().filter(|l| *l == "back").count();
    nav.get().unwrap().pop();
    runtime_core::drain_buffered_microtasks();
    assert_eq!(
        sim.borrow().log.iter().filter(|l| *l == "back").count(),
        backs_before,
        "root pop must not history.back() out of the app, log: {:?}",
        sim.borrow().log
    );
}

/// Regression (owned-slice guard): a popstate whose URL change lives in
/// a NESTED navigator's slice must not remount the parent's screen —
/// remounting would tear the nested navigator down mid-transition.
#[test]
fn regression_nested_stack_popstate_does_not_remount_parent_swap_screen() {
    install_buffering_scheduler();
    let sim = install_sim_history("/");
    let backend = swap_backend();
    let nav: Ref<SwapHandle> = Ref::new();
    let stack_nav: Ref<StackHandle> = Ref::new();
    let docs_builds: Rc<Cell<u32>> = Rc::new(Cell::new(0));

    let _owner = {
        let nav = nav.clone();
        let stack_nav = stack_nav.clone();
        let docs_builds = docs_builds.clone();
        runtime_core::mount(backend.clone(), move || {
            let stack_nav = stack_nav.clone();
            let docs_builds = docs_builds.clone();
            SwapNavigator::new(&HOME)
                .screen(HOME, |_| Screen::new(view(vec![text("HOME CONTENT").into()])))
                .screen(DOCS, move |_| {
                    docs_builds.set(docs_builds.get() + 1);
                    // Nested stack under /docs: index route "" + /detail.
                    const INDEX: Route<()> = Route::<()>::new("index", "");
                    const NESTED_DETAIL: Route<()> = Route::<()>::new("ndetail", "/detail");
                    Screen::new(
                        StackNavigator::new(&INDEX)
                            .screen(INDEX, |_| {
                                Screen::new(view(vec![text("DOCS INDEX").into()]))
                            })
                            .screen(NESTED_DETAIL, |_| {
                                Screen::new(view(vec![text("NESTED DETAIL").into()]))
                            })
                            .bind(stack_nav.clone()),
                    )
                })
                .layout(|nav| view(vec![nav.outlet]).into_element())
                .bind(nav.clone())
                .into()
        })
    };
    runtime_core::drain_buffered_microtasks();

    // Enter the docs tab, then push the nested detail.
    nav.get().unwrap().select(&DOCS, ());
    runtime_core::drain_buffered_microtasks();
    assert!(backend.borrow().contains_text("DOCS INDEX"));
    assert_eq!(docs_builds.get(), 1);

    const NESTED_DETAIL: Route<()> = Route::<()>::new("ndetail", "/detail");
    stack_nav.get().unwrap().push(&NESTED_DETAIL, ());
    runtime_core::drain_buffered_microtasks();
    assert!(backend.borrow().contains_text("NESTED DETAIL"));
    assert_eq!(sim.borrow().current(), "/docs/detail", "nested push composed its base");

    // Browser back to /docs: the NESTED stack pops; the parent swap's
    // owned slice (/docs) is unchanged and must not remount its screen.
    browser_back(&sim);
    runtime_core::drain_buffered_microtasks();
    let b = backend.borrow();
    assert!(b.contains_text("DOCS INDEX"), "nested stack popped to index:\n{}", b.dump());
    assert!(!b.contains_text("NESTED DETAIL"), "nested detail released:\n{}", b.dump());
    assert_eq!(
        docs_builds.get(),
        1,
        "parent swap screen must NOT rebuild when only the nested slice changed"
    );
}

/// Scroll is snapshotted on push and restored on browser back; forward
/// navigations land at the top.
#[test]
fn stack_scroll_snapshot_restores_on_browser_back() {
    install_buffering_scheduler();
    let sim = install_sim_history("/");
    let backend = swap_backend();
    let nav: Ref<StackHandle> = Ref::new();
    let _owner = {
        let nav = nav.clone();
        runtime_core::mount(backend.clone(), move || stack_app(nav.clone()))
    };
    runtime_core::drain_buffered_microtasks();

    // The outlet is the parent of the home screen's root view.
    let outlet = {
        let b = backend.borrow();
        let text_node = b.find_node_with_text("HOME CONTENT").expect("home text");
        let screen_root = b.parent_of(text_node).expect("screen root");
        b.parent_of(screen_root).expect("outlet")
    };

    // The user scrolled home before navigating away.
    backend.borrow_mut().set_node_scroll(&outlet, 0.0, 120.0);

    nav.get().unwrap().push(&DETAIL, ());
    runtime_core::drain_buffered_microtasks();
    assert_eq!(
        backend.borrow().node(outlet).unwrap().scroll,
        (0.0, 0.0),
        "fresh screen starts at the top"
    );

    browser_back(&sim);
    runtime_core::drain_buffered_microtasks();
    assert_eq!(
        backend.borrow().node(outlet).unwrap().scroll,
        (0.0, 120.0),
        "back restores the scroll the user left home at"
    );
}

/// Regression: a cold start at a URL whose tail belongs to a NESTED
/// navigator (`/docs/detail` — parent owns `/docs`, nested stack owns
/// `/detail`) must mount the nested screen AND leave the full URL
/// intact. The root's history-claim seed originally replaced the entry
/// with its OWN owned slice, clobbering the nested remainder (a cold
/// `/alerts` load was rewritten to `/`).
#[test]
fn regression_cold_start_nested_deep_link_preserves_full_url() {
    install_buffering_scheduler();
    let sim = install_sim_history("/docs/detail");
    set_initial_path(Some("/docs/detail".to_string()));

    let backend = swap_backend();
    let nav: Ref<SwapHandle> = Ref::new();
    let stack_nav: Ref<StackHandle> = Ref::new();
    let _owner = {
        let nav = nav.clone();
        let stack_nav = stack_nav.clone();
        runtime_core::mount(backend.clone(), move || {
            let stack_nav = stack_nav.clone();
            SwapNavigator::new(&HOME)
                .screen(HOME, |_| Screen::new(view(vec![text("HOME CONTENT").into()])))
                .screen(DOCS, move |_| {
                    const INDEX: Route<()> = Route::<()>::new("index", "");
                    const NESTED_DETAIL: Route<()> = Route::<()>::new("ndetail", "/detail");
                    Screen::new(
                        StackNavigator::new(&INDEX)
                            .screen(INDEX, |_| {
                                Screen::new(view(vec![text("DOCS INDEX").into()]))
                            })
                            .screen(NESTED_DETAIL, |_| {
                                Screen::new(view(vec![text("NESTED DETAIL").into()]))
                            })
                            .bind(stack_nav.clone()),
                    )
                })
                .layout(|nav| view(vec![nav.outlet]).into_element())
                .bind(nav.clone())
                .into()
        })
    };
    runtime_core::drain_buffered_microtasks();

    let b = backend.borrow();
    assert!(
        b.contains_text("NESTED DETAIL"),
        "cold deep link mounted the nested screen:\n{}",
        b.dump()
    );
    drop(b);
    assert_eq!(
        sim.borrow().current(),
        "/docs/detail",
        "root history-claim must not clobber the nested URL slice, log: {:?}",
        sim.borrow().log
    );
}

/// Regression (docs-app catalog bug): selecting a DIFFERENT URL of the
/// SAME parameterized route must swap the screen — the swap's screen
/// cache and its no-op guard are URL-keyed, not route-name-keyed. Also:
/// re-selecting the active URL is a no-op that must NOT push a
/// duplicate browser-history entry.
#[test]
fn regression_swap_parameterized_route_selects_by_url_not_name() {
    use runtime_core::primitives::navigator::RouteParams;
    use std::collections::HashMap as Map;

    #[derive(Clone)]
    struct EntryParams {
        name: String,
    }
    impl RouteParams for EntryParams {
        fn to_path(&self, pattern: &str) -> String {
            pattern.replace(":name", &self.name)
        }
        fn from_segments(segs: &Map<String, String>) -> Option<Self> {
            segs.get("name").map(|n| EntryParams { name: n.clone() })
        }
    }

    install_buffering_scheduler();
    let sim = install_sim_history("/");
    let backend = swap_backend();
    let nav: Ref<SwapHandle> = Ref::new();
    let _owner = {
        let nav = nav.clone();
        runtime_core::mount(backend.clone(), move || {
            const ENTRY: Route<EntryParams> = Route::<EntryParams>::new("entry", "/entry/:name");
            SwapNavigator::new(&HOME)
                .screen(HOME, |_| Screen::new(view(vec![text("OVERVIEW").into()])))
                .screen(ENTRY, |p: EntryParams| {
                    Screen::new(view(vec![text(format!("ENTRY {}", p.name)).into()]))
                })
                .layout(|nav| view(vec![nav.outlet]).into_element())
                .bind(nav.clone())
                .into()
        })
    };
    runtime_core::drain_buffered_microtasks();

    const ENTRY: Route<EntryParams> = Route::<EntryParams>::new("entry", "/entry/:name");
    let handle = nav.get().unwrap();
    handle.select(&ENTRY, EntryParams { name: "button".into() });
    runtime_core::drain_buffered_microtasks();
    assert!(backend.borrow().contains_text("ENTRY button"));
    assert_eq!(sim.borrow().current(), "/entry/button");

    // Same route NAME, different URL — must swap (the name-keyed cache
    // used to serve the stale "button" screen and the name-keyed no-op
    // guard used to skip entirely).
    handle.select(&ENTRY, EntryParams { name: "card".into() });
    runtime_core::drain_buffered_microtasks();
    let b = backend.borrow();
    assert!(b.contains_text("ENTRY card"), "same-route select swapped:\n{}", b.dump());
    assert!(!b.contains_text("ENTRY button"), "stale entry swapped out:\n{}", b.dump());
    drop(b);
    assert_eq!(sim.borrow().current(), "/entry/card");

    // Re-selecting the ACTIVE URL: no screen change, and no duplicate
    // history entry.
    let pushes_before = sim.borrow().log.iter().filter(|l| l.starts_with("push:")).count();
    handle.select(&ENTRY, EntryParams { name: "card".into() });
    runtime_core::drain_buffered_microtasks();
    assert_eq!(
        sim.borrow().log.iter().filter(|l| l.starts_with("push:")).count(),
        pushes_before,
        "re-selecting the active URL must not push history, log: {:?}",
        sim.borrow().log
    );

    // Browser back walks the entry history: card → button.
    browser_back(&sim);
    runtime_core::drain_buffered_microtasks();
    assert!(backend.borrow().contains_text("ENTRY button"), "back re-selects the previous entry");
}

/// Cold start at a deep-link URL mounts that screen (the walker's
/// initial-path resolution — on web the backend seeds it from
/// `window.location`, here the test seeds it directly), and the root
/// seed claims the history entry via replaceState.
#[test]
fn cold_start_deep_link_mounts_url_screen() {
    install_buffering_scheduler();
    let sim = install_sim_history("/settings");
    set_initial_path(Some("/settings".to_string()));

    let backend = swap_backend();
    let nav: Ref<SwapHandle> = Ref::new();
    let _owner = {
        let nav = nav.clone();
        runtime_core::mount(backend.clone(), move || swap_app(nav.clone()))
    };
    runtime_core::drain_buffered_microtasks();

    let b = backend.borrow();
    assert!(
        b.contains_text("SETTINGS CONTENT"),
        "deep link mounted the URL's screen, not the configured initial:\n{}",
        b.dump()
    );
    assert!(!b.contains_text("HOME CONTENT"), "home not mounted:\n{}", b.dump());
    assert!(
        sim.borrow().log.iter().any(|l| l == "replace:/settings"),
        "root seed claimed the history entry, log: {:?}",
        sim.borrow().log
    );
}
