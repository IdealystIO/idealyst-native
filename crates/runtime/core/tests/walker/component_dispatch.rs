//! User-component dispatch through `BuildElement` — the macro-free
//! struct-literal lowering that replaced per-component `macro_rules!`.
//!
//! `ui! { Foo(a = x) }` lowers to
//! `BuildElement::build(Foo { a: (x).into(), ..<Foo as BuildElement>::defaults() })`.
//! The `..defaults()` base forces every component's props to be `Default`,
//! which is why `Signal`/`Ref` have sentinel `Default` impls. These tests
//! pin the two behaviors that matter:
//!
//!   1. A required NON-`Default` handle prop (a `Signal`) is threaded
//!      through to the component — NOT replaced by the sentinel default.
//!   2. Omitting that required prop fails LOUDLY (the detached sentinel
//!      panics on use), rather than silently rendering with a dead signal.

use std::cell::RefCell;

use runtime_core::{component, signal, ui, Element, Signal};

use crate::common::TestRuntime;

thread_local! {
    /// Values the component observed on its `value` prop, captured during
    /// build so the assertion doesn't depend on Text-render plumbing.
    static SEEN: RefCell<Vec<i32>> = const { RefCell::new(Vec::new()) };
}

#[derive(Default)]
struct LabelProps {
    value: Signal<i32>,
}

#[component]
fn Label(props: &LabelProps) -> Element {
    // Reading the prop signal proves it was threaded through dispatch.
    // (A detached sentinel default would panic here instead.)
    let observed = props.value.get();
    SEEN.with(|s| s.borrow_mut().push(observed));
    ui! {
        Text { "label" }
    }
}

#[test]
fn dispatches_with_required_signal_prop() {
    SEEN.with(|s| s.borrow_mut().clear());
    let rt = TestRuntime::new();
    let s = signal!(7);
    let tree = ui! { Label(value = s) };
    let _owner = rt.render(tree);
    assert_eq!(
        SEEN.with(|s| s.borrow().clone()),
        vec![7],
        "the passed-in signal must reach the component, not the sentinel default",
    );
}

#[test]
#[should_panic(expected = "signal used")]
fn omitting_required_signal_prop_panics_loudly() {
    // `Label()` omits `value`, so the struct-update base supplies the
    // detached sentinel `Signal::default()`. Reading it during render must
    // panic with a clear message — never silently use a dead signal.
    let rt = TestRuntime::new();
    let tree = ui! { Label() };
    let _owner = rt.render(tree);
}
