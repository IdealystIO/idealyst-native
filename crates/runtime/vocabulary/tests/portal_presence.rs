//! Handler-level tests for the P3-set `portal` (+ overlay compositions)
//! and `presence` primitives — the mock-host pattern of `vocab.rs`, plus
//! a manually pumped [`Scheduler`] so the presence timing windows
//! (pre-paint snap → next-frame rest; exit animation → deferred
//! detach-and-drop) are observable instead of collapsing synchronously.
//!
//! Backend-call-stream parity with the old walker is pinned separately
//! by `scene-parity`'s `full_portal_*` / `full_presence_*` goldens;
//! these tests pin vocabulary-local behavior in isolation.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use runtime_core::accessibility::AccessibilityProps;
use runtime_core::primitives::portal::{PortalTarget, ViewportPlacement};
use runtime_core::primitives::presence::{PresenceAnim, PresenceState};
use runtime_core::scheduling::{install_scheduler, ScheduleHandle, Scheduler};
use runtime_core::{Backend, Easing};
use runtime_scene::{dyn_keyed, realize, Realized, Registry};
use runtime_vocabulary::builders::{overlay, portal, presence, text, view};
use runtime_vocabulary::prims::ScreenNav;
use runtime_vocabulary::{register_builtins, LegacyBridge};
use runtime_world::{effect, provide, signal, World};

// ===========================================================================
// Manually pumped scheduler
// ===========================================================================
//
// `runtime_core::scheduling`'s registry is a process-global `OnceLock`,
// but the queues here are thread-local — each `#[test]` thread pumps
// only its own tasks, so parallel tests can't cross-fire.

type Queued = Rc<RefCell<Option<Box<dyn FnOnce()>>>>;

thread_local! {
    static FRAME_QUEUE: RefCell<Vec<Queued>> = const { RefCell::new(Vec::new()) };
    static TIMER_QUEUE: RefCell<Vec<Queued>> = const { RefCell::new(Vec::new()) };
}

struct PumpHandle(Queued);

impl ScheduleHandle for PumpHandle {
    fn cancel(&mut self) {
        // Emptying the slot cancels: the pump skips vacated entries.
        self.0.borrow_mut().take();
    }
}

impl Drop for PumpHandle {
    fn drop(&mut self) {
        self.cancel();
    }
}

struct PumpScheduler;

impl Scheduler for PumpScheduler {
    fn schedule_microtask(&self, f: Box<dyn FnOnce()>) {
        // Tests never rely on microtask deferral; run inline.
        f();
    }

    fn after_animation_frame(&self, f: Box<dyn FnOnce()>) -> Box<dyn ScheduleHandle> {
        let slot: Queued = Rc::new(RefCell::new(Some(f)));
        FRAME_QUEUE.with(|q| q.borrow_mut().push(slot.clone()));
        Box::new(PumpHandle(slot))
    }

    fn after_ms(&self, _delay_ms: i32, f: Box<dyn FnOnce()>) -> Box<dyn ScheduleHandle> {
        let slot: Queued = Rc::new(RefCell::new(Some(f)));
        TIMER_QUEUE.with(|q| q.borrow_mut().push(slot.clone()));
        Box::new(PumpHandle(slot))
    }

    fn raf_loop(&self, _f: Box<dyn FnMut()>) -> Box<dyn ScheduleHandle> {
        let slot: Queued = Rc::new(RefCell::new(None));
        Box::new(PumpHandle(slot))
    }
}

fn ensure_scheduler() {
    install_scheduler(Box::new(PumpScheduler)); // first call wins; later calls no-op
}

fn pump(queue: &'static std::thread::LocalKey<RefCell<Vec<Queued>>>) {
    let tasks: Vec<Queued> = queue.with(|q| std::mem::take(&mut *q.borrow_mut()));
    for slot in tasks {
        // Release the slot borrow BEFORE running: the callback may drop
        // its own spent ScheduledTask, whose cancel re-borrows the slot.
        let f = slot.borrow_mut().take();
        if let Some(f) = f {
            f();
        }
    }
}

fn pump_frame() {
    pump(&FRAME_QUEUE);
}

fn pump_timers() {
    pump(&TIMER_QUEUE);
}

// ===========================================================================
// Recording backend
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

