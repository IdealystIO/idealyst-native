//! `batch()` semantics — multi-write coalescing.
//!
//! `batch(|| { ... })` defers effect notifications until the closure
//! returns. Multiple writes inside the batch produce ONE re-run per
//! dependent effect, not N.

use runtime_core::{batch, signal, Signal};

use crate::common::counted_effect;

/// Two writes to the same signal inside one batch produce ONE effect
/// re-run, not two.
#[test]
fn two_writes_one_signal_collapse_to_one_fire() {
    let s: Signal<i32> = signal!(0);
    let (count, _e) = counted_effect(move || {
        let _ = s.get();
    });
    assert_eq!(count.get(), 1);

    batch(|| {
        s.set(1);
        s.set(2);
        s.set(3);
    });
    assert_eq!(count.get(), 2, "three writes inside a batch fire the effect once");

    // The final value is preserved.
    assert_eq!(s.get(), 3);
}

/// Writes to multiple signals that share a subscriber: the subscriber
/// fires once per affected signal AFTER the batch closes, because the
/// framework deduplicates per (signal, subscriber) — not per batch.
///
/// Actually, more strongly: a single subscriber across N batched
/// signals fires once total (deduplication at the effect level).
#[test]
fn writes_to_multiple_signals_dedupe_per_effect() {
    let a: Signal<i32> = signal!(0);
    let b: Signal<i32> = signal!(0);
    let c: Signal<i32> = signal!(0);

    let (count, _e) = counted_effect(move || {
        let _ = a.get();
        let _ = b.get();
        let _ = c.get();
    });
    assert_eq!(count.get(), 1);

    batch(|| {
        a.set(1);
        b.set(2);
        c.set(3);
    });
    assert_eq!(
        count.get(),
        2,
        "a subscriber sees one re-run per batch, even when multiple deps changed"
    );
}

/// Nested batches: the inner batch's writes commit when the OUTER
/// batch closes. Effects fire once for the whole nest.
#[test]
fn nested_batches_commit_at_outer_close() {
    let s: Signal<i32> = signal!(0);
    let (count, _e) = counted_effect(move || {
        let _ = s.get();
    });
    assert_eq!(count.get(), 1);

    batch(|| {
        s.set(1);
        batch(|| {
            s.set(2);
            batch(|| {
                s.set(3);
            });
            s.set(4);
        });
        s.set(5);
    });

    assert_eq!(count.get(), 2, "nested batches act as one batch");
    assert_eq!(s.get(), 5);
}

/// Writes inside batch ARE visible to subsequent reads inside the
/// same batch — batching defers notification, not the write itself.
#[test]
fn writes_inside_batch_are_immediately_visible() {
    let s: Signal<i32> = signal!(0);

    batch(|| {
        s.set(1);
        assert_eq!(s.get(), 1, "read inside batch sees the write");
        s.set(2);
        assert_eq!(s.get(), 2);
    });

    assert_eq!(s.get(), 2);
}

/// Effects created INSIDE a batch fire their initial run as normal —
/// initial-run is part of construction, not signal-change
/// notification.
#[test]
fn effect_created_inside_batch_fires_initial_immediately() {
    let s: Signal<i32> = signal!(0);

    batch(|| {
        s.set(1);
        let (count, _e) = counted_effect(move || {
            let _ = s.get();
        });
        assert_eq!(count.get(), 1, "initial run fires inside the batch");
    });
}

/// Batch returns the closure's result, like `std::iter::Iterator::collect`.
/// This is a small ergonomic thing but worth pinning so refactors don't
/// silently change the signature.
#[test]
fn batch_returns_closure_result() {
    let result: i32 = batch(|| 42);
    assert_eq!(result, 42);

    let s: Signal<i32> = signal!(0);
    let result: i32 = batch(|| {
        s.set(7);
        s.get()
    });
    assert_eq!(result, 7);
}

/// Empty batch is a no-op — no effects fire and nothing panics.
#[test]
fn empty_batch_is_noop() {
    let s: Signal<i32> = signal!(0);
    let (count, _e) = counted_effect(move || {
        let _ = s.get();
    });
    let initial = count.get();

    batch(|| {});

    assert_eq!(count.get(), initial, "empty batch fires no effects");
}
