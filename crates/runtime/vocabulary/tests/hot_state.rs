//! State surviving a dev-time hot re-run, through the real mount path.
//!
//! `runtime_world::hot_state`'s own unit tests exercise the kernel half
//! against bare `signal()` calls. This suite is the other half: the
//! path stack is driven by the build probe `#[component]` emits, which
//! lives in `glue`, and what has to survive is a tree that was actually
//! `realize`d against a host and then torn down and rebuilt — the exact
//! sequence the dev sidecar runs on `SessionMsg::Rerender`.
//!
//! The components here are spelled with `component_scope` + the probe
//! rather than with `#[component]`, for the same reason the rest of this
//! crate's suites are: `ui!`/`#[component]` live above the vocabulary
//! and emit absolute `glue` paths. `component_scope` is literally what
//! the macro wraps a body in, and `__component_build_probe` is literally
//! what it brackets the body with, so the lifetimes and the path stack
//! under test are the real ones.
//!
//! Run with the feature on:
//!
//! ```text
//! cargo test -p runtime-vocabulary --features hot-reload --test hot_state
//! ```

#![cfg(feature = "hot-reload")]

use host_mock::Harness;
use runtime_scene::{component_scope, Element, Realized};
use host_mock::Node;
use runtime_vocabulary::builders::{text, view};
use runtime_vocabulary::glue::__component_build_probe;
use runtime_world::hot_state;
use runtime_world::{signal, Signal};

/// The recorded op log, joined.
///
/// Not `Harness::tree`: a reactive `text` slot is CREATED empty and
/// filled by an `update_text_by_id` op, so the rendered tree shows `text
/// ""` and says nothing about what is on screen. The op log is where the
/// rendered string actually appears.
fn ops(h: &Harness) -> String {
    h.ops().join("\n")
}

/// A component body: probe, then `component_scope`, exactly as
/// `#[component]` emits it. Returns the element plus the signal it
/// created so a test can drive it.
fn counter(name: &'static str, start: i32) -> (Element, Signal<i32>) {
    let _probe = __component_build_probe(name);
    let mut handle = None;
    let element = component_scope(|| {
        let n = signal(start);
        handle = Some(n);
        view().child(text().content(move || format!("{name}={}", n.get()))).build()
    });
    (element, handle.expect("body ran"))
}

/// One mount, inside a build window, the way the sidecar does it.
fn mounted<R>(h: &Harness, build: impl FnOnce() -> (Element, R)) -> (Realized<Node>, R) {
    hot_state::arm();
    let (element, extra) = h.world.enter(build);
    let realized = h.mount(element);
    hot_state::disarm();
    (realized, extra)
}

/// The headline: a component's signal comes back where the user left
/// it, through a real mount → teardown → re-mount cycle.
#[test]
fn a_components_signal_survives_a_re_run() {
    let h = Harness::new();
    let (realized, n) = mounted(&h, || counter("Counter", 0));
    n.set(41);
    h.flush();
    assert!(ops(&h).contains("Counter=41"), "{}", ops(&h));

    // The sidecar's rerender, in order: harvest out of the live world,
    // then drop, then seed, then rebuild.
    let carried = hot_state::harvest();
    assert_eq!(carried.len(), 1);
    drop(realized);

    hot_state::seed(carried);
    // The "patched" body: same shape, different rendering.
    let (_realized2, n2) = mounted(&h, || {
        let _probe = __component_build_probe("Counter");
        let mut handle = None;
        let element = component_scope(|| {
            let n = signal(0i32);
            handle = Some(n);
            view().child(text().content(move || format!("patched={}", n.get()))).build()
        });
        (element, handle.expect("body ran"))
    });
    h.flush();
    assert_eq!(n2.get(), 41, "the re-run must start from the carried value");
    assert!(
        ops(&h).contains("patched=41"),
        "the PATCHED body must render the CARRIED state:\n{}",
        ops(&h)
    );
}

/// Two components of the same name under one parent are two frames, so
/// their state does not swap places on a re-run. This is the failure
/// that position-based matching has to not have.
#[test]
fn sibling_components_do_not_swap_state_across_a_re_run() {
    let h = Harness::new();
    let (realized, (a, b)) = mounted(&h, || {
        let mut handles = None;
        let element = component_scope(|| {
            let (first, a) = counter("Row", 0);
            let (second, b) = counter("Row", 0);
            handles = Some((a, b));
            view().children(vec![first, second]).build()
        });
        (element, handles.expect("body ran"))
    });
    a.set(1);
    b.set(2);
    h.flush();

    let carried = hot_state::harvest();
    assert_eq!(carried.len(), 2);
    drop(realized);
    hot_state::seed(carried);

    let (_r2, (a2, b2)) = mounted(&h, || {
        let mut handles = None;
        let element = component_scope(|| {
            let (first, a) = counter("Row", 0);
            let (second, b) = counter("Row", 0);
            handles = Some((a, b));
            view().children(vec![first, second]).build()
        });
        (element, handles.expect("body ran"))
    });
    h.flush();
    assert_eq!((a2.get(), b2.get()), (1, 2), "sibling state must not cross over");
}

/// A body that gained state of a different type at an earlier position
/// gets FRESH state rather than another variable's value. Without the
/// type check and the frame poisoning this is the silent-corruption
/// case: the author's new `signal(String)` would be handed an `i32`'s
/// slot, or worse, the counter's value would land in the next variable
/// along.
#[test]
fn a_body_whose_state_layout_changed_gets_fresh_state() {
    let h = Harness::new();
    let (realized, n) = mounted(&h, || counter("Counter", 0));
    n.set(41);
    h.flush();

    let carried = hot_state::harvest();
    drop(realized);
    hot_state::seed(carried);

    // The patched body declares a String FIRST, so ordinal 0's type no
    // longer matches what was recorded.
    let (_r2, (label, count)) = mounted(&h, || {
        let _probe = __component_build_probe("Counter");
        let mut handles = None;
        let element = component_scope(|| {
            let label = signal(String::from("fresh"));
            let count = signal(0i32);
            handles = Some((label, count));
            view().child(text().content(move || format!("{}={}", label.with(|s| s.clone()), count.get()))).build()
        });
        (element, handles.expect("body ran"))
    });
    h.flush();
    assert_eq!(label.with(|s| s.clone()), "fresh");
    assert_eq!(count.get(), 0, "once the layout diverged, the rest of the frame is fresh");
}

/// Nothing carried: an ordinary mount is completely unaffected by the
/// machinery being compiled in.
#[test]
fn a_mount_with_nothing_seeded_uses_the_declared_values() {
    let h = Harness::new();
    let (_realized, n) = mounted(&h, || counter("Counter", 7));
    h.flush();
    assert_eq!(n.get(), 7);
    assert!(ops(&h).contains("Counter=7"), "{}", ops(&h));
}
