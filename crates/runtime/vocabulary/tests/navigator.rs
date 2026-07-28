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

    fn finish(&mut self, _root: u32) {}
}

struct Harness {
    world: World,
    backend: Rc<RefCell<LegacyBridge<Mini>>>,
    registry: Rc<Registry<LegacyBridge<Mini>>>,
    log: Log,
}

fn harness() -> Harness {
    let log: Log = Rc::new(RefCell::new(Vec::new()));
    let backend = Rc::new(RefCell::new(LegacyBridge(Mini { log: log.clone(), next: 0 })));
    let mut registry = Registry::new();
    register_builtins(&mut registry);
    Harness {
        world: World::new(),
        backend,
        registry: Rc::new(registry),
        log,
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
