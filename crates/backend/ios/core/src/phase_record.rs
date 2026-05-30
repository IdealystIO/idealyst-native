//! Phase-record indirection so `backend-ios-core` can push timings
//! into the same aggregator the leaf `backend-ios-mobile` crate owns
//! (which in turn forwards into `runtime_core::debug`), without
//! either crate depending on the other.
//!
//! `backend-ios-mobile::imp::phase_timer::install_core_bridge` is
//! called once during `IosBackend::new` and registers the function
//! pointer below. With `debug-stats` ON the registered closure
//! pushes into `runtime_core::debug::record_apply_phase` so a single
//! `take_phase_counters()` drain sees both crates' work.
//!
//! When `debug-stats` is OFF (default), [`PhaseScope`] and
//! [`scope`] compile away to nothing — same shape as the
//! `PhaseTimer` in the web backend and the leaf iOS crate.

#[cfg(feature = "debug-stats")]
mod imp {
    use std::cell::Cell;
    use std::time::Instant;

    /// Caller signature: phase name + elapsed nanoseconds.
    pub type Recorder = fn(&'static str, u128);

    thread_local! {
        static RECORDER: Cell<Option<Recorder>> = const { Cell::new(None) };
    }

    /// Wire the writer side. Idempotent — the leaf crate calls this
    /// from `IosBackend::new`.
    pub fn install_recorder(recorder: Recorder) {
        RECORDER.with(|c| c.set(Some(recorder)));
    }

    pub struct PhaseScope {
        phase: &'static str,
        start: Instant,
    }

    impl Drop for PhaseScope {
        fn drop(&mut self) {
            let elapsed = self.start.elapsed().as_nanos();
            RECORDER.with(|c| {
                if let Some(recorder) = c.get() {
                    recorder(self.phase, elapsed);
                }
            });
        }
    }

    #[inline]
    pub fn scope(phase: &'static str) -> PhaseScope {
        PhaseScope {
            phase,
            start: Instant::now(),
        }
    }
}

#[cfg(not(feature = "debug-stats"))]
mod imp {
    pub type Recorder = fn(&'static str, u128);

    #[inline(always)]
    pub fn install_recorder(_recorder: Recorder) {}

    pub struct PhaseScope;

    #[inline(always)]
    pub fn scope(_phase: &'static str) -> PhaseScope {
        PhaseScope
    }
}

pub use imp::{install_recorder, scope, PhaseScope, Recorder};
