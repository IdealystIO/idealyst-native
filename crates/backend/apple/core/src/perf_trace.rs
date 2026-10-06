//! Frame-pacing trace for diagnosing animation stutter / scroll jank on
//! iOS/tvOS and macOS.
//!
//! Debug-only (`cfg(debug_assertions)`). Self-installs from
//! [`scheduler::install_scheduler`](crate::scheduler::install_scheduler) and is
//! stripped entirely from release builds, so there is zero cost shipped.
//!
//! ## What it measures and why
//!
//! Two clocks, compared per one-second window:
//!
//! - A **`CADisplayLink` in `NSRunLoopCommonModes`** — fires every vsync and
//!   keeps firing *through* scroll/pan gestures (common modes includes UIKit's
//!   `UITrackingRunLoopMode` / AppKit's `NSEventTrackingRunLoopMode`). Its
//!   inter-fire delta is a direct dropped-frame signal: a delta well over
//!   16.7 ms means the main thread stalled for that frame. The link is vended
//!   from the `CADisplayLink` class on iOS/tvOS and from `+[NSScreen
//!   mainScreen]` on macOS.
//! - The framework's **`raf_loop` animation tick**, counted via [`on_raf_tick`]
//!   (see [`scheduler`](crate::scheduler)). On iOS/tvOS the raf loop is itself a
//!   `CADisplayLink`, so a low `anim` count just means "nothing was animating".
//!   On **macOS the raf loop is a common-mode `NSTimer`** — and the open
//!   question is whether AppKit's event-tracking run-loop mode *starves* that
//!   timer during a trackpad scroll even though it nominally runs in common
//!   modes. If `frames` (the display link) holds ~60 while `anim` collapses
//!   during a scroll, the timer is being starved.
//!
//! Holding the two side by side is the whole point: during a drag/scroll, if the
//! display link keeps ticking (~60/s) but the raf tick count collapses, the
//! spring animations are *frozen for the duration of the gesture* — which reads
//! as stutter. Conversely, a display-link delta that itself blows past the frame
//! budget means the main thread stalled doing real per-frame work.
//!
//! It is **self-silencing**: a window is only logged when something was
//! animating (`raf` ticked) or a long frame occurred, so an idle app prints
//! nothing. Watch the console during a drag.

use std::cell::{Cell, RefCell};
use std::time::Instant;

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, NSObject};
use objc2::{class, declare_class, msg_send, msg_send_id, mutability, sel, ClassType, DeclaredClass};
use objc2_foundation::{MainThreadMarker, NSString};

use crate::log::apple_log;

/// A frame is counted as dropped when its interval exceeds the detected vsync
/// period times this factor. 1.5× a 120 Hz period (8.3 ms) ≈ 12.5 ms; 1.5× a
/// 60 Hz period (16.7 ms) ≈ 25 ms. Hardcoding 20 ms (the old value) was wrong on
/// ProMotion — it silently undercounts, since a 10 ms frame there is already a
/// full dropped frame. Deriving the budget from the link's own cadence makes the
/// count correct on 60/120/ProMotion-variable displays alike.
const DROP_FACTOR: f64 = 1.5;
/// Fallback vsync period (ms) when the screen's nominal refresh is unknown
/// (`maximumFramesPerSecond` == 0). 60 Hz is the safe assumption.
const FALLBACK_VSYNC_MS: f64 = 1000.0 / 60.0;
/// Summary window length (seconds).
const WINDOW_S: f64 = 1.0;

