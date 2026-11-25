//! The macOS animation clock: a [`FrameClock`] driven by a view-vended
//! `CADisplayLink`, with an NSTimer backstop. Backs the scheduler's `raf_loop`
//! and `after_animation_frame` on macOS.
//!
//! Why this shape:
//!
//! - **`-[NSView displayLinkWithTarget:selector:]` (macOS 14+)**, from the
//!   backend's host root view, not `NSScreen`'s: AppKit documents the view's
//!   link as calling back "in-sync with the display the view is on", so it
//!   follows the window when it moves to a screen with a different refresh
//!   rate. A screen-vended link stays on its screen. The link is added in
//!   common modes so it keeps ticking through `NSEventTrackingRunLoopMode`
//!   (scroll, drag, menu tracking).
//! - **One link for every subscriber**, paused (`setPaused:`) when the last
//!   subscriber leaves: no idle frame callbacks, and no link churn per tween.
//! - **NSTimer backstop.** The view's link does not call back while "the view
//!   is hidden, or not on any display" (NSView.h). A clock with only the link
//!   would freeze every animation, anchored-portal tracker and
//!   `after_animation_frame` one-shot in a window that was never shown or is
//!   minimized. The backstop ticks only after the link has been silent for
//!   [`DISPLAY_STALL_US`](crate::frame_clock::DISPLAY_STALL_US), so while the
//!   window is on a display every tick that runs is a vsync tick.
//! - **No `CVDisplayLink` below macOS 14.** Its callback runs on a CoreVideo
//!   thread, so it needs a coalesced hop to the main queue plus a
//!   `CVDisplayLinkSetCurrentCGDisplay` re-bind on every screen change, and the
//!   API is deprecated as of macOS 15 in favour of the view/window/screen links
//!   used here. On macOS 11–13 the timer alone drives the clock, at the
//!   screen's `maximumFramesPerSecond` (macOS 12+; 60 otherwise): the right
//!   number of ticks on a high-refresh display, not phase-locked to vsync.
//!
//! The link and the timer are created on the current thread's run loop. On
//! the main thread (every real caller) that is the main run loop. A libtest
//! worker thread that reaches the scheduler gets its own run loop, which never
//! runs, so a worker's callbacks never fire on the main thread against that
//! worker's thread-local clock.

use std::rc::Rc;
use std::time::Instant;

use block2::StackBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, NSObject};
use objc2::{declare_class, msg_send, msg_send_id, mutability, sel, ClassType, DeclaredClass};
use objc2_foundation::{MainThreadMarker, NSString};

use crate::frame_clock::{FrameClock, FrameSource, FrameSubscription, TickOrigin};

/// Backstop rate while a display link is the primary source: what a
/// timer-only clock ticks at, for the windows the link is silent in.
const BACKSTOP_FPS: f64 = 60.0;

thread_local! {
    static CLOCK: Rc<FrameClock<MacFrameSource>> =
        FrameClock::new(MacFrameSource::default(), after_tick);
    static EPOCH: Instant = Instant::now();
}

/// Once per frame in which a subscriber ran: count the tick for the debug
/// frame-pacing trace, then fire the dispatch hook so the writes the
/// subscribers staged get flushed (one flush per frame, however many loops
/// ran).
fn after_tick() {
    #[cfg(debug_assertions)]
    crate::perf_trace::on_raf_tick();
    crate::dispatch_hook::fire_dispatch_hook();
}

fn now_us() -> u64 {
    EPOCH.with(|e| e.elapsed().as_micros() as u64)
}

/// A frame from either source. `try_with`: a callback arriving during
/// thread-local teardown has no clock left to tick.
fn tick(origin: TickOrigin, site: &'static str) {
    let Ok(clock) = CLOCK.try_with(|c| c.clone()) else {
        return;
    };
    crate::crash::abort_on_panic(site, || {
        clock.on_tick(origin, now_us());
    });
}

pub(crate) fn subscribe_every(f: Box<dyn FnMut() + 'static>) -> FrameSubscription {
    CLOCK.with(|c| c.subscribe_every(f))
}

