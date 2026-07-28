//! Post-dispatch hook: a thread-local `fn()` slot fired by Apple
//! backend infrastructure **after** it has invoked a callback that may
//! run author code (scheduler timers, one-shot frames, `raf_loop`
//! iterations, async-executor future polls).
//!
//! Cross-Apple mirror of `backend-web/src/dispatch_hook.rs` — keep the
//! two in sync.
//!
//! # Why this exists (the bug prevented)
//!
//! The new core (`backend_ios::newcore`, and eventually the other
//! Apple new-core adoptions) stages signal writes; nothing is
//! observable until the flush driver calls `World::flush`. UIKit/AppKit
//! event callbacks get their flush from the dispatch-site glue in the
//! backend's `newcore.rs` (every author callback the backend installs
//! is wrapped to schedule a flush when it returns). But author code
//! also runs from surfaces that are *not* toolkit events:
//!
//! - `runtime_core::scheduling::after_ms` timers (e.g. a debounce that
//!   sets a signal),
//! - one-shot `after_animation_frame` callbacks and `raf_loop`
//!   iterations (animation ticks that stage writes),
//! - futures spawned through the cooperative async executor
//!   (`async_executor.rs` — resource/server-call completions that set
//!   signals).
//!
//! Without this hook, a write staged from any of those would sit
//! uncommitted until some unrelated toolkit event happened to trigger
//! a flush — i.e. "my `after_ms` callback ran but the UI never
//! updated". The old-core reactive system applies writes synchronously
//! and never needs this, so the slot defaults to `None` and every fire
//! site is a single thread-local read — a no-op unless a new-core boot
//! installed the flush driver. Both Apple new-core boots install it:
//! `backend_ios::newcore::start` and `backend_macos::newcore::start`
//! (the macOS P4a NSEvent-monitor + frame-tick pair has been replaced
//! by this hook plus dispatch-site callback wrapping).
//!
//! # What deliberately does NOT fire the hook
//!
//! - **Scheduled microtasks** (`schedule_microtask` /
//!   `drain_buffered_microtasks`). The flush itself is dispatched as a
//!   scheduled microtask; firing the hook after every microtask would
//!   re-schedule a flush from inside the flush's own dispatch and spin
//!   the main queue forever. No author code reaches the microtask
//!   queue outside an already-hooked surface (toolkit events, timers,
//!   frames, future polls), so nothing is lost.
//! - **`render_loop`** (`backend-ios-core`'s NSTimer draw driver): it
//!   drives the wgpu `draw_frame` for old-core hosts and runs no
//!   author reactive code. Animation-clock ticks ride `raf_loop`,
//!   which IS hooked.

use std::cell::Cell;

thread_local! {
    static HOOK: Cell<Option<fn()>> = const { Cell::new(None) };
}

/// Install the post-dispatch hook (replaces any previous one).
/// The Apple new-core boots (`backend_ios::newcore`,
/// `backend_macos::newcore`) install their `schedule_flush` here.
pub fn install_dispatch_hook(f: fn()) {
    HOOK.with(|h| h.set(Some(f)));
}

/// Remove the hook (used by new-core `stop`/teardown paths). Fire
/// sites revert to no-ops.
pub fn clear_dispatch_hook() {
    HOOK.with(|h| h.set(None));
}

/// Fire the hook if installed. Called by backend infrastructure right
/// after a potentially-author-code callback returns. Cheap when no
/// hook is installed (one thread-local `Cell` read).
///
/// `pub` (web's is `pub(crate)`) because Apple fire sites span crates:
/// the scheduler/executor here, plus any leaf-crate driver that later
/// grows an author-code surface. Do not call this from application
/// code — it belongs to backend infrastructure only.
pub fn fire_dispatch_hook() {
    if let Some(f) = HOOK.with(|h| h.get()) {
        f();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    thread_local! {
        static FIRED: Cell<u32> = const { Cell::new(0) };
    }

    fn bump() {
        FIRED.with(|f| f.set(f.get() + 1));
    }

    /// The slot semantics the flush driver relies on: no-op before
    /// install, fires after, silent again after clear (a torn-down
    /// new-core app must not leave a dangling flush hook behind).
    #[test]
    fn hook_fires_only_between_install_and_clear() {
        clear_dispatch_hook(); // clean slate (thread-local)
        FIRED.with(|f| f.set(0));

        fire_dispatch_hook();
        assert_eq!(FIRED.with(|f| f.get()), 0, "no-op before install");

        install_dispatch_hook(bump);
        fire_dispatch_hook();
        fire_dispatch_hook();
        assert_eq!(FIRED.with(|f| f.get()), 2, "fires once per call site");

        clear_dispatch_hook();
        fire_dispatch_hook();
        assert_eq!(FIRED.with(|f| f.get()), 2, "silent after clear");
    }
}
