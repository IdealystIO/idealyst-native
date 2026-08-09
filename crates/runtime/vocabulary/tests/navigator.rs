//! Navigator handler unit tests (P3/P6 port of `walker/navigator.rs` +
//! the swap/stack SDK handlers): screen lifecycle via `Realized`
//! retention, the dispatch-on-flush command channel, world-context
//! navigation state, and the screen style-overlay fold — against the
//! recording `host-mock` harness, which implements the caps surface
//! natively (the op *sequence* parity lives in `scene-parity`'s nav
//! goldens).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use host_mock::Harness;
use runtime_shared::primitives::navigator::Route;
use runtime_shared::{StyleRules, Tokenized};
use runtime_scene::{realize, Realized};
use runtime_vocabulary::builders::{navigator_outlet, stack_navigator, swap_navigator, text, view};
use runtime_vocabulary::on_teardown;
use runtime_vocabulary::prims::{MountPolicy, NavHandle, StackNav, StackRetention, SwapNav};
use runtime_world::inject;

// ===========================================================================
// Harness
// ===========================================================================

/// The stock host-mock harness with this suite's `apply_style` digest
/// installed: width + height + flex_grow ("none" when absent) — enough
/// to prove the overlay fold merged handler rules over the screen's
/// own. Captured `create_link` activations land on
/// `h.shared.link_activations` (the P6 route-link tests fire them like
/// platform clicks).
fn harness() -> Harness {
    let h = Harness::new();
    h.set_style_line(|node, style| {
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
            .map(|v| format!("{v:?}"))
            .unwrap_or_else(|| "none".into());
        let grow = style
            .flex_grow
            .as_ref()
            .map(|g| format!("{g:?}"))
            .unwrap_or_else(|| "none".into());
        format!("apply_style n{node} width={width} height={height} flex_grow={grow}")
    });
    h
}

