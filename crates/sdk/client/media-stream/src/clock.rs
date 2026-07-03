//! A process-wide monotonic capture clock, in microseconds.
//!
//! Every frame and audio chunk a producer writes is stamped with
//! [`now_micros`] at the moment of capture. Two *independent* sources —
//! a `camera` [`MediaStream`](crate::MediaStream) and a `microphone`
//! [`AudioStream`](crate::AudioStream), say — therefore land on **one shared
//! timeline**, which is exactly what a muxer needs to lip-sync audio against
//! video when it writes them to a file.
//!
//! The epoch is "first read of the clock this process" (so the first stamp is
//! near `0`); only *differences* between stamps are meaningful, never the
//! absolute value. The clock is monotonic on native (`Instant`); on web it
//! reads `Date.now()` (see the caveat below).

/// Microseconds since this process first read the capture clock.
///
/// Monotonic and shared across every producer in the process, so timestamps
/// from different capture sources are directly comparable. Used to stamp
/// [`VideoFrame::pts_micros`](crate::VideoFrame) and
/// [`AudioFrame::pts_micros`](crate::AudioFrame).
#[cfg(not(target_arch = "wasm32"))]
pub fn now_micros() -> u64 {
    use std::sync::OnceLock;
    use std::time::Instant;
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_micros() as u64
}

/// Web build: there is no `Instant` time source on `wasm32-unknown-unknown`,
/// so we read wall-clock `Date.now()` (milliseconds) and rebase to a
/// first-read epoch. `Date` is not strictly monotonic — a system-clock
/// adjustment can perturb it — but the web file-writer mux path drives sync
/// through the browser's `MediaRecorder`, which carries its own timeline, so
/// these stamps are only a fallback ordering hint there.
#[cfg(target_arch = "wasm32")]
pub fn now_micros() -> u64 {
    use std::cell::Cell;
    thread_local! {
        static EPOCH_MS: Cell<Option<f64>> = const { Cell::new(None) };
    }
    let now = js_sys::Date::now();
    EPOCH_MS.with(|e| {
        let epoch = e.get().unwrap_or_else(|| {
            e.set(Some(now));
            now
        });
        ((now - epoch).max(0.0) * 1_000.0) as u64
    })
}
