//! Redraw scheduling hook. Platform-agnostic.
//!
//! Many paths inside the backend (`apply_style`, `insert`,
//! `update_*_value`, the animator, …) need to wake the platform
//! event loop so the next frame paints the updated tree. Rather
//! than hardcoding a winit `EventLoopProxy`, the core exposes an
//! installable closure that the platform shell (`host-winit`
//! for winit, a future `-web` for the browser) sets up at
//! startup.
//!
//! Stored in a `OnceLock` with a `Send + Sync` bound so worker
//! threads can ping the event loop without bouncing through the
//! main thread. Single-threaded callers see no behavior change.

use std::sync::OnceLock;

type RedrawFn = Box<dyn Fn() + Send + Sync>;

static REDRAW_HOOK: OnceLock<RedrawFn> = OnceLock::new();

/// Install the platform shell's redraw closure. First call wins;
/// subsequent calls are ignored (handy for tests that swap hosts —
/// install the new one *before* spawning new infra).
pub fn install_redraw_hook(f: RedrawFn) {
    let _ = REDRAW_HOOK.set(f);
}

/// Ask the platform shell to schedule another paint. No-op if no
/// hook is installed yet — typical during the build phase before
/// the shell has wired up its event loop. Safe to call from any
/// thread.
pub fn request_redraw() {
    if let Some(f) = REDRAW_HOOK.get() {
        f();
    }
}
