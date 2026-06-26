//! End-to-end integration test for the runtime-server animation-over-wire path.
//!
//! Mirrors the welcome example's pipeline shape against a recording
//! backend instead of a real client:
//!
//! 1. Install the sidecar scheduler.
//! 2. `mount(recorder, app_fn)` — the closure runs *inside* the root
//!    reactive scope so `after_ms_scoped` / `raf_loop_scoped` survive.
//!    (This is the [`sidecar_mount_regression`] fix.)
//! 3. The app builds a `Element::View` with a `Ref<ViewHandle>`, an
//!    `AnimatedValue<f32>` bound to its opacity, and an
//!    `after_ms_scoped(0, || raf_loop_scoped(|| av.set(...)))` chain
//!    that drives the animation.
//! 4. Drive several `tick_animations` passes. Each tick fires the raf
//!    closure, which calls `av.set(...)`, which routes through
//!    `RecordingViewOps::set_animated_f32`, which emits a
//!    `Command::SetAnimatedF32` onto the recorder's log.
//! 5. Assert that the log contains multiple `SetAnimatedF32` commands
//!    across the ticks — i.e. animation deltas are flowing end-to-end.
//!
//! What this covers that the unit tests don't:
//! - `mount` / `render` boundary works WITH the scheduler installed
//! - `after_ms_scoped` chained into `raf_loop_scoped` survives its
//!   scope long enough to fire repeatedly
//! - `AnimatedValue::bind` correctly routes per-tick writes through
//!   `ViewOps::set_animated_f32` and into the recorder's emit path
//!   (`RECORDER_HANDLE` thread-local, `try_borrow_mut` lock dance,
//!   `Command::SetAnimatedF32` serialization)
//! - The raf re-entrancy fix (`scheduler.rs` `RafSlot { Cell, … }`)
//!   doesn't break the happy-path tick loop
//!
//! The second test specifically exercises the production crash shape:
//! a raf closure that drops its own handle mid-tick. Without the
//! scheduler fix this would panic with `RefCell already borrowed`
//! inside `RafHandle::cancel`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use dev_server::{scheduler, WireRecordingBackend};
use runtime_core::animation::{AnimProp, AnimatedValue};
use runtime_core::{
    after_ms_scoped, mount, node_ref, raf_loop_scoped, Element, Ref,
    RefFill, SafeAreaSides, ViewHandle,
};
use wire::Command;

/// Build a view + bind an opacity AV + schedule a raf-driven write
/// loop. Mirrors the shape of welcome's `use_welcome` /
/// `coordinator.rs` without the visual richness — same primitive
/// types, same scheduling primitives, same `AnimatedValue::bind`
/// routing.
fn make_app() -> impl FnOnce() -> Element + 'static {
    move || {
        let opacity = AnimatedValue::new(0.0_f32);
        let view: Ref<ViewHandle> = node_ref!(ViewHandle);
        opacity.bind(view.clone(), AnimProp::Opacity);
        let view_for_fill = view.clone();

        // Wait one event-loop tick (the sidecar scheduler treats
        // 0ms as "deadline = now"), then start a raf loop that
        // walks opacity 0 → 1 over a handful of frames. Mirrors
        // welcome's Act-1 timeline kick-off.
        let opacity_for_after = opacity.clone();
        after_ms_scoped(0, move || {
            let opacity_for_raf = opacity_for_after.clone();
            let tick_count = Rc::new(Cell::new(0u32));
            raf_loop_scoped(move || {
                let n = tick_count.get() + 1;
                tick_count.set(n);
                opacity_for_raf.set((n as f32) * 0.1);
            });
        });

        Element::View {
            children: vec![],
            style: None,
            ref_fill: Some(RefFill::View(Box::new(move |h| view_for_fill.fill(h)))),
            safe_area_sides: SafeAreaSides::NONE,
            on_touch: None,
            on_wheel: None,
            on_hover: None,
            is_container: false,
            accessibility: Default::default(),
            test_id: None,
        }
    }
}

/// Count `SetAnimatedF32` commands the recorder has accumulated.
fn count_set_animated_f32(recorder: &WireRecordingBackend) -> usize {
    recorder
        .commands_since(0)
        .iter()
        .filter(|c| matches!(c, Command::SetAnimatedF32 { .. }))
        .count()
}