pub(crate) fn subscribe_once(f: Box<dyn FnOnce() + 'static>) -> FrameSubscription {
    CLOCK.with(|c| c.subscribe_once(f))
}

/// Rebind the display link to `view`'s display. A running clock switches over
/// at once; an idle one makes the link on its next start.
pub(crate) fn set_owner(view: Retained<NSObject>) {
    CLOCK.with(|c| c.with_source(|source, running| source.set_owner(view, running)));
}

/// Whether the clock's source is started (nothing subscribed → `false`).
pub(crate) fn is_running() -> bool {
    CLOCK.with(|c| c.is_running())
}

/// Whether the source has a display link (macOS 14+ with an owner view).
pub(crate) fn has_display_link() -> bool {
    CLOCK.with(|c| c.with_source(|source, _| source.link.is_some()))
}

pub(crate) fn is_main_thread() -> bool {
    // libSystem, always linked on Apple targets.
    extern "C" {
        fn pthread_main_np() -> std::os::raw::c_int;
    }
    unsafe { pthread_main_np() != 0 }
}

struct FrameTargetIvars;

declare_class!(
    struct FrameTarget;

    unsafe impl ClassType for FrameTarget {
        type Super = NSObject;
        type Mutability = mutability::InteriorMutable;
        const NAME: &'static str = "IdealystFrameClockDisplayTarget";
    }

    impl DeclaredClass for FrameTarget {
        type Ivars = FrameTargetIvars;
    }

    unsafe impl FrameTarget {
        #[method(frame:)]
        fn frame(&self, _link: &NSObject) {
            tick(TickOrigin::Display, "CADisplayLink frame clock");
        }
    }
);

impl FrameTarget {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(FrameTargetIvars);
        unsafe { msg_send_id![super(this), init] }
    }
}

#[derive(Default)]
struct MacFrameSource {
    /// The view the display link is vended from (the backend's host root).
    owner: Option<Retained<NSObject>>,
    /// The display link, made from `owner` on the first start and kept
    /// (paused) across idle periods.
    link: Option<Retained<NSObject>>,
    /// The link's target. The link retains it too; kept so a rebuilt link
    /// reuses it.
    target: Option<Retained<FrameTarget>>,
    /// The backstop / fallback timer, only while started.
    timer: Option<Retained<NSObject>>,
}

extern "C" {
    static NSRunLoopCommonModes: *const NSString;
}

fn current_run_loop() -> Retained<NSObject> {
    unsafe { msg_send_id![objc2::class!(NSRunLoop), currentRunLoop] }
}

impl MacFrameSource {
    fn set_owner(&mut self, view: Retained<NSObject>, running: bool) {
        if running {
            self.stop();
        }
        if let Some(link) = self.link.take() {
            let _: () = unsafe { msg_send![&*link, invalidate] };
        }
        self.owner = Some(view);
        if running {
            self.start();
        }
    }

    /// Make the display link if the owner can vend one. Stays `None` on
    /// macOS 11–13 (no `-[NSView displayLinkWithTarget:selector:]`), without
    /// an owner, and off the main thread.
    fn ensure_link(&mut self) {
        if self.link.is_some() || !is_main_thread() {
            return;
        }
        let Some(owner) = self.owner.as_ref() else {
            return;
        };
        let responds: bool = unsafe {
            msg_send![&**owner, respondsToSelector: sel!(displayLinkWithTarget:selector:)]
        };
        if !responds {
            return;
        }
        // SAFETY: checked above.
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        let target = self.target.get_or_insert_with(|| FrameTarget::new(mtm)).clone();
        let link: Option<Retained<NSObject>> = unsafe {
            msg_send_id![&**owner, displayLinkWithTarget: &*target, selector: sel!(frame:)]
        };
        let Some(link) = link else {
            return;
        };
        let common_modes: &NSString = unsafe { &*NSRunLoopCommonModes };
        let run_loop = current_run_loop();
        let _: () = unsafe { msg_send![&*link, addToRunLoop: &*run_loop, forMode: common_modes] };
        self.link = Some(link);
    }

