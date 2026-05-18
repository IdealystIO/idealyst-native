//! Android `RenderLoopDriver`: dedicated render thread at ~60fps.

use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use framework_core::driver::{
    install_render_loop_driver, RenderLoopDriver, RenderLoopHandle,
};

/// Target frame interval. 60fps is fine for the UI-level animated
/// surface use case the framework targets; authors who want 120Hz
/// promotion are better off driving the loop themselves against the
/// bare `graphics` primitive.
const FRAME_INTERVAL: Duration = Duration::from_micros(16_667);

/// Register this backend's driver with `framework-core`. Idempotent —
/// first install wins.
pub fn install_render_loop() {
    install_render_loop_driver(Box::new(AndroidRenderLoopDriver));
}

struct AndroidRenderLoopDriver;

impl RenderLoopDriver for AndroidRenderLoopDriver {
    fn start(
        &self,
        closure: Box<dyn FnMut(f32) + Send + 'static>,
    ) -> Box<dyn RenderLoopHandle> {
        Box::new(start_inner(closure))
    }
}

struct AndroidHandle {
    tx: Option<Sender<()>>,
    join: Option<thread::JoinHandle<()>>,
}

impl AndroidHandle {
    fn cancel_inner(&mut self) {
        // Signal first so the worker breaks out of its `recv_timeout`
        // on its next iteration (within FRAME_INTERVAL at worst —
        // typically immediately). Then join so the surrounding `Drop`
        // can't race with a half-running render call.
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(());
        }
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

impl Drop for AndroidHandle {
    fn drop(&mut self) {
        self.cancel_inner();
    }
}

impl RenderLoopHandle for AndroidHandle {
    fn cancel(&mut self) {
        self.cancel_inner();
    }
}

fn start_inner(mut user_fn: Box<dyn FnMut(f32) + Send + 'static>) -> AndroidHandle {
    let (tx, rx) = channel::<()>();
    let started = Instant::now();
    let join = thread::Builder::new()
        .name("framework-render-loop".into())
        .spawn(move || {
            // Block on the cancel channel with a frame-length
            // timeout. When the timeout elapses we fire one frame
            // then loop back to wait again — that paces the loop at
            // ~60fps without relying on wgpu's `present()` to block
            // (which it only does for healthy surfaces; a
            // surface-lost / context-recreated state returns
            // immediately, which would otherwise spin the CPU at
            // 100% until cancellation arrives).
            loop {
                match rx.recv_timeout(FRAME_INTERVAL) {
                    Ok(()) | Err(RecvTimeoutError::Disconnected) => return,
                    Err(RecvTimeoutError::Timeout) => {}
                }
                let elapsed = started.elapsed().as_secs_f32();
                user_fn(elapsed);
            }
        })
        .expect("framework: failed to spawn render loop thread");
    AndroidHandle {
        tx: Some(tx),
        join: Some(join),
    }
}
