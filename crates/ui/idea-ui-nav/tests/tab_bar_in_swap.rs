//! `TabBar` rendered as a real swap-navigator layout.
//!
//! The author intent goes through the vocabulary swap navigator
//! (`runtime_vocabulary::builders::swap_navigator` + the `SwapNav` world
//! context + `navigator_outlet`), mounted on `host_mock::Harness` — proving
//! idea-ui-nav's chrome consumes the nav surface: the themed bar plus the
//! initial screen render around the outlet, and the inactive screen does not
//! mount.

use std::sync::OnceLock;

use idea_theme::theme::{install_idea_theme, light_theme};
use idea_ui_nav::{TabBar, TabBarProps, TabItem};
use runtime_core::Route;
use runtime_vocabulary::glue::scheduling::{install_scheduler, ScheduleHandle, Scheduler};
use runtime_vocabulary::prims::SwapNav;

const HOME: Route<()> = Route::<()>::new("home", "/");
const SETTINGS: Route<()> = Route::<()>::new("settings", "/settings");

/// Immediate-microtask scheduler: the swap handler's deferred work runs
/// inline so the mount op-log is complete when we assert.
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

    let h = host_mock::Harness::new();
    let root = h.world.enter(|| {
        install_idea_theme(light_theme());
        runtime_vocabulary::builders::swap_navigator(&HOME)
            .screen(HOME, |_| {
                runtime_vocabulary::builders::view()
                    .child(runtime_vocabulary::builders::text().content("HOME CONTENT").build())
                    .build()
            })
            .screen(SETTINGS, |_| {
                runtime_vocabulary::builders::view()
                    .child(runtime_vocabulary::builders::text().content("SETTINGS CONTENT").build())
                    .build()
            })
            .layout(|| {
                // The chrome is author layout around the outlet: the TabBar
                // reads the navigator's world context (`SwapNav`).
                let nav = runtime_core::inject::<SwapNav>().expect("SwapNav provided by the navigator mount");
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
    let _realized = h.mount(root);
    h.flush();

    let log = h.take_log().join("\n");
    assert!(log.contains("Home"), "TabBar renders the Home tab label:\n{log}");
    assert!(log.contains("Settings"), "TabBar renders the Settings tab label:\n{log}");
    assert!(log.contains("HOME CONTENT"), "initial screen in outlet:\n{log}");
    assert!(
        !log.contains("SETTINGS CONTENT"),
        "inactive screen not mounted (swap shows one):\n{log}"
    );
}
