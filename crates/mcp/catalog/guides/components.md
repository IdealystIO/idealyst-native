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

Dispatch is transform-free: the call site `Greeting` resolves to the verbatim `Greeting!` invocation macro — no case conversion. The function name, its invocation macro, and the `ui!`/`jsx!` call site are all the same PascalCase identifier ([[ui_naming_convention]]). `#[component]` suppresses the `non_snake_case` lint so PascalCase fns don't warn.

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
pub fn Card(props: &CardProps) -> Element {
    ui! {
        View {
            Text(props.title)
            // ...
        }
    }
}
```

The MCP catalog inlines the schema's per-field docs/constraints into the component's `params` output, so consumers get the full prop API in one place.

### Prop coercion — never write `.into()` at the call site

The invocation macro coerces every prop value into its field type via `.into()`. So you pass the bare value and the macro does the conversion:

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
- Every [[MethodEntry]] from a `methods!` block.
- Every [[AnimationEntry]] from `animated!(...)` calls.

This metadata is what makes the catalog actionable for AI authoring. The more thoroughly you doc-comment your components, the better the catalog gets — without you having to maintain a separate docs site.
