//! RAII phase timer for the Android backend's apply-style and
//! layout-pass hot paths. Mirrors `backend-web/src/phase_timer.rs` —
//! same shape, same gating, same drain-and-clear API.
//!
//! Off-feature behavior: the struct is a zero-sized type, `start` is
//! `#[inline(always)]` and returns `Self`, and the optimizer strips
//! the `let _t = …` to nothing at every call site. So sprinkling
//! `PhaseTimer::start("…")` into per-view loops is free in release
//! builds without the `debug-stats` feature.
//!
//! On-feature behavior: `start` snapshots `now_micros()` into the
//! returned guard; `Drop` records `(phase, duration_us)` into
//! `runtime_core::debug`'s thread-local aggregator. The drain helper
//! reads + clears that table and formats it for one log line. The
//! framework already owns the aggregator (per-call count + total_us +
//! max_us per phase name), so we just plug into it.

/// Real timer when `debug-stats` is on.
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
            start_us: runtime_core::debug::now_micros(),
        }
    }
}

#[cfg(feature = "debug-stats")]
impl Drop for PhaseTimer {
    fn drop(&mut self) {
        let now = runtime_core::debug::now_micros();
        let dur = now.saturating_sub(self.start_us);
        runtime_core::debug::record_apply_phase(self.phase, dur);
    }
}

/// Zero-cost stub when the feature is off. `let _t = PhaseTimer::start(…)`
/// vanishes — useful for sprinkling timers into loops without
/// instrumenting-by-feature-gate at every call site.
#[cfg(not(feature = "debug-stats"))]
pub(crate) struct PhaseTimer;

#[cfg(not(feature = "debug-stats"))]
impl PhaseTimer {
    #[inline(always)]
    pub(crate) fn start(_phase: &'static str) -> Self {
        Self
    }
}

/// Drain the framework's phase-counter table, log it sorted by
/// total time, and clear. Called from `run_layout_pass` at the end of
/// each pass so the next pass starts fresh.
///
/// No-op when `debug-stats` is off (the framework's aggregator is
/// itself a no-op then, so a drain would always be empty).
#[cfg(feature = "debug-stats")]
pub(crate) fn drain_and_log_phase_counters() {
    let counters = runtime_core::debug::take_phase_counters();
    if counters.is_empty() {
        return;
    }
    // Sort by total_us descending — biggest contributors first.
    // `&'static str` is `Ord`, so name ties stay stable.
    let mut entries: Vec<_> = counters.into_iter().collect();
    entries.sort_by(|a, b| b.1.total_us.cmp(&a.1.total_us));
    let mut parts: Vec<String> = Vec::with_capacity(entries.len());
    for (name, c) in &entries {
        parts.push(format!(
            "{name}: {count}x total={total:.1}ms avg={avg:.0}us max={max:.0}us",
            name = name,
            count = c.call_count,
            total = c.total_us as f64 / 1000.0,
            avg = if c.call_count > 0 {
                c.total_us as f64 / c.call_count as f64
            } else {
                0.0
            },
            max = c.max_us as f64,
        ));
    }
    log::info!("[phase-stats] {}", parts.join(" | "));
}

#[cfg(not(feature = "debug-stats"))]
pub(crate) fn drain_and_log_phase_counters() {}
