//! Props-arrival contract — the successor of the old walker's
//! `inline_props.rs` (7) and `component_dispatch.rs` (2).
//!
//! `runtime-macros` has EMISSION tests (it asserts what tokens come out);
//! what died with the walker was the *behavioral* half: that the values a
//! `ui!` call site writes actually reach the component body, that omitted
//! props take their declared defaults, that a `Signal`-typed prop threads
//! through un-wrapped, that a signal handed to a data prop arrives LIVE
//! (`Dynamic`, not snapshotted), that `#[prop(static)]` keeps the bare
//! type, and that a `children: Vec<Element>` param receives the call
//! site's block.
//!
//! Component bodies run eagerly at build on runtime v2 (inside
//! `component_scope`: once, untracked, collected), so building the tree
//! inside `World::enter` is the whole gate — no host needed. Each body
//! records what it observed into a thread-local, so the assertions do not
//! depend on any render plumbing.
//!
//! One **sanctioned divergence** is pinned here rather than the old
//! behavior: omitting a required two-way signal prop mints a fresh
//! default-valued signal instead of panicking on a detached sentinel
//! (`docs/migrating-to-runtime-v2.md`, "Omitted required signal props").
//! The old `component_dispatch.rs::omitting_required_signal_prop_panics_loudly`
//! has no successor by design — its successor is
//! `omitted_required_signal_prop_mints_a_fresh_signal`.

use std::cell::RefCell;
use std::rc::Rc;

use runtime_macros::{component, ui};
use runtime_vocabulary::glue::{signal, Element, Signal};
use runtime_world::{collect_owned, World};

