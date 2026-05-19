//! Redraw scheduling hook. Platform-agnostic.
//!
//! Many paths inside the backend (`apply_style`, `insert`,
//! `update_*_value`, the animator, …) need to wake the platform
//! event loop so the next frame paints the updated tree. Rather
//! than hardcoding a winit `EventLoopProxy`, the core exposes an
//! installable closure that the platform shell (`backend-wgpu-native`
//! for winit, a future `-web` for the browser) sets up at
//! startup.
//!
//! The hook lives in thread-local storage. Single-threaded model
//! matches the framework's reactivity system and the rest of this
//! backend; cross-thread redraws need to post into this thread
//! before calling.

use std::cell::RefCell;

thread_local! {
    static REDRAW_HOOK: RefCell<Option<Box<dyn Fn()>>> = const { RefCell::new(None) };
}

/// Install the platform shell's redraw closure. First call wins;
/// subsequent calls overwrite (handy for tests that swap hosts).
pub fn install_redraw_hook(f: Box<dyn Fn()>) {
    REDRAW_HOOK.with(|h| *h.borrow_mut() = Some(f));
}

/// Ask the platform shell to schedule another paint. No-op if no
/// hook is installed yet — typical during the build phase before
/// the shell has wired up its event loop.
pub fn request_redraw() {
    REDRAW_HOOK.with(|h| {
        if let Some(f) = h.borrow().as_ref() {
            f();
        }
    });
}
