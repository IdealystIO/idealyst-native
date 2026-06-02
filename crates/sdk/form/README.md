# `form`

A cross-platform `Form` container built on the framework's
`Element::External` extension mechanism (with children). On **web** it
lowers to a real `<form>` element — free Enter-to-submit and browser
autofill grouping. On **native** it's a plain passthrough container;
submission is triggered by your submit button.

```rust
use form::prelude::*;       // brings in `Form!`, `form`, `FormProps`
use idea_ui::prelude::*;    // Button, TextInput, …

// App bootstrap (one line per third-party SDK):
let mut backend = WebBackend::new("#app");
form::register(&mut backend);

// The submit action is a plain closure that reads your field signals —
// it is NOT fed by the DOM's FormData. Build it once and share the `Rc`:
// hand it to the form (web Enter-to-submit) AND to your submit button
// (the universal trigger).
let name = signal(String::new());
let on_submit: std::rc::Rc<dyn Fn()> = {
    let name = name.clone();
    std::rc::Rc::new(move || log::info!("submit: {}", name.get()))
};

ui! {
    Form(on_submit = Some(on_submit.clone())) {
        TextInput(value = name.clone())
        Button(label = "Save", on_click = on_submit.clone())
    }
}
```

## Per-platform behavior

| Target | Mechanism |
| --- | --- |
| Web (wasm32) | A real `<form>` wrapping the inputs as DOM descendants. The native `submit` event is wired to `on_submit` after `preventDefault()` (idealyst apps don't POST form-encoded data — the browser must not navigate). Enter-to-submit and autofill work because the inputs are real DOM children. |
| iOS / Android | A plain passthrough container. There is no form `submit` event, so submission is fired by the author's submit button calling `on_submit`. (Return-key / IME-action submit is a *field-level* affordance and belongs on the input.) |
| Other targets | No-op `register`; the framework's `External` placeholder renders, making the missing binding obvious. |

## Why `on_submit` translates across platforms

It's a triggered **action** (a uniform closure), cleanly separated from
its **trigger** (platform-idiomatic) and its **data** (uniform signals).
The same closure compiles and runs everywhere; only what *fires* it
differs per platform.

## Why this is an SDK and not a core primitive

A form has no convergent cross-platform behavior to put behind the
Backend trait: on web `<form>` is a real element (submit-on-enter,
autofill, FormData), while iOS/Android have **no** form construct — their
form affordances live per-field on the inputs, not on a container. So
`Form` is an opinionated SDK on `Element::External`.

## Imperative submit

Bind a [`FormHandle`] to call [`submit`](FormHandle::submit)
programmatically:

```rust
let r: Ref<FormHandle> = /* … */;
ui! { Form(/* … */).bind(r) { /* … */ } }
// later:
r.with(|h| h.submit());
```

On web this calls `form.requestSubmit()` (runs constraint validation,
fires the same `submit` event). On native it's a no-op — invoke your
`on_submit` closure directly.

[`FormHandle`]: src/lib.rs
[`FormHandle::submit`]: src/lib.rs
