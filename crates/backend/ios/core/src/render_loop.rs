//! iOS `RenderLoopDriver`: NSTimer at ~60Hz on the main thread.
//!
//! CADisplayLink would be more accurate but requires declaring a
//! custom ObjC class with a target/action selector pair — heavier
//! ObjC runtime story than the typical "animated surface" use case
//! needs. Authors who want 120Hz Promotion can drive a custom
//! CADisplayLink against the bare `graphics` primitive.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

use runtime_core::driver::{
    install_render_loop_driver, RenderLoopDriver, RenderLoopHandle,
};
use objc2::rc::Retained;
use objc2_foundation::NSObject;

/// Register this backend's driver with `runtime-core`. Idempotent —
/// first install wins.
pub fn install_render_loop() {
    install_render_loop_driver(Box::new(IosRenderLoopDriver));
}

struct IosRenderLoopDriver;

impl RenderLoopDriver for IosRenderLoopDriver {
    fn start(
        &self,
        closure: Box<dyn FnMut(f32) + 'static>,
    ) -> Box<dyn RenderLoopHandle> {
        Box::new(start_inner(closure))
    }
}

struct IosHandle {
    timer: Option<Retained<NSObject>>,
    // Holds the closure alive while the timer fires it. The block
    // inside the timer also holds an Rc clone; the timer's
    // `invalidate` drops that clone, then `cancel()` drops this one
    // and the closure goes with it.
    _state: Rc<RefCell<Box<dyn FnMut(f32) + 'static>>>,
}

impl IosHandle {
    fn cancel_inner(&mut self) {
        if let Some(timer) = self.timer.take() {
            let _: () = unsafe { objc2::msg_send![&timer, invalidate] };
        }
    }
}

impl Drop for IosHandle {
    fn drop(&mut self) {
        self.cancel_inner();
    }
}

impl RenderLoopHandle for IosHandle {
    fn cancel(&mut self) {
        self.cancel_inner();
    }
}

fn start_inner(f: Box<dyn FnMut(f32) + 'static>) -> IosHandle {
    use block2::StackBlock;
    use objc2::msg_send_id;
    let started = Instant::now();
    // `StackBlock::new` needs a `Clone` closure. `Box<dyn FnMut>`
    // isn't `Clone`, so we wrap in `Rc<RefCell<...>>` — cloning the
    // Rc inside the block is cheap, and we hold one strong reference
    // here in `_state` so the closure outlives any timer
    // fire-and-drop race.
    let state: Rc<RefCell<Box<dyn FnMut(f32) + 'static>>> = Rc::new(RefCell::new(f));
    let state_for_block = state.clone();
    let block = StackBlock::new(move |_t: *const NSObject| {
        let elapsed = started.elapsed().as_secs_f32();
        // The NSTimer fire returns into Apple-side ObjC code that is
        // not built with Rust unwind ABI. A panic propagating out is
        // UB on the FFI boundary. `catch_unwind` here exists only to
        // print the panic *before* we abort; the abort is mandatory
        // (project policy: crash-loud). Without the print, the abort
        // from `panic_cannot_unwind` would point at `_CFRunLoopRun`
        // rather than the original Rust panic site.
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
            eprintln!("[backend-ios-core] render-loop panic: {msg}");
            std::process::abort();
        }
    });
    let block = block.copy();
    // Scheduled in `NSDefaultRunLoopMode` (NOT common modes) so the wgpu host's
    // per-frame draw doesn't fire during UIKit scroll/pan gestures. The Metal
    // command-buffer encode + present is expensive enough that competing with
    // scroll for CPU/GPU makes the gesture visibly jumpy. Trading off the GPU
    // draw freezing during a scroll for smooth scrolling is the explicit
    // preferred trade-off (e.g. the website's Simulator demo).
    //
    // This is the EXPENSIVE half of the "decouple draw from tick" split: the
    // cheap animation CLOCK (`backend-apple-core::scheduler::raf_loop`) runs in
    // COMMON modes so AnimatedValue springs keep advancing during a gesture
    // (a drag-to-reorder must not freeze); only THIS GPU draw stays default-mode
    // so it yields to scroll. Keep these two timers' modes distinct on purpose.
    let timer: Retained<NSObject> = unsafe {
        msg_send_id![
            objc2::class!(NSTimer),
            scheduledTimerWithTimeInterval: (1.0 / 60.0) as f64,
            repeats: true,
            block: &*block
        ]
    };
    IosHandle {
        timer: Some(timer),
        _state: state,
    }
}
