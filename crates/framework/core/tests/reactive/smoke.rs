//! Smoke tests — minimal coverage proving the test infrastructure
//! works. When this file fails, the scaffolding is broken (not the
//! framework). Real coverage lives in sibling modules.
//!
//! Each assertion here is also a piece of executable documentation of
//! a deliberate framework behavior. If a behavior changes, the test
//! breaks; if the test breaks, the behavior change is intentional and
//! the test gets updated alongside.

use framework_core::{signal, Signal};

use crate::common::{counted_effect, counted_memo};

#[test]
fn signal_get_set_roundtrip() {
    let s = signal!(0);
    assert_eq!(s.get(), 0);
    s.set(7);
    assert_eq!(s.get(), 7);
}

#[test]
fn effect_fires_initial_and_on_change() {
    let s: Signal<i32> = signal!(0);
    let (counter, _e) = counted_effect(move || {
        let _ = s.get();
    });
    // Initial run when the Effect is created.
    assert_eq!(counter.get(), 1);
    s.set(1);
    assert_eq!(counter.get(), 2);
    s.set(2);
    assert_eq!(counter.get(), 3);
}

/// Framework behavior: `Signal::set` does NOT do an equality check.
/// Every `set` re-fires subscribers, even when the new value equals
/// the old. Equality-aware caching is the job of `memo()` (which
/// requires `T: PartialEq`).
///
/// This is a deliberate design choice: signals stay free of trait
/// bounds, and equality semantics that vary by type (tolerance for
/// floats, ignored sub-fields, etc.) are handled where they make
/// sense rather than imposed globally.
#[test]
fn signal_set_always_refires_even_with_same_value() {
    let s: Signal<i32> = signal!(42);
    let (counter, _e) = counted_effect(move || {
        let _ = s.get();
    });
    assert_eq!(counter.get(), 1);
    s.set(42); // identical value
    assert_eq!(counter.get(), 2, "Signal::set refires unconditionally");
    s.set(42);
    assert_eq!(counter.get(), 3);
}

/// Framework behavior: `memo()` computes its closure twice on
/// creation — once eagerly under `untrack` to seed the output signal
/// with a value (so synchronous readers between `memo()` returning
/// and the Effect's first run see a coherent value), and again on the
/// Effect's first run with tracking enabled (to record the
/// subscription set).
///
/// After creation, subsequent reads of the memo's output do NOT
/// recompute — they read the cached signal value. Recomputation
/// happens only when a tracked dependency changes.
#[test]
fn memo_fires_twice_on_creation_then_cached_for_reads() {
    let s: Signal<i32> = signal!(0);
    let (mcount, m) = counted_memo(move || s.get() * 2);

    assert_eq!(mcount.get(), 2, "memo fires twice on creation");
    assert_eq!(m.get(), 0);

    // Reading the memo many times does not recompute.
    for _ in 0..10 {
        let _ = m.get();
    }
    assert_eq!(mcount.get(), 2, "reads should not recompute the memo");

    // A dep change recomputes the memo exactly once.
    s.set(3);
    assert_eq!(mcount.get(), 3, "dep change should recompute exactly once");
    assert_eq!(m.get(), 6);
}

/// Framework behavior: `memo`'s output signal does NOT change (and
/// does not notify downstream subscribers) when the recomputed value
/// equals the previous one under `PartialEq`. This is the cache
/// optimization callers depend on.
#[test]
fn memo_skips_propagation_when_output_unchanged() {
    let s: Signal<i32> = signal!(0);
    let (mcount, m) = counted_memo(move || s.get() / 10); // 0..9 all map to 0
    let (downstream_count, _e) = counted_effect(move || {
        let _ = m.get();
    });

    assert_eq!(mcount.get(), 2, "memo fires twice on creation");
    assert_eq!(downstream_count.get(), 1, "downstream effect fires once on creation");

    // s changes (0 → 5), but memo output is still 0; downstream
    // shouldn't refire.
    s.set(5);
    assert_eq!(mcount.get(), 3, "memo recomputes on dep change");
    assert_eq!(
        downstream_count.get(),
        1,
        "downstream stays at 1 because memo output didn't change"
    );

    // s changes (5 → 10), memo output flips to 1; downstream fires once.
    s.set(10);
    assert_eq!(mcount.get(), 4);
    assert_eq!(downstream_count.get(), 2);
}
