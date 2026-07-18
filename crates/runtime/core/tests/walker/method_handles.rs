//! `#[method]` fns + the auto-injected `bind_to` prop — the component
//! imperative surface, bound through the ORDINARY `ui!` tag form.
//!
//! This is the modernized `methods!` story: nested `#[method]` fns become
//! a generated `{Component}Handle`; the macro injects a
//! `bind_to: Option<Ref<Handle>>` prop (inline-props path) and fills it
//! in-body, so `ui! { Tag(bind_to = h) }` binds with no `Bindable`
//! return and no fn-call form. Callers invoke via `h.get()` (never
//! `.with()` — methods write signals, and `with` holds the arena borrow).

use std::cell::Cell;

use runtime_core::{component, signal, text, ui, Element, Ref, Signal};

use crate::common::TestRuntime;

thread_local! {
    /// The component smuggles its state signal out so tests can observe
    /// method effects without depending on text-render plumbing.
    static VALUE: Cell<Option<Signal<i32>>> = const { Cell::new(None) };
}

#[component]
fn Tally(#[prop(default = 0)] start: i32) -> Element {
    let value: Signal<i32> = signal(start.get());
    VALUE.with(|v| v.set(Some(value)));

    /// Add `n` to the tally.
    #[method]
    fn bump(n: i32) {
        value.update(|v| *v += n);
    }

    /// Reset to zero.
    #[method]
    fn reset() {
        value.set(0);
    }

    ui! {
        text(move || format!("{}", value.get()))
    }
}

#[test]
fn tag_form_binds_the_method_handle() {
    let rt = TestRuntime::new();
    let h: Ref<TallyHandle> = Ref::new();

    // The ORDINARY tag form — `bind_to` is a real (auto-injected) prop
    // on the generated TallyProps, mixed freely with authored props.
    let _owner = rt.render(ui! { Tally(start = 10, bind_to = h) });

    let value = VALUE.with(|v| v.get()).expect("component built");
    assert_eq!(value.get(), 10);

    // `.get()` clones the handle out (releasing the arena borrow), so
    // methods that write signals are safe to call.
    let handle = h.get().expect("bind_to must fill with the handle at build");
    handle.bump(5);
    assert_eq!(value.get(), 15, "method writes drive the component's state");
    handle.reset();
    assert_eq!(value.get(), 0);
}

#[test]
fn bind_to_is_optional_like_any_prop() {
    // Omitting it takes the injected Option's default (None) — the
    // component builds and behaves normally without a binder.
    let rt = TestRuntime::new();
    let _owner = rt.render(ui! { Tally(start = 3) });
    let value = VALUE.with(|v| v.get()).expect("component built");
    assert_eq!(value.get(), 3);
}

#[test]
fn component_returns_plain_element_on_the_new_path() {
    // No `Bindable` in the signature or the return — the fn-call form
    // yields an Element-compatible value like any other component.
    fn assert_element(_: Element) {}
    let rt = TestRuntime::new();
    // Building outside ui! still works; the handle simply goes unbound.
    let _owner = rt.render({
        let e: Element = ui! { Tally() };
        assert_element(ui! { Tally(start = 1) });
        e
    });
}
