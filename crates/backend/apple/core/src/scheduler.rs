//! Apple scheduler: NSTimer for `after_ms`, DispatchQueue.main.async
//! for microtasks, NSTimer at 60Hz for `raf_loop`.
//!
//! Pure Foundation — works the same on iOS, tvOS, and macOS. The
//! UIKit-flavored leaf crates and the AppKit-flavored macOS backend
//! both consume this through [`install_scheduler`].
//!
//! `runtime_core::scheduling` falls back to synchronous execution
//! on native when no scheduler is installed — fine for
//! `schedule_microtask` (immediate dispatch is correct semantics on
//! single-threaded native), but **wrong for `after_ms`** since
//! firing-now defeats the delay. The long-press touch recognizer is
//! the first thing to trip over this; presence animations and any
//! other timer-driven feature follow.
//!
//! Hosts call [`install_scheduler`] once at startup, before the
//! first `runtime_core::render(...)`.

use std::cell::RefCell;
use std::rc::Rc;

use block2::StackBlock;
use runtime_core::scheduling::{install_scheduler as install, ScheduleHandle, Scheduler};
use objc2::msg_send_id;
use objc2::rc::Retained;
use objc2_foundation::NSObject;

/// Register this scheduler with `runtime-core`. Idempotent — first
/// install wins. Safe to call from any Apple host (iOS / tvOS / macOS).
pub fn install_scheduler() {
    install(Box::new(AppleScheduler));
    // Wire `runtime_core::debug_log` through NSLog so author-side
    // diagnostic instrumentation (e.g. in the welcome example's
    // raf_loop) surfaces in `xcrun simctl spawn booted log show`.
    // First-install wins; subsequent calls no-op.
    runtime_core::scheduling::install_debug_log(Box::new(|msg| {
        crate::log::apple_log(msg);
    }));
    // Route the author-facing logger (`runtime_core::log` / `log_info!`)
    // through NSLog too, so app logs surface in the Xcode/Console.app
    // unified log rather than only stderr. First-install wins.
    crate::log::install_logger();
    // Install the cooperative main-thread async executor alongside the
    // scheduler. Without it, `runtime_core::driver::spawn_async` falls back
    // to `pollster::block_on` ON THE MAIN THREAD — fine for short one-shot
    // futures, but a long-running `recv` loop (`use_sse` / `use_socket`)
    // would block the main thread forever and freeze the UI. The executor
    // polls cooperatively on the main queue instead. First-install wins.
    // Gated on `async-driver` (the feature that brings `runtime_core::driver`
    // into scope); without it there is no `spawn_async` to host.
    #[cfg(feature = "async-driver")]
    crate::async_executor::install_async_executor();
}

struct AppleScheduler;

// SAFETY: `AppleScheduler` is a unit struct with no fields. The
// `Scheduler: Send + Sync` bound is satisfied trivially because we
// hold no shared state on the struct itself — every per-timer state
// is local to the closure scope. The wrapped closures themselves
// aren't `Send`, but the trait doesn't require them to be — and we
// only ever invoke them from the main thread anyway (NSTimer's
// scheduling targets the main run loop).
unsafe impl Send for AppleScheduler {}
unsafe impl Sync for AppleScheduler {}

impl Scheduler for AppleScheduler {
    fn schedule_microtask(&self, f: Box<dyn FnOnce() + 'static>) {
        // `dispatch_async(dispatch_get_main_queue(), block)` — the
        // canonical "run this on the next main-loop iteration"
        // primitive on Apple platforms. Independent of NSRunLoop
        // mode, so it fires reliably even when UIKit is inside a
        // tracking mode (scroll, touch) where default-mode NSTimers
        // are paused. NSTimer with 0 delay was the previous impl;
        // it silently dropped microtasks scheduled during high-
        // frequency layout activity because the runloop never
        // entered the timer's mode. libdispatch sidesteps that.
        dispatch_main_async(f);
    }

    fn after_animation_frame(
        &self,
        f: Box<dyn FnOnce() + 'static>,
    ) -> Box<dyn ScheduleHandle> {
        // 1/60s timer ≈ next animation frame. Not as precise as
        // CADisplayLink but matches the render-loop driver's
        // approach in the leaf crates; good enough for the
        // framework's frame-aligned scheduling needs.
        Box::new(after_ms_inner(16, f))
    }