struct Recorder {
    window_start: Option<Instant>,
    last_frame: Option<Instant>,
    /// Display-link fires (≈ vsyncs) this window.
    frames: u32,
    /// Display-link deltas over the drop budget this window.
    long_frames: u32,
    /// Worst single display-link delta this window (ms).
    worst_ms: f64,
    /// Nominal vsync period (ms), from the screen's `maximumFramesPerSecond`,
    /// read once at install. The authoritative refresh budget — NOT estimated
    /// from observed intervals (a CADisplayLink can fire a late frame then a
    /// catch-up frame, yielding a sub-period interval that corrupts a running
    /// minimum). `FALLBACK_VSYNC_MS` until `set_nominal_vsync` runs.
    nominal_vsync_ms: f64,
    /// Framework `raf_loop` ticks this window (animation advances).
    raf_ticks: u32,
    /// Kept alive for the process lifetime so the link keeps firing.
    _link: Option<Retained<NSObject>>,
    _target: Option<Retained<DisplayProbe>>,
}

impl Recorder {
    fn new() -> Self {
        Self {
            window_start: None,
            last_frame: None,
            frames: 0,
            long_frames: 0,
            worst_ms: 0.0,
            nominal_vsync_ms: FALLBACK_VSYNC_MS,
            raf_ticks: 0,
            _link: None,
            _target: None,
        }
    }
}

thread_local! {
    static REC: RefCell<Recorder> = RefCell::new(Recorder::new());
    static INSTALLED: Cell<bool> = const { Cell::new(false) };
}

/// Record the screen's nominal refresh, in frames per second, as the dropped-
/// frame budget basis. A value of 0 (unknown) leaves the fallback in place.
fn set_nominal_vsync(max_fps: f64) {
    if max_fps > 0.0 {
        REC.with(|r| r.borrow_mut().nominal_vsync_ms = 1000.0 / max_fps);
    }
}

/// Record one framework animation tick. Called from `scheduler::raf_loop`'s
/// per-frame block (debug builds only).
pub fn on_raf_tick() {
    REC.with(|r| r.borrow_mut().raf_ticks += 1);
}

/// One display-link fire: update frame pacing and, once per window, log a
/// summary if anything was animating or janky.
fn on_display_tick() {
    let now = Instant::now();
    REC.with(|cell| {
        let mut r = cell.borrow_mut();
        let ws = *r.window_start.get_or_insert(now);
        if let Some(last) = r.last_frame {
            let ms = now.duration_since(last).as_secs_f64() * 1000.0;
            r.frames += 1;
            if ms > r.worst_ms {
                r.worst_ms = ms;
            }
            // Dropped = interval over the refresh-relative budget.
            if ms > r.nominal_vsync_ms * DROP_FACTOR {
                r.long_frames += 1;
            }
        }
        r.last_frame = Some(now);

        let elapsed = now.duration_since(ws).as_secs_f64();
        if elapsed >= WINDOW_S {
            // Self-silence: only report windows with activity.
            if r.raf_ticks > 0 || r.long_frames > 0 {
                let fps = r.frames as f64 / elapsed;
                // Now that the animation clock is CADisplayLink-driven, it ticks
                // every frame WHILE animating and 0 when idle — so a low `anim`
                // count means "nothing was animating this window", NOT starvation
                // (that was the old NSTimer failure mode). The signal that
                // actually matters for stutter is DROPPED FRAMES: display-link
                // intervals over the budget, i.e. the main thread stalled.
                let hint = if r.long_frames > 0 {
                    "  <- DROPPED FRAMES (main thread stalled — heavy per-frame work)"
                } else {
                    ""
                };
                let hz = 1000.0 / r.nominal_vsync_ms;
                let budget_ms = r.nominal_vsync_ms * DROP_FACTOR;
                apple_log(&format!(
                    "[perf] {frames} frames ({fps:.0} fps, {hz:.0}Hz display) · \
                     anim {raf}/{frames} · dropped(>{budget:.1}ms) {longn} · \
                     worst {worst:.1}ms{hint}",
                    frames = r.frames,
                    fps = fps,
                    hz = hz,
                    raf = r.raf_ticks,
                    budget = budget_ms,
                    longn = r.long_frames,
                    worst = r.worst_ms,
                    hint = hint,
                ));
            }
            r.window_start = Some(now);
            r.frames = 0;
            r.long_frames = 0;
            r.worst_ms = 0.0;
            r.raf_ticks = 0;
        }
    });
}

