//! Cheap RAII phase timer that reports into framework-core's debug
//! `record_apply_phase` aggregator. Zero overhead when the
//! `debug-stats` feature is off — the struct vanishes and call sites
//! become a `let _ = ();` that the optimizer strips.
//!
//! Usage:
//!
//! ```ignore
//! let _t = crate::phase_timer::PhaseTimer::start("set_attribute");
//! element.set_attribute("class", &class_name).expect("set class");
//! // Timer fires on drop at end of scope.
//! ```
//!
//! We chose RAII over an `instrument!` macro that wraps a block
//! because the existing apply code branches and returns from inside
//! its conditional arms — RAII drops at scope exit regardless of
//! which arm ran.

#[cfg(feature = "debug-stats")]
pub(crate) struct PhaseTimer {
    phase: &'static str,
    start_us: u64,
}

#[cfg(feature = "debug-stats")]
impl PhaseTimer {
    pub(crate) fn start(phase: &'static str) -> Self {
        Self {
            phase,
            start_us: framework_core::debug::now_micros(),
        }
    }
}

#[cfg(feature = "debug-stats")]
impl Drop for PhaseTimer {
    fn drop(&mut self) {
        let now = framework_core::debug::now_micros();
        let dur = now.saturating_sub(self.start_us);
        framework_core::debug::record_apply_phase(self.phase, dur);
    }
}

// Zero-cost stub when `debug-stats` is off. The macro below expands to
// a `let _t = PhaseTimer::start(...)` that the optimizer fully elides.
#[cfg(not(feature = "debug-stats"))]
pub(crate) struct PhaseTimer;

#[cfg(not(feature = "debug-stats"))]
impl PhaseTimer {
    #[inline(always)]
    pub(crate) fn start(_phase: &'static str) -> Self {
        Self
    }
}
