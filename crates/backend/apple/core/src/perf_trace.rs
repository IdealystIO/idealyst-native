//! Frame-pacing trace for diagnosing animation stutter on iOS/tvOS.
//!
//! Debug-only (`cfg(debug_assertions)`), iOS/tvOS-only. Self-installs from
//! [`scheduler::install_scheduler`](crate::scheduler::install_scheduler) and is
//! stripped entirely from release builds, so there is zero cost shipped.
//!
//! ## What it measures and why
//!
//! Two clocks, compared per one-second window:
//!
//! - A **`CADisplayLink` in `NSRunLoopCommonModes`** — fires every vsync and
//!   keeps firing *through* scroll/pan gestures (common modes). Its inter-fire
//!   delta is a direct dropped-frame signal: a delta well over 16.7 ms means the
//!   main thread stalled for that frame.
//! - The framework's **`raf_loop` animation tick** (a 60 Hz `NSTimer` in the
//!   DEFAULT run-loop mode — see [`scheduler`](crate::scheduler)), counted via
//!   [`on_raf_tick`]. Default mode means it is PAUSED while UIKit is in
//!   `UITrackingRunLoopMode` (an active scroll/drag gesture).
//!
//! Holding the two side by side is the whole point: during a drag, if the
//! display link keeps ticking (~60/s) but the raf tick count collapses, the
//! spring animations (cards sliding to open a gap, the drop glide) are *frozen
//! for the duration of the gesture* — which reads as stutter. The summary line
//! flags that as `RAF STARVED`.
//!
//! It is **self-silencing**: a window is only logged when something was
//! animating (`raf` ticked) or a long frame occurred, so an idle app prints
//! nothing. Watch the console during a drag.

use std::cell::{Cell, RefCell};
use std::time::Instant;

use objc2::rc::Retained;
use objc2::runtime::NSObject;
use objc2::{class, declare_class, msg_send, msg_send_id, mutability, sel, ClassType, DeclaredClass};
use objc2_foundation::{MainThreadMarker, NSString};

use crate::log::apple_log;

/// A frame slower than this (ms) is counted as a hitch. A clean 60 fps frame is
/// ~16.7 ms; 20 ms means at least the start of a dropped frame.
const LONG_FRAME_MS: f64 = 20.0;
/// Summary window length (seconds).
const WINDOW_S: f64 = 1.0;

struct Recorder {
    window_start: Option<Instant>,
    last_frame: Option<Instant>,
    /// Display-link fires (≈ vsyncs) this window.
    frames: u32,
    /// Display-link deltas over [`LONG_FRAME_MS`] this window.
    long_frames: u32,
    /// Worst single display-link delta this window (ms).
    worst_ms: f64,
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
            if ms > LONG_FRAME_MS {
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
                apple_log(&format!(
                    "[perf] {frames} frames ({fps:.0} fps) · anim {raf}/{frames} · \
                     dropped(>{long_ms:.0}ms) {longn} · worst {worst:.1}ms{hint}",
                    frames = r.frames,
                    fps = fps,
                    raf = r.raf_ticks,
                    long_ms = LONG_FRAME_MS,
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
/// otherwise). Called from `install_scheduler` in debug iOS/tvOS builds.
pub fn install() {
    if INSTALLED.with(|c| c.replace(true)) {
        return;
    }
    // SAFETY: `install_scheduler` (our only caller) runs once at startup on the
    // main thread, before the first render — the same assumption the rest of the
    // Apple scheduler makes.
    let mtm = unsafe { MainThreadMarker::new_unchecked() };

    let target = DisplayProbe::new(mtm);
    // `displayLinkWithTarget:selector:` returns an autoreleased CADisplayLink
    // (msg_send_id retains it). The link retains its target.
    let link: Retained<NSObject> = unsafe {
        msg_send_id![
            class!(CADisplayLink),
            displayLinkWithTarget: &*target,
            selector: sel!(tick:)
        ]
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

    REC.with(|cell| {
        let mut r = cell.borrow_mut();
        r._link = Some(link);
        r._target = Some(target);
    });

    apple_log(
        "[perf] frame-pacing trace ON (debug build). Logs per second while \
         animating; watch 'raf' ticks vs frames during a drag.",
    );
}
