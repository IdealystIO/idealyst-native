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
//!
//! It also receives inbound links — custom-scheme deep links (the
//! `kAEGetURL` Apple Event) and universal links — through
//! `application:openURLs:`, and hands them to the framework.
//!
//! # The launch link arrives after the first mount
//!
//! The host mounts the app BEFORE `NSApp.run()`, and AppKit delivers the
//! URL that launched the app from inside `run` (between
//! `applicationWillFinishLaunching:` and `applicationDidFinishLaunching:`)
//! — too late to seed the navigators' launch slot the way UIKit's
//! `launchOptions` and Android's launch `Intent` do. So a URL that arrives
//! before `applicationDidFinishLaunching:` is treated as THE launch link:
//! recorded as `initial_link()` and routed to its screen without reaching
//! observers or interceptors — the same contract the launch link has on
//! every other target (it routes, and `on_link` sees only later links).
//! Its navigation commits on the first flush, before the first frame is
//! drawn.

use std::cell::Cell;

use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol};
use objc2::{declare_class, msg_send_id, mutability, ClassType, DeclaredClass};
use objc2_app_kit::{NSApplication, NSApplicationDelegate};
use objc2_foundation::{MainThreadMarker, NSArray, NSNotification, NSURL};

pub(crate) struct IdealystAppDelegateIvars {
    /// `applicationDidFinishLaunching:` has fired — URLs from now on are
    /// warm links, not the launch link (module docs).
    finished_launching: Cell<bool>,
}

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

        #[method(applicationDidFinishLaunching:)]
        fn did_finish_launching(&self, _notification: &NSNotification) {
            self.ivars().finished_launching.set(true);
        }

        /// Custom-scheme and universal links, at launch and while running.
        #[method(application:openURLs:)]
        fn open_urls(&self, _application: &NSApplication, urls: &NSArray<NSURL>) {
            let launching = !self.ivars().finished_launching.get();
            for url in urls.iter() {
                let Some(url) = (unsafe { url.absoluteString() }) else { continue };
                let url = url.to_string();
                if launching {
                    backend_macos::newcore::deliver_launch_link(&url);
                } else {
                    backend_macos::newcore::deliver_inbound_link(&url);
                }
            }
        }
    }
);

impl IdealystAppDelegate {
    pub(crate) fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(IdealystAppDelegateIvars { finished_launching: Cell::new(false) });
        unsafe { msg_send_id![super(this), init] }
    }
}
