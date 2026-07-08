+++
title = "Implementing a Component"
order = 27
tags = ["components", "patterns", "reactivity", "best-practices"]
+++

# Implementing a Component

This is the end-to-end shape of a real idea-ui component — the template
every component in `crates/ui/idea-ui/src/components/` follows. [[components]]
covers what the `#[component]` macro records; this covers what *you* write.

Three pieces, always in this order:

1. a **props struct** wrapped with `#[props]` (reactive-by-default),
2. the **`#[component]` fn** — `&Props` for a leaf, owned `Props` for a
   container,
3. a **body** composed with [[ui]], plus tests.

## The full skeleton

```rust
use runtime_core::{component, ui, ChildList, Element, IdealystSchema, Reactive};

// `#[props]` MUST sit ABOVE the derives — it rewrites the field types
// before `IdealystSchema` / `Default` see them.
#[runtime_core::props]
#[derive(Default, IdealystSchema)]
pub struct BadgeProps {
    /// The text shown in the badge. `#[props]` rewrote this to
    /// `Reactive<String>`, so a call site may pass a literal, a
    /// `Signal<String>`, or `rx!(…)`.
    pub label: String,
    /// Semantic palette. Also became `Reactive<Tone>`.
    pub tone: Tone,
}

/// A small pill that labels or counts something.
#[component]
pub fn Badge(props: &BadgeProps) -> Element {
    let label = props.label.clone();
    ui! {
        text(style = badge_style(props.tone.clone())) { label }
    }
}
```

The call site never restates the wrapping — pass bare values, dispatch
coerces via `.into()`:

```rust
ui! {
    Badge(label = "New", tone = tone::Primary)   // static
    Badge(label = count_text, tone = tone::Danger) // count_text: Signal<String> → live
}
```

## 1. The props struct

### `#[props]` makes every data field reactive-by-default

`#[props]` (spell it `#[runtime_core::props]` when `runtime_core` isn't
glob-imported) rewrites each scalar **data** field `T` → `Reactive<T>`, so
a `ui!` call site can hand it a `Signal`/`rx!` and it carries through live —
without you hand-wrapping every field. A bare (non-signal) value stays a
zero-cost `Static` snapshot on the build-time fast path.

It **skips** shapes that aren't reactive data:

| Skipped | Why |
| --- | --- |
| `Rc`/`Arc`/`Box<dyn Fn…>`, bare `fn` | handlers, not sink-consumed data |
| `Element`, `Vec<…>`, `ChildList` | children have their own reactivity |
| `Ref`, `Bound`, `Bindable`, `RefFill`, `Action` | imperative handles |
| `Signal`, `Reactive`, `Rx` | already reactive (idempotent — never double-wraps) |

`Option<Inner>` is looked through: `Option<String>` →
`Reactive<Option<String>>`, but `Option<Rc<dyn Fn…>>` is left alone.

### Per-field overrides

- `#[prop(static)]` forces a bare `T` — use it for a genuinely build-time
  value or a non-`Clone` type (e.g. a slot-override `Option<Rc<StyleSheet>>`).
- `#[prop(reactive)]` forces the wrap when the heuristic misses (e.g. a
  type alias hiding a data type, or a `Vec` you deliberately want live).

Both attributes are stripped before the struct is re-emitted.

```rust
#[runtime_core::props]
#[derive(IdealystSchema)]
pub struct ButtonProps {
    pub label: String,                     // → Reactive<String>
    pub tone: ToneRef,                     // → Reactive<ToneRef>
    pub on_click: Rc<dyn Fn()>,            // left alone (handler)
    pub children: Vec<Element>,            // left alone (children)
    #[prop(static)]
    pub style: Option<Rc<StyleSheet>>,     // stays bare (non-Clone-friendly override)
}
```

### `Default`

Struct-literal dispatch needs a `Default`. Derive it when every field is
`Default`-able; otherwise hand-write `impl Default` (e.g. to seed
`tone: Reactive::Static(ToneRef::default())` or a non-`Default` starting
value). See `ButtonProps`/`CardProps` in the source for the hand-written form.

### `IdealystSchema` records the prop API for the catalog

Add `#[derive(IdealystSchema)]` and put a `///` doc comment on every field —
that derive is what flows the per-field docs (and any
`#[schema(constraint = "…")]` hint) into the MCP catalog so
`describe_component` returns the full prop surface. Without it, the docs stay
in your source but never reach tooling.

## 2. The `#[component]` fn — signature tells the story

`#[component]` takes **exactly one parameter** and emits the dispatch glue:
the `pub type Name = NameProps` tag alias, the `BuildElement` impl, and an
`#[allow(non_snake_case)]` so the PascalCase fn name doesn't warn.

Pick the signature by whether the component owns children:

**Leaf / passive** — takes `props: &FooProps` (by reference). This is the
common case (Button, Checkbox, Divider, Badge):

```rust
#[component]
pub fn Divider(props: &DividerProps) -> Element { /* … */ }
```

**Container** — declare `#[component(children)]` and take **owned**
`props: FooProps`, so you can move `children: Vec<Element>` out and flatten
incoming fragments with `ChildList::append_to` before rendering (Card,
Center, Stack):

```rust
#[component(children)]
pub fn Card(props: CardProps) -> Element {
    let mut children: Vec<Element> = Vec::with_capacity(props.children.len());
    for c in props.children {
        ChildList::append_to(c, &mut children);
    }
    ui! { view(style = card_style()) { children } }
}
```

