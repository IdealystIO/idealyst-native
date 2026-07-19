//! Repro for "the sidebar doesn't re-theme": a STATIC token-driven
//! style on a node built inside the navigator's author `.layout`
//! chrome must re-apply when the theme swaps (`update_tokens`),
//! exactly like a node inside a screen does.
//!
//! The chrome is built inside the walker's retained nav-chrome scope
//! (`build_layout_with_outlet`); its static styles register with the
//! theme cohort under that scope. If cohort registration mis-anchors
//! for chrome nodes, everything in the app re-tints on a dark-mode
//! toggle EXCEPT the sidebar/header chrome — the user-visible bug.

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::OnceLock;

use mock_backend::MockBackend;
use runtime_core::primitives::navigator::Screen;
use runtime_core::scheduling::{install_scheduler, ScheduleHandle, Scheduler};
use runtime_core::{
    text, view, Color, Element, IntoElement, Ref, Route, StyleApplication, StyleRules, StyleSheet,
    TokenEntry, TokenValue, Tokenized,
};
use swap_navigator::{SwapBuilder, SwapHandle, SwapHandler, SwapNavigator, SwapPresentation};

const HOME: Route<()> = Route::<()>::new("home", "/");

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

fn chrome_style() -> StyleApplication {
    static KEY: u8 = 0;
    let sheet = runtime_core::cached_stylesheet(&KEY as *const u8 as usize, || {
        Rc::new(StyleSheet::r#static(StyleRules {
            background: Some(Tokenized::token("tok-chrome-bg", Color("#cccccc".into()))),
            ..Default::default()
        }))
    });
    StyleApplication::new(sheet)
}

fn screen_style() -> StyleApplication {
    static KEY: u8 = 0;
    let sheet = runtime_core::cached_stylesheet(&KEY as *const u8 as usize, || {
        Rc::new(StyleSheet::r#static(StyleRules {
            background: Some(Tokenized::token("tok-screen-bg", Color("#dddddd".into()))),
            ..Default::default()
        }))
    });
    StyleApplication::new(sheet)
}

fn bg_is(rules: &StyleRules, hex: &str) -> bool {
    rules
        .background
        .as_ref()
        .map(|t| t.resolve().0 == hex)
        .unwrap_or(false)
}

#[test]
fn regression_chrome_static_token_style_reapplies_on_theme_swap() {
    install_buffering_scheduler();

    // Light "theme".
    runtime_core::update_tokens(&[
        TokenEntry { name: "tok-chrome-bg", value: TokenValue::Color(Color("#ffffff".into())) },
        TokenEntry { name: "tok-screen-bg", value: TokenValue::Color(Color("#fefefe".into())) },
    ]);

    let mut mock = MockBackend::new();
    mock.register_navigator::<SwapPresentation, _>(|| Box::new(SwapHandler::<MockBackend>::new()));
    let backend = Rc::new(RefCell::new(mock));

    let nav: Ref<SwapHandle> = Ref::new();
    let nav_for_app = nav.clone();
    let _owner: Box<dyn Any> = Box::new(runtime_core::mount(backend.clone(), move || {
        let nav = nav_for_app.clone();
        SwapNavigator::new(&HOME)
            .screen(HOME, |_| {
                Screen::new(
                    view(vec![text("CONTENT").into()])
                        .with_style(screen_style())
                        .into_element(),
                )
            })
            // Author chrome: a token-styled "sidebar" next to the outlet.
            .layout(|ctx| {
                let sidebar = view(vec![text("SIDEBAR").into()])
                    .with_style(chrome_style())
                    .into_element();
                view(vec![sidebar, ctx.outlet]).into_element()
            })
            .bind(nav)
            .into()
    }));
    runtime_core::drain_buffered_microtasks();

    {
        let b = backend.borrow();
        assert!(b.any_node_styled(|r| bg_is(r, "#ffffff")), "chrome bg at light:\n{}", b.dump());
        assert!(b.any_node_styled(|r| bg_is(r, "#fefefe")), "screen bg at light:\n{}", b.dump());
    }

    // Dark "theme" swap.
    runtime_core::update_tokens(&[
        TokenEntry { name: "tok-chrome-bg", value: TokenValue::Color(Color("#111111".into())) },
        TokenEntry { name: "tok-screen-bg", value: TokenValue::Color(Color("#222222".into())) },
    ]);
    runtime_core::drain_buffered_microtasks();

    let b = backend.borrow();
    assert!(
        b.any_node_styled(|r| bg_is(r, "#222222")),
        "screen node re-applied on theme swap (control):\n{}",
        b.dump()
    );
    assert!(
        b.any_node_styled(|r| bg_is(r, "#111111")),
        "CHROME node re-applied on theme swap — the sidebar-stays-light bug:\n{}",
        b.dump()
    );
}

