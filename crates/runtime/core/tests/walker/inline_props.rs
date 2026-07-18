//! Inline-props `#[component]` dispatch — Leptos-style fn-parameter
//! props, end to end through `ui!`.
//!
//! `#[component] fn Badge(label: String, #[prop(default = 3)] count: i32)`
//! generates `BadgeProps` (fields wrapped `Reactive<T>` by the `#[props]`
//! rules), a `Default` impl carrying the per-arg defaults, and the
//! `BuildElement` glue — so `ui! { Badge(label = "hi") }` lowers through
//! the exact same struct-literal dispatch as the explicit-struct form.
//! These tests pin the observable contract: values arrive, omitted props
//! take their declared defaults, a `Signal`-typed prop threads through
//! un-wrapped, a signal passed to a data prop arrives `Dynamic` (live),
//! `#[prop(static)]` keeps the bare type, and a `children: Vec<Element>`
//! param receives the call site's children block.

use std::cell::RefCell;
use std::rc::Rc;

use runtime_core::{component, signal, ui, Element, Signal};

use crate::common::TestRuntime;

thread_local! {
    /// What each component observed during build; assertions read this so
    /// they don't depend on Text-render plumbing.
    static SEEN: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

fn seen() -> Vec<String> {
    SEEN.with(|s| s.borrow().clone())
}

fn record(entry: String) {
    SEEN.with(|s| s.borrow_mut().push(entry));
}

fn reset() {
    SEEN.with(|s| s.borrow_mut().clear());
}

/// Inline data props: `label` arrives as `Reactive<String>` (the
/// `#[props]` wrap), `count` takes its `#[prop(default = …)]` when the
/// call site omits it.
#[component]
fn Badge(label: String, #[prop(default = 3)] count: i32) -> Element {
    record(format!("{}:{}:{}", label.get(), count.get(), label.is_static()));
    ui! {
        text { "badge" }
    }
}

#[test]
fn inline_props_receive_call_site_values() {
    reset();
    let rt = TestRuntime::new();
    let tree = ui! { Badge(label = "hi", count = 9) };
    let _owner = rt.render(tree);
    assert_eq!(seen(), vec!["hi:9:true"], "both provided props must arrive");
}

#[test]
fn omitted_prop_takes_declared_default() {
    reset();
    let rt = TestRuntime::new();
    let tree = ui! { Badge(label = "hi") };
    let _owner = rt.render(tree);
    assert_eq!(seen(), vec!["hi:3:true"], "`count` must fall back to `#[prop(default = 3)]`");
}

#[test]
fn signal_into_data_prop_arrives_dynamic() {
    reset();
    let rt = TestRuntime::new();
    let s = signal(String::from("live"));
    let tree = ui! { Badge(label = s) };
    let _owner = rt.render(tree);
    // `is_static() == false` proves the signal carried through as
    // `Reactive::Dynamic` — a live read, not a build-time snapshot.
    assert_eq!(seen(), vec!["live:3:false"]);
}

/// A `Signal<T>` param is a skip-shape: it must thread through bare (not
/// double-wrapped), same as an explicit-struct `Signal` field.
#[component]
fn Meter(value: Signal<i32>) -> Element {
    record(format!("meter:{}", value.get()));
    ui! {
        text { "meter" }
    }
}

#[test]
fn signal_typed_prop_threads_through_unwrapped() {
    reset();
    let rt = TestRuntime::new();
    let s = signal(7);
    let tree = ui! { Meter(value = s) };
    let _owner = rt.render(tree);
    assert_eq!(seen(), vec!["meter:7"]);
}

/// `#[prop(static)]` opts a field out of the `Reactive` wrap: the body
/// sees the bare declared type. `Option<Rc<dyn Fn()>>` is the §9.6
/// optional-callback shape — skipped by the wrap heuristic, defaulting to
/// `None` with no `#[prop(default)]` needed.
#[component]
fn Chip(#[prop(static)] size: u8, on_press: Option<Rc<dyn Fn()>>) -> Element {
    let bare: u8 = size; // type proof: `size` is NOT Reactive<u8>
    record(format!("chip:{}:{}", bare, on_press.is_some()));
    ui! {
        text { "chip" }
    }
}

#[test]
fn prop_static_keeps_bare_type_and_optional_callback_defaults_none() {
    reset();
    let rt = TestRuntime::new();
    let tree = ui! { Chip(size = 2) };
    let _owner = rt.render(tree);
    assert_eq!(seen(), vec!["chip:2:false"]);
}

#[test]
fn optional_callback_passes_through() {
    reset();
    let rt = TestRuntime::new();
    let cb: Rc<dyn Fn()> = Rc::new(|| {});
    let tree = ui! { Chip(size = 5, on_press = Some(cb)) };
    let _owner = rt.render(tree);
    assert_eq!(seen(), vec!["chip:5:true"]);
}

/// A param named `children: Vec<Element>` receives the call site's
/// `{ … }` block — same field contract as an explicit-struct container.
#[component]
fn Frame(title: String, children: Vec<Element>) -> Element {
    record(format!("frame:{}:{}", title.get(), children.len()));
    ui! {
        view() {
            children
        }
    }
}

#[test]
fn children_param_receives_children_block() {
    reset();
    let rt = TestRuntime::new();
    let tree = ui! {
        Frame(title = "t") {
            text { "a" }
            text { "b" }
        }
    };
    let _owner = rt.render(tree);
    assert_eq!(seen(), vec!["frame:t:2"]);
}
