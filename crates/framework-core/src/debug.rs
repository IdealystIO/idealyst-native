//! Compile-time gated render telemetry.
//!
//! This module exists only when the `debug-stats` Cargo feature is
//! enabled on `framework-core`. When off, the module is `#[cfg]`'d
//! out of the crate entirely — no atomic, no event log, no symbol
//! pollution. Same goes for the walker-side instrumentation in
//! `lib.rs` and the macro-emitted component enter/exit calls in
//! `framework-macros`.
//!
//! ## What gets recorded
//!
//! Each instrumented site pushes a `DebugEvent` into a thread-local
//! `Vec`. Events carry a `u64` microsecond timestamp derived from
//! the platform's monotonic clock (`performance.now()` on web,
//! `Instant::now()` on native).
//!
//! Component events come from `#[component]`'s macro-emitted
//! enter/exit calls. Build / backend / effect events come from
//! framework-core's walker — backends themselves are never touched,
//! the instrumentation wraps the walker's calls into the backend.
//!
//! ## Reading the log
//!
//! Authors call `take_events()` to drain the log (returns everything
//! recorded since last drain). For the common "per-component
//! summary" case, `component_summary(&events)` aggregates by name.
//! Anything more complex is the user's to compute from the raw
//! events.

use std::cell::RefCell;
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Tag for a primitive kind. Used in build/backend events to identify
/// what kind of node was being built. Mirrors `Primitive`'s variants
/// at a coarser granularity — we don't need full payload, just the
/// kind.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum PrimitiveKind {
    View,
    Text,
    Button,
    Image,
    TextInput,
    Toggle,
    ScrollView,
    Slider,
    WebView,
    Video,
    ActivityIndicator,
    Virtualizer,
    Graphics,
    Navigator,
    When,
    Switch,
    Link,
    Overlay,
}