    /// The timer-only rate: the owner's screen's (else the main screen's)
    /// `maximumFramesPerSecond` (macOS 12+), else 60.
    fn fallback_fps(&self) -> f64 {
        if !is_main_thread() {
            return BACKSTOP_FPS;
        }
        let window_screen = self.owner.as_ref().and_then(|owner| unsafe {
            let window: Option<Retained<NSObject>> = msg_send_id![&**owner, window];
            let window = window?;
            let screen: Option<Retained<NSObject>> = msg_send_id![&*window, screen];
            screen
        });
        // `NSScreen` is looked up by name: this crate doesn't link AppKit, and
        // a backend crate's host test binary may not load it.
        let screen = window_screen.or_else(|| {
            let class = AnyClass::get("NSScreen")?;
            unsafe { msg_send_id![class, mainScreen] }
        });
        let Some(screen) = screen else {
            return BACKSTOP_FPS;
        };
        let responds: bool =
            unsafe { msg_send![&*screen, respondsToSelector: sel!(maximumFramesPerSecond)] };
        if !responds {
            return BACKSTOP_FPS;
        }
        let fps: isize = unsafe { msg_send![&*screen, maximumFramesPerSecond] };
        if (1..=480).contains(&fps) {
            fps as f64
        } else {
            BACKSTOP_FPS
        }
    }
}

impl FrameSource for MacFrameSource {
    fn start(&mut self) {
        self.ensure_link();
        let fps = match &self.link {
            Some(link) => {
                let _: () = unsafe { msg_send![&**link, setPaused: false] };
                BACKSTOP_FPS
            }
            None => self.fallback_fps(),
        };
        let block = StackBlock::new(|_t: *const NSObject| {
            tick(TickOrigin::Timer, "NSTimer frame clock");
        });
        let block = block.copy();
        let timer: Retained<NSObject> = unsafe {
            msg_send_id![
                objc2::class!(NSTimer),
                timerWithTimeInterval: 1.0 / fps,
                repeats: true,
                block: &*block
            ]
        };
        let common_modes: &NSString = unsafe { &*NSRunLoopCommonModes };
        let run_loop = current_run_loop();
        let _: () = unsafe { msg_send![&*run_loop, addTimer: &*timer, forMode: common_modes] };
        self.timer = Some(timer);
    }

    fn stop(&mut self) {
        if let Some(link) = &self.link {
            let _: () = unsafe { msg_send![&**link, setPaused: true] };
        }
        if let Some(timer) = self.timer.take() {
            let _: () = unsafe { msg_send![&*timer, invalidate] };
        }
    }
}

#[cfg(test)]
mod tests {
    //! The real AppKit-free paths: this crate's test binary doesn't load
    //! AppKit, and libtest runs on worker threads, so these exercise the
    //! timer-only source. Display-link behaviour is covered by
    //! `backend-macos`'s `raf_display_rate` integration test.
    use super::*;
    use std::cell::Cell;

    /// Start/stop against the real NSTimer source: no idle timer once the last
    /// subscriber leaves, and no display link off the main thread.
    #[test]
    fn source_starts_on_subscribe_and_stops_when_idle() {
        assert!(!is_running());
        let ran = Rc::new(Cell::new(0));
        let r = ran.clone();
        let every = subscribe_every(Box::new(move || r.set(r.get() + 1)));
        assert!(is_running());
        assert!(!has_display_link(), "worker thread, no owner: timer only");
        let once = subscribe_once(Box::new(|| {}));
        drop(every);
        assert!(is_running(), "the pending one-shot keeps the clock running");
        drop(once);
        assert!(!is_running(), "nothing subscribed, nothing scheduled");
        CLOCK.with(|c| c.with_source(|s, _| assert!(s.timer.is_none(), "timer invalidated")));
        assert_eq!(ran.get(), 0, "this thread's run loop never ran");
    }
}
