//! The authored surface — `StackNavigator::new(&route).screen(…)
//! .layout(|nav| …).retention(…).bind(nav)`, per-screen header options
//! via `StackScreenExt` (`Screen::new(x).title("…")`), and
//! `header_state(&nav.screen_chrome)` — mounted through the vocabulary's
//! stack handler on the shared `host-mock` recording substrate
//! (crates/dev/host-mock).
//!
//! Covers: mount, push/pop round-trip through the bound
//! `Ref<StackHandle>`, the header-options carrier (chrome republished
//! per navigation, downcast-or-default), and `link(route = …)` pushing
//! inside a stack screen. Retention semantics + cold deep links live in
//! `stack_local.rs`.

use std::cell::RefCell;
use std::rc::Rc;

use host_mock::Harness;
use runtime_vocabulary::builders::{text, view};
use runtime_vocabulary::glue::{IntoElement, Ref};
use stack_navigator::{
    header_state, Route, Screen, StackBuilder, StackContext, StackHandle, StackNavigator,
    StackRetention, StackScreenExt,
};

fn harness() -> Harness {
    let h = Harness::new();
    // Mirror the historical Mini's recorded op set (creates / insert /
    // clear_children only): style + state families the mock now records
    // stay out of the log so the suite's expectations carry unchanged.
    h.mute(&[
        "apply_style",
        "on_node_unstyled",
        "mark_container",
        "attach_states",
        "set_disabled",
        "attach_html_class",
        "register_stylesheet",
        "unregister_stylesheet",
        "install_tokens",
        "update_tokens",
        "update_text",
    ]);
    h
}

const ROOT: Route<()> = Route::new("root", "/");
const DETAIL: Route<()> = Route::new("detail", "/detail");

struct App {
    nav: Ref<StackHandle>,
    /// Titles the author chrome observed, captured by re-deriving
    /// `header_state` after each flush (the layout closure stores the
    /// signal here).
    chrome: Rc<RefCell<Option<StackContext>>>,
}

/// Mount the SAME-SOURCE authored app: fluent SDK builder, `.title()`
/// header options on the screens, a layout closure taking a
/// `StackContext` and splatting the outlet.
fn mount_app(h: &Harness, retention: StackRetention) -> (runtime_scene::Realized<u32>, App) {
    let nav: Ref<StackHandle> = Ref::new();
    let chrome: Rc<RefCell<Option<StackContext>>> = Rc::new(RefCell::new(None));

    let element = {
        let chrome = chrome.clone();
        StackNavigator::new(&ROOT)
            .screen(ROOT, |_| {
                Screen::new(text().content("root-body").build()).title("Root")
            })
            .screen(DETAIL, |_| {
                // No options: the default (empty-title) header state —
                // the downcast-or-default contract.
                text().content("detail-body").build()
            })
            .retention(retention)
            .layout(move |nav_ctx: StackContext| {
                let outlet = {
                    // Keep the context (signals + pop) for the test;
                    // splat the one-shot outlet.
                    let StackContext {
                        outlet,
                        active_route,
                        active_path,
                        depth,
                        can_go_back,
                        pop,
                        screen_chrome,
                    } = nav_ctx;
                    *chrome.borrow_mut() = Some(StackContext {
                        outlet: view().build(), // placeholder; never splatted
                        active_route,
                        active_path,
                        depth,
                        can_go_back,
                        pop,
                        screen_chrome,
                    });
                    outlet
                };
                view()
                    .child(text().content("header"))
                    .child(outlet)
                    .build()
            })
            .bind(nav)
            .into_element()
    };
    let realized = h.mount(element);
    (realized, App { nav, chrome })
}

