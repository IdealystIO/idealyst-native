//! New-core suite (P6 SDK retarget): the SAME authored surface —
//! `SwapNavigator::new(&route).screen(…).layout(|nav| …)
//! .mount_policy(…).bind(nav)` — mounted through the vocabulary's
//! swap handler on a minimal recording backend (the vocab-test harness
//! pattern: old-core `Backend` mock bridged through `LegacyBridge`).
//! Covers: same-source mount + op-log shape, select round-trip through
//! the bound `Ref<SwapHandle>`, URL-sync registration through the SDK
//! path, and `link(route = …)` end-to-end (glue constructor → ambient
//! `LinkActivator` → Select).
#![cfg(feature = "new-core")]

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use runtime_core::accessibility::AccessibilityProps;
use runtime_core::primitives::link::LinkConfig;
use runtime_core::primitives::navigator::NavCommand;
use runtime_core::{Backend, StyleRules};
use runtime_scene::{realize, Realized, Registry};
use runtime_vocabulary::builders::{text, view};
use runtime_vocabulary::handlers::nav_url_sync::{
    clear_url_sync_service, install_url_sync_service, CommittedKind, NavSyncKind,
    NavSyncRegistration, UrlSyncService,
};
use runtime_vocabulary::register_builtins;
use runtime_vocabulary::LegacyBridge;
use runtime_world::World;
use swap_navigator::{
    MountPolicy, Route, Screen, SwapBuilder, SwapContext, SwapHandle, SwapNavigator,
};

// `Ref` + `IntoElement` are glue surface (a same-source app gets them
// from its per-core prelude).
use runtime_vocabulary::glue::{IntoElement, Ref};

// ===========================================================================
// Minimal recording backend (vocab-test harness pattern)
// ===========================================================================

type Log = Rc<RefCell<Vec<String>>>;
type Activations = Rc<RefCell<Vec<Rc<dyn Fn()>>>>;