// A bare `CADisplayLink` target. The link only has a target/selector API (no
// block initializer), so we declare a tiny class with a `tick:` method.
struct DisplayProbeIvars;

declare_class!(
    struct DisplayProbe;

    unsafe impl ClassType for DisplayProbe {
        type Super = NSObject;
        type Mutability = mutability::InteriorMutable;
        const NAME: &'static str = "IdealystPerfDisplayProbe";
    }

    impl DeclaredClass for DisplayProbe {
        type Ivars = DisplayProbeIvars;
    }

    unsafe impl DisplayProbe {
        #[method(tick:)]
        fn tick(&self, _sender: &NSObject) {
            on_display_tick();
        }
    }
);

impl DisplayProbe {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(DisplayProbeIvars);
        unsafe { msg_send_id![super(this), init] }
    }
}

/// Install the frame-pacing trace. Idempotent; main-thread only (no-op
/// otherwise). Called from `install_scheduler` in debug iOS/tvOS/macOS builds.
pub fn install() {
    // The display link and the screen are main-thread objects, so off the main
    // thread this is a no-op, as documented. `install_scheduler` is reached off
    // the main thread by libtest's worker threads in the backends' host tests.
    if !is_main_thread() {
        return;
    }
    if INSTALLED.with(|c| c.replace(true)) {
        return;
    }
    // SAFETY: checked just above.
    let mtm = unsafe { MainThreadMarker::new_unchecked() };
    install_on_main(mtm);
}

fn is_main_thread() -> bool {
    // libSystem, always linked on Apple targets. Used instead of
    // `MainThreadMarker::new()`, which needs objc2-foundation's `NSThread`
    // feature for one call.
    extern "C" {
        fn pthread_main_np() -> std::os::raw::c_int;
    }
    unsafe { pthread_main_np() != 0 }
}

/// Look up an Objective-C class by name, or `None` when its framework is not
/// loaded in this process.
///
/// Why not `class!(..)`: that macro PANICS when the class is missing, and this
/// crate deliberately links neither UIKit nor AppKit (see Cargo.toml). The app
/// always has them — the leaf backend and the app link them — but a host test
/// binary built from a backend crate does not. `cargo test -p
/// backend-ios-mobile` on a Mac compiles this file's macOS branch into a binary
/// that never links AppKit, and `class!(NSScreen)` aborted every test that
/// reached `install_scheduler`. A debug-only trace must never take the process
/// down, so a missing class means "nothing to pace against" and we skip.
fn lookup_class(name: &str) -> Option<&'static AnyClass> {
    AnyClass::get(name)
}