thread_local! {
    /// What each component observed during its build.
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

/// Build `f`'s tree in a fresh world and drop everything it created.
/// The component body has already run by the time this returns.
fn build_in_world(f: impl FnOnce() -> Element) {
    let world = World::new();
    world.enter(|| {
        let (element, owned) = collect_owned(f);
        drop(element);
        drop(owned);
    });
}

// ===========================================================================
// Inline props
// ===========================================================================

/// `label` arrives as `Reactive<String>` (the `#[props]` wrap), `count`
/// takes its `#[prop(default = …)]` when the call site omits it.
#[component]
fn Badge(label: String, #[prop(default = 3)] count: i32) -> Element {
    record(format!(
        "{}:{}:{}",
        label.get(),
        count.get(),
        label.is_static()
    ));
    ui! { text { "badge" } }
}

#[test]
fn inline_props_receive_call_site_values() {
    reset();
    build_in_world(|| ui! { Badge(label = "hi", count = 9) });
    assert_eq!(
        seen(),
        vec!["hi:9:true"],
        "both provided props must arrive, and a literal must arrive STATIC"
    );
}

#[test]
fn omitted_prop_takes_declared_default() {
    reset();
    build_in_world(|| ui! { Badge(label = "hi") });
    assert_eq!(
        seen(),
        vec!["hi:3:true"],
        "the omitted `count` must take its #[prop(default = 3)]"
    );
}

#[test]
fn signal_into_data_prop_arrives_dynamic() {
    reset();
    let world = World::new();
    world.enter(|| {
        let name = signal(String::from("live"));
        let (element, owned) = collect_owned(|| ui! { Badge(label = name) });
        drop(element);
        drop(owned);
    });
    assert_eq!(
        seen(),
        vec!["live:3:false"],
        "a signal handed to a DATA prop must arrive Dynamic (is_static == false), \
         not snapshotted to a constant — that is what keeps the child reactive"
    );
}

/// A `Signal`-typed prop is threaded through UN-wrapped (no
/// `Reactive<T>`), so the child can write back — the two-way shape
/// (`text_input.value`, `toggle.value`).
#[component]
fn Meter(value: Signal<i32>) -> Element {
    record(format!("meter:{}", value.get()));
    value.set(value.get() + 1);
    ui! { text { "meter" } }
}

#[test]
fn signal_typed_prop_threads_through_unwrapped() {
    reset();
    let world = World::new();
    let captured = world.enter(|| {
        let count = signal(7i32);
        let (element, owned) = collect_owned(|| ui! { Meter(value = count) });
        drop(element);
        // The child's write is staged; commit it and read back through
        // the PARENT's handle — same slot, so the prop was threaded, not
        // copied into a fresh signal.
        world.flush();
        let observed = count.get();
        drop(owned);
        observed
    });
    assert_eq!(seen(), vec!["meter:7"], "the parent's value arrives");
    assert_eq!(
        captured, 8,
        "the child's write must land on the PARENT's signal (threaded un-wrapped)"
    );
}

/// `#[prop(static)]` opts out of the `Reactive<T>` wrap (bare `u8`), and
/// an `Option<Rc<dyn Fn()>>` callback prop defaults to `None`.
#[component]
fn Chip(#[prop(static)] size: u8, on_press: Option<Rc<dyn Fn()>>) -> Element {
    record(format!("chip:{}:{}", size, on_press.is_some()));
    if let Some(cb) = on_press {
        cb();
    }
    ui! { text { "chip" } }
}

#[test]
fn prop_static_keeps_bare_type_and_optional_callback_defaults_none() {
    reset();
    build_in_world(|| ui! { Chip(size = 4) });
    assert_eq!(
        seen(),
        vec!["chip:4:false"],
        "#[prop(static)] arrives as a bare value; the absent callback is None"
    );
}

#[test]
fn optional_callback_passes_through() {
    reset();
    let fired = Rc::new(RefCell::new(false));
    let f = fired.clone();
    build_in_world(move || {
        ui! {
            Chip(
                size = 1,
                on_press = Rc::new(move || *f.borrow_mut() = true) as Rc<dyn Fn()>,
            )
        }
    });
    assert_eq!(seen(), vec!["chip:1:true"]);
    assert!(*fired.borrow(), "the author's callback is the one that arrived");
}

/// A `children: Vec<Element>` param receives the call site's children
/// block (the container-component shape).
#[component]
fn Frame(title: String, children: Vec<Element>) -> Element {
    record(format!("frame:{}:{}", title.get(), children.len()));
    ui! { view() { children } }
}

#[test]
fn children_param_receives_children_block() {
    reset();
    build_in_world(|| {
        ui! {
            Frame(title = "t") {
                text { "a" }
                text { "b" }
            }
        }
    });
    assert_eq!(
        seen(),
        vec!["frame:t:2"],
        "both children from the call site's block reach the `children` param"
    );
}

// ===========================================================================
// Explicit-struct dispatch (`BuildElement`)
// ===========================================================================

/// The explicit-props form: `ui! { Label(value = sig) }` lowers to
/// `BuildElement::build(LabelProps { value: (sig).into(),
/// ..defaults() })`. The `..defaults()` base is why every props struct
/// must be `Default` — and why a REQUIRED handle prop must be proven to
/// survive it instead of being replaced by the default.
///
/// Hand-rolled `Default` (not `#[derive]`): the world kernel's `Signal`
/// has no `Default` — a handle must belong to a world — so a props
/// struct with a signal field supplies the base explicitly. That is the
/// new-core replacement for the old core's detached sentinel, and the
/// reason the divergence below exists at all.
pub struct LabelProps {
    pub value: Signal<i32>,
}

impl Default for LabelProps {
    fn default() -> Self {
        LabelProps {
            value: runtime_vocabulary::glue::fresh_signal(0),
        }
    }
}

#[component]
fn Label(props: &LabelProps) -> Element {
    record(format!("label:{}", props.value.get()));
    ui! { text { "label" } }
}

#[test]
fn dispatches_with_required_signal_prop() {
    reset();
    let world = World::new();
    world.enter(|| {
        let v = signal(42i32);
        let (element, owned) = collect_owned(|| ui! { Label(value = v) });
        drop(element);
        drop(owned);
    });
    assert_eq!(
        seen(),
        vec!["label:42"],
        "the caller's signal — not the `..defaults()` base — reaches the component"
    );
}

/// **Sanctioned divergence** (migration guide): omitting the required
/// signal prop mints a FRESH default-valued signal on runtime v2, where
/// the old core panicked on the detached sentinel. The component builds
/// and renders with unshared state; nothing aborts.
#[test]
fn omitted_required_signal_prop_mints_a_fresh_signal() {
    reset();
    build_in_world(|| ui! { Label() });
    assert_eq!(
        seen(),
        vec!["label:0"],
        "the omitted prop mints a fresh signal at the type's default value \
         (old core: panic on the detached sentinel — documented divergence)"
    );
}
