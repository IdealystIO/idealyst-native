//! Web haptics — `navigator.vibrate(...)`, through a web-glue binding
//! declared here (own-web-bindings phase 3).
//!
//! The Vibration API is the only web haptics primitive, and it's
//! duration-only: there is **no** impact-style or notification-type concept,
//! just "vibrate for N ms" (or a pattern of on/off ms). So every effect here
//! is **approximated** as a short ms pattern (documented below). It's also
//! widely unsupported on desktop and on iOS Safari — the call is a safe no-op
//! there, which matches this SDK's best-effort contract.
//!
//! Patterns (ms):
//! - `impact(style)` → a single pulse whose length tracks the style's weight
//!   (light → short, heavy → long). There's no amplitude control on web, so
//!   weight is conveyed purely by duration.
//! - `notify(feedback)` → a short multi-pulse `[on, off, on, …]` pattern so
//!   success / warning / error feel distinguishable.
//! - `selection()` → a single very short pulse (a "tick").
//!
//! Runnable on web where the browser supports the Vibration API (primarily
//! Android Chrome); elsewhere `navigator.vibrate` is absent or returns
//! `false` and the call does nothing.

use crate::{ImpactStyle, NotificationFeedback};

// Single-pulse durations (ms) for impact weights.
const MS_LIGHT: u32 = 10;
const MS_MEDIUM: u32 = 20;
const MS_HEAVY: u32 = 40;
const MS_SOFT: u32 = 12;
const MS_RIGID: u32 = 16;
const MS_SELECTION: u32 = 5;

fn impact_ms(style: ImpactStyle) -> u32 {
    match style {
        ImpactStyle::Light => MS_LIGHT,
        ImpactStyle::Medium => MS_MEDIUM,
        ImpactStyle::Heavy => MS_HEAVY,
        ImpactStyle::Soft => MS_SOFT,
        ImpactStyle::Rigid => MS_RIGID,
    }
}

web_glue::import! {
    // 1 in a window context (which always has a `navigator`).
    fn js_has_navigator() -> u32 = "() => typeof window === 'undefined' ? 0 : 1";
    // `navigator.vibrate(...)` over `n` u32 ms values at `ptr`: the array
    // (pattern) overload when `arr` is 1, else the single-duration one
    // with the first value. The values are copied out of
    // wasm memory before the call, never retained. A browser without the
    // Vibration API (Safari, Firefox desktop) has no `vibrate` at all, so
    // calling it would throw a TypeError through the wasm frames — the
    // guard is what makes those browsers the no-op this SDK promises
    // (regression: `tests/web_vibrate.rs`). Its boolean
    // result is dropped: `false` just means the device/browser won't
    // vibrate, which is the best-effort no-op.
    fn js_vibrate(ptr: usize, n: usize, arr: u32) =
        "(p, n, arr) => { if (typeof window === 'undefined' || typeof navigator.vibrate !== 'function') return; \
           const w = G.u32(); const b = (p >>> 0) >>> 2; \
           navigator.vibrate(arr ? Array.from(w.subarray(b, b + (n >>> 0))) : w[b]); }";
}

/// Whether a `Navigator` exists — `false` outside a window context (e.g. a
/// worker), in which case every call is a no-op.
fn has_navigator() -> bool {
    unsafe { js_has_navigator() != 0 }
}

/// `navigator.vibrate(ms)` — a single pulse.
fn vibrate_once(ms: u32) {
    let one = [ms];
    unsafe { js_vibrate(one.as_ptr() as usize, 1, 0) }
}

/// `navigator.vibrate([on, off, on, …])` — a multi-pulse pattern.
fn vibrate_pattern(pattern: &[u32]) {
    unsafe { js_vibrate(pattern.as_ptr() as usize, pattern.len(), 1) }
}

pub(crate) fn impact(style: ImpactStyle) {
    vibrate_once(impact_ms(style));
}

pub(crate) fn notify(feedback: NotificationFeedback) {
    // Distinct on/off patterns so the three outcomes feel different.
    match feedback {
        NotificationFeedback::Success => vibrate_pattern(&[15]),
        NotificationFeedback::Warning => vibrate_pattern(&[20, 60, 20]),
        NotificationFeedback::Error => vibrate_pattern(&[40, 50, 40, 50, 40]),
    }
}

pub(crate) fn selection() {
    vibrate_once(MS_SELECTION);
}

pub(crate) fn is_supported() -> bool {
    // Honest-but-coarse: we report "supported" when a `Navigator` exists.
    // A precise feature-detect (`"vibrate" in navigator`) needs reflection
    // that buys little here — on a browser without the Vibration API the
    // `vibrate_*` calls return `false` and do nothing, so the effect functions
    // stay correct no-ops regardless of what this predicate says.
    has_navigator()
}