#[test]
fn end_to_end_welcome_shape_emits_animation_deltas_across_ticks() {
    scheduler::install();

    let recorder = WireRecordingBackend::new();
    let backend_rc = Rc::new(RefCell::new(recorder.clone()));
    let _owner = mount(backend_rc, make_app());

    // Before any tick: the initial render fired (creating the View
    // + Insert + Finish), but the after_ms_scoped(0) hasn't been
    // drained yet — no SetAnimatedF32 commands should exist.
    let baseline = count_set_animated_f32(&recorder);
    assert_eq!(
        baseline, 0,
        "no animation deltas should exist before the first drive_pending"
    );

    // First tick: drives the after_ms(0) deadline → schedules the
    // raf_loop → raf body runs once → opacity.set(0.1) → bind
    // handler fires → SetAnimatedF32 hits the recorder.
    recorder.tick_animations(std::time::Duration::from_millis(16));
    let after_first = count_set_animated_f32(&recorder);
    assert!(
        after_first >= 1,
        "first tick must emit at least one SetAnimatedF32 \
         (got {after_first}). If this is 0, either after_ms_scoped \
         was cancelled before firing (mount-vs-render regression) \
         or the AV bind isn't reaching the recorder \
         (RECORDER_HANDLE plumbing).",
    );

    // Drive several more ticks. Each one should produce another
    // delta as the raf body fires once per drive_pending pass.
    for _ in 0..5 {
        recorder.tick_animations(std::time::Duration::from_millis(16));
    }
    let after_many = count_set_animated_f32(&recorder);
    assert!(
        after_many > after_first,
        "raf-driven animation must keep emitting deltas across ticks \
         (had {after_first} after first tick, now {after_many}). \
         If this regresses, drive_pending stopped re-firing live \
         raf closures — check the put-back path in scheduler.rs.",
    );
}

/// Reproduces the production crash from the welcome example: a raf
/// closure runs, something inside it causes the raf handle to drop
/// (a reactive cleanup, a `drop(handle)` call, …), which fires
/// `RafHandle::Drop → cancel`. With the old scheduler this panicked
/// with `RefCell already borrowed` because `drive_pending` held a
/// `borrow_mut` on the slot across the closure call.
///
/// Test shape: build a tree that owns a `Cell<Option<RafLoop>>`, the
/// raf body takes the handle out of the cell and drops it on its
/// first invocation. The cancel must NOT re-enter the slot's borrow.
#[test]
fn end_to_end_raf_dropping_own_handle_inside_mount_does_not_panic() {
    scheduler::install();

    let recorder = WireRecordingBackend::new();
    let backend_rc = Rc::new(RefCell::new(recorder.clone()));

    let fired = Rc::new(Cell::new(0u32));
    let fired_for_app = fired.clone();
    let app = move || {
        let fired_inner = fired_for_app.clone();
        let handle_slot: Rc<RefCell<Option<runtime_core::scheduling::RafLoop>>> =
            Rc::new(RefCell::new(None));
        let handle_slot_for_raf = handle_slot.clone();
        // `raf_loop` (not _scoped) — we want a handle to drop
        // explicitly rather than rely on scope cleanup.
        let raf = runtime_core::raf_loop(move || {
            fired_inner.set(fired_inner.get() + 1);
            // Drop this raf's own handle mid-tick. The old scheduler
            // panicked here; the new one cleanly cancels.
            if let Some(h) = handle_slot_for_raf.borrow_mut().take() {
                drop(h);
            }
        });
        *handle_slot.borrow_mut() = Some(raf);

        Element::View {
            children: vec![],
            style: None,
            ref_fill: None,
            safe_area_sides: SafeAreaSides::NONE,
            on_touch: None,
            on_wheel: None,
            on_hover: None,
            is_container: false,
            accessibility: Default::default(),
            test_id: None,
        }
    };

    let _owner = mount(backend_rc, app);

    // Drive a tick — must not panic.
    recorder.tick_animations(std::time::Duration::from_millis(16));
    assert_eq!(fired.get(), 1, "raf body must have run exactly once");

    // Drive more ticks — the cancelled raf must NOT re-fire and
    // must NOT panic on subsequent drives.
    for _ in 0..5 {
        recorder.tick_animations(std::time::Duration::from_millis(16));
    }
    assert_eq!(
        fired.get(),
        1,
        "cancelled raf must not tick again after its handle was dropped"
    );
}