/// The REAL repro shape: the cohort driver must survive screen
/// disposal. Before the fix the driver was installed lazily at the
/// first static-style attach — under the outlet model that's the
/// first SCREEN's scope — so `MountPolicy::LazyDisposing` navigation
/// dropped the driver and wiped the whole cohort map, orphaning the
/// still-mounted CHROME. A theme toggle after any navigation then
/// re-tinted the (freshly registered) screen content but left the
/// sidebar/header on the old theme.
#[test]
fn regression_chrome_retints_after_lazy_disposing_navigation() {
    use swap_navigator::MountPolicy;
    const OTHER: Route<()> = Route::<()>::new("other", "/other");

    install_buffering_scheduler();

    runtime_core::update_tokens(&[
        TokenEntry { name: "tok-chrome-bg", value: TokenValue::Color(Color("#ffffff".into())) },
        TokenEntry { name: "tok-screen-bg", value: TokenValue::Color(Color("#fefefe".into())) },
    ]);

    let mut mock = MockBackend::new();
    mock.register_navigator::<SwapPresentation, _>(|| Box::new(SwapHandler::<MockBackend>::new()));
    let backend = Rc::new(RefCell::new(mock));

    let nav: Ref<SwapHandle> = Ref::new();
    let nav_for_app = nav.clone();
    let _owner: Box<dyn Any> = Box::new(runtime_core::mount(backend.clone(), move || {
        let nav = nav_for_app.clone();
        SwapNavigator::new(&HOME)
            .screen(HOME, |_| {
                Screen::new(
                    view(vec![text("HOME").into()])
                        .with_style(screen_style())
                        .into_element(),
                )
            })
            .screen(OTHER, |_| {
                Screen::new(
                    view(vec![text("OTHER").into()])
                        .with_style(screen_style())
                        .into_element(),
                )
            })
            .mount_policy(MountPolicy::LazyDisposing)
            .layout(|ctx| {
                let sidebar = view(vec![text("SIDEBAR").into()])
                    .with_style(chrome_style())
                    .into_element();
                view(vec![sidebar, ctx.outlet]).into_element()
            })
            .bind(nav)
            .into()
    }));
    runtime_core::drain_buffered_microtasks();

    // Navigate: disposes HOME's scope. Before the fix this also killed
    // the theme-cohort driver (owned by HOME's scope) and wiped the
    // chrome's cohort entry.
    nav.get().expect("handle").select(&OTHER, ());
    runtime_core::drain_buffered_microtasks();

    // Snapshot the chrome node's apply count BEFORE the swap. (The
    // rules the mock stores keep their `Tokenized::Token`s, so
    // asserting on a re-resolved VALUE would pass vacuously — the
    // token registry already has the new value. What the bug broke is
    // the RE-APPLY reaching the backend, so count applies.)
    let (chrome_node, chrome_applies_before) = {
        let b = backend.borrow();
        let text_id = b.find_node_with_text("SIDEBAR").expect("sidebar text node");
        let styled = b.parent_of(text_id).expect("sidebar styled view");
        (styled, b.node(styled).unwrap().styles_applied)
    };

    // Theme swap AFTER the navigation.
    runtime_core::update_tokens(&[
        TokenEntry { name: "tok-chrome-bg", value: TokenValue::Color(Color("#111111".into())) },
        TokenEntry { name: "tok-screen-bg", value: TokenValue::Color(Color("#222222".into())) },
    ]);
    runtime_core::drain_buffered_microtasks();

    let b = backend.borrow();
    let chrome_applies_after = b.node(chrome_node).unwrap().styles_applied;
    assert!(
        chrome_applies_after > chrome_applies_before,
        "chrome node must RE-APPLY on a theme swap after a LazyDisposing \
         navigation (applies before={chrome_applies_before} after={chrome_applies_after}) \
         — the sidebar-stays-on-old-theme bug:\n{}",
        b.dump()
    );
}

