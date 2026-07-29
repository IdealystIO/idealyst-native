//! Navigator handler unit tests (P3/P6 port of `walker/navigator.rs` +
//! the swap/stack SDK handlers): screen lifecycle via `Realized`
//! retention, the dispatch-on-flush command channel, world-context
//! navigation state, and the screen style-overlay fold — against a
//! minimal recording backend bridged through `LegacyBridge` (the op
//! *sequence* parity lives in `scene-parity`'s nav goldens).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use runtime_core::accessibility::AccessibilityProps;
use runtime_core::primitives::navigator::Route;
use runtime_core::{Backend, StyleRules, Tokenized};
use runtime_scene::{realize, Realized, Registry};
use runtime_vocabulary::builders::{navigator_outlet, stack_navigator, swap_navigator, text, view};
use runtime_vocabulary::prims::{MountPolicy, NavHandle, StackNav, StackRetention, SwapNav};
use runtime_vocabulary::{on_teardown, register_builtins, LegacyBridge};
use runtime_world::{inject, World};

// ===========================================================================
// Minimal recording backend
// ===========================================================================

type Log = Rc<RefCell<Vec<String>>>;

struct Mini {
    log: Log,
    next: u32,
    /// Link activations received via `create_link` (the P6 route-link
    /// tests fire them like platform clicks).
    link_activations: Rc<RefCell<Vec<Rc<dyn Fn()>>>>,
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

