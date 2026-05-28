//! Android `RenderLoopDriver`: per-frame callback on the main looper.
//!
//! Runs the closure on the UI thread via `runtime_core`'s main-thread
//! [`raf_loop`], matching web (rAF) and iOS (NSTimer). Earlier this
//! drove a dedicated `std::thread`, which forced a `Send` bound up
//! through the author-facing `render_loop` signature (and a
//! per-target `#[cfg]` in `runtime-core`). Driving frames on the UI
//! thread keeps the closure `!Send`, so it can hold `Rc<RefCell<…>>`
//! and `!Send` wgpu state — uniform with every other backend. A
//! backend that later wants to offload GPU work to a worker thread
//! marshals across the boundary itself rather than leaking `Send`.

use runtime_core::driver::{install_render_loop_driver, RenderLoopDriver, RenderLoopHandle};
use runtime_core::scheduling::{raf_loop, RafLoop};
use runtime_core::time::now_micros;

/// Register this backend's driver with `runtime-core`. Idempotent —
/// first install wins.
pub fn install_render_loop() {
    install_render_loop_driver(Box::new(AndroidRenderLoopDriver));
}

struct AndroidRenderLoopDriver;

impl RenderLoopDriver for AndroidRenderLoopDriver {
    fn start(&self, mut closure: Box<dyn FnMut(f32) + 'static>) -> Box<dyn RenderLoopHandle> {
        // Anchor elapsed time at start; each frame reports seconds
        // since. `raf_loop` fires on the main looper, so `closure`
        // never crosses a thread boundary.
        let started_us = now_micros();
        let raf = raf_loop(move || {
            let elapsed = now_micros().saturating_sub(started_us) as f32 / 1_000_000.0;
            closure(elapsed);
        });
        Box::new(AndroidHandle { raf: Some(raf) })
    }
}

struct AndroidHandle {
    // Dropping the `RafLoop` cancels the pending frame and stops the
    // auto-rearm — so the field's own `Drop` covers teardown.
    raf: Option<RafLoop>,
}

impl RenderLoopHandle for AndroidHandle {
    fn cancel(&mut self) {
        self.raf = None;
    }
}
