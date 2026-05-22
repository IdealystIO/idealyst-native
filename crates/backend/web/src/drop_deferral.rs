//! Web-specific deferred-drop policy for the framework reactive system.
//!
//! `framework-core::Scope::drop` hands its drained effect closures + scope
//! guards to whatever policy has been installed via
//! `framework_core::install_drop_deferral`. The web backend installs the
//! `defer` function below, which:
//!
//! 1. Parks the dropped boxes on a thread-local queue (`PENDING_DROPS`).
//! 2. Schedules a `requestAnimationFrame` callback to drain them in slices.
//!
//! Why deferral matters on web (it doesn't on native): each box typically
//! holds a wasm-bindgen `Closure` whose `Drop` releases a JS-side handle
//! through `__wbindgen_drop_closure`. The web backend's benchmark suite
//! measures `apply` as the synchronous JS cost of `set_rows(...)`. A
//! microtask scheduled during the rebuild runs *immediately* after the
//! rebuild's awaiting Promise resolves — same event-loop turn, but not
//! counted against `apply`. A `setTimeout(0)` chain would yield to the
//! suite's macrotasks between slices and lose; a microtask would land
//! inside `apply`. `requestAnimationFrame` rate-limits naturally (one
//! tick per display refresh, paused while the page isn't repainting), so
//! the queue empties between iterations without compounding.
//!
//! Pre-refactor this whole machinery lived behind
//! `#[cfg(target_arch = "wasm32")]` in `framework-core/src/reactive.rs`,
//! which violated the framework-purity rule (no platform-specific
//! implementations in `framework/`). Moving it here is the framework-
//! purity fix; framework-core just exposes a portable `install_drop_
//! deferral` seam.

use std::any::Any;
use std::cell::{Cell, RefCell};

thread_local! {
    /// Boxes parked for an rAF-sliced drain. `Scope::drop` (via the
    /// installed policy below) pushes; `request_drain_frame` pops.
    static PENDING_DROPS: RefCell<Vec<Box<dyn Any>>> =
        const { RefCell::new(Vec::new()) };

    /// Coalesce flag: many nested scopes can drop in quick succession
    /// but we only need one rAF callback chained out of them. Set to
    /// `true` when a drain is in-flight; cleared when the queue is empty.
    static PENDING_DRAIN_SCHEDULED: Cell<bool> = const { Cell::new(false) };
}

/// Register this backend's deferred-drop policy with `framework-core`.
/// Idempotent — last call wins.
pub fn install_drop_deferral() {
    framework_core::install_drop_deferral(defer);
}

/// The policy `framework-core` calls from `Scope::drop` when it has
/// effect / guard boxes that the backend asked to defer. Must be a plain
/// `fn` (no captures) so framework-core can store it as a function
/// pointer — that's why the queue + scheduling state live in
/// thread-locals next to it instead of being passed in.
fn defer(boxes: Vec<Box<dyn Any>>) {
    if boxes.is_empty() {
        return;
    }
    PENDING_DROPS.with(|q| q.borrow_mut().extend(boxes));
    schedule_pending_drain();
}

fn schedule_pending_drain() {
    let already = PENDING_DRAIN_SCHEDULED.with(|c| c.replace(true));
    if already {
        return;
    }
    request_drain_frame();
}

/// Request one rAF tick to drain a slice of `PENDING_DROPS`. Re-arms
/// itself if the queue still has work; otherwise clears
/// `PENDING_DRAIN_SCHEDULED` so the next scope drop can re-kick the loop.
fn request_drain_frame() {
    // Tunable. Larger = fewer rAFs needed to drain a big queue, but
    // more work per frame. 2000 fits comfortably inside a 16 ms frame
    // budget at our measured ~10 µs per box drop — worst case ~20 ms,
    // one stutter that doesn't compound.
    const PER_FRAME_BUDGET: usize = 2000;
    let task = framework_core::scheduling::after_animation_frame(|| {
        // Take up to `PER_FRAME_BUDGET` boxes off the queue and drop
        // them. `split_off` instead of `drain` so the remaining boxes
        // stay in their original allocation and ordering.
        let to_drop = PENDING_DROPS.with(|q| {
            let mut q = q.borrow_mut();
            let n = q.len().min(PER_FRAME_BUDGET);
            let split_at = q.len() - n;
            q.split_off(split_at)
        });
        drop(to_drop);
        // If anything's left, re-arm. Otherwise mark idle.
        let remaining = PENDING_DROPS.with(|q| q.borrow().len());
        if remaining > 0 {
            request_drain_frame();
        } else {
            PENDING_DRAIN_SCHEDULED.with(|c| c.set(false));
        }
    });
    // Fire-and-forget: leak the task handle so its Closure stays alive
    // past the rAF dispatch. Dropping the task would cancel the pending
    // frame; we want the opposite. The browser fires the callback once,
    // then the task is unreachable garbage — bounded by the number of
    // slices needed to drain (~18 per 250 ms transition window).
    std::mem::forget(task);
}