    fn after_ms(
        &self,
        delay_ms: i32,
        f: Box<dyn FnOnce() + 'static>,
    ) -> Box<dyn ScheduleHandle> {
        Box::new(after_ms_inner(delay_ms, f))
    }

    fn raf_loop(&self, f: Box<dyn FnMut() + 'static>) -> Box<dyn ScheduleHandle> {
        // Recurring 60Hz timer. CADisplayLink would be more accurate
        // but requires a custom ObjC class with a target/action
        // pair — same trade-off the render-loop drivers in the leaf
        // crates make.
        //
        // Scheduled in `NSDefaultRunLoopMode` (NOT `kCFRunLoopCommonModes`)
        // so the per-frame tick does NOT fire during UIKit
        // scroll/pan gestures. The wgpu host's `draw_frame` is
        // expensive enough (Metal command-buffer encode + present)
        // that competing with scroll for CPU/GPU makes the gesture
        // visibly jumpy. Pausing tick during scroll trades animation
        // smoothness during the gesture for scroll smoothness; the
        // user said this was the preferable trade-off.
        let state: Rc<RefCell<Box<dyn FnMut() + 'static>>> = Rc::new(RefCell::new(f.take_inner()));
        let state_for_block = state.clone();
        let block = StackBlock::new(move |_t: *const NSObject| {
            (state_for_block.borrow_mut())();
        });
        let block = block.copy();
        let timer: Retained<NSObject> = unsafe {
            msg_send_id![
                objc2::class!(NSTimer),
                scheduledTimerWithTimeInterval: (1.0 / 60.0) as f64,
                repeats: true,
                block: &*block
            ]
        };
        Box::new(NsTimerHandle {
            timer: Some(timer),
            _state: AnyState::Raf(state),
        })
    }
}

/// Internal helper trait — works around `Box<dyn FnMut>` not having
/// `take_inner`. We just need a way to move the boxed `FnMut` out
/// of `Box` into `Rc<RefCell<Box>>` for sharing with the StackBlock.
trait TakeInner {
    type Inner;
    fn take_inner(self) -> Self::Inner;
}
impl TakeInner for Box<dyn FnMut() + 'static> {
    type Inner = Box<dyn FnMut() + 'static>;
    fn take_inner(self) -> Self::Inner {
        self
    }
}

fn after_ms_inner(delay_ms: i32, f: Box<dyn FnOnce() + 'static>) -> NsTimerHandle {
    // Convert the FnOnce into a take-once cell wrapped in Rc so the
    // ObjC block (which needs Clone) can hold it.
    let cell: Rc<RefCell<Option<Box<dyn FnOnce() + 'static>>>> =
        Rc::new(RefCell::new(Some(f)));
    // Zero-delay path: route through libdispatch instead of an
    // NSTimer at interval 0. Per
    // [[project_apple_microtask_libdispatch]], `scheduledTimerWith…`
    // at interval 0 is unreliable — the runloop must enter the
    // timer's mode for it to fire, and "next iteration" is a tiny
    // window that high-frequency layout activity (a fresh
    // `Host::mount` triggering Taffy reflow + Metal encode) can
    // squeeze out. The welcome's `session::after_ms(glare_start,
    // body)` collapses to delay_ms=0 on resume (session has long
    // since passed `glare_start`); when that 0-delay body silently
    // never fires, the inner `raf_loop_scoped` never gets
    // registered, animations stay frozen, and `av_writes` is stuck
    // on the diagnostic counter.
    //
    // `dispatch_async(main_q, block)` is mode-independent and
    // guaranteed to drain on the next main-thread iteration.
    // Cancellation is honoured via the existing `cell`: if the
    // owning `NsTimerHandle` drops before the block fires, the
    // cell is cleared and the dispatched block becomes a no-op.
    if delay_ms <= 0 {
        let cell_for_dispatch = cell.clone();
        dispatch_main_async(Box::new(move || {
            // Bind through a let so the `RefMut` temporary dies at the
            // semicolon \u{2014} otherwise it lives through the whole
            // if-let body. The closure `g()` regularly drops Rcs whose
            // `NsTimerHandle::Drop` re-borrows this same cell via
            // `cancel_inner`, and the second `borrow_mut` panics
            // ("RefCell already borrowed"). Real crash seen during
            // first-render settle on the iOS website.
            let taken = cell_for_dispatch.borrow_mut().take();
            if let Some(g) = taken {
                g();
            }
        }));
        // No NSTimer to invalidate — cancellation is the cell.take()
        // race resolved by holding `_state` in the returned handle.
        return NsTimerHandle {
            timer: None,
            _state: AnyState::Once(cell),
        };
    }
    let cell_for_block = cell.clone();
    let block = StackBlock::new(move |_t: *const NSObject| {
        // Same `RefMut`-lifetime fix as the libdispatch branch above:
        // bind through a let so the borrow ends before `g()` runs.
        let taken = cell_for_block.borrow_mut().take();
        if let Some(g) = taken {
            g();
        }
    });
    let block = block.copy();
    // `scheduledTimerWithTimeInterval:repeats:block:` requires a
    // non-negative interval.
    let interval = (delay_ms as f64) / 1000.0;
    let timer: Retained<NSObject> = unsafe {
        msg_send_id![
            objc2::class!(NSTimer),
            scheduledTimerWithTimeInterval: interval,
            repeats: false,
            block: &*block
        ]
    };
    NsTimerHandle {
        timer: Some(timer),
        _state: AnyState::Once(cell),
    }
}

