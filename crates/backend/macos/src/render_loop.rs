//! macOS `RenderLoopDriver`: NSTimer at ~60Hz on the main thread.
//!
//! The AppKit port of `backend-ios-core::render_loop` — same NSTimer +
//! block shape (the `scheduledTimerWithTimeInterval:repeats:block:`
//! constructor is identical on both platforms). CVDisplayLink would be
//! more accurate but requires a callback thread-hop back to the main
//! thread; the typical consumer (an embedded `host-wgpu` preview like
//! the website's Simulator) doesn't need frame-exact pacing.
//!
//! Scheduled in the DEFAULT run-loop mode (NOT common modes) on
//! purpose: the wgpu host's per-frame Metal encode + present must not
//! compete with NSScrollView's event-tracking mode during a scroll or
//! the gesture goes visibly jumpy. The cheap animation CLOCK
//! (`backend-apple-core::scheduler::raf_loop`) runs in common modes so
//! `AnimatedValue` springs keep advancing during gestures; only this
//! GPU draw yields. Same deliberate split as iOS — keep the two
//! timers' modes distinct.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

use objc2::rc::Retained;
use objc2_foundation::NSObject;
use runtime_shared::driver::{install_render_loop_driver, RenderLoopDriver, RenderLoopHandle};

/// Register this backend's driver with `runtime-core`. Idempotent —
/// first install wins. Called by `host-appkit` during app boot so
/// every macOS app has a working `render_loop` without per-app wiring.
pub fn install_render_loop() {
    install_render_loop_driver(Box::new(MacosRenderLoopDriver));
}

struct MacosRenderLoopDriver;

impl RenderLoopDriver for MacosRenderLoopDriver {
    fn start(&self, closure: Box<dyn FnMut(f32) + 'static>) -> Box<dyn RenderLoopHandle> {
        Box::new(start_inner(closure))
    }
}

struct MacosHandle {
    timer: Option<Retained<NSObject>>,
    // Holds the closure alive while the timer fires it. The block
    // inside the timer also holds an Rc clone; the timer's
    // `invalidate` drops that clone, then `cancel()` drops this one
    // and the closure goes with it.
    _state: Rc<RefCell<Box<dyn FnMut(f32) + 'static>>>,
}

impl MacosHandle {
    fn cancel_inner(&mut self) {
        if let Some(timer) = self.timer.take() {
            let _: () = unsafe { objc2::msg_send![&timer, invalidate] };
        }
    }
}

impl Drop for MacosHandle {
    fn drop(&mut self) {
        self.cancel_inner();
    }
}

impl RenderLoopHandle for MacosHandle {
    fn cancel(&mut self) {
        self.cancel_inner();
    }
}

fn start_inner(f: Box<dyn FnMut(f32) + 'static>) -> MacosHandle {
    use block2::StackBlock;
    use objc2::msg_send_id;
    let started = Instant::now();
    // `StackBlock::new` needs a `Clone` closure. `Box<dyn FnMut>`
    // isn't `Clone`, so wrap in `Rc<RefCell<...>>` — cloning the Rc
    // inside the block is cheap, and the strong reference in `_state`
    // keeps the closure alive across a timer fire-and-drop race.
    let state: Rc<RefCell<Box<dyn FnMut(f32) + 'static>>> = Rc::new(RefCell::new(f));
    let state_for_block = state.clone();
    let block = StackBlock::new(move |_t: *const NSObject| {
        let elapsed = started.elapsed().as_secs_f32();
        // The NSTimer fire returns into Apple-side ObjC code not built
        // with the Rust unwind ABI — a panic crossing that boundary is
        // UB. `catch_unwind` exists only to print the panic before the
        // mandatory abort (project policy: crash-loud), so the abort
        // points at the Rust panic site rather than `_CFRunLoopRun`.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            (state_for_block.borrow_mut())(elapsed);
        }));
        if let Err(payload) = result {
            let msg = if let Some(s) = payload.downcast_ref::<String>() {
                s.clone()
            } else if let Some(s) = payload.downcast_ref::<&'static str>() {
                (*s).to_string()
            } else {
                "<non-string panic payload>".to_string()
            };
            eprintln!("[backend-macos] render-loop panic: {msg}");
            std::process::abort();
        }
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
    MacosHandle {
        timer: Some(timer),
        _state: state,
    }
}
