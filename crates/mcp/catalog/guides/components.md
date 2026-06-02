+++
title = "Authoring Components"
order = 25
tags = ["components", "core"]
+++

# Authoring Components

`#[component]` turns a free function into a reusable composable. The macro handles three things at once: it rewrites the body for reactivity, generates the dispatch glue `ui!`/`jsx!` use (a `Name` tag alias plus a `BuildElement` impl on its props — no per-component `macro_rules!`), and registers a [[ComponentEntry]] in the MCP catalog so AI/tooling can discover it.

## A minimal component

```rust
use runtime_core::*;

#[component]
pub fn Greeting(name: &str) -> Element {
    ui! {
        Text(text_fmt!("Hello, {}!", name))
    }
}
```

Inside `ui!` elsewhere:

```rust
ui! {
    View {
        Greeting(name = "World")
    }
}
```

Dispatch is transform-free: the call site `Greeting` resolves to the verbatim `Greeting` props-type alias `#[component]` emits — no case conversion. The function name, that tag-alias type, and the `ui!`/`jsx!` call site are all the same PascalCase identifier ([[ui_naming_convention]]). `#[component]` suppresses the `non_snake_case` lint so PascalCase fns don't warn.

## Documenting components and props

Documentation is plain Rust doc comments — there's no separate doc format.

- **Component docs**: a `///` doc comment on the `#[component]` function. It flows to the component's catalog entry and is what `describe_component` returns + what `list_components` shows as the one-line `summary`.
- **Prop docs**: a `///` on each field of the props struct. These are captured **only when the struct derives `IdealystSchema`** — that derive is what records the per-field docs (and optional `#[schema(constraint = "…")]` hints) into the catalog. Without it, the prop docs stay in your source but never reach the catalog.

```rust
use runtime_core::{component, ui, Element, IdealystSchema};

/// Props for the [`Card`] component.
#[derive(Default, IdealystSchema)]
pub struct CardProps {
    /// Card title displayed in the header.
    pub title: String,
    /// Optional secondary line under the title.
    #[schema(constraint = "max 80 chars")]
    pub subtitle: Option<String>,
}

/// A titled surface that groups related content.
///
/// Lines after the first become the component's full docs; the first
/// line is its catalog `summary`.
#[component]
pub fn Card(props: &CardProps) -> Element {
    let title = props.title.clone();
    ui! {
        view() {
            text() { title }
            // ...
        }
    }
}
```

Notes:
- `#[derive(Default)]` is already required by `#[component]` dispatch — just add `IdealystSchema` alongside it.
- `#[schema(constraint = "…")]` is an optional free-form hint (e.g. a range, a max length); it surfaces as the field's `constraint`.
- The MCP catalog inlines the schema's per-field docs/constraints into the component's `params[].schema`, so `describe_component` returns the full prop API — docs, types, and constraints — in one place.

### Enforcing docs at compile time — the `strict-docs` feature

Turning on the `strict-docs` feature (`runtime-core/strict-docs`, forwarded to the macros) makes the **compiler** reject undocumented surface:

- a `#[component]` with no `///` →
  `error: component \`Card\` is missing documentation. Add a \`///\` doc comment …`
- a field of an `#[derive(IdealystSchema)]` struct (or an enum variant) with no `///` →
  `error: prop \`subtitle\` is missing documentation. Add a \`///\` doc comment …`

This is a deliberately strict gate — not the everyday workflow — for catching gaps at build time instead of in a later catalog audit. It's off by default; enable it per build (`cargo build --features runtime-core/strict-docs`, or expose your own `strict-docs = ["runtime-core/strict-docs"]` passthrough) — typically in CI.

Two things to know:
- It's independent of `mcp`: you can enforce docs without building a catalog. (The `IdealystSchema` derive resolves under either feature.)
- Because Cargo unifies features across the whole build, `strict-docs` applies to **every** macro expansion in the dependency graph — including component libraries you depend on. Enable it when your whole component graph is documented; a dependency with an undocumented component will fail the build too.

### Prop coercion — never write `.into()` at the call site

The DSL coerces every prop value into its field type via `.into()` (it builds the props struct literal for you). So you pass the bare value and dispatch does the conversion:

```rust
ui! {
    Typography(content = "Hello", kind = typography_kind::H1)   // not H1.into()
    Badge(label = label_string, tone = tone::Primary)            // not tone::Primary.into()
}
```

Writing `.into()` yourself (`kind = typography_kind::H1.into()`) is a compile error — it double-converts (`(H1.into()).into()`), which is ambiguous. Pass the value bare.

### Reactive props — live text with `Reactive<T>` and `rx!`

A prop typed `Reactive<T>` can carry either a static value or a live one, decided by the value's TYPE (no `.get()` scanning):

```rust
pub struct LabelProps {
    pub content: Reactive<String>,   // static OR live
}
```

```rust
ui! {
    Typography(content = "Static")                       // From<&str>  → static
    Typography(content = title_signal)                   // From<Signal> → live
    Typography(content = rx!(format!("{}×", count.get()))) // rx!         → live (computed)
}
```

`rx!(expr)` wraps a computed expression as a live `Reactive`; a bare `Signal<String>` is live too. The component renders it with plain `text(props.content.clone())` — a live value re-paints just that text node when its signals change, with no parent rebuild. idea-ui's `Typography` (`content`), `Button`/`Badge`/`Tag` (`label`), and `Alert` (`title`) all take `Reactive<String>`.

## Methods — imperative handles

When a parent needs to imperatively poke a child (`.focus()`, `.scroll_to_top()`, `.reset()`), declare a `methods!` block:

```rust
#[component]
pub fn Counter(props: &CounterProps) -> Bindable<CounterHandle> {
    let value = props.value;
    methods! {
        /// Reset the counter to zero.
        fn reset(&self) { value.set(0); }
        /// Bump the counter by `n`.
        fn bump_by(&self, n: i32) { value.update(|v| *v += n); }
    }
    ui! { /* ... */ }
}
```

The macro generates a sibling `CounterHandle` type with the methods as accessors, and emits one [[MethodEntry]] per method into the MCP catalog. v1 limit: methods must return `()`.

## Animations

`animated!(...)` declares an [[AnimatedValue]] in the component body. The MCP walker captures these so the catalog reflects every animation the component owns:

```rust
#[component]
pub fn Fader(props: &FaderProps) -> Element {
    let opacity = animated!(0.0_f32);
    // ... animate opacity, bind it into a style ...
}
```

## What gets recorded in the catalog

For every `#[component]`:
- The function's name, module path, file/line.
- Doc comments (concatenated, one entry).
- Parameter list (names + pretty-printed types).
- The `composes` graph — every `Foo` / `<Foo>` you reference inside `ui!`/`jsx!`.
- Per-prop docs/types/constraints, when the props struct derives `IdealystSchema` — inlined into `params[].schema`.
- Every [[MethodEntry]] from a `methods!` block.
- Every [[AnimationEntry]] from `animated!(...)` calls.

This metadata is what makes the catalog actionable for AI authoring. The more thoroughly you doc-comment your components, the better the catalog gets — without you having to maintain a separate docs site.