/// Handle wrapper. `Drop` and explicit `cancel` both invalidate the
/// NSTimer; the closure's strong ref is held in `_state` so it
/// outlives a fire-then-drop race.
struct NsTimerHandle {
    timer: Option<Retained<NSObject>>,
    _state: AnyState,
}

/// Strong refs kept solely so the closure outlives a fire-then-drop
/// race with the timer's block. The fields are read by `Drop` order
/// — not by name — but rustc can't see that, hence
/// `#[allow(dead_code)]`.
#[allow(dead_code)]
enum AnyState {
    Once(Rc<RefCell<Option<Box<dyn FnOnce() + 'static>>>>),
    Raf(Rc<RefCell<Box<dyn FnMut() + 'static>>>),
}

impl NsTimerHandle {
    fn cancel_inner(&mut self) {
        if let Some(timer) = self.timer.take() {
            let _: () = unsafe { objc2::msg_send![&timer, invalidate] };
        }
        // Clear the FnOnce cell so a libdispatch path (timer=None,
        // delay was 0 — see `after_ms_inner`) becomes a no-op when
        // the dispatched block eventually fires. Without this, the
        // block would run the body *after* the owning reactive scope
        // dropped, leaking a `raf_loop_scoped` whose `on_cleanup`
        // attaches to a dead scope and is never invoked — the
        // NSTimer it installs ticks forever, hits AVs whose listeners
        // have already been unsubscribed, and animations appear
        // frozen after pause/resume. Same idea NSTimer's `invalidate`
        // gives us in the non-zero-delay path; the cell is the
        // libdispatch equivalent.
        if let AnyState::Once(cell) = &self._state {
            cell.borrow_mut().take();
        }
    }
}

impl Drop for NsTimerHandle {
    fn drop(&mut self) {
        self.cancel_inner();
    }
}

impl ScheduleHandle for NsTimerHandle {
    fn cancel(&mut self) {
        self.cancel_inner();
    }
}

// ===========================================================================
// libdispatch FFI for `schedule_microtask`
// ===========================================================================
//
// We can't use NSTimer for microtasks: timers added to the main
// runloop run in `NSDefaultRunLoopMode` (or whichever mode the timer
// is registered in), and UIKit drops to tracking modes (e.g.
// `UITrackingRunLoopMode`) during touch / scroll, suspending those
// timers. `dispatch_async(main_queue, block)` runs in the dispatch
// system independent of NSRunLoop mode and is the canonical way to
// say "do this on the main thread on the next iteration".
//
// `block2::StackBlock` already builds a libdispatch-compatible block;
// libdispatch retains it for execution.

#[link(name = "System", kind = "dylib")]
extern "C" {
    // `pub(crate)` so the cooperative async executor (`async_executor.rs`)
    // reuses the exact same main-queue dispatch primitive.
    pub(crate) fn dispatch_async(queue: *const std::ffi::c_void, block: *const std::ffi::c_void);
    pub(crate) static _dispatch_main_q: std::ffi::c_void;
}

fn dispatch_main_async(f: Box<dyn FnOnce() + 'static>) {
    let cell: Rc<RefCell<Option<Box<dyn FnOnce() + 'static>>>> =
        Rc::new(RefCell::new(Some(f)));
    let cell_for_block = cell.clone();
    let block = StackBlock::new(move || {
        // Block is invoked through libdispatch's main-queue drain
        // (extern "C"). A Rust panic propagating out aborts the
        // process via `panic_cannot_unwind` with no readable message.
        // catch_unwind here is purely to print the panic location
        // *before* we abort \u{2014} the abort is mandatory so we never
        // keep running on the partially-invariant state that produced
        // the panic. Crash-loud is the project policy; see
        // [[project-refmut-lifetime-reentrancy]] for the bug that
        // motivated tightening this.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // Bind the take() out so the `RefMut` temporary dies before
            // `g()` runs — same reentrancy fix as the after_ms_inner
            // branches above.
            let taken = cell_for_block.borrow_mut().take();
            if let Some(g) = taken {
                g();
            }
        }));
        if let Err(payload) = result {
            let msg = if let Some(s) = payload.downcast_ref::<String>() {
                s.clone()
            } else if let Some(s) = payload.downcast_ref::<&'static str>() {
                (*s).to_string()
            } else {
                "<non-string panic payload>".to_string()
            };
            eprintln!("[backend-apple-core] microtask panic: {msg}");
            std::process::abort();
        }
    });
    // libdispatch needs a heap-allocated block (StackBlock lives on
    // the stack; .copy() promotes to heap and refcounts via _Block_copy).
    let block = block.copy();
    let block_ptr: *const std::ffi::c_void = &*block as *const _ as *const std::ffi::c_void;
    unsafe {
        dispatch_async(
            &_dispatch_main_q as *const _ as *const std::ffi::c_void,
            block_ptr,
        );
    }
    // The block holds an Rc<RefCell<Option<Box<FnOnce>>>>; that Rc
    // is captured into the block's heap copy. libdispatch keeps the
    // block alive until it fires + dispatches, then releases. We
    // also leak our local `block` Retained so the heap block isn't
    // double-released before libdispatch is done with it.
    std::mem::forget(block);
    std::mem::forget(cell);
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
#[cfg(target_os = "macos")]
mod tests {
    //! Regression: dropping an `after_ms_inner(0, …)` handle MUST
    //! clear the FnOnce cell so the still-pending `dispatch_main_async`
    //! block becomes a no-op when libdispatch eventually fires it.
    //!
    //! The bug this prevents: the wgpu Host's `pause()` →
    //! `Host::unmount()` chain runs scope cleanups, dropping every
    //! `ScheduledTask` registered via `after_ms_scoped`. Pre-fix, the
    //! 0-delay dispatch path's drop was a no-op (no NSTimer to
    //! invalidate, cell left full), so the dispatched body fired
    //! AFTER the scope was gone — installing a `raf_loop_scoped`
    //! whose `on_cleanup` attached to a dead scope and was never
    //! invoked. The NSTimer it scheduled then ticked forever, hitting
    //! AVs whose listeners had already been unsubscribed and showing
    //! up as frozen animations after the next remount.
    //!
    //! Gated to `target_os = "macos"`: this exercises the apple
    //! scheduler against the host's libdispatch; iOS cargo-test
    //! workflows are non-trivial to set up. The code under test is
    //! identical on both targets.
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    #[test]
    fn after_ms_inner_zero_delay_clears_cell_on_drop() {
        let fired = Rc::new(Cell::new(false));
        let fired_clone = fired.clone();
        let handle = after_ms_inner(0, Box::new(move || fired_clone.set(true)));

        // The handle's `_state` carries the cell holding the FnOnce.
        // Clone the Rc so we can observe its contents post-drop.
        let cell_observe = match &handle._state {
            AnyState::Once(c) => c.clone(),
            AnyState::Raf(_) => panic!("after_ms_inner returned Raf state"),
        };
        assert!(
            cell_observe.borrow().is_some(),
            "cell should hold the FnOnce until either the dispatched \
             block fires or the handle is dropped",
        );

        // Drop the handle — this simulates `on_cleanup` firing when
        // the owning reactive scope dies. `cancel_inner` must empty
        // the cell so the pending libdispatch block becomes a no-op.
        drop(handle);
        assert!(
            cell_observe.borrow().is_none(),
            "dropping the NsTimerHandle must clear the cell so the \
             pending libdispatch block fires as a no-op",
        );

        // Sanity: the body never ran (we never drained main_q, but
        // even if libdispatch fired the block right now, the cleared
        // cell would make `take()` return None).
        assert!(
            !fired.get(),
            "body must NOT have run before / during this test",
        );
    }
}
