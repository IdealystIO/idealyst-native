//! AppKit host shell for the `backend-macos` native backend.
//!
//! Boots NSApplication, opens a single NSWindow with a flipped
//! content view, installs the backend, and mounts the author's scene
//! through `backend_macos::newcore::start`. The window is the host's
//! responsibility (per the macOS spec — host owns, injects content
//! view); the backend never touches NSApplication or NSWindow
//! directly.
//!
//! See `docs/macos-backend-plan.md` for the design.

#[cfg(target_os = "macos")]
mod app;

#[cfg(target_os = "macos")]
mod boot;

#[cfg(target_os = "macos")]
mod app_delegate;

#[cfg(target_os = "macos")]
pub use app::{RunError, RunOptions};

#[cfg(target_os = "macos")]
pub use boot::{run, run_with};

/// Compatibility path. The windowed boot used to live behind a
/// `newcore` module while the framework carried two cores; callers and
/// docs spell it `host_appkit::newcore::run` / `::run_with`. There is
/// one core now and the entries live at the crate root ([`crate::run`],
/// [`crate::run_with`]) — this re-export keeps the historical paths
/// resolving.
pub mod newcore {
    pub use crate::{run, run_with};
}

// runtime-server variant. Mirrors `run` but, instead of mounting the user's
// app() locally, connects to an runtime-server dev-server and applies the
// command stream the sidecar produces. Only present when the
// `runtime-server` Cargo feature is on (forwards to
// `backend-macos/runtime-server`).
#[cfg(all(target_os = "macos", feature = "runtime-server"))]
pub use app::run_aas;

/// Run `f` once, after [`run`] / [`run_with`] has mounted the app and
/// just before it enters the AppKit run loop.
///
/// The reason this exists: the Robot bridge's poll is SCHEDULER-driven
/// (`bridge::schedule_periodic_poll` bails outright when no scheduler is
/// installed), and the scheduler is installed inside [`run_with`], which
/// then blocks in `NSApplication::run` for the process lifetime. A
/// generated wrapper therefore cannot start the bridge before handing
/// control over (no scheduler yet) and has nowhere to do it after. This
/// threads it between the two. Started too early, a relay connection
/// comes up and its commands queue forever; never started, the CLI's
/// relay answers every verb "no app connected" — the empty-Inspector
/// symptom a macOS dev build had.
///
/// Kept generic (rather than a bridge-specific hook) so `host-appkit`
/// needs no dependency on the robot surface; the wrapper owns that, gated
/// on its own `dev` feature. Same shape as `host_gtk::on_main_loop_start`.
/// Hooks run in registration order; one registered after the loop has
/// started never runs.
pub fn on_main_loop_start<F: FnOnce() + 'static>(f: F) {
    MAIN_LOOP_START.with(|hooks| hooks.borrow_mut().push(Box::new(f)));
}

thread_local! {
    static MAIN_LOOP_START: std::cell::RefCell<Vec<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Drain and run every [`on_main_loop_start`] hook, in registration
/// order. The list is taken first, so a hook may register another.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn run_main_loop_start_hooks() {
    let hooks = MAIN_LOOP_START.with(|h| std::mem::take(&mut *h.borrow_mut()));
    for hook in hooks {
        hook();
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    #[test]
    fn main_loop_start_hooks_run_once_in_order() {
        let log: Rc<RefCell<Vec<u8>>> = Rc::default();
        for i in 0..3 {
            let log = log.clone();
            super::on_main_loop_start(move || log.borrow_mut().push(i));
        }
        assert!(log.borrow().is_empty(), "registering must not run the hook");
        super::run_main_loop_start_hooks();
        super::run_main_loop_start_hooks();
        assert_eq!(*log.borrow(), vec![0, 1, 2]);
    }
}

#[cfg(not(target_os = "macos"))]
mod stub;

#[cfg(not(target_os = "macos"))]
pub use stub::{run, run_with, RunError, RunOptions};

#[cfg(all(not(target_os = "macos"), feature = "runtime-server"))]
pub use stub::run_aas;
