//! `memo` / `memo_with` — cached derived values.
//!
//! Beyond the smoke tests, this covers:
//! - Chained memos (memo of a memo)
//! - Custom equality (`memo_with`)
//! - Tracking inside the compute body
//! - The "no write during memo compute" panic
//! - Mixing memos and effects downstream

use framework_core::{memo, memo_with, signal, Signal};

use crate::common::{counted_effect, counted_memo};

/// A memo that reads another memo: changes in the source signal
/// propagate through both layers, each recomputing exactly once per
/// effective change.
#[test]
fn chained_memos_propagate_one_recompute_each() {
    let s: Signal<i32> = signal!(0);

    let (a_count, a) = counted_memo(move || s.get() + 1);
    let (b_count, _b) = counted_memo(move || a.get() * 2);

    // Initial: each memo fires twice on creation (initial + effect run).
    assert_eq!(a_count.get(), 2);
    assert_eq!(b_count.get(), 2);

    s.set(5);
    assert_eq!(a_count.get(), 3, "a recomputes once");
    assert_eq!(b_count.get(), 3, "b recomputes once");
}

/// A memo whose output is shared by multiple effects: each effect
/// fires once per memo update; memo computes once per dep change.
#[test]
fn memo_shared_by_multiple_effects_computes_once() {
    let s: Signal<i32> = signal!(0);
    let (mcount, m) = counted_memo(move || s.get() * 10);

    let (e1, _e1h) = counted_effect(move || {
        let _ = m.get();
    });
    let (e2, _e2h) = counted_effect(move || {
        let _ = m.get();
    });
    let (e3, _e3h) = counted_effect(move || {
        let _ = m.get();
    });

    assert_eq!(mcount.get(), 2, "memo: initial + effect first run");
    assert_eq!(e1.get(), 1);
    assert_eq!(e2.get(), 1);
    assert_eq!(e3.get(), 1);

    s.set(1);
    assert_eq!(mcount.get(), 3, "one recompute for one dep change");
    assert_eq!(e1.get(), 2);
    assert_eq!(e2.get(), 2);
    assert_eq!(e3.get(), 2);
}

/// `memo_with` skips downstream notification when the custom equality
/// returns true — even if the underlying recompute produced a value
/// that's not byte-identical.
#[test]
fn memo_with_skips_propagation_on_custom_eq() {
    let s: Signal<f32> = signal!(0.0);
    let m = memo_with(
        |a: &f32, b: &f32| (a - b).abs() < 0.01, // "close enough"
        move || s.get(),
    );

    let (downstream, _e) = counted_effect(move || {
        let _ = m.get();
    });
    assert_eq!(downstream.get(), 1);

    // Sub-threshold change: tolerance treats as equal; downstream
    // should NOT fire.
    s.set(0.005);
    assert_eq!(
        downstream.get(),
        1,
        "memo_with eq tolerance suppressed propagation"
    );

    // Super-threshold change: downstream fires.
    s.set(0.5);
    assert_eq!(downstream.get(), 2);
}

/// Writing to a Signal from inside a memo's compute closure panics.
/// This catches the "side effect in a derivation" bug at the point
/// of the bad write.
#[test]
#[should_panic(expected = "memo")]
fn writing_signal_inside_memo_compute_panics() {
    let s: Signal<i32> = signal!(0);
    let sink: Signal<i32> = signal!(0);

    // Memo's compute closure writes to a signal — this should panic
    // via the `assert_not_in_memo_compute` guard.
    let _ = memo(move || {
        sink.set(42); // panic here
        s.get()
    });
}

/// `update` on a Signal from inside a memo compute also panics — the
/// guard covers `update`, not just `set`.
#[test]
#[should_panic(expected = "memo")]
fn updating_signal_inside_memo_compute_panics() {
    let sink: Signal<i32> = signal!(0);

    let _ = memo(move || {
        sink.update(|v| *v += 1);
        0
    });
}

/// A memo with a tuple-of-signals deps reads all of them and
/// recomputes when ANY changes, exactly once.
#[test]
fn memo_with_multiple_deps_recomputes_once_per_any_change() {
    let a: Signal<i32> = signal!(0);
    let b: Signal<i32> = signal!(0);
    let c: Signal<i32> = signal!(0);

    let (mcount, m) = counted_memo(move || a.get() + b.get() + c.get());

    assert_eq!(mcount.get(), 2);

    a.set(1);
    assert_eq!(mcount.get(), 3);
    b.set(2);
    assert_eq!(mcount.get(), 4);
    c.set(3);
    assert_eq!(mcount.get(), 5);
    assert_eq!(m.get(), 6);
}

/// Reading a memo inside another effect's body subscribes that
/// effect to the memo's output signal — not to the memo's
/// underlying deps. Changing a dep that doesn't change the memo
/// output should NOT fire the subscriber effect.
#[test]
fn effect_subscribes_to_memo_output_not_to_memo_deps() {
    let s: Signal<i32> = signal!(0);
    let m = memo(move || s.get() / 10); // 0..9 → 0

    let (count, _e) = counted_effect(move || {
        let _ = m.get();
    });
    assert_eq!(count.get(), 1);

    // s changes a bunch but the memo output stays 0.
    for v in [1, 2, 3, 4, 5, 6, 7, 8, 9] {
        s.set(v);
    }
    assert_eq!(
        count.get(),
        1,
        "downstream effect stayed at 1 — memo output never changed"
    );

    // s rolls over to 10; memo output flips to 1; downstream fires.
    s.set(10);
    assert_eq!(count.get(), 2);
}

/// Memo dep set can shrink/grow dynamically — the underlying Effect
/// tracks reads on each compute, like any other Effect.
#[test]
fn memo_dynamic_deps() {
    let cond: Signal<bool> = signal!(true);
    let a: Signal<i32> = signal!(10);
    let b: Signal<i32> = signal!(20);

    let (mcount, m) = counted_memo(move || if cond.get() { a.get() } else { b.get() });

    assert_eq!(m.get(), 10);
    let after_init = mcount.get();

    // cond=true → reads a, not b.
    a.set(11);
    assert_eq!(mcount.get(), after_init + 1);
    assert_eq!(m.get(), 11);

    // Writing to b shouldn't recompute.
    b.set(99);
    assert_eq!(mcount.get(), after_init + 1, "b is not subscribed when cond=true");

    // Flip cond → recompute, now subscribed to b.
    cond.set(false);
    assert_eq!(mcount.get(), after_init + 2);
    assert_eq!(m.get(), 99);

    b.set(100);
    assert_eq!(mcount.get(), after_init + 3);

    a.set(50);
    assert_eq!(
        mcount.get(),
        after_init + 3,
        "a is no longer subscribed after cond flipped"
    );
}
