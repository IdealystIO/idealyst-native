//! Apple-platform helper: schedule a recurring callback onto the
//! main run loop via libdispatch. Identical mechanism on iOS, iOS
//! Simulator, and macOS — both platforms ship the same
//! `libSystem`/libdispatch ABI.
//!
//! Pre-extraction this lived inline in [`backend-ios-mobile::aas`]
//! and [`backend-macos::aas`] as a verbatim copy-paste of the
//! `dispatch_async_f` background-thread pump. Each platform's runtime-server
//! shell duplicated the panic trampoline + the 16ms sleep loop. We
//! extracted it here so:
//!
//! 1. The unsafe FFI declarations live in one place.
//! 2. The panic-trap pattern (libdispatch is C, can't unwind through
//!    it) only needs to be reviewed once.
//! 3. Future platform shells (tvOS, visionOS) get the pump for free.
//!
//! Android uses Java's `Handler.post` and isn't covered here.

#![cfg(any(target_os = "macos", target_os = "ios"))]

use std::sync::Arc;
use std::time::Duration;

/// Start a background thread that wakes every `interval` and
/// schedules `tick` on the main run loop via `dispatch_async_f`.
/// Runs for the life of the process — the spawned thread is leaked
/// (matches the lifecycle of an runtime-server app's main UI thread, which
/// only exits when the app terminates).
///
/// `tick` is wrapped in `catch_unwind` before being dispatched, so
/// a panic inside the callback never crosses the libdispatch FFI
/// boundary (which is C and can't unwind through). Panics are
/// captured via the panic-payload formatter; callers that want
/// to log the message can pass a closure that does so themselves.
///
/// Cadence is a soft 60Hz; the actual rate is constrained by the
/// main run loop's drain cadence (it may coalesce multiple posted
/// blocks per vsync). For runtime-server-shell tick work this is exactly
/// what's wanted — we get one tick per UI frame regardless of
/// jitter.
pub fn start_dispatch_main_tick<F>(tick: F)
where
    F: Fn() + Send + Sync + 'static,
{
    let tick = Arc::new(tick);
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(16));
        let cloned: Arc<dyn Fn() + Send + Sync> = tick.clone();
        // Box the trait object so the raw pointer we hand
        // libdispatch is `Sized` (`Box<dyn Fn>` is a fat pointer;
        // we need a heap-allocated wrapper to get a plain `*mut`).
        let ctx: *mut Arc<dyn Fn() + Send + Sync> = Box::into_raw(Box::new(cloned));
        unsafe {
            dispatch_async_f(
                &_dispatch_main_q as *const _ as *const std::ffi::c_void,
                ctx as *mut std::ffi::c_void,
                dispatch_trampoline,
            );
        }
    });
}

extern "C" {
    static _dispatch_main_q: std::ffi::c_void;
    fn dispatch_async_f(
        queue: *const std::ffi::c_void,
        context: *mut std::ffi::c_void,
        work: extern "C" fn(*mut std::ffi::c_void),
    );
}

extern "C" fn dispatch_trampoline(ctx: *mut std::ffi::c_void) {
    // Reclaim the Box we leaked when scheduling so the Arc inside
    // gets cleanly decremented after the tick fires.
    let arc: Box<Arc<dyn Fn() + Send + Sync>> =
        unsafe { Box::from_raw(ctx as *mut Arc<dyn Fn() + Send + Sync>) };
    let tick = *arc;
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        tick();
    }));
    if let Err(payload) = result {
        let msg = if let Some(s) = payload.downcast_ref::<&'static str>() {
            (*s).to_string()
        } else if let Some(s) = payload.downcast_ref::<String>() {
            s.clone()
        } else {
            "<non-string panic payload>".to_string()
        };
        eprintln!("[runtime-server::apple] main-tick panic absorbed: {msg}");
    }
}
