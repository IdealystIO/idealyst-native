//! `resource(deps, fetcher)` — async data as a reactive primitive.
//!
//! Feature-gated behind `async-driver`. Without an installed async
//! executor (via `install_async_executor`), `spawn_async` falls back
//! to `pollster::block_on`, which is synchronous — that's the path
//! these tests run under.
//!
//! Covered: synchronous success, refetch, dep-change re-running,
//! `ResourceState` field shape, accessor methods.
//!
//! Race-condition tests (where an OLD fetch resolves AFTER a NEWER
//! one was issued) require a custom executor that lets us defer
//! resolution — those live in runtime-core's inline tests, not
//! here, because driving them requires `pub(crate)` knobs.

#![cfg(feature = "async-driver")]

use runtime_core::{resource, signal, Signal};

/// Synchronous fetcher → immediate success state.
#[test]
fn resource_with_sync_ok_fetcher() {
    let trigger: Signal<i32> = signal!(0);
    let r = resource(trigger, |_id: i32, _cancel| async move { Ok::<&str, &str>("hello") });

    let s = r.state();
    assert!(!s.loading);
    assert_eq!(s.data, Some("hello"));
    assert_eq!(s.error, None);
}

/// Synchronous fetcher → error state.
#[test]
fn resource_with_sync_err_fetcher() {
    let trigger: Signal<i32> = signal!(0);
    let r = resource(trigger, |_id: i32, _cancel| async move { Err::<&str, &str>("boom") });

    let s = r.state();
    assert!(!s.loading);
    assert_eq!(s.data, None);
    assert_eq!(s.error, Some("boom"));
}

/// Dep change re-runs the fetcher with the new input.
#[test]
fn resource_refetches_on_dep_change() {
    let id: Signal<i32> = signal!(1);
    let r = resource(id, |id: i32, _cancel| async move { Ok::<String, &str>(format!("user-{id}")) });

    assert_eq!(r.data(), Some("user-1".to_string()));

    id.set(2);
    assert_eq!(r.data(), Some("user-2".to_string()));

    id.set(42);
    assert_eq!(r.data(), Some("user-42".to_string()));
}

/// `refetch()` re-runs the fetcher without changing deps.
#[test]
fn resource_refetch_method() {
    use std::cell::Cell;
    use std::rc::Rc;
    let call_count: Rc<Cell<usize>> = Rc::new(Cell::new(0));
    let count_for_fetcher = call_count.clone();

    let id: Signal<i32> = signal!(0);
    let r = resource(id, move |_id: i32, _cancel| {
        let c = count_for_fetcher.clone();
        async move {
            c.set(c.get() + 1);
            Ok::<usize, &str>(c.get())
        }
    });

    assert_eq!(call_count.get(), 1);
    assert_eq!(r.data(), Some(1));

    r.refetch();
    assert_eq!(call_count.get(), 2);
    assert_eq!(r.data(), Some(2));

    r.refetch();
    assert_eq!(call_count.get(), 3);
    assert_eq!(r.data(), Some(3));
}

/// Accessor methods agree with the snapshot returned by `state()`.
#[test]
fn accessors_match_state_snapshot() {
    let id: Signal<i32> = signal!(0);
    let r = resource(id, |_: i32, _cancel| async move { Ok::<i32, &str>(42) });

    let s = r.state();
    assert_eq!(s.data, r.data());
    assert_eq!(s.error, r.error());
    assert_eq!(s.loading, r.loading());
}

/// `ResourceState` is a struct with three independent fields, not an
/// enum. Field access shape is what the docs promise.
#[test]
fn resource_state_struct_shape() {
    let id: Signal<i32> = signal!(0);
    let r = resource(id, |_: i32, _cancel| async move { Ok::<i32, &str>(42) });

    let s = r.state();
    let _data: Option<i32> = s.data;
    let _error: Option<&str> = s.error;
    let _loading: bool = s.loading;
}

/// Reads of `r.data()` are tracked — using one inside an Effect
/// subscribes the effect to data changes (via the internal state
/// signal).
#[test]
fn resource_data_reads_are_tracked() {
    use std::cell::Cell;
    use std::rc::Rc;
    let id: Signal<i32> = signal!(1);
    let r = resource(id, |id: i32, _cancel| async move { Ok::<i32, &str>(id * 10) });

    let observed: Rc<Cell<Option<i32>>> = Rc::new(Cell::new(None));
    let obs = observed.clone();

    let _e = runtime_core::Effect::new(move || {
        obs.set(r.data());
    });

    assert_eq!(observed.get(), Some(10));

    id.set(2);
    assert_eq!(observed.get(), Some(20), "effect re-fired with new data");

    id.set(5);
    assert_eq!(observed.get(), Some(50));
}
