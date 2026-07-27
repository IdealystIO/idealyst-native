//! Read/write capability halves — `Signal::split` / `read_only` /
//! `write_only`, the `ReadSignal`/`WriteSignal` newtypes, and the
//! type-level guarantee that `memo` outputs are read-only.
//!
//! The halves are zero-cost views over the SAME arena slot: identical
//! tracking, identical generational stale-safety. What changes is only
//! what the TYPE lets the holder do — these tests pin that the split
//! doesn't alter any runtime semantics.

use std::cell::Cell;
use std::rc::Rc;

use runtime_core::{memo, signal, IntoElement, Reactive, ReadSignal, Signal, WriteSignal};

use crate::common::{counted_effect, TestRuntime};

#[test]
fn split_halves_share_the_slot() {
    let s = signal(1);
    let (read, write) = s.split();
    write.set(5);
    assert_eq!(read.get(), 5, "write half must be visible through the read half");
    // The unified handle stays valid — the halves are views, not a move.
    assert_eq!(s.get(), 5);
    s.set(9);
    assert_eq!(read.get(), 9);
    // Same slot id end to end (what f-string text bindings/robot key on).
    assert_eq!(s.id(), read.id());
    assert_eq!(s.id(), write.id());
}

#[test]
fn read_half_tracks_like_the_unified_handle() {
    let s: Signal<i32> = signal(0);
    let read = s.read_only();
    let (counter, _e) = counted_effect(move || {
        let _ = read.get();
    });
    assert_eq!(counter.get(), 1, "initial run");
    s.set(1);
    assert_eq!(counter.get(), 2, "a ReadSignal read must subscribe the effect");
}

#[test]
fn write_half_inside_effect_does_not_subscribe() {
    // A WriteSignal has no read surface, so an effect that only WRITES
    // through it can't accidentally subscribe itself to that slot — the
    // capability the split exists to encode.
    let trigger: Signal<i32> = signal(0);
    let out: Signal<i32> = signal(0);
    let write = out.write_only();
    let (counter, _e) = counted_effect(move || {
        let n = trigger.get();
        write.set(n * 10);
    });
    assert_eq!(counter.get(), 1);
    trigger.set(3);
    assert_eq!(counter.get(), 2, "re-fires on the tracked trigger only");
    assert_eq!(out.get(), 30);
    // Writing `out` from elsewhere must NOT re-fire the effect (it never
    // read `out`, so it holds no subscription to it).
    out.set(999);
    assert_eq!(counter.get(), 2, "the write-only slot is not a dependency");
}

#[test]
fn regression_stale_write_half_is_a_safe_noop_after_scope_drop() {
    // Same generational-no-op contract as Signal::set (see
    // [[project_generational_signal_handles]] — the deferred-set crash
    // fix). The wrapper must not bypass it.
    let rt = TestRuntime::new();
    let escaped: Rc<Cell<Option<WriteSignal<i32>>>> = Rc::new(Cell::new(None));
    let slot = escaped.clone();
    let owner = rt.render_with(move || {
        let s: Signal<i32> = signal(1);
        slot.set(Some(s.write_only()));
        runtime_core::view(Vec::new()).into_element()
    });
    let write = escaped.take().expect("write half escaped the scope");
    drop(owner); // scope tears down; the slot is freed
    write.set(2); // must be a silent no-op, not a panic
    write.update(|v| *v += 1); // likewise
}

#[test]
fn memo_output_is_read_only_and_live() {
    let s = signal(2);
    // Compile-time pin: memo returns the READ half. (The old return type
    // was a writable Signal<T> — an author `.set()` on it "worked" until
    // the next dependency change silently clobbered it.)
    let doubled: ReadSignal<i32> = memo(move || s.get() * 2);
    assert_eq!(doubled.get(), 4);
    s.set(10);
    assert_eq!(doubled.get(), 20);
}

#[test]
fn read_signal_coerces_into_reactive_prop_value() {
    // The prop path: a memo output (ReadSignal) must flow into a
    // `Reactive<T>` field exactly like a unified Signal does — as a LIVE
    // Dynamic, not a snapshot.
    let s = signal(1);
    let m = memo(move || s.get() + 100);
    let prop: Reactive<i32> = m.into();
    assert!(!prop.is_static(), "a ReadSignal prop must arrive Dynamic");
    assert_eq!(prop.get(), 101);
    s.set(5);
    assert_eq!(prop.get(), 105, "the coerced prop must stay live");
}

#[test]
fn halves_are_copy_and_default_constructible() {
    // Copy: two closures can capture the same half without `.clone()`
    // ceremony — same ergonomics contract as `Signal` itself.
    let (read, write) = signal(7).split();
    let a = move || read.get();
    let b = move || read.get();
    write.set(8);
    assert_eq!(a() + b(), 16);

    // Default: the detached sentinel exists so `#[props]` structs with
    // ReadSignal/WriteSignal fields satisfy the `BuildElement: Default`
    // contract (required handle props overwrite the sentinel at the call
    // site, same as `Signal`).
    let _r: ReadSignal<i32> = ReadSignal::default();
    let _w: WriteSignal<i32> = WriteSignal::default();
}

#[test]
fn write_half_set_gates_notification() {
    let s: Signal<i32> = signal(1);
    let write = s.write_only();
    let (counter, _e) = counted_effect(move || {
        let _ = s.get();
    });
    assert_eq!(counter.get(), 1);
    write.set(1); // equal — must not notify (guarded default)
    assert_eq!(counter.get(), 1, "equal set through the write half is silent");
    write.set(2);
    assert_eq!(counter.get(), 2);
    write.set_always(2); // same value, explicit always-notify
    assert_eq!(counter.get(), 3, "set_always through the write half fires");
    write.touch();
    assert_eq!(counter.get(), 4, "touch through the write half fires");
    write.set_untracked(9); // silent write
    assert_eq!(counter.get(), 4, "set_untracked through the write half is silent");
}