`ChildList::append_to` flattens a fragment (a child that is itself a list)
into flat siblings — this is the ONE legitimate `Vec<Element>` you build by
hand. Authoring *new* children in a push loop is not (see [[component-hygiene]]).

Other `#[component(…)]` args: `default(field = expr, …)` for per-field
defaults the invocation fills in, and `external` / `external(tag = "…")` to
mark the component for `idealyst export` (Web Component generation).

## 3. The body — compose with `ui!`

**Primitives are lowercase, components are PascalCase — strictly.** The leaf
primitives (`view`, `text`, `button`, `image`, `icon`, `text_input`,
`scroll_view`, `slider`, `toggle`, `link`, `overlay`, `presence`, `flat_list`,
`graphics`, …) are snake_case *only*. A PascalCase tag *always* routes to
`#[component]` dispatch — which is what lets a library define a component
named `Image`/`Link`/`Toggle` without the primitive shadowing it. Mirrors
React's `<div>` vs `<MyButton>`.

Build children **inside** the macro — `for`, `if`/`if let`/`match`, and
bare-identifier splats all work there:

```rust
ui! {
    view(style = list_style()) {
        if let Some(title) = header {
            text(style = header_style()) { title }
        }
        for row in rows {
            Row(label = row.name, tone = row.tone)
        }
    }
}
```

For a **reactive** `for` over a `Signal<Vec<_>>`, add a compile-time key so
reconciliation is stable: `for row in rows, key = row.id { … }`.

### Routing a reactive prop into its effect

Because `#[props]` made data fields `Reactive<T>`, a value can be static or
live. The idiomatic routing keeps the **static fast path** while letting a
live value re-run in place. Two shapes cover almost everything:

**Style-variant sink** (Divider, Stack) — a static value takes the build-time
path; a live one is passed through `derived(...)` so the builder emits a
reactive style source. Reading `.get()` *inside* the closure is what
subscribes the apply-style effect:

```rust
let style = match props.axis.clone() {
    Reactive::Static(axis) => DividerStyle().axis(axis),
    dynamic => DividerStyle().axis(derived(move || dynamic.get())),
};
ui! { view(style = style) {} }
```

**Structural rebuild** (Checkbox) — when a prop change swaps *which* elements
render (not just their style), wrap the subtree in `switch(scrutinee, arm)`:
the scrutinee closure reads `.get()` (subscribing), and the arm rebuilds when
its value changes:

```rust
let glyph = runtime_core::switch(
    move || value.get(),
    move |on: &bool| {
        if !*on { return ui! { view {} }.into_element(); }
        ui! { text { "✓" } }.into_element()
    },
);
```

> Depth note: the reactive **fast-path split** (static vs live, per-node
> foreground re-resolution, layered `with_computed` style, slot overrides) is
> where the hard components earn their length. When you need it, read
> `button.rs` and `card.rs` in `crates/ui/idea-ui/src/components/` — they're
> the fully-commented reference. Most components don't need more than the two
> shapes above.

### Optional callbacks: bind only when present

For an `Option<Rc<dyn Fn()>>`, attach the handler conditionally — an
unconditional closure that no-ops on `None` can block hit-test fall-through
on some backends:

```rust
if let Some(cb) = on_press {
    bound = bound.on_press(move || (cb)());
}
```

## 4. Tests

Every component ships with tests, and every bug fix ships with a regression
test named after the bug. Install a theme, build the component, and assert on
its resolved output by matching the returned `Element`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use idea_theme::theme::{install_idea_theme, light_theme};
    use runtime_core::{resolve_style, AlignItems, StyleSource};

    #[test]
    fn row_align_center_resolves_to_align_items_center() {
        install_idea_theme(light_theme());
        let el = Stack(StackProps {
            axis: Reactive::Static(StackAxis::Row),
            align: Reactive::Static(StackAlign::Center),
            ..Default::default()
        });
        let app = match el {
            Element::View { style: Some(StyleSource::Static(a)), .. } => a,
            _ => panic!("Stack renders a statically-styled View"),
        };
        assert_eq!(resolve_style(&app).align_items, Some(AlignItems::Center));
    }
}
```

Note the props are constructed with `Reactive::Static(...)` in tests — the
struct literal is post-`#[props]`, so the fields are the wrapped types.

## Checklist

- [ ] `#[runtime_core::props]` sits **above** the derives.
- [ ] Data fields are bare (`#[props]` wraps them); handlers/children/refs
      are left as-is; non-reactive fields carry `#[prop(static)]`.
- [ ] `#[derive(IdealystSchema)]` + a `///` on every field.
- [ ] A `Default` (derived or hand-written).
- [ ] Leaf → `props: &Props`; container → `#[component(children)]` + owned
      `props: Props` + a `ChildList::append_to` flatten loop.
- [ ] Body is `ui!`; primitives lowercase, components PascalCase; children
      built inside the macro.
- [ ] Reactive props routed via the `Reactive::Static … / derived(...)` sink
      or `switch(...)`, reading `.get()` inside the closure.
- [ ] Tests present; bug fixes carry a named regression test.

See [[component-hygiene]] for the DO/DON'T rules, [[reactivity]] for signals
and effects, and [[styling]] / [[theming]] for the stylesheet surface these
components lean on.