/// Regression: N reactive-styled nodes SHARING one resolution key
/// (same sheet + variants) must ALL keep re-applying across repeated
/// theme swaps.
///
/// Before the fix, a style effect only subscribed to token signals
/// via the reads inside `Tokenized::resolve()` — which happen on
/// resolution-cache MISSES only. Within each fan-out the first
/// resolver of a key subscribed and every later effect hit the cache
/// and went permanently deaf, so each successive theme toggle
/// repainted fewer nodes (on the iOS website, `color-surface` was
/// down to 2-3 subscribers and toggles visibly did nothing; web hid
/// the collapse behind the CSS `var()` cascade). Style effects now
/// subscribe to the tokens VERSION signal unconditionally.
#[test]
fn regression_shared_resolution_key_effects_survive_repeated_theme_swaps() {
    install_buffering_scheduler();

    runtime_core::update_tokens(&[TokenEntry {
        name: "tok-shared-bg",
        value: TokenValue::Color(Color("#ffffff".into())),
    }]);

    fn shared_style() -> StyleApplication {
        static KEY: u8 = 0;
        let sheet = runtime_core::cached_stylesheet(&KEY as *const u8 as usize, || {
            Rc::new(StyleSheet::r#static(StyleRules {
                background: Some(Tokenized::token("tok-shared-bg", Color("#000000".into()))),
                ..Default::default()
            }))
        });
        StyleApplication::new(sheet)
    }

    let backend = Rc::new(RefCell::new(MockBackend::new()));
    let _owner: Box<dyn Any> = Box::new(runtime_core::mount(backend.clone(), move || {
        // Two nodes, same sheet, same (empty) variant set — one shared
        // resolution key. REACTIVE style closures (the ui! shape) so
        // each node owns an Effect.
        let a = view(vec![text("A").into()])
            .with_style(move || shared_style())
            .into_element();
        let b = view(vec![text("B").into()])
            .with_style(move || shared_style())
            .into_element();
        view(vec![a, b]).into_element()
    }));
    runtime_core::drain_buffered_microtasks();

    let (node_a, node_b) = {
        let b = backend.borrow();
        let ta = b.find_node_with_text("A").expect("text A");
        let tb = b.find_node_with_text("B").expect("text B");
        (b.parent_of(ta).expect("view A"), b.parent_of(tb).expect("view B"))
    };
    let applies = |b: &MockBackend, n: u64| b.node(n).unwrap().styles_applied;

    // Two consecutive swaps; BOTH nodes must re-apply on EACH.
    for (round, color) in [("first", "#111111"), ("second", "#eeeeee")] {
        let (a0, b0) = {
            let b = backend.borrow();
            (applies(&b, node_a), applies(&b, node_b))
        };
        runtime_core::update_tokens(&[TokenEntry {
            name: "tok-shared-bg",
            value: TokenValue::Color(Color(color.to_string())),
        }]);
        runtime_core::drain_buffered_microtasks();
        let b = backend.borrow();
        assert!(
            applies(&b, node_a) > a0,
            "node A re-applied on the {round} swap (before={a0} after={})",
            applies(&b, node_a)
        );
        assert!(
            applies(&b, node_b) > b0,
            "node B re-applied on the {round} swap — the cache-hit \
             subscriber-collapse bug (before={b0} after={})",
            applies(&b, node_b)
        );
    }
}
