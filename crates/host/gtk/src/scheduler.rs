//! GTK/GLib implementation of `runtime_core::scheduling::Scheduler`.
//!
//! The framework routes `after_ms`, `raf_loop`, and `schedule_microtask`
//! through a single installed [`Scheduler`]. Without one, off-web
//! `after_ms` runs *synchronously* (delays collapse — every act of the
//! welcome timeline fires at once) and `raf_loop` is inert, so no
//! `AnimatedValue` ever ticks. This installs a scheduler backed by GLib
//! main-loop sources.
//!
//! Unlike the winit host (which needs a worker thread + event-loop proxy
//! because its closures must not be `Send`), GLib timeouts already run
//! their closures on the main thread via `*_local`, which is exactly
//! where the framework's `!Send` `Rc`-capturing closures must run. So
//! this is a direct mapping with no cross-thread plumbing.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use gtk4::glib;
use runtime_core::scheduling::{install_scheduler, ScheduleHandle, Scheduler};

/// Install the GTK scheduler. Call once, on the main thread, before the
/// app mounts (i.e. before any author `effect!` runs). Idempotent at the
/// framework level (`install_scheduler` uses a `OnceLock`).
pub fn install() {
    install_scheduler(Box::new(GtkScheduler));
}

/// Zero-sized; all live state is the per-source GLib closure. The
/// `Send + Sync` the trait requires is satisfied by the empty struct —
/// every method is only ever called on the GTK main thread (same
/// rationale as the winit host's `WinitScheduler`).
struct GtkScheduler;
// SAFETY: no shared state on the struct; sources are created and run on
// the single GTK main thread.
unsafe impl Send for GtkScheduler {}
unsafe impl Sync for GtkScheduler {}

/// Handle wrapping a GLib source. `done` guards against removing a
/// one-shot source that already fired (removing a spent `SourceId`
/// trips a GLib critical), while still letting `cancel`/`Drop` stop a
/// still-pending timer or a forever `raf_loop`.
struct SourceHandle {
    id: Option<glib::SourceId>,
    done: Rc<Cell<bool>>,
}

impl ScheduleHandle for SourceHandle {
    fn cancel(&mut self) {
        if let Some(id) = self.id.take() {
            if !self.done.get() {
                id.remove();
            }
        }
    }
}

impl Drop for SourceHandle {
    fn drop(&mut self) {
        self.cancel();
    }
}

impl Scheduler for GtkScheduler {
    fn schedule_microtask(&self, f: Box<dyn FnOnce() + 'static>) {
        // Run after the current stack unwinds, on the main thread.
        // Fire-and-forget by contract (no handle returned to cancel).
        glib::source::idle_add_local_once(f);
    }

    fn after_animation_frame(&self, f: Box<dyn FnOnce() + 'static>) -> Box<dyn ScheduleHandle> {
        // One animation frame ≈ 16 ms — matches the other backends.
        self.after_ms(16, f)
    }

    fn after_ms(&self, delay_ms: i32, f: Box<dyn FnOnce() + 'static>) -> Box<dyn ScheduleHandle> {
        let done = Rc::new(Cell::new(false));
        let done_cb = done.clone();
        let mut once = Some(f);
        let id = glib::source::timeout_add_local(
            Duration::from_millis(delay_ms.max(0) as u64),
            move || {
                done_cb.set(true);
                if let Some(f) = once.take() {
                    f();
                }
                glib::ControlFlow::Break
            },
        );
        Box::new(SourceHandle { id: Some(id), done })
    }

    fn raf_loop(&self, mut f: Box<dyn FnMut() + 'static>) -> Box<dyn ScheduleHandle> {
        // ~60 Hz pulse; `done` stays false for the life of the loop so
        // cancel/Drop removes the still-active source.
        let done = Rc::new(Cell::new(false));
        let id = glib::source::timeout_add_local(Duration::from_millis(16), move || {
            f();
            glib::ControlFlow::Continue
        });
        Box::new(SourceHandle { id: Some(id), done })
    }
}
