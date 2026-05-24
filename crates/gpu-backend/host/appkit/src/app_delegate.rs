//! `IdealystAppDelegate` — minimal `NSApplicationDelegate` that
//! terminates the process when the last window closes.
//!
//! Without this, `Cmd-W` / clicking the red traffic light closes the
//! window but leaves `NSApp` running in the background — the user has
//! to Cmd-Q to actually quit. For a single-window app that's just a
//! footgun: the process keeps holding ports / file handles / etc.
//!
//! `applicationShouldTerminateAfterLastWindowClosed:` returning `YES`
//! is the AppKit-blessed knob for "this app dies with its window."

use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol};
use objc2::{declare_class, msg_send_id, mutability, ClassType, DeclaredClass};
use objc2_app_kit::{NSApplication, NSApplicationDelegate};
use objc2_foundation::MainThreadMarker;

pub(crate) struct IdealystAppDelegateIvars;

declare_class!(
    pub(crate) struct IdealystAppDelegate;

    unsafe impl ClassType for IdealystAppDelegate {
        type Super = NSObject;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "IdealystAppDelegate";
    }

    impl DeclaredClass for IdealystAppDelegate {
        type Ivars = IdealystAppDelegateIvars;
    }

    unsafe impl NSObjectProtocol for IdealystAppDelegate {}

    unsafe impl NSApplicationDelegate for IdealystAppDelegate {
        /// Called by NSApp when the last window closes. Returning
        /// `true` makes the run loop exit cleanly — `NSApp.run()`
        /// returns, the host's `run(...)` returns, and the process
        /// terminates.
        #[method(applicationShouldTerminateAfterLastWindowClosed:)]
        fn should_terminate_after_last_window_closed(
            &self,
            _sender: &NSApplication,
        ) -> bool {
            true
        }
    }
);

impl IdealystAppDelegate {
    pub(crate) fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(IdealystAppDelegateIvars);
        unsafe { msg_send_id![super(this), init] }
    }
}
