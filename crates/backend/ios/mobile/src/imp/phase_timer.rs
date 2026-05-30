//! Cheap RAII phase timer that reports into runtime-core's debug
//! `record_apply_phase` aggregator. Mirrors the web backend's
//! `PhaseTimer` shape (see `crates/backend/web/src/phase_timer.rs`)
//! so the same opt-in workflow applies to both:
//!
//!   * `debug-stats` Cargo feature OFF (default): the struct compiles
//!     away and call sites become a no-op `let _ = ();` that the
//!     optimizer fully strips.
//!   * `debug-stats` ON: each guard's `Drop` calls
//!     `runtime_core::debug::record_apply_phase` with the elapsed
//!     duration; consumers drain via
//!     `runtime_core::debug::take_phase_counters()` (and
//!     [`take_and_dump`] gives a quick stderr summary).
//!
//! Usage:
//!
//! ```ignore
//! let _t = crate::imp::phase_timer::PhaseTimer::start("apply_frames_loop");
//! // ... work ...
//! // timer fires on scope exit
//! ```
//!
//! Phase names are aggregation keys — keep them stable and
//! descriptive. See the existing names registered in `mod.rs` /
//! `backend-ios-core::style` for the convention.

#[cfg(feature = "debug-stats")]
use runtime_core::debug;

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
            start_us: debug::now_micros(),
        }
    }
}

#[cfg(feature = "debug-stats")]
impl Drop for PhaseTimer {
    fn drop(&mut self) {
        let now = debug::now_micros();
        let dur = now.saturating_sub(self.start_us);
        debug::record_apply_phase(self.phase, dur);
    }
}

// Stub when the feature is off. Same shape, zero work; the
// optimizer strips the start/drop call pair to nothing.
#[cfg(not(feature = "debug-stats"))]
pub(crate) struct PhaseTimer;

#[cfg(not(feature = "debug-stats"))]
impl PhaseTimer {
    #[inline(always)]
    pub(crate) fn start(_phase: &'static str) -> Self {
        Self
    }
}

/// Wire `backend-ios-core::phase_record::scope` to push into the
/// shared `runtime_core::debug::record_apply_phase` aggregator so
/// timings from both crates surface in the same counter map. Called
/// once during `IosBackend::new`. No-op when `debug-stats` is off
/// (`phase_record::scope` is itself a zero-cost guard at that point).
#[cfg(feature = "debug-stats")]
pub(crate) fn install_core_bridge() {
    backend_ios_core::phase_record::install_recorder(|phase, ns| {
        // `record_apply_phase` takes microseconds; convert from the
        // `Instant::elapsed().as_nanos()` the core indirection uses.
        runtime_core::debug::record_apply_phase(phase, (ns / 1000) as u64);
    });
}

#[cfg(not(feature = "debug-stats"))]
pub(crate) fn install_core_bridge() {}

/// Drain the runtime-core phase counters and print them to stderr in
/// a sorted-by-total-time table. Call from a meaningful boundary
/// (end of a layout pass, after a navigation, etc.) to see what's
/// dominating that window. No-op when `debug-stats` is off (counters
/// are empty by construction).
#[cfg(feature = "debug-stats")]
pub(crate) fn take_and_dump(label: &str) {
    let counters = runtime_core::debug::take_phase_counters();
    if counters.is_empty() {
        return;
    }
    let mut snapshot: Vec<_> = counters.into_iter().collect();
    snapshot.sort_by_key(|(_, p)| std::cmp::Reverse(p.total_us));
    backend_ios_core::ios_log(&format!("[phase-timer] {} ───────────────", label));
    for (phase, counter) in snapshot {
        let total_ms = (counter.total_us as f64) / 1000.0;
        let avg_us = if counter.call_count == 0 {
            0.0
        } else {
            (counter.total_us as f64) / (counter.call_count as f64)
        };
        let max_us = counter.max_us as f64;
        backend_ios_core::ios_log(&format!(
            "[phase-timer]   {phase:<32} {count:>6}× avg {avg:>7.1}us  max {max:>7.1}us  total {total:>7.2}ms",
            phase = phase,
            count = counter.call_count,
            avg = avg_us,
            max = max_us,
            total = total_ms,
        ));
    }
}

#[cfg(not(feature = "debug-stats"))]
#[inline(always)]
pub(crate) fn take_and_dump(_label: &str) {}
