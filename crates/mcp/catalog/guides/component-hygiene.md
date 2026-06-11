+++
title = "Component Hygiene"
order = 45
tags = ["components", "patterns", "best-practices"]
+++

# Component Hygiene

Components in this framework have one canonical shape. New or modified
components conform to it — these are the rules a generated component
should already follow, not a cleanup pass for later.

## Declare with `#[component]`

Always declare a component with the [[component]] attribute. It
generates the props struct's `BuildElement` impl, the
`pub type Tag = TagProps` alias that makes `Tag(...)` work as a
[[ui]] call site, and the `Default` glue struct-literal dispatch needs.
Don't hand-roll `BuildElement` impls, builder methods, or a
`pascal_to_snake` shim — if the macro can't express what you need, grow
the macro (`#[component(children)]` for containers,
`#[component(default(field = expr))]` for non-`Default` starts).

Name container fns PascalCase to match the tag (`fn Card`, not
`fn card`). The fn-call form `Card(props)` and the struct-literal form
`ui! { Card(...) }` both work.

## Render with `ui!` (or `jsx!`)

Compose the tree with [[ui]]. [[jsx]] is a fine peer — pick one per
file and stay in it; don't mix `ui!` / `jsx!` / hand-built `Element` in
one component without a documented reason.

**Primitives are lowercase, components are PascalCase — strictly.**
The leaf primitives ([[view]], [[text]], `button`, `image`, `icon`,
`text_input`, `scroll_view`, `slider`, `toggle`, …) are snake_case
*only*. A PascalCase tag always routes to `#[component]` dispatch, which
is what lets a library define a component named `Image` / `Link` /
`Toggle` without the primitive shadowing it. Mirrors React's `<div>` vs
`<MyButton>`.

## Build children inside the macro

Don't assemble a `Vec<Element>` outside the macro and splat it in to
populate a parent. `ui!` supports `for item in items { … }` iteration,
`if` / `if let` / `match` branches, and bare-identifier child splats —
use those.

```rust
// NO — children built ad-hoc outside the macro
let mut kids = Vec::new();
kids.push(ui! { Foo() });
kids.push(ui! { Bar() });
ui! { view { kids } }

// YES — children live where they're rendered
ui! {
    view {
        Foo()
        Bar()
        for item in items {
            Row(label = item.name)
        }
    }
}
```

The out-of-macro push loop defeats keyed reconciliation, hides children
from reactive-scope inference, and obscures the tree. The one legitimate
`Vec<Element>` shape is a container component that accepts
`children: Vec<Element>` as a prop and flattens incoming fragments with
[[children]] / `ChildList::append_to` before splatting — that's about
flattening *received* children, not authoring new ones in a loop.

## Conditionals and iteration belong inside the macro

If a child only sometimes appears, express it with `if` / `if let` /
`match` *inside* `ui!` — not by conditionally pushing into a `Vec`
before the macro call. Same for iteration: `for … { … }` inside the
macro is the standard form.

## Effects: prefer `effect!` over a bare `Effect::new`

Inside a component body, write side effects with [[effect]], not a bare
`Effect::new(...)` whose handle you bind by hand. The macro inserts the
`move ||`, tracks dependencies by what the body reads (no deps array),
and binds the handle so it adopts the surrounding scope. Reach for
`Effect::new` directly only when you genuinely need to *hold* the handle
(store it, `.persist()` it). See [[reactivity]].

```rust
let count = signal!(0);
effect!({
    log::info!("count is {}", count.get());   // re-runs when count changes
});
```

Pair with `on_cleanup(...)` for teardown — it fires before the next
re-run and on disposal. Don't reach for `mem::forget` to keep an effect
alive; that's what scope adoption and `.persist()` are for.

## Optional callbacks: bind only when present

For `Option<Rc<dyn Fn()>>` props, attach the handler conditionally
rather than wiring an unconditional closure that silently no-ops on
`None`:

```rust
if let Some(cb) = on_press {
    bound = bound.on_press(move || (cb)());
}
```

A silent no-op handler can block hit-test fall-through on some backends
and confuses event-routing assertions.

## Promote a helper to a component when it grows

A snake_case `fn xyz() -> Element` is fine as a one-off, prop-less,
single-call-site helper. The moment it takes a parameter, gets a second
call site, or grows variants, promote it to a `#[component]` so call
sites use struct-literal syntax (`Phones(variant = …)`) instead of
positional arguments. Don't grow positional helper signatures.

## Catch drift automatically: `idealyst lint`

Several of the rules above are machine-checkable, and the `idealyst lint`
command checks them over your source:

- `prefer-signal-macro` / `prefer-effect-macro` / `prefer-memo-macro` —
  flags raw `Signal::new` / `Effect::new` / `memo(…)` where the
  `signal!` / `effect!` / `memo!` macro is intended (the macro anchors the
  handle to the owning scope; the raw call is the "my signal stopped
  updating" footgun).
- `prefer-ui-macro` — flags elements built by hand (`builder::…`,
  `BuildElement::build`, `Element::View { … }`) instead of `ui!` / `jsx!`.
- `component-pascal-case` — flags a `#[component]` fn that isn't
  PascalCase.

Every rule is individually configurable (`off` / `warn` / `error`) in
`idealyst-lint.toml` and suppressible inline with
`// idealyst-lint-disable-next-line <rule>`. Run `idealyst lint --rules`
to list them. The same engine drives a rust-analyzer
`check.overrideCommand` so the findings appear as inline editor squiggles
— see `crates/tools/lint/README.md`. For a build that must never compile a
misnamed component, the `strict-naming` Cargo feature turns
`component-pascal-case` into a hard `compile_error!`.

See [[components]] for the component model and [[reactivity]] for the
state and effect APIs the rules above lean on.
