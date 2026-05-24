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
        // DispatchQueue.main.async equivalent — schedule for the
        // next main-thread iteration without waiting on a duration.
        // `dispatch_after_f` with delay 0 would also work, but
        // `dispatch_async_f` is cheaper and matches the "microtask"
        // semantic exactly. We piggy-back on an NSTimer with zero
        // delay for simplicity since the rest of this file already
        // uses that machinery.
        let _ = after_ms_inner(0, f);
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
    let cell_for_block = cell.clone();
    let block = StackBlock::new(move |_t: *const NSObject| {
        if let Some(g) = cell_for_block.borrow_mut().take() {
            g();
        }
    });
    let block = block.copy();
    // `scheduledTimerWithTimeInterval:repeats:block:` requires a
    // non-negative interval. NSTimer's "fire as soon as possible"
    // is interval 0 with repeats:false.
    let interval = (delay_ms.max(0) as f64) / 1000.0;
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