/// One recorded telemetry event. `at_us` is monotonic microseconds
/// since program start (or since the platform's chosen epoch — the
/// absolute value doesn't matter, only deltas).
#[derive(Clone, Debug)]
pub enum DebugEvent {
    /// A `#[component]` function was entered. Emitted by the macro.
    ComponentEnter { name: &'static str, at_us: u64 },
    /// A `#[component]` function returned. Emitted by the macro.
    ComponentExit { name: &'static str, at_us: u64 },

    /// The framework's build walker started processing a primitive.
    BuildEnter { kind: PrimitiveKind, at_us: u64 },
    /// The build walker finished processing a primitive (after the
    /// backend create call and any subtree builds).
    BuildExit { kind: PrimitiveKind, at_us: u64 },

    /// Framework called the backend's `create_*` for a primitive.
    /// Wraps the backend call site in the walker; the backend
    /// itself doesn't know about the timing.
    BackendCreateEnter { kind: PrimitiveKind, at_us: u64 },
    BackendCreateExit { kind: PrimitiveKind, at_us: u64 },

    /// Framework's `attach_style` ran an apply-style effect cycle.
    /// Covers `resolve_style` + `backend.apply_style` (or
    /// `apply_styled_states`).
    ApplyStyleEnter { at_us: u64 },
    ApplyStyleExit { at_us: u64 },

    /// Style resolution hit the memoization cache.
    StyleCacheHit { at_us: u64 },
    /// Style resolution missed the cache and computed fresh rules.
    StyleCacheMiss { at_us: u64 },

    /// A reactive effect fired (initial run or signal change).
    EffectFired { at_us: u64 },

    /// FlatList virtualizer mounted an item at the given index.
    VirtualizerMount { index: usize, scope_id: u64, at_us: u64 },
    /// FlatList virtualizer released a previously-mounted item.
    VirtualizerRelease { scope_id: u64, at_us: u64 },
}

/// Per-component aggregate produced by `component_summary`.
#[derive(Default, Clone, Debug)]
pub struct ComponentSummary {
    /// Number of times this component was entered.
    pub call_count: u64,
    /// Sum of `(exit_at_us - enter_at_us)` for all matched
    /// enter/exit pairs. Inclusive — includes time spent in
    /// sub-components called during this one's body.
    pub total_inclusive_us: u64,
    /// Largest single inclusive duration observed.
    pub max_inclusive_us: u64,
}

/// Aggregate counters for the backend's apply-style sub-phases. Each
/// entry pairs a call count with total time spent. We use aggregated
/// counters rather than per-call events because the perf screen fires
/// 1000+ calls per theme toggle and that would flood the event log.
///
/// Backends report into these via [`record_apply_phase`]. Phase names
/// are backend-specific strings — the web backend reports things like
/// "set_attribute", "insert_rule", "key_hash". Phases that don't
/// apply on a given backend simply never get recorded.
#[derive(Default, Clone, Debug)]
pub struct PhaseCounter {
    pub call_count: u64,
    pub total_us: u64,
    pub max_us: u64,
}

// ---------------------------------------------------------------------------
// Public API — readers
// ---------------------------------------------------------------------------

thread_local! {
    static EVENT_LOG: RefCell<Vec<DebugEvent>> = const { RefCell::new(Vec::new()) };
    static EVENT_LIMIT: RefCell<Option<usize>> = const { RefCell::new(None) };
    static START_INSTANT: RefCell<Option<TimeOrigin>> = const { RefCell::new(None) };
    /// Aggregated phase counters. Keyed by `&'static str` phase name so
    /// backends can report into it cheaply with no allocation.
    static PHASE_COUNTERS: RefCell<HashMap<&'static str, PhaseCounter>> =
        RefCell::new(HashMap::new());
}

/// Drain the event log. Returns everything recorded since the last
/// drain (or since program start). Resets the log to empty.
pub fn take_events() -> Vec<DebugEvent> {
    EVENT_LOG.with(|log| std::mem::take(&mut *log.borrow_mut()))
}

/// Clear the event log without returning it. For when you want to
/// start a fresh measurement window.
pub fn clear_events() {
    EVENT_LOG.with(|log| log.borrow_mut().clear());
}

/// Cap the event log size. When more events are recorded than the
/// limit, the oldest are dropped (ring-buffer behavior). `None` =
/// unlimited (the default).
pub fn set_event_log_limit(limit: Option<usize>) {
    EVENT_LIMIT.with(|l| *l.borrow_mut() = limit);
}

/// Convenience: aggregate component enter/exit pairs into per-name
/// stats. Inclusive timing — nested sub-component time is counted
/// in the outer component's total.
///
/// Mismatched enter/exit pairs (orphan exits, or enters without a
/// matching exit) are silently skipped — they shouldn't occur in
/// normal use; if they do, it indicates a panic or stack imbalance.
pub fn component_summary(events: &[DebugEvent]) -> HashMap<&'static str, ComponentSummary> {
    let mut out: HashMap<&'static str, ComponentSummary> = HashMap::new();
    let mut stack: Vec<(&'static str, u64)> = Vec::new();
    for e in events {
        match e {
            DebugEvent::ComponentEnter { name, at_us } => {
                stack.push((name, *at_us));
            }
            DebugEvent::ComponentExit { name, at_us } => {
                if let Some(pos) = stack.iter().rposition(|(n, _)| *n == *name) {
                    let (_, start) = stack.remove(pos);
                    let dur = at_us.saturating_sub(start);
                    let entry = out.entry(name).or_default();
                    entry.call_count += 1;
                    entry.total_inclusive_us += dur;
                    if dur > entry.max_inclusive_us {
                        entry.max_inclusive_us = dur;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Recording — called by walker instrumentation + macro-emitted code
// ---------------------------------------------------------------------------

/// Current monotonic microseconds. Used by every record_* function
/// AND by the walker's two-sided instrumentation (caller reads
/// `now_micros()` before/after the wrapped call).
pub fn now_micros() -> u64 {
    let origin = START_INSTANT.with(|o| {
        let mut s = o.borrow_mut();
        if s.is_none() {
            *s = Some(TimeOrigin::capture());
        }
        s.as_ref().unwrap().clone()
    });
    origin.elapsed_micros()
}

/// Push an event into the log, honoring the configured limit.
fn push(event: DebugEvent) {
    EVENT_LOG.with(|log| {
        let mut v = log.borrow_mut();
        let limit = EVENT_LIMIT.with(|l| *l.borrow());
        if let Some(max) = limit {
            if v.len() >= max {
                v.remove(0);
            }
        }
        v.push(event);
    });
}

pub fn record_component_enter(name: &'static str) {
    push(DebugEvent::ComponentEnter { name, at_us: now_micros() });
}

pub fn record_component_exit(name: &'static str) {
    push(DebugEvent::ComponentExit { name, at_us: now_micros() });
}

pub fn record_build_enter(kind: PrimitiveKind) {
    push(DebugEvent::BuildEnter { kind, at_us: now_micros() });
}

pub fn record_build_exit(kind: PrimitiveKind) {
    push(DebugEvent::BuildExit { kind, at_us: now_micros() });
}

pub fn record_backend_create_enter(kind: PrimitiveKind) {
    push(DebugEvent::BackendCreateEnter { kind, at_us: now_micros() });
}

pub fn record_backend_create_exit(kind: PrimitiveKind) {
    push(DebugEvent::BackendCreateExit { kind, at_us: now_micros() });
}

pub fn record_apply_style_enter() {
    push(DebugEvent::ApplyStyleEnter { at_us: now_micros() });
}

pub fn record_apply_style_exit() {
    push(DebugEvent::ApplyStyleExit { at_us: now_micros() });
}

pub fn record_style_cache_hit() {
    push(DebugEvent::StyleCacheHit { at_us: now_micros() });
}

pub fn record_style_cache_miss() {
    push(DebugEvent::StyleCacheMiss { at_us: now_micros() });
}

pub fn record_effect_fired() {
    push(DebugEvent::EffectFired { at_us: now_micros() });
}

pub fn record_virtualizer_mount(index: usize, scope_id: u64) {
    push(DebugEvent::VirtualizerMount { index, scope_id, at_us: now_micros() });
}

pub fn record_virtualizer_release(scope_id: u64) {
    push(DebugEvent::VirtualizerRelease { scope_id, at_us: now_micros() });
}

/// Record one phase observation: `phase` ran for `duration_us`. Backends
/// call this from inside `apply_style` / `apply_styled_states` with
/// fine-grained labels (`"set_attribute"`, `"insert_rule"`,
/// `"key_hash"`, etc.) so we can see which sub-step dominates a
/// theme-toggle storm.
pub fn record_apply_phase(phase: &'static str, duration_us: u64) {
    PHASE_COUNTERS.with(|c| {
        let mut c = c.borrow_mut();
        let entry = c.entry(phase).or_default();
        entry.call_count += 1;
        entry.total_us += duration_us;
        if duration_us > entry.max_us {
            entry.max_us = duration_us;
        }
    });
}

/// Drain the phase counters. Returns a snapshot and resets all
/// counters to zero. Pair with `take_events` when collecting both
/// per-component and per-phase numbers for one measurement window.
pub fn take_phase_counters() -> HashMap<&'static str, PhaseCounter> {
    PHASE_COUNTERS.with(|c| std::mem::take(&mut *c.borrow_mut()))
}

/// Clear the phase counters without returning them.
pub fn clear_phase_counters() {
    PHASE_COUNTERS.with(|c| c.borrow_mut().clear());
}

// ---------------------------------------------------------------------------
// Time source — platform-dependent
// ---------------------------------------------------------------------------

/// Platform-agnostic time origin. On web, uses `performance.now()`.
/// On native, uses `Instant::now()`. Captured once on first use and
/// reused; `elapsed_micros()` returns micros since capture.
#[derive(Clone)]
enum TimeOrigin {
    #[cfg(target_arch = "wasm32")]
    WebPerf {
        epoch: f64, // performance.now() at capture
    },
    #[cfg(not(target_arch = "wasm32"))]
    Native {
        epoch: std::time::Instant,
    },
}

impl TimeOrigin {
    fn capture() -> Self {
        #[cfg(target_arch = "wasm32")]
        {
            // Use js_sys to access performance.now(). If unavailable
            // (running outside a browser), fall back to 0 — timing
            // will all read 0, but the framework won't crash.
            let epoch = web_performance_now().unwrap_or(0.0);
            return TimeOrigin::WebPerf { epoch };
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            return TimeOrigin::Native { epoch: std::time::Instant::now() };
        }
    }

    fn elapsed_micros(&self) -> u64 {
        match self {
            #[cfg(target_arch = "wasm32")]
            TimeOrigin::WebPerf { epoch } => {
                let now = web_performance_now().unwrap_or(*epoch);
                ((now - epoch) * 1000.0).max(0.0) as u64
            }
            #[cfg(not(target_arch = "wasm32"))]
            TimeOrigin::Native { epoch } => epoch.elapsed().as_micros() as u64,
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn web_performance_now() -> Option<f64> {
    // Access window.performance.now() via raw js_sys reflection so
    // we don't take a hard dep on web-sys. We already depend on
    // js_sys transitively for the virtualizer, but framework-core
    // doesn't currently — so keep this self-contained via a JS eval.
    use wasm_bindgen::prelude::*;
    let window = js_sys::global();
    let perf = js_sys::Reflect::get(&window, &JsValue::from_str("performance")).ok()?;
    if perf.is_undefined() || perf.is_null() {
        return None;
    }
    let now_fn = js_sys::Reflect::get(&perf, &JsValue::from_str("now")).ok()?;
    let now_fn: js_sys::Function = now_fn.dyn_into().ok()?;
    let ret = now_fn.call0(&perf).ok()?;
    ret.as_f64()
}