/// The body of [`install`], given proof of the main thread. Returns whether a
/// display link was installed (`false` when the display API isn't available in
/// this process, or there is no screen).
fn install_on_main(mtm: MainThreadMarker) -> bool {
    // `displayLinkWithTarget:selector:` returns an autoreleased CADisplayLink
    // (msg_send_id retains it). The link retains its target.
    //
    // The factory differs by platform: iOS/tvOS vend it off the `CADisplayLink`
    // class itself; macOS (14+) has no such class method — the display link is
    // vended from an `NSScreen`/`NSView`/`NSWindow`. We use `+[NSScreen
    // mainScreen]`, which needs no view and tracks the primary display's vsync.
    // Each platform branch names only its own toolkit's classes: `NSScreen`
    // does not exist on iOS/tvOS, `UIScreen` does not exist on macOS.
    #[cfg(any(target_os = "ios", target_os = "tvos"))]
    let link: Retained<NSObject> = {
        let Some(display_link_class) = lookup_class("CADisplayLink") else {
            apple_log("[perf] frame-pacing trace: QuartzCore not loaded, skipping.");
            return false;
        };
        // The screen's nominal refresh is the dropped-frame budget basis.
        if let Some(screen_class) = lookup_class("UIScreen") {
            let screen: Option<Retained<NSObject>> =
                unsafe { msg_send_id![screen_class, mainScreen] };
            if let Some(screen) = screen {
                let max_fps: isize = unsafe { msg_send![&*screen, maximumFramesPerSecond] };
                set_nominal_vsync(max_fps as f64);
            }
        }
        let target = DisplayProbe::new(mtm);
        let link = unsafe {
            msg_send_id![
                display_link_class,
                displayLinkWithTarget: &*target,
                selector: sel!(tick:)
            ]
        };
        REC.with(|cell| cell.borrow_mut()._target = Some(target));
        link
    };
    #[cfg(target_os = "macos")]
    let link: Retained<NSObject> = {
        let Some(screen_class) = lookup_class("NSScreen") else {
            // AppKit not loaded (a host test binary of a backend crate).
            apple_log("[perf] frame-pacing trace: AppKit not loaded, skipping.");
            return false;
        };
        let screen: Option<Retained<NSObject>> =
            unsafe { msg_send_id![screen_class, mainScreen] };
        let Some(screen) = screen else {
            // Headless / no attached display — nothing to pace against.
            apple_log("[perf] frame-pacing trace: no main screen, skipping.");
            return false;
        };
        // The screen's nominal refresh is the dropped-frame budget basis
        // (e.g. 120 on ProMotion). `maximumFramesPerSecond` is macOS 12+.
        let max_fps: isize = unsafe { msg_send![&*screen, maximumFramesPerSecond] };
        set_nominal_vsync(max_fps as f64);
        let target = DisplayProbe::new(mtm);
        let link = unsafe {
            msg_send_id![
                &*screen,
                displayLinkWithTarget: &*target,
                selector: sel!(tick:)
            ]
        };
        REC.with(|cell| cell.borrow_mut()._target = Some(target));
        link
    };

    // Common modes so it keeps ticking during scroll/drag tracking — that is
    // exactly the window we want to observe.
    extern "C" {
        static NSRunLoopCommonModes: *const NSString;
    }
    let run_loop: Retained<NSObject> =
        unsafe { msg_send_id![class!(NSRunLoop), mainRunLoop] };
    let common_modes: &NSString = unsafe { &*NSRunLoopCommonModes };
    let _: () = unsafe { msg_send![&*link, addToRunLoop: &*run_loop, forMode: common_modes] };

    REC.with(|cell| cell.borrow_mut()._link = Some(link));

    apple_log(
        "[perf] frame-pacing trace ON (debug build). Logs per second while \
         animating; watch 'raf' ticks vs frames during a drag.",
    );
    true
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    /// Regression: `cargo test -p backend-ios-mobile` on a Mac failed four
    /// tests with "class NSScreen could not be found" — the macOS branch used
    /// `class!(NSScreen)`, which panics when AppKit isn't loaded, and a backend
    /// crate's host test binary doesn't link AppKit. This crate's own test
    /// binary doesn't either, so it reproduces the same process shape.
    #[test]
    fn regression_install_without_appkit_skips_instead_of_panicking() {
        assert!(
            AnyClass::get("NSScreen").is_none(),
            "precondition: this test binary must not load AppKit, or it no \
             longer exercises the missing-class path"
        );
        // SAFETY: with AppKit absent the install bails before touching any
        // main-thread-only object; the marker is never used.
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        assert!(!install_on_main(mtm), "no display API, so no link is installed");
        REC.with(|r| assert!(r.borrow()._link.is_none()));
    }

    /// `install` is documented as a no-op off the main thread; libtest runs
    /// tests on worker threads, so this is the path every backend host test
    /// that reaches `install_scheduler` takes.
    #[test]
    fn install_off_main_thread_is_a_no_op() {
        assert!(!is_main_thread());
        install();
        assert!(!INSTALLED.with(|c| c.get()), "off-main install must not latch INSTALLED");
    }
}
