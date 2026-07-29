//! New-core suite (P6 SDK retarget): the SAME authored surface —
//! `StackNavigator::new(&route).screen(…).layout(|nav| …)
//! .retention(…).bind(nav)`, per-screen header options via
//! `StackScreenExt` (`Screen::new(x).title("…")`), and
//! `header_state(&nav.screen_chrome)` — mounted through the
//! vocabulary's stack handler on a minimal recording backend. Covers:
//! same-source mount, push/pop round-trip through the bound
//! `Ref<StackHandle>`, the header-options carrier (chrome republished
//! per navigation, downcast-or-default), and `link(route = …)` pushing
//! inside a stack screen.
#![cfg(feature = "new-core")]

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use runtime_core::accessibility::AccessibilityProps;
use runtime_core::primitives::link::LinkConfig;
use runtime_core::{Backend, StyleRules};
use runtime_scene::{realize, Registry};
use runtime_vocabulary::builders::{text, view};
use runtime_vocabulary::glue::{IntoElement, Ref};
use runtime_vocabulary::register_builtins;
use runtime_vocabulary::LegacyBridge;
use runtime_world::World;
use stack_navigator::{
    header_state, Route, Screen, StackBuilder, StackContext, StackHandle, StackNavigator,
    StackRetention, StackScreenExt,
};

// ===========================================================================
// Minimal recording backend (vocab-test harness pattern)
// ===========================================================================

type Log = Rc<RefCell<Vec<String>>>;
type Activations = Rc<RefCell<Vec<Rc<dyn Fn()>>>>;

struct Mini {
    log: Log,
    next: u32,
    link_activations: Activations,
}

impl Mini {
    fn mint(&mut self, kind: &str) -> u32 {
        let n = self.next;
        self.next += 1;
        self.log.borrow_mut().push(format!("create n{n} {kind}"));
        n
    }
}

impl Backend for Mini {
    type Node = u32;

    fn create_view(&mut self, _a11y: &AccessibilityProps) -> u32 {
        self.mint("view")
    }

    fn create_text(&mut self, content: &str, _a11y: &AccessibilityProps) -> u32 {
        let n = self.next;
        self.next += 1;
        self.log
            .borrow_mut()
            .push(format!("create n{n} text {content:?}"));
        n
    }

    fn create_button(
        &mut self,
        label: &str,
        _on_click: &runtime_core::Action,
        _leading: Option<&runtime_core::IconData>,
        _trailing: Option<&runtime_core::IconData>,
        _a11y: &AccessibilityProps,
    ) -> u32 {
        let n = self.next;
        self.next += 1;
        self.log
            .borrow_mut()
            .push(format!("create n{n} button {label:?}"));
        n
    }

    fn create_link(&mut self, config: LinkConfig, _a11y: &AccessibilityProps) -> u32 {
        self.link_activations.borrow_mut().push(config.on_activate);
        self.mint("link")
    }

    fn update_text(&mut self, _node: &u32, _content: &str) {}
    fn update_link_url(&mut self, _node: &u32, _url: &str) {}
    fn apply_style(&mut self, _node: &u32, _style: &Rc<StyleRules>) {}
    fn on_node_unstyled(&mut self, _node: &u32) {}
    fn mark_container(&mut self, _node: &u32) {}

    fn insert(&mut self, parent: &mut u32, child: u32) {
        self.log
            .borrow_mut()
            .push(format!("insert n{parent} <- n{child}"));
    }

    fn clear_children(&mut self, node: &u32) {
        self.log.borrow_mut().push(format!("clear_children n{node}"));
    }

    fn finish(&mut self, _root: u32) {}
}

struct Harness {
    world: World,
    backend: Rc<RefCell<LegacyBridge<Mini>>>,
    registry: Rc<Registry<LegacyBridge<Mini>>>,
    log: Log,
    link_activations: Activations,
}

fn harness() -> Harness {
    let log: Log = Rc::new(RefCell::new(Vec::new()));
    let link_activations: Activations = Rc::new(RefCell::new(Vec::new()));
    let backend = Rc::new(RefCell::new(LegacyBridge(Mini {
        log: log.clone(),
        next: 0,
        link_activations: link_activations.clone(),
    })));
    let mut registry = Registry::new();
    register_builtins(&mut registry);
    Harness {
        world: World::new(),
        backend,
        registry: Rc::new(registry),
        log,
        link_activations,
    }
}

impl Harness {
    fn take_log(&self) -> Vec<String> {
        std::mem::take(&mut *self.log.borrow_mut())
    }
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
    let realized = h.world.enter(|| realize(&h.backend, &h.registry, element));
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
    // `.title("Root")`, published exactly like the old
    // `screen_chrome` slot. The publish is STAGED at mount (the new
    // core's staged-commit model) and commits on the boot flush —
    // every real boot flushes once after realize.
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
        // the `set_always` parity the rev stamp exists for).
        let state = header_state(&ctx.screen_chrome).expect("published on pop");
        assert_eq!(state.title, "Root");
    });

    // Popped screen torn down, revealed screen re-shown from RETAINED
    // state (no rebuild): the only creates since push were none.
    let log = h.take_log().join("\n");
    assert!(!log.contains("create"), "retained root not rebuilt: {log}");
}

/// A `link(route = …)` inside a STACK screen PUSHES (the stack half of
/// the activator contract — old links fell back to `Push` on stacks).
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
    let _realized = h.world.enter(|| realize(&h.backend, &h.registry, element));
    h.take_log();

    let activate = h.link_activations.borrow()[0].clone();
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
