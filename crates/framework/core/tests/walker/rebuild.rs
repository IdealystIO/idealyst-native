//! Rebuild-shape tests.
//!
//! Reproduces the variant pattern the bench's "rebuild" suite drives:
//! a reactive `match` on a mode signal, with the active arm reading
//! a row-count signal and emitting a `Primitive::Repeat { count, ... }`.
//! Set the count signal to a new value, drain microtasks, assert
//! the backend actually saw the new row count.
//!
//! Catches:
//! - Switch arm-rebuild not actually re-running on inner-signal change
//! - Repeat count getting clamped / truncated somewhere
//! - clear_children skipping rows from the previous render

use std::rc::Rc;

use framework_core::{signal, switch, text, view, IntoPrimitive, Primitive, Signal};

use crate::common::{Event, TestRuntime};

/// Helper: count CreateText events in a slice.
fn count_create_text(events: &[Event]) -> usize {
    events
        .iter()
        .filter(|e| matches!(e, Event::CreateText { .. }))
        .count()
}

/// Helper: count CreateView events in a slice.
fn count_create_view(events: &[Event]) -> usize {
    events
        .iter()
        .filter(|e| matches!(e, Event::CreateView))
        .count()
}

/// Build the same tree shape the bench variant uses for its row-list:
/// a switch over a mode signal whose active arm reads a count signal
/// and emits a `Primitive::Repeat` of view+text rows.
fn rebuild_tree(mode: Signal<u32>, count: Signal<usize>) -> Primitive {
    switch(
        move || mode.get(),
        move |m| match m {
            0 => {
                let n = count.get();
                let row_builder: Box<dyn Fn(usize) -> Primitive> =
                    Box::new(|i| {
                        view(vec![text(format!("Row #{}", i)).into_primitive()])
                            .into_primitive()
                    });
                view(vec![Primitive::Repeat {
                    count: n,
                    row_builder,
                }])
                .into_primitive()
            }
            _ => view(Vec::new()).into_primitive(),
        },
    )
}

/// REGRESSION + CONTRACT TEST.
///
/// The bench's rebuild suite alternates two row counts via
/// `set_rows(n)`. The variant USED to skip writing the `mode`
/// signal when it was already 0, which caused this test failure
/// (and the silent "MAX never renders" bug in the bench): bare
/// `count.set(n)` doesn't rebuild because the surrounding reactive
/// `match mode.get() { ... }` only subscribes to `mode` — arm-body
/// reads happen inside an `untrack(..)` microtask and don't
/// subscribe the Switch's Effect.
///
/// The fix is to follow the bench's variant pattern: write the
/// row count, then touch the Switch's discriminant (here `mode`)
/// unconditionally. `Signal::set` notifies subscribers regardless
/// of value-equality, so `mode.set(0)` while mode is already 0
/// still fires the Switch effect, which re-runs its arm body and
/// picks up the latest `count.get()`.
#[test]
fn rebuild_pattern_actually_rebuilds_to_new_count() {
    let rt = TestRuntime::new();
    let mode: Signal<u32> = signal!(0u32);
    let count: Signal<usize> = signal!(50usize);

    let _owner = rt.render(rebuild_tree(mode, count));

    // Initial mount: 50 rows.
    let initial = rt.events();
    let initial_texts = count_create_text(&initial);
    assert_eq!(
        initial_texts, 50,
        "initial mount of count=50 should produce exactly 50 CreateText events, got {} \
         (events: {:?})",
        initial_texts, initial,
    );

    // Bump the count to 500 — using the bench's set_rows pattern:
    // write the count, then touch the discriminant.
    rt.backend_mut().clear_events();
    count.set(500);
    mode.set(0); // fires the Switch effect

    let after = rt.events();
    let new_texts = count_create_text(&after);
    assert_eq!(
        new_texts, 500,
        "after count.set(500) + mode.set(0), expected 500 NEW CreateText events \
         (full rebuild), got {} — if this is 50, the arm body is reading stale count \
         (likely because we touched mode BEFORE count); if it's 0, the Switch's effect \
         didn't fire at all. Events: {:?}",
        new_texts, after,
    );

    // And the inverse: shrink back to a smaller count.
    rt.backend_mut().clear_events();
    count.set(10);
    mode.set(0);
    let after_shrink = rt.events();
    let shrunk_texts = count_create_text(&after_shrink);
    assert_eq!(
        shrunk_texts, 10,
        "after count.set(10) + mode.set(0), expected exactly 10 NEW CreateText events, \
         got {}. Events: {:?}",
        shrunk_texts, after_shrink,
    );
}

/// Stress the actual bench scale: rowsA=1000 → rowsB=10000.
/// "MAX doesn't render" was the bench-visible symptom of the
/// untrack-arm-body limitation.
#[test]
fn rebuild_at_bench_scale_produces_max_rows() {
    let rt = TestRuntime::new();
    let mode: Signal<u32> = signal!(0u32);
    let count: Signal<usize> = signal!(1000usize);

    let _owner = rt.render(rebuild_tree(mode, count));

    let initial = rt.events();
    assert_eq!(
        count_create_text(&initial),
        1000,
        "initial mount of count=1000 should produce 1000 CreateText events",
    );

    // Flip to MAX — count first, then touch discriminant.
    rt.backend_mut().clear_events();
    count.set(10_000);
    mode.set(0);

    let max_events = rt.events();
    let max_texts = count_create_text(&max_events);
    assert_eq!(
        max_texts, 10_000,
        "after count.set(10000) + mode.set(0), expected 10000 NEW CreateText events. \
         Got {}. (event total: {})",
        max_texts,
        max_events.len(),
    );
}

/// LIMITATION TEST — documents the framework's behavior so future
/// changes (e.g. removing the `untrack` around arm-body build)
/// surface here.
///
/// Bare `count.set(n)` with no discriminant write does NOT rebuild,
/// because the reactive `match` only subscribes the Switch's
/// Effect to the discriminant function's reads. Arm-body reads
/// happen inside `untrack(..)` and don't subscribe.
///
/// If a future framework change makes arm-body reads subscribe,
/// THIS test starts emitting events on `count.set` alone — at
/// which point the comment in [`set_rows`] (in the bench variant)
/// can be relaxed.
#[test]
fn bare_count_set_without_discriminant_touch_does_not_rebuild() {
    let rt = TestRuntime::new();
    let mode: Signal<u32> = signal!(0u32);
    let count: Signal<usize> = signal!(100usize);

    let _owner = rt.render(rebuild_tree(mode, count));

    rt.backend_mut().clear_events();
    count.set(999);

    let after = rt.events();
    let texts = count_create_text(&after);
    assert_eq!(
        texts, 0,
        "bare count.set without touching the discriminant should produce 0 new \
         CreateText events (this documents the current framework behavior — \
         if you're seeing >0, the framework now tracks arm-body reads and the \
         variant's set_rows can drop its `mode.set` line). Events: {:?}",
        after,
    );
}

// Suppress unused-import warning if `Rc` isn't needed in some build
// configurations.
#[allow(dead_code)]
fn _force_rc_use() -> Rc<u32> {
    Rc::new(0)
}
