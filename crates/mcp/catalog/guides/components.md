+++
title = "Authoring Components"
order = 25
tags = ["components", "core"]
+++

# Authoring Components

`#[component]` turns a free function into a reusable composable. The macro handles three things at once: it rewrites the body for reactivity, generates a sibling `name!` invocation macro you can use inside `ui!`/`jsx!`, and registers a [[ComponentEntry]] in the MCP catalog so AI/tooling can discover it.

## A minimal component

```rust
use runtime_core::*;

#[component]
pub fn greeting(name: &str) -> Primitive {
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

The macro converts the call-site `Greeting` (PascalCase) to `greeting` (snake_case) to find the per-component invocation macro. Function names are snake_case; call sites are PascalCase ([[ui_naming_convention]]).

## Props structs and `IdealystSchema`

For richer prop metadata — per-field docs, constraints — declare a props struct and derive `IdealystSchema`:

```rust
#[derive(IdealystSchema)]
pub struct CardProps<'a> {
    /// Card title displayed in the header.
    pub title: &'a str,
    /// Optional secondary line.
    #[schema(constraint = "max 80 chars")]
    pub subtitle: Option<&'a str>,
}

#[component]
pub fn card(props: &CardProps) -> Primitive {
    ui! {
        View {
            Text(props.title)
            // ...
        }
    }
}
```

The MCP catalog inlines the schema's per-field docs/constraints into the component's `params` output, so consumers get the full prop API in one place.

## Methods — imperative handles

When a parent needs to imperatively poke a child (`.focus()`, `.scroll_to_top()`, `.reset()`), declare a `methods!` block:

```rust
#[component]
pub fn counter(props: &CounterProps) -> Bindable<CounterHandle> {
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
pub fn fader(props: &FaderProps) -> Primitive {
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
- Every [[MethodEntry]] from a `methods!` block.
- Every [[AnimationEntry]] from `animated!(...)` calls.

This metadata is what makes the catalog actionable for AI authoring. The more thoroughly you doc-comment your components, the better the catalog gets — without you having to maintain a separate docs site.
