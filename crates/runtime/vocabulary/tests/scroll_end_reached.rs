//! `on_end_reached`, spelled the way an author spells it, all the way to
//! the backend.
//!
//! 47d014a2 found that `scroll_view(on_end_reached = cb)` inside `ui!`
//! COMPILED and reached nothing: the macro lowered only `horizontal`,
//! and an attribute it did not know was dropped in silence. Diagnosing
//! it took an instrumented device build — the giveaway was that not
//! even the mount-time log line printed, which ruled out the backend and
//! pointed at a prop that never arrived.
//!
//! These mount through the real `realize` path against `host-mock`, so
//! every hop is the one a real backend takes: the `ui!` lowering → the
//! `glue` setter → the prim field → the handler → the backend's
//! `observe_scroll_end` cap → the author's callback. The mock records the
//! cap call and keeps the callback, so a test can both prove the observer
//! was installed with the right parameters and fire it to stand in for
//! the reader arriving.

use std::cell::Cell;
use std::rc::Rc;

use host_mock::Harness;
use runtime_macros::ui;
use runtime_vocabulary::glue::{text, Element};
use runtime_world::signal;

fn harness() -> Harness {
    let h = Harness::new();
    h.shared.splice.set(true);
    h
}

fn observe_lines(h: &Harness) -> Vec<String> {
    h.shared
        .log
        .borrow()
        .iter()
        .filter(|l| l.starts_with("observe_scroll_end"))
        .cloned()
        .collect()
}

/// Regression: the inline spelling reaches the backend, with the
/// threshold the author gave, and the callback it installs is the
/// author's.
#[test]
fn regression_scroll_view_on_end_reached_spelled_inline_reaches_the_backend() {
    let h = harness();
    let arrivals = Rc::new(Cell::new(0u32));
    let counter = arrivals.clone();
    let tree: Element = h.world.enter(|| {
        ui! {
            scroll_view(
                on_end_reached = move || counter.set(counter.get() + 1),
                end_reached_threshold = 400.0,
            ) {
                text("row")
            }
        }
    });
    let _realized = h.mount(tree);
    h.flush();

    let lines = observe_lines(&h);
    assert_eq!(lines.len(), 1, "exactly one observer installed; log:\n{lines:?}");
    assert!(
        lines[0].ends_with("horizontal=false threshold=400"),
        "the author's axis and threshold arrive intact: {}",
        lines[0]
    );

    let observers = h.shared.end_observers.borrow();
    let (_, _, _, on_end) = observers.first().expect("the callback was kept");
    on_end();
    on_end();
    assert_eq!(arrivals.get(), 2, "the backend holds the author's own callback");
}

/// A `scroll_view` without `on_end_reached` installs nothing — the
/// observer is not free on every backend (iOS shares a delegate for it),
/// so it is only asked for when the author asked.
#[test]
fn scroll_view_without_on_end_reached_installs_no_observer() {
    let h = harness();
    let tree: Element = h.world.enter(|| ui! { scroll_view() { text("row") } });
    let _realized = h.mount(tree);
    h.flush();
    assert!(observe_lines(&h).is_empty());
    assert!(h.shared.end_observers.borrow().is_empty());
}

/// `horizontal` rides along: the observer must watch the axis the
/// scroller actually travels on, or it measures the wrong dimension and
/// never fires.
#[test]
fn a_horizontal_scroll_view_observes_its_x_axis() {
    let h = harness();
    let tree: Element = h.world.enter(|| {
        ui! {
            scroll_view(horizontal = true, on_end_reached = || {}) {
                text("cell")
            }
        }
    });
    let _realized = h.mount(tree);
    h.flush();
    let lines = observe_lines(&h);
    assert_eq!(lines.len(), 1);
    assert!(lines[0].contains("horizontal=true"), "{}", lines[0]);
}

/// Same trap on the virtualizer — the primitive paging exists to serve.
#[test]
fn regression_flat_list_on_end_reached_spelled_inline_reaches_the_backend() {
    let h = harness();
    let arrivals = Rc::new(Cell::new(0u32));
    let counter = arrivals.clone();
    let tree: Element = h.world.enter(|| {
        let items = signal(vec![1u32, 2, 3]);
        ui! {
            flat_list(
                data = items,
                render = |_i, n: &u32| text(format!("{n}")).into(),
                on_end_reached = move || counter.set(counter.get() + 1),
                end_reached_threshold = 800.0,
            )
        }
    });
    let _realized = h.mount(tree);
    h.flush();

    let lines = observe_lines(&h);
    assert_eq!(lines.len(), 1, "log:\n{lines:?}");
    assert!(lines[0].ends_with("horizontal=false threshold=800"), "{}", lines[0]);

    let observers = h.shared.end_observers.borrow();
    let (_, _, _, on_end) = observers.first().unwrap();
    on_end();
    assert_eq!(arrivals.get(), 1);
}