    fn update_text(&mut self, node: &u32, content: &str) {
        self.log
            .borrow_mut()
            .push(format!("update_text n{node} {content:?}"));
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

    fn apply_style(&mut self, node: &u32, style: &Rc<StyleRules>) {
        // Digest only the fields the tests assert on (width + height +
        // flex_grow — enough to prove the overlay fold merged handler
        // rules over the screen's own).
        let width = style
            .width
            .as_ref()
            .map(|w| format!("{w:?}"))
            .unwrap_or_else(|| "none".into());
        let height = style
            .height
            .as_ref()
            .map(|h| format!("{h:?}"))
            .unwrap_or_else(|| "none".into());
        let grow = style
            .flex_grow
            .as_ref()
            .map(|g| format!("{g:?}"))
            .unwrap_or_else(|| "none".into());
        self.log.borrow_mut().push(format!(
            "apply_style n{node} width={width} height={height} flex_grow={grow}"
        ));
    }

    fn on_node_unstyled(&mut self, node: &u32) {
        self.log.borrow_mut().push(format!("on_node_unstyled n{node}"));
    }

    fn mark_container(&mut self, node: &u32) {
        self.log.borrow_mut().push(format!("mark_container n{node}"));
    }

    fn insert(&mut self, parent: &mut u32, child: u32) {
        self.log.borrow_mut().push(format!("insert n{parent} <- n{child}"));
    }

    fn clear_children(&mut self, node: &u32) {
        self.log.borrow_mut().push(format!("clear_children n{node}"));
    }

    fn create_link(
        &mut self,
        config: runtime_core::primitives::link::LinkConfig,
        _a11y: &AccessibilityProps,
    ) -> u32 {
        self.link_activations.borrow_mut().push(config.on_activate);
        let n = self.next;
        self.next += 1;
        self.log
            .borrow_mut()
            .push(format!("create n{n} link route={:?}", config.route));
        n
    }

    fn update_link_url(&mut self, _node: &u32, _url: &str) {}

    fn finish(&mut self, _root: u32) {}
}

struct Harness {
    world: World,
    backend: Rc<RefCell<LegacyBridge<Mini>>>,
    registry: Rc<Registry<LegacyBridge<Mini>>>,
    log: Log,
    link_activations: Rc<RefCell<Vec<Rc<dyn Fn()>>>>,
}

fn harness() -> Harness {
    let log: Log = Rc::new(RefCell::new(Vec::new()));
    let link_activations: Rc<RefCell<Vec<Rc<dyn Fn()>>>> = Rc::new(RefCell::new(Vec::new()));
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

fn px(w: f32) -> StyleRules {
    StyleRules {
        width: Some(Tokenized::Literal(runtime_core::Length::Px(w))),
        // A field the stack's flow-fill overlay does NOT set — proves
        // the overlay fold MERGES with the screen's own style rather
        // than replacing it.
        height: Some(Tokenized::Literal(runtime_core::Length::Px(20.0))),
        ..Default::default()
    }
}

const HOME: Route<()> = Route::new("home", "/");
const ABOUT: Route<()> = Route::new("about", "/about");
const DETAIL: Route<()> = Route::new("detail", "/detail");

/// A screen body that bumps `builds` when the route builder runs and
/// fires `torn` when the screen's `Realized` drops.
fn probe_screen(
    label: &'static str,
    builds: Rc<Cell<u32>>,
    torn: Rc<Cell<u32>>,
) -> impl Fn(()) -> runtime_scene::Element + 'static {
    move |_| {
        builds.set(builds.get() + 1);
        let torn = torn.clone();
        on_teardown(move || torn.set(torn.get() + 1));
        view()
            .style(px(10.0))
            .child(text().content(label))
            .build()
    }
}

// ===========================================================================
// Swap navigator
// ===========================================================================

struct SwapFixture {
    _realized: Realized<u32>,
    handle: NavHandle,
    ctx: SwapNav,
    builds_home: Rc<Cell<u32>>,
    builds_about: Rc<Cell<u32>>,
    torn_home: Rc<Cell<u32>>,
    torn_about: Rc<Cell<u32>>,
}

fn mount_swap(h: &Harness, policy: MountPolicy) -> SwapFixture {
    let builds_home = Rc::new(Cell::new(0));
    let builds_about = Rc::new(Cell::new(0));
    let torn_home = Rc::new(Cell::new(0));
    let torn_about = Rc::new(Cell::new(0));
    let handle_slot: Rc<RefCell<Option<NavHandle>>> = Rc::new(RefCell::new(None));
    let ctx_slot: Rc<RefCell<Option<SwapNav>>> = Rc::new(RefCell::new(None));

    let element = {
        let handle_slot = handle_slot.clone();
        let ctx_slot = ctx_slot.clone();
        swap_navigator(&HOME)
            .screen(HOME, probe_screen("home", builds_home.clone(), torn_home.clone()))
            .screen(
                ABOUT,
                probe_screen("about", builds_about.clone(), torn_about.clone()),
            )
            .mount_policy(policy)
            .layout(move || {
                // The author layout reads the world-context SwapNav (the
                // old SwapContext, re-homed) and splats the outlet.
                *ctx_slot.borrow_mut() = inject::<SwapNav>();
                view()
                    .child(text().content("chrome"))
                    .child(navigator_outlet())
                    .build()
            })
            .on_handle(move |handle| *handle_slot.borrow_mut() = Some(handle))
            .build()
    };
    let realized = realize(&h.backend, &h.registry, element);
    let handle = handle_slot
        .borrow_mut()
        .take()
        .expect("on_handle filled at mount");
    let ctx = ctx_slot
        .borrow_mut()
        .take()
        .expect("SwapNav provided during layout build");
    SwapFixture {
        _realized: realized,
        handle,
        ctx,
        builds_home,
        builds_about,
        torn_home,
        torn_about,
    }
}

#[test]
fn swap_cached_screen_is_not_rebuilt_on_return() {
    let h = harness();
    let world = h.world.clone();
    world.enter(|| {
        let fx = mount_swap(&h, MountPolicy::LazyPersistent);
        assert_eq!(fx.builds_home.get(), 1, "initial screen mounted once");
        assert_eq!(fx.builds_about.get(), 0, "lazy: about not mounted yet");
        h.take_log();

        // Switch to about: mounts fresh.
        fx.handle.select(&ABOUT, ());
        world.flush();
        assert_eq!(fx.builds_about.get(), 1);
        let log = h.take_log();
        assert!(
            log.iter().any(|l| l.contains("text \"about\"")),
            "about screen built: {log:?}"
        );

        // Return to home: the cached Realized is re-inserted — the route
        // builder must NOT run again, and no create ops may appear.
        fx.handle.select(&HOME, ());
        world.flush();
        assert_eq!(
            fx.builds_home.get(),
            1,
            "cached screen must not be rebuilt on return"
        );
        assert_eq!(fx.torn_home.get(), 0, "persistent: home scope stayed alive");
        let log = h.take_log();
        assert!(
            log.iter().all(|l| !l.starts_with("create")),
            "return-to-cached emits no creates: {log:?}"
        );
        assert!(
            log.iter().any(|l| l.starts_with("clear_children")),
            "outlet swap clears the outgoing screen: {log:?}"
        );
    });
}

#[test]
fn swap_select_same_key_is_a_noop() {
    let h = harness();
    let world = h.world.clone();
    world.enter(|| {
        let fx = mount_swap(&h, MountPolicy::LazyPersistent);
        h.take_log();
        // Selecting the already-active URL: zero structural ops, no build.
        fx.handle.select(&HOME, ());
        world.flush();
        assert_eq!(fx.builds_home.get(), 1);
        let log = h.take_log();
        assert!(log.is_empty(), "same-key select must be a no-op, got {log:?}");
    });
}

#[test]
fn swap_context_on_select_switches_screens() {
    let h = harness();
    let world = h.world.clone();
    world.enter(|| {
        let fx = mount_swap(&h, MountPolicy::LazyPersistent);
        // The chrome-facing bare-name select (old SwapContext::on_select).
        (fx.ctx.on_select)("about");
        world.flush();
        assert_eq!(fx.builds_about.get(), 1);
        assert_eq!(fx.ctx.active_route.get(), "about", "route mirror updated");
        assert_eq!(fx.ctx.active_path.get(), "/about", "path mirror updated");
    });
}

#[test]
fn swap_lazy_disposing_evicts_and_remounts() {
    let h = harness();
    let world = h.world.clone();
    world.enter(|| {
        let fx = mount_swap(&h, MountPolicy::LazyDisposing);
        h.take_log();

        // Switch away: the home screen's Realized drops (cleanups fire).
        fx.handle.select(&ABOUT, ());
        world.flush();
        assert_eq!(fx.torn_home.get(), 1, "disposing policy drops the evicted scope");
        let log = h.take_log();
        assert!(
            log.iter().any(|l| l.starts_with("on_node_unstyled")),
            "styled screen teardown notifies the backend: {log:?}"
        );

        // Return: re-mounts fresh (build count climbs).
        fx.handle.select(&HOME, ());
        world.flush();
        assert_eq!(fx.builds_home.get(), 2, "disposed screen re-mounts on return");
    });
}

#[test]
#[should_panic(expected = "navigator_outlet()")]
fn swap_layout_without_outlet_panics_in_debug() {
    let h = harness();
    let world = h.world.clone();
    world.enter(|| {
        let element = swap_navigator(&HOME)
            .screen(HOME, |_| view().build())
            .layout(|| view().child(text().content("no outlet here")).build())
            .build();
        let _realized = realize(&h.backend, &h.registry, element);
    });
}

// ===========================================================================
// Stack navigator
// ===========================================================================

struct StackFixture {
    _realized: Realized<u32>,
    handle: NavHandle,
    ctx: StackNav,
    builds_home: Rc<Cell<u32>>,
    builds_detail: Rc<Cell<u32>>,
    torn_home: Rc<Cell<u32>>,
    torn_detail: Rc<Cell<u32>>,
}

fn mount_stack(h: &Harness, retention: StackRetention) -> StackFixture {
    let builds_home = Rc::new(Cell::new(0));
    let builds_detail = Rc::new(Cell::new(0));
    let torn_home = Rc::new(Cell::new(0));
    let torn_detail = Rc::new(Cell::new(0));
    let handle_slot: Rc<RefCell<Option<NavHandle>>> = Rc::new(RefCell::new(None));
    let ctx_slot: Rc<RefCell<Option<StackNav>>> = Rc::new(RefCell::new(None));

    let element = {
        let handle_slot = handle_slot.clone();
        let ctx_slot = ctx_slot.clone();
        stack_navigator(&HOME)
            .screen(HOME, probe_screen("home", builds_home.clone(), torn_home.clone()))
            .screen(
                DETAIL,
                probe_screen("detail", builds_detail.clone(), torn_detail.clone()),
            )
            .retention(retention)
            .layout(move || {
                *ctx_slot.borrow_mut() = inject::<StackNav>();
                view()
                    .child(text().content("header"))
                    .child(navigator_outlet())
                    .build()
            })
            .on_handle(move |handle| *handle_slot.borrow_mut() = Some(handle))
            .build()
    };
    let realized = realize(&h.backend, &h.registry, element);
    let handle = handle_slot
        .borrow_mut()
        .take()
        .expect("on_handle filled at mount");
    let ctx = ctx_slot
        .borrow_mut()
        .take()
        .expect("StackNav provided during layout build");
    StackFixture {
        _realized: realized,
        handle,
        ctx,
        builds_home,
        builds_detail,
        torn_home,
        torn_detail,
    }
}

#[test]
fn stack_pop_drops_the_popped_screens_scope() {
    let h = harness();
    let world = h.world.clone();
    world.enter(|| {
        let fx = mount_stack(&h, StackRetention::Retain);
        assert_eq!(fx.builds_home.get(), 1);

        fx.handle.push(&DETAIL, ());
        world.flush();
        assert_eq!(fx.builds_detail.get(), 1);
        assert_eq!(fx.ctx.depth.get(), 2);
        assert!(fx.ctx.can_go_back.get());
        h.take_log();

        // Pop: the detail screen's Realized drops (cleanups fire) and the
        // RETAINED home screen is revealed without a rebuild.
        (fx.ctx.pop)();
        world.flush();
        assert_eq!(fx.torn_detail.get(), 1, "popped screen scope dropped");
        assert_eq!(fx.builds_home.get(), 1, "retained screen revealed, not rebuilt");
        assert_eq!(fx.torn_home.get(), 0);
        assert_eq!(fx.ctx.depth.get(), 1);
        assert!(!fx.ctx.can_go_back.get());
        assert_eq!(fx.ctx.active_route.get(), "home", "pop mirrors the revealed route");
        let log = h.take_log();
        assert!(
            log.iter().any(|l| l.starts_with("clear_children")),
            "reveal swaps the outlet: {log:?}"
        );
        assert!(
            log.iter().all(|l| !l.contains("text \"home\"")),
            "home must not be re-created on pop: {log:?}"
        );
    });
}

#[test]
fn stack_pop_at_root_is_a_noop() {
    let h = harness();
    let world = h.world.clone();
    world.enter(|| {
        let fx = mount_stack(&h, StackRetention::Retain);
        h.take_log();
        (fx.ctx.pop)();
        world.flush();
        assert_eq!(fx.ctx.depth.get(), 1);
        assert_eq!(fx.torn_home.get(), 0);
        let log = h.take_log();
        assert!(log.is_empty(), "root pop must be a no-op, got {log:?}");
    });
}

#[test]
fn stack_rebuild_retention_disposes_covered_and_remounts_on_pop() {
    let h = harness();
    let world = h.world.clone();
    world.enter(|| {
        let fx = mount_stack(&h, StackRetention::Rebuild);
        fx.handle.push(&DETAIL, ());
        world.flush();
        // Browser semantics: the covered home screen was disposed…
        assert_eq!(fx.torn_home.get(), 1, "push disposes the covered screen");
        (fx.ctx.pop)();
        world.flush();
        // …and pop re-mounts it from its URL like a fresh navigation.
        assert_eq!(fx.builds_home.get(), 2, "pop re-mounts the revealed screen");
        assert_eq!(fx.torn_detail.get(), 1);
    });
}

#[test]
fn stack_screen_root_gets_the_flow_fill_overlay() {
    let h = harness();
    let world = h.world.clone();
    world.enter(|| {
        let _fx = mount_stack(&h, StackRetention::Retain);
        // The initial screen's root gets ONE apply_style carrying the
        // flow-fill overlay FOLDED over its own rules — the
        // `set_screen_style_overlay` override-layer contract: the
        // overlay wins on conflicts (width → 100%, exactly like the old
        // override layer which resolves last), while fields the overlay
        // doesn't set survive from the screen's own style (height 20px).
        let log = h.log.borrow().clone();
        assert!(
            log.iter().any(|l| l.starts_with("apply_style")
                && l.contains("width=Literal(Percent(100.0))")
                && l.contains("height=Literal(Px(20.0))")
                && l.contains("flex_grow=Literal(1.0)")),
            "screen root style must fold the flow-fill overlay over its own rules: {log:?}"
        );
    });
}

// ===========================================================================
// Teardown: the whole navigator
// ===========================================================================

#[test]
fn dropping_the_navigator_drops_every_cached_screen() {
    let h = harness();
    let world = h.world.clone();
    world.enter(|| {
        let fx = mount_swap(&h, MountPolicy::LazyPersistent);
        fx.handle.select(&ABOUT, ());
        world.flush();
        assert_eq!(fx.torn_home.get(), 0, "cached, still alive");
        let torn_home = fx.torn_home.clone();
        let torn_about = fx.torn_about.clone();
        drop(fx);
        // Dropping the Realized IS the navigator teardown: driver effect,
        // chrome, and every cached screen's scope die together.
        assert_eq!(torn_home.get(), 1, "cached home scope dropped with the navigator");
        assert_eq!(torn_about.get(), 1, "active about scope dropped with the navigator");
    });
}

// ===========================================================================
// Robot nav registry (P5 remainder) — the handler-side wiring:
// registration at mount, live back-stack snapshots, dispatch marks
// "current", teardown deregisters. The verb JSON shape is pinned in
// `robot.rs`'s bridge tests; this exercises the registry against REAL
// navigator state.
// ===========================================================================

#[cfg(feature = "robot")]
#[test]
fn robot_nav_registry_tracks_stack_state_and_teardown() {
    use runtime_vocabulary::robot::{all_navigators, ElementKind, Query, Robot};

    let robot = Robot::new();
    robot.reset();
    let h = harness();
    let world = h.world.clone();
    world.enter(|| {
        let fx = mount_stack(&h, StackRetention::Retain);

        // Registered at mount, linked to the ElementKind::Navigator
        // element the same mount registered.
        let navs = all_navigators();
        assert_eq!(navs.len(), 1, "one mounted navigator");
        let snap = &navs[0];
        assert_eq!(snap.type_name, "stack_navigator");
        assert_eq!(snap.active_route, "home");
        assert_eq!(snap.active_path, "/");
        assert_eq!(snap.depth, 1);
        assert!(!snap.can_go_back);
        assert_eq!(snap.stack, vec![("home".to_string(), "/".to_string())]);
        let nav_el = robot
            .find(Query::kind(ElementKind::Navigator))
            .expect("navigator element registered");
        assert_eq!(snap.element_id, Some(nav_el.id.0), "element link");
        assert!(snap.is_current, "cold-start root becomes current");

        // Push commits on the flush; the snapshot reads the LIVE stack.
        fx.handle.push(&DETAIL, ());
        world.flush();
        let navs = all_navigators();
        let snap = &navs[0];
        assert_eq!(snap.active_route, "detail");
        assert_eq!(snap.active_path, "/detail");
        assert_eq!(snap.depth, 2);
        assert!(snap.can_go_back);
        assert_eq!(
            snap.stack,
            vec![
                ("home".to_string(), "/".to_string()),
                ("detail".to_string(), "/detail".to_string()),
            ],
            "back-stack root-first, current last"
        );

        // Pop reveals home again.
        (fx.ctx.pop)();
        world.flush();
        let navs = all_navigators();
        assert_eq!(navs[0].depth, 1);
        assert_eq!(navs[0].active_route, "home");

        // Teardown deregisters (the on_teardown probe owned by the
        // navigator's Realized).
        drop(fx);
        assert!(
            all_navigators().is_empty(),
            "navigator must deregister with its subtree"
        );
    });
    robot.reset();
}

#[cfg(feature = "robot")]
#[test]
fn robot_nav_registry_swap_snapshot_is_depthless() {
    use runtime_vocabulary::robot::all_navigators;

    let robot = runtime_vocabulary::robot::Robot::new();
    robot.reset();
    let h = harness();
    let world = h.world.clone();
    world.enter(|| {
        let fx = mount_swap(&h, MountPolicy::LazyPersistent);
        (fx.ctx.on_select)("about");
        world.flush();
        let navs = all_navigators();
        assert_eq!(navs.len(), 1);
        let snap = &navs[0];
        assert_eq!(snap.type_name, "swap_navigator");
        assert_eq!(snap.active_route, "about");
        assert_eq!(snap.active_path, "/about");
        assert_eq!(snap.depth, 1, "swap is depth-less");
        assert!(!snap.can_go_back);
        assert_eq!(
            snap.stack,
            vec![("about".to_string(), "/about".to_string())],
            "swap reports its single active entry"
        );
        assert!(snap.is_current, "dispatch marked the swap current");
    });
    robot.reset();
}

// ===========================================================================
// P6: header-options carrier (Screen + ScreenChrome) + link activator
// ===========================================================================

/// The stack republishes `screen_chrome` on EVERY navigation, even when
/// the two screens carry IDENTICAL (absent) options — the screen
/// underneath swapped, and the old handler used `set_always` for
/// exactly this. The rev stamp is what makes the guarded new-core `set`
/// notify; a plain options payload would compare equal and freeze the
/// author header.
#[test]
fn stack_chrome_republishes_on_every_navigation() {
    use runtime_vocabulary::prims::ScreenChrome;

    let h = harness();
    let world = h.world.clone();
    world.enter(|| {
        let fx = mount_stack(&h, StackRetention::Retain);
        world.flush();
        let fires = Rc::new(Cell::new(0u32));
        {
            let fires = fires.clone();
            let chrome: runtime_world::Signal<ScreenChrome> =
                inject::<StackNav>().expect("StackNav ambient").screen_chrome;
            runtime_world::effect(move || {
                let _ = chrome.get();
                fires.set(fires.get() + 1);
            });
        }
        world.flush();
        assert_eq!(fires.get(), 1, "effect's first run");

        // Both screens are optionless — identical payloads. Push and
        // pop must each notify anyway.
        fx.handle.push(&DETAIL, ());
        world.flush();
        assert_eq!(fires.get(), 2, "push republished chrome");
        (fx.ctx.pop)();
        world.flush();
        assert_eq!(fires.get(), 3, "pop republished chrome");
    });
}

/// `Screen::with`-carried options ride the back-stack: the ACTIVE
/// screen's payload is what `screen_chrome` holds, and a pop reveals
/// the covered screen's payload again (options survive retention).
#[test]
fn stack_screen_options_follow_the_active_screen() {
    use runtime_vocabulary::builders::stack_navigator;
    use runtime_vocabulary::prims::Screen;

    #[derive(Clone, PartialEq, Debug)]
    struct Title(&'static str);

    let h = harness();
    let world = h.world.clone();
    let handle_slot: Rc<RefCell<Option<NavHandle>>> = Rc::new(RefCell::new(None));
    let ctx_slot: Rc<RefCell<Option<StackNav>>> = Rc::new(RefCell::new(None));
    world.enter(|| {
        let element = {
            let handle_slot = handle_slot.clone();
            let ctx_slot = ctx_slot.clone();
            stack_navigator(&HOME)
                .screen(HOME, |_| {
                    Screen::new(view().build()).with(Title("Home"))
                })
                .screen(DETAIL, |_| view().build()) // optionless
                .retention(StackRetention::Retain)
                .layout(move || {
                    *ctx_slot.borrow_mut() = inject::<StackNav>();
                    view().child(navigator_outlet()).build()
                })
                .on_handle(move |h| *handle_slot.borrow_mut() = Some(h))
                .build()
        };
        let _realized = realize(&h.backend, &h.registry, element);
        world.flush();

        let ctx = ctx_slot.borrow_mut().take().expect("StackNav provided");
        let title_of = |ctx: &StackNav| {
            ctx.screen_chrome
                .get()
                .options
                .as_ref()
                .and_then(|o| o.downcast_ref::<Title>().cloned())
        };
        assert_eq!(title_of(&ctx), Some(Title("Home")), "seat published options");

        let handle = handle_slot.borrow_mut().take().expect("on_handle");
        handle.push(&DETAIL, ());
        world.flush();
        assert_eq!(title_of(&ctx), None, "optionless top publishes None");

        handle.pop();
        world.flush();
        assert_eq!(
            title_of(&ctx),
            Some(Title("Home")),
            "pop republishes the revealed screen's retained options"
        );
    });
}

/// A route link mounted in a NESTED navigator's screen targets the
/// INNER navigator (the innermost `LinkActivator` shadows the outer,
/// save/restore) — the old ambient-navigator stack invariant. The
/// outer link (in the outer screen but outside the inner navigator)
/// keeps targeting the outer.
#[test]
fn link_activator_targets_the_innermost_navigator() {
    use runtime_vocabulary::builders::{link, swap_navigator};

    let h = harness();
    let world = h.world.clone();
    let inner_about_builds = Rc::new(Cell::new(0u32));
    let outer_about_builds = Rc::new(Cell::new(0u32));

    world.enter(|| {
        let element = {
            let inner_about_builds = inner_about_builds.clone();
            let outer_about_builds = outer_about_builds.clone();
            swap_navigator(&HOME)
                .screen(HOME, move |_| {
                    // The outer HOME screen hosts: a link of its own
                    // (targets OUTER) + a whole nested swap whose
                    // screen hosts a link (targets INNER).
                    let inner_about_builds = inner_about_builds.clone();
                    view()
                        .child(
                            link()
                                .route(&ABOUT, ())
                                .child(text().content("outer-link"))
                                .build(),
                        )
                        .child(
                            swap_navigator(&HOME)
                                .screen(HOME, |_| {
                                    view()
                                        .child(
                                            link()
                                                .route(&ABOUT, ())
                                                .child(text().content("inner-link"))
                                                .build(),
                                        )
                                        .build()
                                })
                                .screen(ABOUT, move |_| {
                                    inner_about_builds.set(inner_about_builds.get() + 1);
                                    text().content("inner-about").build()
                                })
                                .build(),
                        )
                        .build()
                })
                .screen(ABOUT, move |_| {
                    outer_about_builds.set(outer_about_builds.get() + 1);
                    text().content("outer-about").build()
                })
                .build()
        };
        let _realized = realize(&h.backend, &h.registry, element);
        world.flush();

        // Mount order: outer-link mounts first, then the nested
        // navigator's inner-link.
        assert_eq!(h.link_activations.borrow().len(), 2);
        let outer_activate = h.link_activations.borrow()[0].clone();
        let inner_activate = h.link_activations.borrow()[1].clone();

        // Inner link → INNER navigator selects; the outer stays on HOME
        // (its ABOUT never builds).
        inner_activate();
        world.flush();
        assert_eq!(inner_about_builds.get(), 1, "inner navigator selected");
        assert_eq!(outer_about_builds.get(), 0, "outer untouched");

        // Outer link → OUTER navigator selects.
        outer_activate();
        world.flush();
        assert_eq!(outer_about_builds.get(), 1, "outer navigator selected");
    });
}

/// A route link mounted OUTSIDE any navigator silently no-ops on
/// activation — the old `link()` posture (never a panic; chrome taps
/// must not crash).
#[test]
fn link_route_outside_a_navigator_noops() {
    use runtime_vocabulary::builders::link;

    let h = harness();
    let world = h.world.clone();
    world.enter(|| {
        let element = link()
            .route(&ABOUT, ())
            .child(text().content("orphan"))
            .build();
        let _realized = realize(&h.backend, &h.registry, element);
        let activate = h.link_activations.borrow()[0].clone();
        activate(); // must not panic
        world.flush();
    });
}

/// The SDK's presentation label rides `.nav_label(...)` into the robot
/// nav registry's `type_name` (wire parity with the old bridge, which
/// served `std::any::type_name::<SwapPresentation>()`); bare vocabulary
/// mounts keep the builder-name fallback (pinned by the two
/// registry tests above).
#[cfg(feature = "robot")]
#[test]
fn robot_nav_snapshot_carries_the_sdk_presentation_label() {
    use runtime_vocabulary::builders::swap_navigator;
    use runtime_vocabulary::robot::all_navigators;

    let robot = runtime_vocabulary::robot::Robot::new();
    robot.reset();
    let h = harness();
    let world = h.world.clone();
    world.enter(|| {
        let element = swap_navigator(&HOME)
            .screen(HOME, |_| view().build())
            .nav_label("swap_navigator::SwapPresentation")
            .build();
        let _realized = realize(&h.backend, &h.registry, element);
        let navs = all_navigators();
        assert_eq!(navs.len(), 1);
        assert_eq!(navs[0].type_name, "swap_navigator::SwapPresentation");
    });
    robot.reset();
}

// ===========================================================================
// SSG route discovery (the new-core leg of `backend_ssr::render_all`)
// ===========================================================================

/// Both navigator mounts publish their screen path patterns to the
/// shared (old-core, thread-local) route collector — the hook the SSG
/// crawl drains after each page render to discover the next literal
/// paths. Without this, `backend_ssr::newcore::render_all` would only
/// ever emit `/` for a new-core app.
#[test]
fn navigator_mounts_publish_routes_to_the_ssg_collector() {
    use runtime_core::primitives::navigator::{enable_route_collector, take_route_collector};

    // Swap.
    let h = harness();
    let world = h.world.clone();
    world.enter(|| {
        enable_route_collector();
        let element = swap_navigator(&HOME)
            .screen(HOME, |_| view().build())
            .screen(ABOUT, |_| view().build())
            .build();
        let _realized = realize(&h.backend, &h.registry, element);
        let mut found = take_route_collector().expect("collector was enabled");
        found.sort_unstable();
        assert_eq!(found, vec!["/", "/about"], "swap mount publishes every screen path");
    });

    // Stack.
    let h = harness();
    let world = h.world.clone();
    world.enter(|| {
        enable_route_collector();
        let element = stack_navigator(&HOME)
            .screen(HOME, |_| view().build())
            .screen(DETAIL, |_| view().build())
            .build();
        let _realized = realize(&h.backend, &h.registry, element);
        let mut found = take_route_collector().expect("collector was enabled");
        found.sort_unstable();
        assert_eq!(found, vec!["/", "/detail"], "stack mount publishes every screen path");
    });

    // Off = no-op (live backends never enable it).
    let h = harness();
    let world = h.world.clone();
    world.enter(|| {
        let element = swap_navigator(&HOME).screen(HOME, |_| view().build()).build();
        let _realized = realize(&h.backend, &h.registry, element);
        assert!(take_route_collector().is_none(), "collector off: nothing recorded");
    });
}