fn state_digest(state: &PresenceState) -> String {
    if *state == PresenceState::rest() {
        "rest".to_string()
    } else {
        format!(
            "op={:?} ty={:?}",
            state.opacity, state.translate_y
        )
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

    fn create_pressable(&mut self, _on_click: Rc<dyn Fn()>, _a11y: &AccessibilityProps) -> u32 {
        self.mint("pressable")
    }

    // Required (non-defaulted) Backend items the suite never exercises.
    fn create_button(
        &mut self,
        _label: &str,
        _on_click: &runtime_core::Action,
        _leading: Option<&runtime_core::IconData>,
        _trailing: Option<&runtime_core::IconData>,
        _a11y: &AccessibilityProps,
    ) -> u32 {
        self.mint("button")
    }

    fn update_text(&mut self, node: &u32, content: &str) {
        self.log
            .borrow_mut()
            .push(format!("update_text n{node} {content:?}"));
    }

    fn create_portal(
        &mut self,
        _target: PortalTarget,
        _on_dismiss: Option<Rc<dyn Fn()>>,
        _trap_focus: bool,
        _a11y: &AccessibilityProps,
    ) -> u32 {
        self.mint("portal")
    }

    fn release_portal(&mut self, node: &u32) {
        self.log.borrow_mut().push(format!("release_portal n{node}"));
    }

    fn set_portal_hidden(&mut self, node: &u32, hidden: bool) {
        self.log
            .borrow_mut()
            .push(format!("set_portal_hidden n{node} {hidden}"));
    }

    fn create_presence_placeholder(&mut self, _a11y: &AccessibilityProps) -> u32 {
        self.mint("presence_placeholder")
    }

    fn apply_presence(
        &mut self,
        node: &u32,
        state: PresenceState,
        transition: Option<(u32, Easing)>,
    ) {
        let t = match transition {
            Some((ms, _)) => format!("{ms}ms"),
            None => "snap".to_string(),
        };
        self.log
            .borrow_mut()
            .push(format!("apply_presence n{node} {} {t}", state_digest(&state)));
    }

    fn apply_style(&mut self, node: &u32, _style: &Rc<runtime_core::StyleRules>) {
        self.log.borrow_mut().push(format!("apply_style n{node}"));
    }

    fn insert(&mut self, parent: &mut u32, child: u32) {
        self.log
            .borrow_mut()
            .push(format!("insert n{parent} <- n{child}"));
    }

    fn insert_at(&mut self, parent: &mut u32, child: u32, index: usize) {
        self.log
            .borrow_mut()
            .push(format!("insert_at n{parent} <- n{child} @ {index}"));
    }

    fn remove_child(&mut self, parent: &u32, child: &u32) {
        self.log
            .borrow_mut()
            .push(format!("remove_child n{parent} -x n{child}"));
    }

    fn clear_children(&mut self, node: &u32) {
        self.log.borrow_mut().push(format!("clear_children n{node}"));
    }

    fn supports_child_splice(&self) -> bool {
        // Spliced mode: a presence hole's child splices directly into
        // the placeholder, so assertions read naturally.
        true
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
    ensure_scheduler();
    let log: Log = Rc::new(RefCell::new(Vec::new()));
    let backend = Rc::new(RefCell::new(LegacyBridge(Mini {
        log: log.clone(),
        next: 0,
    })));
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
    fn realize(&self, element: runtime_scene::Element) -> Realized<u32> {
        self.world
            .enter(|| realize(&self.backend, &self.registry, element))
    }

    fn take_log(&self) -> Vec<String> {
        std::mem::take(&mut *self.log.borrow_mut())
    }

    fn flush(&self) {
        self.world.flush();
    }
}

/// Register a probe effect in the CURRENT build scope whose cleanup
/// bumps `drops` — makes subtree teardown observable.
fn drop_probe(drops: &Rc<Cell<usize>>) {
    let drops = drops.clone();
    effect(move || {
        let drops = drops.clone();
        move || drops.set(drops.get() + 1)
    });
}

fn fade(ms: u32) -> PresenceAnim {
    PresenceAnim::fade(ms, Easing::Linear)
}

// ===========================================================================
// Portal
// ===========================================================================

#[test]
fn portal_mounts_children_and_releases_on_swap_out() {
    let h = harness();
    let open = h.world.enter(|| signal(true));
    let _root = h.realize(
        view()
            .child(dyn_keyed(
                move || open.get(),
                |&on| {
                    if on {
                        portal(PortalTarget::Viewport(ViewportPlacement::Center))
                            .child(text().content("inside"))
                            .build()
                    } else {
                        text().content("closed").build()
                    }
                },
            ))
            .build(),
    );
    assert_eq!(
        h.take_log(),
        [
            "create n0 view",
            "create n1 portal",
            "create n2 text \"inside\"",
            "insert n1 <- n2",
            "insert_at n0 <- n1 @ 0",
        ]
    );

    open.set(false);
    h.flush();
    let log = h.take_log();
    // Spliced dispose order: node out first, then the subtree's
    // teardown (which fires release_portal), then the replacement.
    assert_eq!(
        log,
        [
            "remove_child n0 -x n1",
            "release_portal n1",
            "create n3 text \"closed\"",
            "insert_at n0 <- n3 @ 0",
        ]
    );
}

#[test]
fn portal_visibility_tracks_screen_nav_active_route() {
    let h = harness();
    let active = h.world.enter(|| signal("home"));
    let _root = h.world.enter(|| {
        // What a navigator's mount_screen will do: provide the screen's
        // nav context so portals inside the screen can follow the
        // active route.
        provide(ScreenNav {
            active_route: active.read_only(),
            route: "home",
        });
        realize(
            &h.backend,
            &h.registry,
            portal(PortalTarget::Viewport(ViewportPlacement::Center))
                .child(text().content("modal"))
                .build(),
        )
    });
    let log = h.take_log();
    assert!(
        log.contains(&"set_portal_hidden n0 false".to_string()),
        "mount applies the initial visibility: {log:?}"
    );

    active.set("settings");
    h.flush();
    assert_eq!(h.take_log(), ["set_portal_hidden n0 true"]);

    active.set("home");
    h.flush();
    assert_eq!(h.take_log(), ["set_portal_hidden n0 false"]);
}

#[test]
fn overlay_composition_backdrop_first_then_content_wrapper() {
    let h = harness();
    let dismissed = Rc::new(Cell::new(0usize));
    let dismissed_c = dismissed.clone();
    let _root = h.realize(
        overlay()
            .on_dismiss(move || dismissed_c.set(dismissed_c.get() + 1))
            .child(text().content("body"))
            .build(),
    );
    // Backdrop pressable mounts FIRST (paints behind), then the content
    // wrapper view hosting the caller's children.
    assert_eq!(
        h.take_log(),
        [
            "create n0 portal",
            "create n1 pressable",
            "insert n0 <- n1",
            "create n2 view",
            "create n3 text \"body\"",
            "insert n2 <- n3",
            "insert n0 <- n2",
        ]
    );
    let _ = dismissed;
}

// ===========================================================================
// Presence
// ===========================================================================

#[test]
fn presence_placeholder_is_bare_and_in_flow() {
    // The placeholder mounts through create_presence_placeholder with NO
    // style applied by the handler, and sits in normal child flow
    // between its siblings — forcing `absolute; inset: 0` here is the
    // bug that collapsed stacked toasts (macOS in-flow placeholder fix).
    let h = harness();
    let _root = h.realize(
        view()
            .child(text().content("before"))
            .child(presence(|| text().content("toast").build()).present(false))
            .child(text().content("after"))
            .build(),
    );
    let log = h.take_log();
    assert_eq!(
        log,
        [
            "create n0 view",
            "create n1 text \"before\"",
            "insert n0 <- n1",
            "create n2 presence_placeholder",
            "insert n0 <- n2",
            "create n3 text \"after\"",
            "insert n0 <- n3",
        ]
    );
    assert!(
        !log.iter().any(|op| op.starts_with("apply_style n2")),
        "handler must not style the placeholder: {log:?}"
    );
}

#[test]
fn presence_enter_snaps_pre_paint_then_animates_to_rest() {
    let h = harness();
    let open = h.world.enter(|| signal(false));
    let _root = h.realize(
        presence(|| text().content("card").build())
            .present(open)
            .enter(fade(200))
            .build(),
    );
    assert_eq!(h.take_log(), ["create n0 presence_placeholder"]);

    open.set(true);
    h.flush();
    // Child mounts, snaps to the enter state with NO transition (the
    // pre-paint snap), and nothing else until the next frame.
    assert_eq!(
        h.take_log(),
        [
            "create n1 text \"card\"",
            "insert_at n0 <- n1 @ 0",
            "apply_presence n1 op=Some(0.0) ty=None snap",
        ]
    );

    pump_frame();
    // One frame later: animate to rest with the enter transition.
    assert_eq!(h.take_log(), ["apply_presence n1 rest 200ms"]);
}

#[test]
fn presence_exit_retires_animates_then_detaches_and_drops() {
    let h = harness();
    let open = h.world.enter(|| signal(true));
    let drops = Rc::new(Cell::new(0usize));
    let drops_c = drops.clone();
    let _root = h.realize(
        presence(move || {
            drop_probe(&drops_c);
            text().content("card").build()
        })
        .present(open)
        .exit(fade(150))
        .build(),
    );
    h.take_log();

    open.set(false);
    h.flush();
    // The exit transform applies with the node STILL ATTACHED (no
    // remove_child yet) and the child scope STILL ALIVE — the retire
    // hook holds the subtree open for the animation window.
    assert_eq!(h.take_log(), ["apply_presence n1 op=Some(0.0) ty=None 150ms"]);
    assert_eq!(drops.get(), 0, "child scope must stay alive during exit");

    pump_timers();
    // Timer fires: detach FIRST, then the scope drops (the spliced
    // dispose rule, now the retire hook's responsibility).
    assert_eq!(h.take_log(), ["remove_child n0 -x n1"]);
    assert_eq!(drops.get(), 1, "child scope drops after the exit window");
}

#[test]
fn presence_without_exit_unmounts_immediately() {
    let h = harness();
    let open = h.world.enter(|| signal(true));
    let drops = Rc::new(Cell::new(0usize));
    let drops_c = drops.clone();
    let _root = h.realize(
        presence(move || {
            drop_probe(&drops_c);
            text().content("card").build()
        })
        .present(open)
        .build(),
    );
    h.take_log();

    open.set(false);
    h.flush();
    assert_eq!(h.take_log(), ["remove_child n0 -x n1"]);
    assert_eq!(drops.get(), 1, "no exit animation: teardown is immediate");
}

#[test]
fn presence_quick_exit_cancels_pending_enter() {
    // Show then hide BEFORE the enter's animation frame fires: the
    // pending rest-apply must be cancelled (old walker guard — the
    // child must not animate toward rest while exiting).
    let h = harness();
    let open = h.world.enter(|| signal(false));
    let _root = h.realize(
        presence(|| text().content("card").build())
            .present(open)
            .enter(fade(200))
            .exit(fade(150))
            .build(),
    );
    h.take_log();

    open.set(true);
    h.flush();
    h.take_log(); // mount + snap

    open.set(false);
    h.flush();
    assert_eq!(h.take_log(), ["apply_presence n1 op=Some(0.0) ty=None 150ms"]);

    pump_frame();
    assert_eq!(
        h.take_log(),
        Vec::<String>::new(),
        "cancelled enter frame must not re-apply rest over the exit state"
    );
    pump_timers();
    assert_eq!(h.take_log(), ["remove_child n0 -x n1"]);
}

#[test]
fn presence_re_present_mid_exit_builds_fresh_while_old_finishes() {
    // The retire-hook model's documented semantic: flipping back true
    // during an exit builds a FRESH child (enter applies) while the
    // retired one finishes its exit on its own timer. The exit timer is
    // deliberately NOT cancelled — cancelling would drop the retired
    // scope without detaching its nodes (a permanent ghost view).
    let h = harness();
    let open = h.world.enter(|| signal(true));
    let _root = h.realize(
        presence(|| text().content("card").build())
            .present(open)
            .enter(fade(200))
            .exit(fade(150))
            .build(),
    );
    h.take_log();

    open.set(false);
    h.flush();
    assert_eq!(h.take_log(), ["apply_presence n1 op=Some(0.0) ty=None 150ms"]);

    open.set(true);
    h.flush();
    // Fresh child n2 mounts (with its enter snap) while n1 is still
    // attached and fading.
    assert_eq!(
        h.take_log(),
        [
            "create n2 text \"card\"",
            "insert_at n0 <- n2 @ 0",
            "apply_presence n2 op=Some(0.0) ty=None snap",
        ]
    );

    pump_timers();
    // The old child's exit completes: only n1 is detached; n2 stays.
    assert_eq!(h.take_log(), ["remove_child n0 -x n1"]);

    pump_frame();
    assert_eq!(h.take_log(), ["apply_presence n2 rest 200ms"]);
}
