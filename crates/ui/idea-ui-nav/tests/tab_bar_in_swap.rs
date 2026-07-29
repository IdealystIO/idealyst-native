//! `TabBar` rendered as a real swap-navigator layout — BOTH cores.
//!
//! Old core: mounts the swap-navigator SDK on `MockBackend` and asserts
//! the themed bar + initial screen render around the outlet. New core:
//! the SAME author intent against the VOCABULARY swap navigator
//! (`runtime_vocabulary::builders::swap_navigator` + `SwapNav` world
//! context + `navigator_outlet`) mounted on the scene-parity mock host —
//! proving idea-ui-nav's chrome consumes the new nav surface. The two
//! harnesses fork per core because the navigator RUNTIMES differ (the
//! nav SDK's own retarget is a later P6 wave); `TabBar` itself is
//! same-source.

// The new-core leg needs the scene-parity harness, which lives behind
// `new-core-harness` (NOT `new-core` — a consumer app graph must never
// drag scene-parity, whose old-side scenarios pin the SDKs' old-core
// surface; see Cargo.toml). Plain `new-core` compiles this file empty.
#![cfg(any(not(feature = "new-core"), feature = "new-core-harness"))]

#[cfg(not(feature = "new-core"))]
mod old_core {
    use std::any::Any;
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::OnceLock;

    use idea_theme::theme::{install_idea_theme, light_theme};
    use idea_ui_nav::{TabBar, TabBarProps, TabItem};
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
}

#[cfg(feature = "new-core")]
mod new_core {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::OnceLock;

    use idea_theme::theme::{install_idea_theme, light_theme};
    use idea_ui_nav::{TabBar, TabBarProps, TabItem};
    use runtime_core::Route;
    use runtime_scene::{realize, Registry};
    use runtime_vocabulary::glue::scheduling::{install_scheduler, ScheduleHandle, Scheduler};
    use runtime_vocabulary::prims::SwapNav;
    use runtime_vocabulary::LegacyBridge;
    use runtime_world::{inject, World};
    use scene_parity::full::FullRecorder;
    use scene_parity::{Mode, Recorder};

    const HOME: Route<()> = Route::<()>::new("home", "/");
    const SETTINGS: Route<()> = Route::<()>::new("settings", "/settings");

    /// Immediate-microtask scheduler (newcore-app e2e pattern): the swap
    /// handler's deferred work runs inline so the mount op-log is
    /// complete when we assert.
    struct InlineScheduler;
    struct InertHandle;
    impl ScheduleHandle for InertHandle {
        fn cancel(&mut self) {}
    }
    impl Scheduler for InlineScheduler {
        fn schedule_microtask(&self, f: Box<dyn FnOnce()>) {
            f();
        }
        fn after_animation_frame(&self, _f: Box<dyn FnOnce()>) -> Box<dyn ScheduleHandle> {
            Box::new(InertHandle)
        }
        fn after_ms(&self, _d: i32, _f: Box<dyn FnOnce()>) -> Box<dyn ScheduleHandle> {
            Box::new(InertHandle)
        }
        fn raf_loop(&self, _f: Box<dyn FnMut()>) -> Box<dyn ScheduleHandle> {
            Box::new(InertHandle)
        }
    }

    fn setup() {
        static INSTALLED: OnceLock<()> = OnceLock::new();
        INSTALLED.get_or_init(|| install_scheduler(Box::new(InlineScheduler)));
    }

    #[test]
    fn tab_bar_renders_as_swap_layout_chrome() {
        setup();

        type Bridged = LegacyBridge<FullRecorder>;
        let rec = Recorder::default();
        let backend: Rc<RefCell<Bridged>> = Rc::new(RefCell::new(LegacyBridge(
            FullRecorder::new(rec.clone(), Mode::Spliced),
        )));
        let mut registry: Registry<Bridged> = Registry::new();
        runtime_vocabulary::register_builtins(&mut registry);
        let registry = Rc::new(registry);

        let world = World::new();
        let root = world.enter(|| {
            install_idea_theme(light_theme());
            runtime_vocabulary::builders::swap_navigator(&HOME)
                .screen(HOME, |_| {
                    runtime_vocabulary::builders::view()
                        .child(runtime_vocabulary::builders::text().content("HOME CONTENT").build())
                        .build()
                })
                .screen(SETTINGS, |_| {
                    runtime_vocabulary::builders::view()
                        .child(
                            runtime_vocabulary::builders::text().content("SETTINGS CONTENT").build(),
                        )
                        .build()
                })
                .layout(|| {
                    // The chrome is author layout around the outlet: the
                    // TabBar reads the navigator's world context
                    // (`SwapNav`) — the vocabulary mirror of the old
                    // `SwapContext` wiring.
                    let nav = inject::<SwapNav>().expect("SwapNav provided by the navigator mount");
                    let bar = TabBarProps {
                        items: vec![
                            TabItem::new("home", "Home"),
                            TabItem::new("settings", "Settings"),
                        ],
                        active_route: nav.active_route,
                        on_select: nav.on_select.clone(),
                        ..Default::default()
                    };
                    runtime_vocabulary::builders::view()
                        .child(runtime_vocabulary::navigator_outlet().build())
                        .child(TabBar(bar))
                        .build()
                })
                .build()
        });
        let _realized = world.enter(|| realize(&backend, &registry, root));
        world.flush();

        let log = rec.take_ops().join("\n");
        assert!(log.contains("Home"), "TabBar renders the Home tab label:\n{log}");
        assert!(log.contains("Settings"), "TabBar renders the Settings tab label:\n{log}");
        assert!(log.contains("HOME CONTENT"), "initial screen in outlet:\n{log}");
        assert!(
            !log.contains("SETTINGS CONTENT"),
            "inactive screen not mounted (swap shows one):\n{log}"
        );
    }
}
