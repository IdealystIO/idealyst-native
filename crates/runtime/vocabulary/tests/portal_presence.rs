//! Handler-level tests for the P3-set `portal` (+ overlay compositions)
//! and `presence` primitives — driven through the `host-mock` recording
//! host (the caps surface implemented natively, no `Backend`, no
//! `LegacyBridge`), plus the manually pumped [`host_mock::pump`]
//! scheduler so the presence timing windows (pre-paint snap →
//! next-frame rest; exit animation → deferred detach-and-drop) are
//! observable instead of collapsing synchronously.
//!
//! Backend-call-stream parity with the old walker is pinned separately
//! by `scene-parity`'s `full_portal_*` / `full_presence_*` goldens;
//! these tests pin vocabulary-local behavior in isolation.

use std::cell::Cell;
use std::rc::Rc;

// `pump_frames` aliased to the pre-host-mock suite's name `pump_frame`
// so the call sites' vocabulary is unchanged.
use host_mock::pump::{install_scheduler, pump_frames as pump_frame, pump_timers};
use runtime_shared::primitives::portal::{PortalTarget, ViewportPlacement};
use runtime_shared::primitives::presence::PresenceAnim;
use runtime_shared::Easing;
use runtime_scene::{dyn_keyed, realize};
use runtime_vocabulary::builders::{overlay, portal, presence, text, view};
use runtime_vocabulary::prims::ScreenNav;
use runtime_world::{effect, provide, signal};

fn harness() -> host_mock::Harness {
    install_scheduler(); // first call wins; later calls no-op
    let h = host_mock::Harness::new();
    // Spliced mode: a presence hole's child splices directly into the
    // placeholder, so assertions read naturally (the historical Mini's
    // `supports_child_splice() = true`).
    h.shared.splice.set(true);
    // The historical Mini logged bare `apply_style n{node}` (no field
    // digest) — keep the suite's expected lines byte-stable.
    h.set_style_line(|n, _| format!("apply_style n{n}"));
    h
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
    let _root = h.mount(
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

/// A portal mounted AFTER the scope that provided `ScreenNav` has been
/// torn down must not abort the app.
///
/// The reported crash: `provide`/`inject` was an unowned world-level
/// map, so a navigator's `ScreenNav` — carrying `active_route`, a `Copy`
/// handle owned by the NAVIGATOR's scope — outlived the navigator. An
/// auth gate or route swap drops the navigator, freeing the slot; the
/// next portal to mount (an anchored menu in a persistent shell) injects
/// the surviving entry and `effect` runs its body immediately, so
/// `active_route.get()` hits a freed slot and aborts with
/// `stale-signal-handle` during that portal's own mount.
///
/// Both halves are pinned here: the entry must be retracted with its
/// scope, and a portal must survive being handed a dead one anyway.
#[test]
fn regression_portal_mounts_after_screen_nav_scope_is_torn_down() {
    let h = harness();

    // A navigator scope: owns `active_route` and publishes it.
    let (active, nav_scope) = h.world.enter(|| {
        runtime_world::collect_owned(|| {
            let active = signal("home");
            provide(ScreenNav { active_route: active.read_only(), route: "home" });
            active
        })
    });

    // The gate fires: the navigator is destroyed, freeing `active_route`.
    drop(nav_scope);
    assert!(!active.read_only().is_alive(), "the navigator's scope freed the route signal");

    // The shell (which outlived the navigator) now mounts a portal. This
    // is the ~200ms-after-login crash.
    let _root = h.world.enter(|| {
        realize(
            &h.backend,
            &h.registry,
            portal(PortalTarget::Viewport(ViewportPlacement::Center))
                .child(text().content("menu"))
                .build(),
        )
    });

    // It mounted, and it installed no visibility effect — with no live
    // screen to track there is nothing to hide against.
    let log = h.take_log();
    assert!(
        !log.iter().any(|l| l.starts_with("set_portal_hidden")),
        "no visibility effect without a live ScreenNav: {log:?}"
    );
    assert!(log.iter().any(|l| l.ends_with("portal")), "the portal still mounts: {log:?}");
}

#[test]
fn overlay_composition_backdrop_first_then_content_wrapper() {
    let h = harness();
    let dismissed = Rc::new(Cell::new(0usize));
    let dismissed_c = dismissed.clone();
    let _root = h.mount(
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
    let _root = h.mount(
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
    let _root = h.mount(
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
    let _root = h.mount(
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
    let _root = h.mount(
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
    let _root = h.mount(
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
    let _root = h.mount(
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
