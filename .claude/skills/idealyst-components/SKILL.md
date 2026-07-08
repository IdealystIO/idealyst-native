---
name: idealyst-components
description: How to implement an idea-ui component idiomatically in THIS repo — the #[props] + #[component] + ui! shape, signature choice, children flattening, reactive routing, and tests. Use when writing a new component, modifying one in crates/ui/idea-ui/src/components/, or reviewing whether a component conforms to the canonical shape.
---

# Implementing an idea-ui component

Every component in `crates/ui/idea-ui/src/components/` follows **one** canonical
shape. This skill is the practical checklist for writing or reviewing one. The
binding rules live in `CLAUDE.md` §9 (Component implementation standards) — this
skill is how to satisfy them, not a replacement for them.

Authoritative long-form reference (kept in sync, also served by the MCP):
[`crates/mcp/catalog/guides/idiomatic-components.md`](../../../crates/mcp/catalog/guides/idiomatic-components.md).
Read it for the full worked template; use the files below as ground truth.

## Canonical reference files (read these, don't guess)

| Pattern | File |
| --- | --- |
| Leaf, single reactive prop routed to a style sink | `crates/ui/idea-ui/src/components/divider.rs` |
| Container: owned props + `ChildList::append_to` flatten | `crates/ui/idea-ui/src/components/card.rs`, `stack.rs`, `center.rs` |
| Structural `switch(...)` rebuild + controlled `Signal` | `crates/ui/idea-ui/src/components/checkbox.rs` |
| Full reactive fast-path split (static vs live, slot overrides) | `crates/ui/idea-ui/src/components/button.rs` |
| `#[props]` macro behavior (what wraps / what's skipped) | `crates/runtime/macros/src/props_attr.rs` |
| `#[component]` args + signature rules | `crates/runtime/macros/src/component_attr.rs`, `invocation_macro.rs` |

## The shape, in order

**1. Props struct — `#[props]` ABOVE the derives.**

```rust
#[runtime_core::props]           // rewrites data fields T → Reactive<T>
#[derive(Default, IdealystSchema)]
pub struct FooProps {
    /// Doc comment on EVERY field (IdealystSchema records it for the catalog).
    pub label: String,           // → Reactive<String>
    pub tone: ToneRef,           // → Reactive<ToneRef>
    pub on_change: Rc<dyn Fn(bool)>,   // handler — left alone
    pub children: Vec<Element>,        // children — left alone
    #[prop(static)]
    pub style: Option<Rc<StyleSheet>>, // forced bare (non-reactive override)
}
```

`#[props]` skips handlers (`Rc`/`Arc`/`Box<dyn Fn>`), children (`Element`/`Vec`/
`ChildList`), imperative handles (`Ref`/`Bound`/`Bindable`/`RefFill`/`Action`),
and already-reactive sources (`Signal`/`Reactive`/`Rx`). `Option<Inner>` is
looked through. Overrides: `#[prop(static)]` forces bare `T`; `#[prop(reactive)]`
forces the wrap. Hand-write `impl Default` when a field can't derive it.

**2. The fn — exactly one param, signature picks leaf vs container.**

- Leaf/passive → `#[component] pub fn Foo(props: &FooProps) -> Element`.
- Container → `#[component(children)] pub fn Foo(props: FooProps) -> Element`,
  then flatten:
  ```rust
  let mut children: Vec<Element> = Vec::with_capacity(props.children.len());
  for c in props.children { ChildList::append_to(c, &mut children); }
  ui! { view(style = s) { children } }
  ```
- Other args: `#[component(default(field = expr))]`, `#[component(external)]`.

**3. Body — `ui!`. Primitives lowercase, components PascalCase, strictly.**
Build children inside the macro (`for … , key = expr { }`, `if`/`match`, splats).
Route a reactive prop via `match props.x.clone() { Reactive::Static(v) => sink.x(v),
dynamic => sink.x(derived(move || dynamic.get())) }`, or `switch(scrutinee, arm)`
for a structural rebuild — always read `.get()` INSIDE the closure so the effect
subscribes. Optional callbacks: bind only when `Some` (never an unconditional
no-op closure).

**4. Tests — non-negotiable (CLAUDE.md §1, §8).**
`install_idea_theme(light_theme())`, build the component, `match` the returned
`Element` and assert on `resolve_style(...)`. Bug fixes ship a regression test
named after the bug. Run `cargo test -p idea-ui`.

## Reviewing a component

Walk the checklist at the bottom of the reference guide. The machine-checkable
subset is caught by `idealyst lint` (`prefer-signal-macro`, `prefer-ui-macro`,
`component-pascal-case`, …) — run it before claiming conformance.

## When you edit a component, keep docs aligned (CLAUDE.md §2)

If you change a component's behavior or its canonical shape, update
`crates/mcp/catalog/guides/idiomatic-components.md` and `component-hygiene.md` in
the same change — they're the served docs and drift silently otherwise.