#[test]
fn same_source_stack_app_mounts_with_header_options() {
    let h = harness();
    let (_realized, app) = mount_app(&h, StackRetention::Retain);

    let log = h.take_log().join("\n");
    assert!(log.contains("text \"root-body\""), "{log}");
    assert!(log.contains("text \"header\""), "{log}");
    assert!(app.nav.get().is_some(), "Ref<StackHandle> filled at mount");

    // The seated root's options reached the chrome signal: title from
    // `.title("Root")`. The publish is STAGED at mount (the
    // staged-commit model) and commits on the boot flush — every real
    // boot flushes once after realize.
    h.world.flush();
    let chrome = app.chrome.borrow();
    let ctx = chrome.as_ref().expect("layout ran");
    let state = h
        .world
        .enter(|| header_state(&ctx.screen_chrome))
        .expect("header state after seat");
    assert_eq!(state.title, "Root");
    assert!(!state.hidden);
    assert!(!state.native, "backend-neutral handler publishes native=false");
}

#[test]
fn push_pop_round_trip_updates_chrome_and_back_state() {
    let h = harness();
    let (_realized, app) = mount_app(&h, StackRetention::Retain);
    h.take_log();

    // Push (outside enter — event-handler posture), commit on flush.
    app.nav.get().expect("bound").push(&DETAIL, ());
    h.world.flush();
    let log = h.take_log().join("\n");
    assert!(log.contains("text \"detail-body\""), "{log}");

    let chrome = app.chrome.borrow();
    let ctx = chrome.as_ref().expect("layout ran");
    h.world.enter(|| {
        assert_eq!(ctx.depth.get(), 2);
        assert!(ctx.can_go_back.get());
        assert_eq!(ctx.active_route.get(), "detail");
        // The optionless detail screen still publishes a (default)
        // header state — and it REPLACED root's titled one.
        let state = header_state(&ctx.screen_chrome).expect("published on push");
        assert_eq!(state.title, "");
    });

    // Pop via the context's own `pop` (the author back-button path).
    let pop = h.world.enter(|| ctx.pop.clone());
    pop();
    h.world.flush();
    h.world.enter(|| {
        assert_eq!(ctx.depth.get(), 1);
        assert!(!ctx.can_go_back.get());
        assert_eq!(ctx.active_route.get(), "root");
        // Chrome republished the revealed screen's options (rev bump —
        // what the rev stamp exists for: the options value may be equal,
        // and an equality-guarded `set` would swallow the republish).
        let state = header_state(&ctx.screen_chrome).expect("published on pop");
        assert_eq!(state.title, "Root");
    });

    // Popped screen torn down, revealed screen re-shown from RETAINED
    // state (no rebuild): the only creates since push were none.
    let log = h.take_log().join("\n");
    assert!(!log.contains("create"), "retained root not rebuilt: {log}");
}

/// A `link(route = …)` inside a STACK screen PUSHES (the stack half of
/// the link-activator contract; the swap half rewrites to `Select`).
#[test]
fn link_route_pushes_in_a_stack_screen() {
    let h = harness();
    let nav: Ref<StackHandle> = Ref::new();
    let element = StackNavigator::new(&ROOT)
        .screen(ROOT, |_| {
            Screen::new(
                runtime_vocabulary::glue::primitives::link::link(
                    &DETAIL,
                    (),
                    vec![text().content("go-detail").build()],
                )
                .into_element(),
            )
        })
        .screen(DETAIL, |_| text().content("detail-body").build())
        .retention(StackRetention::Retain)
        .bind(nav)
        .into_element();
    let _realized = h.mount(element);
    h.take_log();

    let activate = h.shared.link_activations.borrow()[0].clone();
    activate();
    h.world.flush();

    let log = h.take_log().join("\n");
    assert!(log.contains("text \"detail-body\""), "link pushed: {log}");
    // Depth grew — prove it was a PUSH (pop reveals the link screen).
    app_pop(&h, &nav);
    let log = h.take_log().join("\n");
    assert!(log.contains("clear_children"), "pop revealed root: {log}");
}

fn app_pop(h: &Harness, nav: &Ref<StackHandle>) {
    nav.get().expect("bound").pop();
    h.world.flush();
}