struct Mini {
    log: Log,
    next: u32,
    /// Link activations the backend received (`create_link`), so the
    /// test can fire a "user click" like a real platform would.
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
        let n = self.next;
        self.next += 1;
        self.log
            .borrow_mut()
            .push(format!("create n{n} link route={:?} url={:?}", config.route, config.url));
        n
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

const HOME: Route<()> = Route::new("home", "/");
const ABOUT: Route<()> = Route::new("about", "/about");

/// Mount the SAME-SOURCE authored app: fluent SDK builder, layout
/// closure taking a `SwapContext` (outlet + reactive state + on_select),
/// `Screen::new(...)` render closures, `.bind(Ref<SwapHandle>)`.
fn mount_app(h: &Harness, builds: (Rc<Cell<u32>>, Rc<Cell<u32>>)) -> (Realized<u32>, Ref<SwapHandle>) {
    let (builds_home, builds_about) = builds;
    let nav: Ref<SwapHandle> = Ref::new();

    let element = {
        let nav = nav;
        SwapNavigator::new(&HOME)
            .screen(HOME, move |_| {
                builds_home.set(builds_home.get() + 1);
                Screen::new(text().content("home-body").build())
            })
            .screen(ABOUT, move |_| {
                builds_about.set(builds_about.get() + 1);
                Screen::new(text().content("about-body").build())
            })
            .mount_policy(MountPolicy::LazyPersistent)
            .layout(move |nav_ctx: SwapContext| {
                // Chrome reads the reactive state + splats the outlet —
                // the website's authored shape.
                let _route_now = nav_ctx.active_route;
                view()
                    .child(text().content("chrome"))
                    .child(nav_ctx.outlet)
                    .build()
            })
            .bind(nav)
            .into_element()
    };
    let realized = h.world.enter(|| realize(&h.backend, &h.registry, element));
    (realized, nav)
}

#[test]
fn same_source_swap_app_mounts_and_binds_the_ref() {
    let h = harness();
    let builds_home = Rc::new(Cell::new(0));
    let builds_about = Rc::new(Cell::new(0));
    let (_realized, nav) = mount_app(&h, (builds_home.clone(), builds_about.clone()));

    // The initial screen mounted exactly once, chrome built, and the
    // screen body sits in the outlet (op-log shape: home text created
    // before the chrome, then inserted into the outlet view).
    assert_eq!(builds_home.get(), 1);
    assert_eq!(builds_about.get(), 0);
    let log = h.take_log().join("\n");
    assert!(log.contains("text \"home-body\""), "{log}");
    assert!(log.contains("text \"chrome\""), "{log}");

    // `.bind` filled the Ref with a live typed handle.
    assert!(nav.get().is_some(), "Ref<SwapHandle> filled at mount");
}

#[test]
fn select_round_trip_via_bound_handle() {
    let h = harness();
    let builds_home = Rc::new(Cell::new(0));
    let builds_about = Rc::new(Cell::new(0));
    let (_realized, nav) = mount_app(&h, (builds_home.clone(), builds_about.clone()));
    h.take_log();

    // Event-handler posture: dispatch OUTSIDE enter, commit on flush.
    nav.get().expect("bound").select(&ABOUT, ());
    h.world.flush();
    assert_eq!(builds_about.get(), 1, "about mounted on first select");
    let log = h.take_log().join("\n");
    assert!(log.contains("text \"about-body\""), "{log}");
    assert!(log.contains("clear_children"), "outlet swapped: {log}");

    // Back to home: cached (LazyPersistent) — the builder must NOT
    // re-run; the node re-inserts.
    nav.get().expect("bound").select(&HOME, ());
    h.world.flush();
    assert_eq!(builds_home.get(), 1, "cached screen not rebuilt");
    let log = h.take_log().join("\n");
    assert!(log.contains("clear_children"), "{log}");
    assert!(!log.contains("create"), "no new nodes on cached return: {log}");
}

// ===========================================================================
// URL-sync registration through the SDK path
// ===========================================================================

#[derive(Default)]
struct SyncProbe {
    registered: RefCell<Vec<(NavSyncKind, String, String, usize)>>,
    before: RefCell<Vec<String>>,
    after: RefCell<Vec<CommittedKind>>,
    deregistered: Cell<u32>,
}

struct ProbeService(Rc<SyncProbe>);

impl UrlSyncService for ProbeService {
    fn register(&self, reg: NavSyncRegistration) -> Option<u64> {
        self.0.registered.borrow_mut().push((
            reg.kind,
            reg.base.clone(),
            reg.active_path.clone(),
            reg.depth,
        ));
        Some(7)
    }
    fn before_command(&self, id: u64, cmd: &NavCommand) -> bool {
        assert_eq!(id, 7);
        if let NavCommand::Select { url, .. } = cmd {
            self.0.before.borrow_mut().push(url.clone());
        }
        false
    }
    fn after_commit(&self, id: u64, kind: CommittedKind) {
        assert_eq!(id, 7);
        self.0.after.borrow_mut().push(kind);
    }
    fn deregister(&self, id: u64) {
        assert_eq!(id, 7);
        self.0.deregistered.set(self.0.deregistered.get() + 1);
    }
}

/// The SDK-built navigator registers with the host's URL-sync service
/// (kind + committed initial), hooks every dispatch and commit, and
/// deregisters at teardown — the seam the web backend's
/// `newcore_url_sync` rides. Regression for the SDK path specifically:
/// the old SDK opted in via `control.enable_url_sync()`; on the new
/// core registration must happen with NO SDK call.
#[test]
fn url_sync_registration_through_the_sdk_path() {
    let h = harness();
    let probe: Rc<SyncProbe> = Rc::default();
    install_url_sync_service(Rc::new(ProbeService(probe.clone())));

    let (realized, nav) = mount_app(&h, (Rc::new(Cell::new(0)), Rc::new(Cell::new(0))));
    {
        let regs = probe.registered.borrow();
        assert_eq!(regs.len(), 1, "one navigator registered");
        let (kind, base, active, depth) = &regs[0];
        assert_eq!(*kind, NavSyncKind::Swap);
        assert_eq!(base, "");
        assert_eq!(active, "/");
        assert_eq!(*depth, 1, "swap is depth-less");
    }

    nav.get().expect("bound").select(&ABOUT, ());
    h.world.flush();
    assert_eq!(probe.before.borrow().as_slice(), ["/about"]);
    assert_eq!(probe.after.borrow().as_slice(), [CommittedKind::Forward]);

    h.world.enter(|| drop(realized));
    assert_eq!(probe.deregistered.get(), 1, "deregistered at teardown");
    clear_url_sync_service();
}

// ===========================================================================
// link(route = …) end-to-end
// ===========================================================================

/// A `link(route = …)` (the glue constructor `ui!` lowers to) inside a
/// swap screen resolves the ambient `LinkActivator` at mount and
/// Selects — "links switch, never push", the old swap activator
/// contract — through the staged dispatch (commit on flush).
#[test]
fn link_route_selects_in_a_swap_screen() {
    let h = harness();
    let builds_about = Rc::new(Cell::new(0));
    let nav: Ref<SwapHandle> = Ref::new();

    let element = {
        let builds_about = builds_about.clone();
        SwapNavigator::new(&HOME)
            .screen(HOME, move |_| {
                // The authored `ui! { link(route = ABOUT) { text { … } } }`
                // shape, post-macro: the glue three-positional
                // constructor.
                Screen::new(
                    runtime_vocabulary::glue::primitives::link::link(
                        &ABOUT,
                        (),
                        vec![text().content("go-about").build()],
                    )
                    .into_element(),
                )
            })
            .screen(ABOUT, move |_| {
                builds_about.set(builds_about.get() + 1);
                Screen::new(text().content("about-body").build())
            })
            .bind(nav)
            .into_element()
    };
    let _realized = h.world.enter(|| realize(&h.backend, &h.registry, element));

    // The backend saw a real in-app link with route metadata + href.
    let log = h.take_log().join("\n");
    assert!(log.contains("link route=\"about\" url=\"/about\""), "{log}");

    // Fire the platform activation (outside enter, like a click) and
    // flush — the swap must SELECT the about screen.
    let activate = h.link_activations.borrow()[0].clone();
    activate();
    h.world.flush();
    assert_eq!(builds_about.get(), 1, "link activation selected /about");
    let log = h.take_log().join("\n");
    assert!(log.contains("text \"about-body\""), "{log}");
}