fn px(w: f32) -> StyleRules {
    StyleRules {
        width: Some(Tokenized::Literal(runtime_shared::Length::Px(w))),
        // A field the stack's flow-fill overlay does NOT set — proves
        // the overlay fold MERGES with the screen's own style rather
        // than replacing it.
        height: Some(Tokenized::Literal(runtime_shared::Length::Px(20.0))),
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
        let log = h.ops();
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
// Screen-scope teardown: `on_scope_drop` from a screen body
// ===========================================================================

/// A registry of live claims, keyed by a monotonic id so a re-mounted
/// screen is distinguishable from the one it replaced (an id that never
/// leaves proves a leak; a fresh id proves a real re-build).
#[derive(Clone, Default)]
struct Claims {
    next: Rc<Cell<u32>>,
    live: Rc<RefCell<Vec<String>>>,
}

impl Claims {
    fn claim(&self, label: &'static str) {
        let id = self.next.get() + 1;
        self.next.set(id);
        let entry = format!("{id}:{label}");
        self.live.borrow_mut().push(entry.clone());
        let live = self.live.clone();
        runtime_world::on_scope_drop(move || live.borrow_mut().retain(|e| *e != entry));
    }

    fn live(&self) -> Vec<String> {
        self.live.borrow().clone()
    }
}

/// A screen body that registers a claim with `on_scope_drop` — the
/// author-facing "release this at unmount" hook (a registry entry, a
/// subscription, a listener). Deliberately NOT `probe_screen`'s
/// `on_teardown`: that parks an effect in the ambient collector, so it
/// cannot see whether the screen body's registrations anchored to the
/// screen's own scope or to whatever effect happened to be running.
fn claim_screen(
    label: &'static str,
    claims: Claims,
) -> impl Fn(()) -> runtime_scene::Element + 'static {
    move |_| {
        claims.claim(label);
        view().child(text().content(label)).build()
    }
}

/// The baseline every other test in this section is measured against: a
/// screen's `on_scope_drop` fires when THAT screen goes away, seated or
/// selected, and the navigator's own teardown releases whatever is still
/// mounted. Passes against the pre-`unanchored` code too — under
/// `LazyDisposing` from a root-mounted navigator, driver-anchored
/// teardown happened to coincide with eviction. The two regressions it
/// masks are the tests below.
#[test]
fn swap_screen_claims_follow_the_screen_lifecycle() {
    let h = harness();
    let world = h.world.clone();
    world.enter(|| {
        let claims = Claims::default();
        let element = swap_navigator(&HOME)
            .screen(HOME, claim_screen("home", claims.clone()))
            .screen(ABOUT, claim_screen("about", claims.clone()))
            .mount_policy(MountPolicy::LazyDisposing)
            .layout(|| view().child(navigator_outlet()).build())
            .on_handle(|_| {})
            .build();
        let realized = realize(&h.backend, &h.registry, element);
        let ctx = world.enter(|| inject::<SwapNav>()).expect("SwapNav provided");
        assert_eq!(claims.live(), vec!["1:home"], "seated screen claimed");

        (ctx.on_select)("about");
        world.flush();
        assert_eq!(
            claims.live(),
            vec!["2:about"],
            "the seated screen's claim must be released when it is evicted"
        );

        (ctx.on_select)("home");
        world.flush();
        assert_eq!(
            claims.live(),
            vec!["3:home"],
            "the re-mounted screen claims fresh; the evicted one released"
        );

        drop(realized);
        assert!(
            claims.live().is_empty(),
            "navigator teardown releases the active screen's claim: {:?}",
            claims.live()
        );
    });
}

/// A CACHED screen's claim must survive navigation. Under
/// `LazyPersistent` the screen is never torn down, so its
/// `on_scope_drop` must not fire — anchoring it to the navigator's
/// driver effect made it fire on the next navigation, retracting a
/// registration whose screen is still mounted (and still reachable when
/// the user comes back).
#[test]
fn regression_swap_cached_screen_keeps_its_claim_across_navigation() {
    let h = harness();
    let world = h.world.clone();
    world.enter(|| {
        let claims = Claims::default();
        let element = swap_navigator(&HOME)
            .screen(HOME, claim_screen("home", claims.clone()))
            .screen(ABOUT, claim_screen("about", claims.clone()))
            .screen(DETAIL, claim_screen("detail", claims.clone()))
            .mount_policy(MountPolicy::LazyPersistent)
            .layout(|| view().child(navigator_outlet()).build())
            .on_handle(|_| {})
            .build();
        let realized = realize(&h.backend, &h.registry, element);
        let ctx = world.enter(|| inject::<SwapNav>()).expect("SwapNav provided");

        (ctx.on_select)("about");
        world.flush();
        (ctx.on_select)("detail");
        world.flush();
        assert_eq!(
            claims.live(),
            vec!["1:home", "2:about", "3:detail"],
            "persistent screens stay mounted, so every claim stays live"
        );

        drop(realized);
        assert!(
            claims.live().is_empty(),
            "navigator teardown releases every cached screen's claim: {:?}",
            claims.live()
        );
    });
}

/// The same leak, in the shape a real app hits it: the navigator sits
/// inside a reactive region (an auth gate, a shell `when`), so its mount
/// handler — and with it the inline realize of the seated screen — runs
/// inside the region's DRIVER effect. That effect never re-runs while
/// the guard holds, so a claim anchored to it never releases: the
/// landing screen's registration outlives every navigation.
#[test]
fn regression_swap_seated_screen_under_a_reactive_region_fires_its_teardown() {
    let h = harness();
    let world = h.world.clone();
    world.enter(|| {
        let claims = Claims::default();
        let element = runtime_vocabulary::glue::when(
            || true,
            {
                let claims = claims.clone();
                move || {
                    swap_navigator(&HOME)
                        .screen(HOME, claim_screen("home", claims.clone()))
                        .screen(ABOUT, claim_screen("about", claims.clone()))
                        .mount_policy(MountPolicy::LazyDisposing)
                        .layout(|| view().child(navigator_outlet()).build())
                        .build()
                }
            },
            || view().build(),
        );
        let _realized = realize(&h.backend, &h.registry, view().child(element).build());
        let ctx = world.enter(|| inject::<SwapNav>()).expect("SwapNav provided");
        assert_eq!(claims.live(), vec!["1:home"], "seated screen claimed");

        (ctx.on_select)("about");
        world.flush();
        assert_eq!(
            claims.live(),
            vec!["2:about"],
            "the seated screen's claim must not outlive its screen just because \
             the navigator mounted inside someone else's effect"
        );
    });
}

/// A driver effect can re-run WITHOUT rebuilding what it mounted —
/// that is `when`'s guard dedup (a predicate reading an extra signal
/// re-fires the driver; an unchanged boolean keeps the mounted branch).
/// Everything the navigator built under that driver — chrome and seated
/// screen alike — must be untouched by such a re-run, which it is only
/// because neither anchored its teardown to the driver.
#[test]
fn regression_navigator_survives_a_deduped_re_run_of_the_effect_that_mounted_it() {
    let h = harness();
    let world = h.world.clone();
    world.enter(|| {
        let claims = Claims::default();
        let tick = runtime_world::signal(0u32);
        let element = runtime_vocabulary::glue::when(
            move || {
                // Reads more than its boolean: the driver re-fires on every
                // tick, the guard value never moves, the branch stays.
                let _ = tick.get();
                true
            },
            {
                let claims = claims.clone();
                move || {
                    let chrome_claims = claims.clone();
                    swap_navigator(&HOME)
                        .screen(HOME, claim_screen("home", claims.clone()))
                        .screen(ABOUT, claim_screen("about", claims.clone()))
                        .mount_policy(MountPolicy::LazyPersistent)
                        .layout(move || {
                            chrome_claims.claim("chrome");
                            view().child(navigator_outlet()).build()
                        })
                        .build()
                }
            },
            || view().build(),
        );
        let realized = realize(&h.backend, &h.registry, view().child(element).build());
        let ctx = world.enter(|| inject::<SwapNav>()).expect("SwapNav provided");
        assert_eq!(claims.live(), vec!["1:home", "2:chrome"]);

        tick.set(1);
        world.flush();
        assert_eq!(
            claims.live(),
            vec!["1:home", "2:chrome"],
            "a deduped driver re-run must not retract what it mounted"
        );

        (ctx.on_select)("about");
        world.flush();
        tick.set(2);
        world.flush();
        assert_eq!(
            claims.live(),
            vec!["1:home", "2:chrome", "3:about"],
            "still no retraction, with a persistent screen cached behind"
        );

        drop(realized);
        assert!(claims.live().is_empty(), "everything releases at teardown");
    });
}

/// The stack navigator seats its root screen the same way the swap one
/// does (`seat_initial` after the inline realize), and pushes from its
/// driver effect — so both halves of the anchor bug reach it too. A
/// pushed screen's claim must survive the pop that reveals it again, and
/// the root's must live until the navigator does.
#[test]
fn regression_stack_screen_claims_track_the_stack_not_the_driver() {
    let h = harness();
    let world = h.world.clone();
    world.enter(|| {
        let claims = Claims::default();
        let handle_slot: Rc<RefCell<Option<NavHandle>>> = Rc::new(RefCell::new(None));
        let element = {
            let handle_slot = handle_slot.clone();
            stack_navigator(&HOME)
                .screen(HOME, claim_screen("home", claims.clone()))
                .screen(DETAIL, claim_screen("detail", claims.clone()))
                .screen(ABOUT, claim_screen("about", claims.clone()))
                .retention(StackRetention::Retain)
                .layout(|| view().child(navigator_outlet()).build())
                .on_handle(move |handle| *handle_slot.borrow_mut() = Some(handle))
                .build()
        };
        let realized = realize(&h.backend, &h.registry, element);
        let handle = handle_slot.borrow_mut().take().expect("on_handle filled");

        // Two pushes: the SECOND one re-runs the driver effect while the
        // first pushed screen is still on the stack. That is the case a
        // driver-anchored teardown gets wrong — it retracts a screen the
        // user can still pop back to.
        handle.push(&DETAIL, ());
        world.flush();
        handle.push(&ABOUT, ());
        world.flush();
        assert_eq!(
            claims.live(),
            vec!["1:home", "2:detail", "3:about"],
            "every screen on a retaining stack keeps its claim"
        );

        handle.pop();
        world.flush();
        assert_eq!(
            claims.live(),
            vec!["1:home", "2:detail"],
            "pop releases exactly the popped screen's claim"
        );

        handle.pop();
        world.flush();
        assert_eq!(claims.live(), vec!["1:home"], "back at the root screen");

        drop(realized);
        assert!(
            claims.live().is_empty(),
            "navigator teardown releases the root screen's claim: {:?}",
            claims.live()
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

/// A navigator's published context must not outlive the navigator.
///
/// `StackNav` / `SwapNav` / `ScreenNav` all carry the navigator's OWN
/// signals (`active_route`, `depth`, the dispatch `tick`), which die with
/// its mount scope. While context entries were unowned, a navigator
/// destroyed by a route gate or an auth swap left its entry published;
/// the next `inject` — a portal's `ScreenNav` lookup, a chrome rebuild —
/// handed out handles onto freed slots and the first read aborted the app
/// with `stale-signal-handle`.
#[test]
fn regression_navigator_context_dies_with_the_navigator() {
    let h = harness();
    let world = h.world.clone();
    world.enter(|| {
        let fx = mount_stack(&h, StackRetention::Retain);
        world.flush();

        // Alive: chrome rebuilding after mount still resolves the nav.
        let live = inject::<StackNav>().expect("StackNav injectable while the navigator lives");
        assert!(live.active_route.is_alive());

        // The gate fires — the whole navigator subtree is dropped.
        drop(fx);
        world.flush();

        assert!(
            inject::<StackNav>().is_none(),
            "the navigator's provision must be retracted with its scope — an entry \
             holding its freed signals is the stale-handle crash"
        );
        assert!(
            inject::<runtime_vocabulary::prims::ScreenNav>().is_none(),
            "the screen's ScreenNav goes too: it is what portals inject"
        );
        assert!(!live.active_route.is_alive(), "and the signals it carried are gone");
    });
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
            // `StackNav` stays injectable for the navigator's LIFETIME
            // (its provision is owned by the navigator's mount scope), so
            // chrome that rebuilds reactively after mount still resolves
            // it. It is retracted when the navigator unmounts — see
            // `regression_navigator_context_dies_with_the_navigator`.
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
        assert_eq!(h.shared.link_activations.borrow().len(), 2);
        let outer_activate = h.link_activation(0);
        let inner_activate = h.link_activation(1);

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
        let activate = h.link_activation(0);
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
    use runtime_shared::primitives::navigator::{enable_route_collector, take_route_collector};

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

// ===========================================================================
// NavHandle identity
// ===========================================================================

/// `NavHandle: PartialEq` compares the DISPATCHER, so a handle is equal
/// to its own clones and unequal to a handle on any other navigator.
///
/// Framework-core impl, so it is pinned here rather than in the SDKs: it
/// is what lets `Signal<NavHandle>` exist at all (the handle types are
/// bounded on `PartialEq` at signal creation and `get`, not just on the
/// guarded `set`), and `SwapHandle`/`StackHandle` derive from it rather
/// than re-deriving the rule.
///
/// The unequal half is the load-bearing one: two navigators mounted in
/// the same app hand out structurally identical handles, and if those
/// compared equal, re-targeting a control from one navigator to the other
/// would be swallowed by the guarded `set` and the UI would keep driving
/// the old navigator.
#[test]
fn nav_handles_compare_by_navigator_identity() {
    let h = harness();
    let world = h.world.clone();
    world.enter(|| {
        let a = mount_swap(&h, MountPolicy::LazyPersistent);
        let b = mount_swap(&h, MountPolicy::LazyPersistent);

        assert!(
            a.handle == a.handle.clone(),
            "clones of one handle drive the same navigator and must compare equal"
        );
        assert!(
            a.handle != b.handle,
            "handles onto two separately mounted navigators must compare unequal"
        );
    });
}

/// Identity must not drift as the navigator's STATE changes — the handle
/// names the navigator, not its current route. A handle that stopped
/// equalling its own earlier clone after a `select` would re-fire every
/// subscriber holding it.
#[test]
fn nav_handle_identity_survives_navigation() {
    let h = harness();
    let world = h.world.clone();
    world.enter(|| {
        let f = mount_swap(&h, MountPolicy::LazyPersistent);
        let before = f.handle.clone();

        f.handle.select(&ABOUT, ());
        world.flush();

        assert!(
            f.handle == before,
            "navigating must not change which navigator the handle names"
        );
    });
}

// ===========================================================================
// Nested navigator base composition
// ===========================================================================

#[derive(Debug, Clone, PartialEq)]
struct IdParams {
    id: String,
}

impl runtime_shared::primitives::navigator::RouteParams for IdParams {
    fn to_path(&self, pattern: &str) -> String {
        pattern.replace(":id", &self.id)
    }
    fn from_segments(
        segments: &std::collections::HashMap<String, String>,
    ) -> Option<Self> {
        segments.get("id").map(|id| IdParams { id: id.clone() })
    }
}

const LIST: Route<()> = Route::new("list", "");
const ITEM: Route<IdParams> = Route::new("item", "/:id");
const INNER_INDEX: Route<()> = Route::new("inner-index", "");
const INNER_LEAF: Route<()> = Route::new("inner-leaf", "/leaf");

/// A navigator nested inside a `:param` screen composes its URLs onto
/// the parent screen's CONCRETE path, never the registered pattern.
/// Publishing the pattern as the nav base made the inner navigator emit
/// `/:id/leaf` — a literal placeholder in the address bar, which on
/// reload resolves back to an entity whose id is the string ":id".
#[test]
fn nested_navigator_base_is_the_concrete_parent_path() {
    let h = harness();
    let world = h.world.clone();
    world.enter(|| {
        let outer_slot: Rc<RefCell<Option<NavHandle>>> = Rc::new(RefCell::new(None));
        let inner_slot: Rc<RefCell<Option<NavHandle>>> = Rc::new(RefCell::new(None));
        let inner_ctx: Rc<RefCell<Option<StackNav>>> = Rc::new(RefCell::new(None));

        let element = {
            let outer_slot = outer_slot.clone();
            let inner_slot = inner_slot.clone();
            let inner_ctx = inner_ctx.clone();
            stack_navigator(&LIST)
                .screen(LIST, |_| text().content("list").build())
                .screen(ITEM, move |_p: IdParams| {
                    let inner_slot = inner_slot.clone();
                    let inner_ctx = inner_ctx.clone();
                    stack_navigator(&INNER_INDEX)
                        .screen(INNER_INDEX, |_| text().content("index").build())
                        .screen(INNER_LEAF, |_| text().content("leaf").build())
                        .layout(move || {
                            *inner_ctx.borrow_mut() = inject::<StackNav>();
                            view().child(navigator_outlet()).build()
                        })
                        .on_handle(move |handle| *inner_slot.borrow_mut() = Some(handle))
                        .build()
                })
                .on_handle(move |handle| *outer_slot.borrow_mut() = Some(handle))
                .build()
        };
        let _realized = realize(&h.backend, &h.registry, element);

        let outer = outer_slot.borrow_mut().take().expect("outer handle");
        outer.push(&ITEM, IdParams { id: "p1".to_string() });
        world.flush();

        let ctx = inner_ctx.borrow_mut().take().expect("inner StackNav");
        assert_eq!(
            ctx.active_path.get(),
            "/p1",
            "the nested navigator's index sits at the parent's concrete path"
        );

        let inner = inner_slot.borrow_mut().take().expect("inner handle");
        inner.push(&INNER_LEAF, ());
        world.flush();

        assert_eq!(
            ctx.active_path.get(),
            "/p1/leaf",
            "a nested push composes onto the concrete parent path, not the pattern"
        );
    });
}
