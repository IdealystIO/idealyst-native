//! Compile-time gated render telemetry.
//!
//! This module exists only when the `debug-stats` Cargo feature is
//! enabled on `runtime-core`. When off, the module is `#[cfg]`'d
//! out of the crate entirely — no atomic, no event log, no symbol
//! pollution. Same goes for the walker-side instrumentation in
//! `lib.rs` and the macro-emitted component enter/exit calls in
//! `runtime-macros`.
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
//! runtime-core's walker — backends themselves are never touched,
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
use std::panic::Location;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Tag for a primitive kind. Used in build/backend events to identify
/// what kind of node was being built. Mirrors `Element`'s variants
/// at a coarser granularity — we don't need full payload, just the
/// kind.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum PrimitiveKind {
    View,
    Text,
    Button,
    Pressable,
    Icon,
    Image,
    TextInput,
    TextArea,
    Toggle,
    ScrollView,
    Slider,
    ActivityIndicator,
    Virtualizer,
    Graphics,
    Navigator,
    When,
    Switch,
    Each,
    Link,
    Portal,
    External,
    Presence,
    Lazy,
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

    /// A reactive flush (transaction) began: one or more signal writes
    /// woke a set of subscriber effects. `trigger_signals` are the raw
    /// `SignalId`s that dirtied this window (one for an immediate write,
    /// several for a `batch`); `effect_count` is how many effects will run.
    /// Nestable — an effect that writes a signal opens a child transaction.
    TxnEnter {
        trigger_signals: Vec<u64>,
        effect_count: usize,
        at_us: u64,
    },
    /// The matching close of a [`TxnEnter`].
    TxnExit { at_us: u64 },

    /// One reactive effect ran to completion during a flush. `us` is the
    /// wall time of the effect's closure body only (compute + backend
    /// mutation calls) — NOT the layout/paint commit, which is separate.
    EffectRun { effect_id: u64, us: u64, at_us: u64 },

    /// The layout/paint commit for a flush: the backend's reactive-idle
    /// hook ran (a synchronous layout pass) when the outermost mutation
    /// window closed. `us` is its wall time. Emitted inside the enclosing
    /// transaction, after its effects — this is usually where a "long
    /// render" actually spends its time.
    Commit { us: u64, at_us: u64 },

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
    static START_MICROS: RefCell<Option<u64>> = const { RefCell::new(None) };
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
///
/// Anchored to the first call so deltas are "since debug-stats was
/// first observed." Reads route through `runtime_core::time` —
/// the active backend's `TimeSource` if installed, else the native
/// `Instant` fallback (or `0` on wasm32 if no backend installed).
pub fn now_micros() -> u64 {
    let start = START_MICROS.with(|s| {
        let mut s = s.borrow_mut();
        *s.get_or_insert_with(crate::time::now_micros)
    });
    crate::time::now_micros().saturating_sub(start)
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

/// Open a reactive transaction (flush window). Called at the two fan-out
/// roots (`fan_out_now`, the `batch` flush) with the triggering signal ids
/// and the number of effects about to run.
pub fn record_txn_enter(trigger_signals: Vec<u64>, effect_count: usize) {
    push(DebugEvent::TxnEnter {
        trigger_signals,
        effect_count,
        at_us: now_micros(),
    });
}

/// Close the transaction opened by [`record_txn_enter`].
pub fn record_txn_exit() {
    push(DebugEvent::TxnExit { at_us: now_micros() });
}

/// Record one effect's closure-body wall time (`us`), keyed by its
/// `EffectId`. Emitted by `run_effect` around the closure invocation.
pub fn record_effect_run(effect_id: u32, us: u64) {
    push(DebugEvent::EffectRun {
        effect_id: effect_id as u64,
        us,
        at_us: now_micros(),
    });
}

/// Record the layout/paint commit wall time (`us`) for the just-closed
/// outermost flush. Emitted from `run_effects` around the idle-hook drop.
pub fn record_commit(us: u64) {
    push(DebugEvent::Commit {
        us,
        at_us: now_micros(),
    });
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
// Reactive identity — name the opaque signal/effect ids
// ---------------------------------------------------------------------------
//
// `SignalId`/`EffectId` are opaque `u32`s in the arena. A profile keyed by
// "effect #4823" is useless, so on creation we stash the author-code
// source location (via `#[track_caller]`, resolved at compile time — the
// `&'static Location` costs nothing to *capture*, only to store here) plus,
// for effects, the innermost `#[component]` that was building at the time and
// an optional binding label the walker attaches ("text#<id>" / "style#<id>").
//
// Keyed by raw `u32` id in this module's own thread-locals rather than as
// parallel `Arena` vectors: keeps the identity concern out of the reactive
// hot-path structs and mirrors how the rest of `debug` owns its state. Slots
// are effectively never reused for the life of a session (see
// `arena_stats`); if a generational recycle ever does reuse an id, `insert`
// overwrites with the newer creator, which is the correct label anyway.

/// Creation-site identity for one effect.
#[derive(Clone, Debug)]
pub struct EffectDebug {
    /// Author-code call site of the effect's constructor (`effect! { … }`,
    /// a reactive binding, `memo`, …).
    pub location: &'static Location<'static>,
    /// Innermost `#[component]` body executing when the effect was created,
    /// if any. `None` for effects made outside a component build (app init,
    /// an event handler, a service install).
    pub component: Option<&'static str>,
    /// Binding label the walker attaches for node-mutating effects
    /// ("text#<id>", "style#<id>"), naming the *node* the effect drives.
    pub label: Option<String>,
}

thread_local! {
    static SIGNAL_DEBUG: RefCell<HashMap<u32, &'static Location<'static>>> =
        RefCell::new(HashMap::new());
    static EFFECT_DEBUG: RefCell<HashMap<u32, EffectDebug>> =
        RefCell::new(HashMap::new());
}

/// Record a signal's creation site. Called from `Signal::new`.
pub fn record_signal_created(id: u32, location: &'static Location<'static>) {
    SIGNAL_DEBUG.with(|m| {
        m.borrow_mut().insert(id, location);
    });
}

/// Record an effect's creation site + owning component. Called from
/// `Effect::create`.
pub fn record_effect_created(
    id: u32,
    location: &'static Location<'static>,
    component: Option<&'static str>,
) {
    EFFECT_DEBUG.with(|m| {
        m.borrow_mut().insert(
            id,
            EffectDebug {
                location,
                component,
                label: None,
            },
        );
    });
}

/// Attach a binding label to an already-created effect (e.g. the walker
/// naming the node a text/style effect drives). No-op if the effect id was
/// never recorded.
pub fn label_effect(id: u32, label: &str) {
    EFFECT_DEBUG.with(|m| {
        if let Some(e) = m.borrow_mut().get_mut(&id) {
            e.label = Some(label.to_string());
        }
    });
}

/// Human-readable label for a signal id: its `file:line` creation site, or
/// `signal#<id>` if untracked.
pub fn signal_label(id: u32) -> String {
    SIGNAL_DEBUG.with(|m| {
        m.borrow()
            .get(&id)
            .map(|loc| format!("{}:{}", loc.file(), loc.line()))
            .unwrap_or_else(|| format!("signal#{id}"))
    })
}

/// Human-readable label for an effect id: its binding label or `file:line`,
/// suffixed with the owning component in brackets when known. `effect#<id>`
/// if untracked.
pub fn effect_label(id: u32) -> String {
    EFFECT_DEBUG.with(|m| {
        m.borrow()
            .get(&id)
            .map(|e| {
                let base = e
                    .label
                    .clone()
                    .unwrap_or_else(|| format!("{}:{}", e.location.file(), e.location.line()));
                match e.component {
                    Some(c) => format!("{base} [{c}]"),
                    None => base,
                }
            })
            .unwrap_or_else(|| format!("effect#{id}"))
    })
}

/// Drop all recorded signal/effect identity. Independent of `clear_events`
/// because identity must outlive event drains (a drained event still
/// references ids resolved later). For a fresh profiling session.
pub fn clear_reactive_debug() {
    SIGNAL_DEBUG.with(|m| m.borrow_mut().clear());
    EFFECT_DEBUG.with(|m| m.borrow_mut().clear());
}

// ---------------------------------------------------------------------------
// Reactive-profile reporting — fold the event stream into per-flush records
// ---------------------------------------------------------------------------

/// One reactive flush, folded from the event stream. The direct answer to
/// "what did this signal write cost, and where did the time go?"
#[derive(Clone, Debug)]
pub struct TxnRecord {
    /// Human-readable creation sites of the signal(s) that triggered this
    /// flush (usually one; several for a `batch`).
    pub trigger_labels: Vec<String>,
    /// Number of effects the flush woke.
    pub effect_count: usize,
    /// Per-effect `(label, us)` — the closure-body cost of each effect run,
    /// in run order. Labels resolve via the Phase-1 identity tables.
    pub effects: Vec<(String, u64)>,
    /// Sum of `effects` — total time in effect closures.
    pub effect_us: u64,
    /// Layout/paint commit time attributed to this flush.
    pub commit_us: u64,
    /// Wall time from `TxnEnter` to `TxnExit`.
    pub total_us: u64,
}

struct PartialTxn {
    trigger_labels: Vec<String>,
    effect_count: usize,
    effects: Vec<(String, u64)>,
    commit_us: u64,
    start_us: u64,
}

/// Fold a drained event stream into per-transaction records, resolving
/// signal/effect ids to labels. Sorted by `total_us` descending — the
/// slowest flush first.
///
/// Nested flushes (an effect that writes a signal) are folded to the
/// INNERMOST open transaction, so each appears as its own record. An outer
/// effect's `us` includes the time its nested flush took, so outer
/// `effect_us` and the nested record's cost overlap — read the nested record
/// for the finer attribution.
pub fn txn_report(events: &[DebugEvent]) -> Vec<TxnRecord> {
    let mut stack: Vec<PartialTxn> = Vec::new();
    let mut out: Vec<TxnRecord> = Vec::new();
    for e in events {
        match e {
            DebugEvent::TxnEnter {
                trigger_signals,
                effect_count,
                at_us,
            } => {
                stack.push(PartialTxn {
                    trigger_labels: trigger_signals
                        .iter()
                        .map(|s| signal_label(*s as u32))
                        .collect(),
                    effect_count: *effect_count,
                    effects: Vec::new(),
                    commit_us: 0,
                    start_us: *at_us,
                });
            }
            DebugEvent::EffectRun { effect_id, us, .. } => {
                if let Some(top) = stack.last_mut() {
                    top.effects.push((effect_label(*effect_id as u32), *us));
                }
            }
            DebugEvent::Commit { us, .. } => {
                if let Some(top) = stack.last_mut() {
                    top.commit_us += *us;
                }
            }
            DebugEvent::TxnExit { at_us } => {
                if let Some(p) = stack.pop() {
                    let effect_us = p.effects.iter().map(|(_, u)| *u).sum();
                    out.push(TxnRecord {
                        trigger_labels: p.trigger_labels,
                        effect_count: p.effect_count,
                        effects: p.effects,
                        effect_us,
                        commit_us: p.commit_us,
                        total_us: at_us.saturating_sub(p.start_us),
                    });
                }
            }
            _ => {}
        }
    }
    out.sort_by(|a, b| b.total_us.cmp(&a.total_us));
    out
}

/// Aggregate transactions by triggering signal — the "which signal causes
/// long renders" view. Returns `(signal_label, total_us, txn_count)` sorted
/// by `total_us` descending, where `total_us` sums `effect_us + commit_us`
/// across every flush that signal triggered.
///
/// A multi-trigger `batch` attributes its full cost to each of its trigger
/// signals (batches usually have a single trigger, so this rarely
/// over-counts).
pub fn slow_signals(events: &[DebugEvent]) -> Vec<(String, u64, u64)> {
    let mut agg: HashMap<String, (u64, u64)> = HashMap::new();
    for txn in txn_report(events) {
        let cost = txn.effect_us + txn.commit_us;
        for label in &txn.trigger_labels {
            let entry = agg.entry(label.clone()).or_default();
            entry.0 += cost;
            entry.1 += 1;
        }
    }
    let mut v: Vec<(String, u64, u64)> =
        agg.into_iter().map(|(k, (t, c))| (k, t, c)).collect();
    v.sort_by(|a, b| b.1.cmp(&a.1));
    v
}

/// Render a human-readable reactive profile from a drained event stream:
/// the top signals by total cost, then the slowest transactions with a
/// per-effect breakdown. `top_txns` caps the transaction list. Shared by
/// every surface (native log dump, wasm export, inspector panel) so they
/// present the same view. Returns an empty string when there are no flushes.
pub fn format_reactive_profile(events: &[DebugEvent], top_txns: usize) -> String {
    use std::fmt::Write as _;

    let txns = txn_report(events);
    if txns.is_empty() {
        return String::new();
    }
    let signals = slow_signals(events);

    let mut out = String::new();
    let _ = writeln!(out, "== reactive profile ==");
    let _ = writeln!(out, "slow signals (Σ effect+commit µs, txn count):");
    for (label, total_us, count) in signals.iter().take(top_txns.max(1)) {
        let _ = writeln!(out, "  {total_us:>8}µs  ×{count:<4} {label}");
    }
    let _ = writeln!(out, "slowest transactions (by wall time):");
    for (i, t) in txns.iter().take(top_txns).enumerate() {
        let triggers = t.trigger_labels.join(", ");
        let _ = writeln!(
            out,
            "  [{n:>2}] {total:>8}µs  triggers={triggers}  effects={ec} (Σ {eus}µs)  commit={cus}µs",
            n = i + 1,
            total = t.total_us,
            ec = t.effect_count,
            eus = t.effect_us,
            cus = t.commit_us,
        );
        // Show the heaviest few effects in this transaction.
        let mut ranked: Vec<&(String, u64)> = t.effects.iter().collect();
        ranked.sort_by(|a, b| b.1.cmp(&a.1));
        for (label, us) in ranked.iter().take(5) {
            let _ = writeln!(out, "         {us:>8}µs  {label}");
        }
    }
    out
}

#[cfg(test)]
mod identity_tests {
    use super::*;

    #[test]
    fn signal_and_effect_labels_resolve() {
        clear_reactive_debug();

        // Signal created here — `signal()`/`Signal::new` are `#[track_caller]`,
        // so the recorded location is THIS file.
        let s = crate::signal(0i32);
        let sid = s.id() as u32;
        let slabel = signal_label(sid);
        assert!(
            slabel.contains("debug.rs"),
            "signal label should carry its author-code creation site, got {slabel:?}"
        );

        // Effect created inside a `#[component]` build → component attribution.
        let probe = crate::reactive::__component_build_probe("FooComponent");
        let e = crate::reactive::Effect::new(move || {
            let _ = s.get();
        });
        drop(probe);
        let elabel = effect_label(e.raw_id());
        assert!(
            elabel.contains("FooComponent"),
            "effect label should name the owning component, got {elabel:?}"
        );

        // An explicit binding label overrides the file:line base.
        label_effect(e.raw_id(), "text#42");
        let elabel2 = effect_label(e.raw_id());
        assert!(
            elabel2.starts_with("text#42"),
            "binding label should take precedence, got {elabel2:?}"
        );

        // Unknown ids fall back to a stable placeholder.
        assert_eq!(signal_label(u32::MAX), format!("signal#{}", u32::MAX));
        assert_eq!(effect_label(u32::MAX), format!("effect#{}", u32::MAX));
    }
}

#[cfg(test)]
mod txn_tests {
    use super::*;

    #[test]
    fn flush_folds_into_a_transaction_attributed_to_the_signal() {
        clear_reactive_debug();

        // One signal, three effects subscribing to it (inside a component so
        // the effects carry component attribution too).
        let probe = crate::reactive::__component_build_probe("Widget");
        let s = crate::signal(0i32);
        let effects: Vec<_> = (0..3)
            .map(|_| {
                crate::reactive::Effect::new(move || {
                    let _ = s.get();
                })
            })
            .collect();
        drop(probe);

        // Drop the initial-run events; profile only the write's flush.
        clear_events();
        s.set(1);
        let events = take_events();

        let report = txn_report(&events);
        assert_eq!(report.len(), 1, "one write → one transaction, got {report:?}");
        let txn = &report[0];
        assert_eq!(txn.effect_count, 3, "all three subscribers woke");
        assert_eq!(txn.effects.len(), 3, "three EffectRun events folded in");
        assert_eq!(txn.trigger_labels.len(), 1);
        assert!(
            txn.trigger_labels[0].contains("debug.rs"),
            "transaction attributed to the signal's creation site, got {:?}",
            txn.trigger_labels[0]
        );
        // Commit event is present (0µs here — no backend idle hook installed).
        // The field exists and folded without panicking; that's the contract.
        let _ = txn.commit_us;

        // slow_signals attributes the flush cost to that one signal.
        let slow = slow_signals(&events);
        assert_eq!(slow.len(), 1, "one triggering signal");
        assert!(slow[0].0.contains("debug.rs"));
        assert_eq!(slow[0].2, 1, "triggered exactly one transaction");

        // The shared formatter renders a non-empty, signal-attributed report.
        let text = format_reactive_profile(&events, 10);
        assert!(text.contains("reactive profile"));
        assert!(text.contains("debug.rs"), "report names the trigger, got:\n{text}");

        drop(effects);
    }
}

