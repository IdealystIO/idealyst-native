+++
title = "Concepts: Primitives, Components, Style"
order = 20
tags = ["intro", "core"]
+++

# Concepts

Idealyst is built on three orthogonal layers. Understanding the split is the single most useful piece of mental model:

## 1. Primitives — the structural skeleton

[[View]], [[Text]], [[Button]], [[ScrollView]], [[Image]], [[Icon]], [[TextInput]], [[Toggle]], [[Slider]], [[ActivityIndicator]], [[Video]], [[Link]], [[Portal]], [[Presence]], [[When]], [[Switch]], [[Repeat]], [[External]], [[Graphics]], [[Virtualizer]].

These are leaf nodes. Each backend implements them natively — `View` is a `UIView` on iOS, a `FrameLayout` on Android, a `<div>` on web. **Authors never re-implement primitives** ([[backend_owns_rendering]]).

To extend the framework with new "primitive-like" things, register an [[External]] kind via the per-backend `ExternalRegistry` (see [[third_party_extension]]). The maps and webview SDKs are reference implementations.

## 2. Components — your reusable units

Mark a function with `#[component]` and you get a reusable unit you can drop into `ui!`/`jsx!`:

```rust
#[component]
pub fn greeting(name: &str) -> Primitive {
    ui! {
        Text(text_fmt!("Hello, {}!", name))
    }
}

// elsewhere
ui! {
    Greeting(name = "Idealyst")
}
```

The `#[component]` macro:
- Rewrites the body for reactivity (signals capture into closures correctly).
- Emits a sibling `greeting!` invocation macro for use inside `ui!`.
- Registers a [[ComponentEntry]] into the MCP catalog so AI/tooling can discover it.

## 3. Style — orthogonal to structure

`stylesheet!` declares typed style descriptors with variants, transitions, and per-state overlays:

```rust
stylesheet! {
    pub button_style<MyTheme> {
        base(theme) {
            padding: 8,
            background_color: theme.colors.primary,
        }
        state pressed(theme) {
            background_color: darken(theme.colors.primary, 0.1),
        }
    }
}
```

The four valid state names are [[hovered]], [[pressed]], [[focused]], and [[disabled]] (see `list_states`). Authors cannot add new state names — the cross-platform contract is fixed.

## Why the split matters

The renderer applies style via an independent `Effect` per primitive. A reactive content change doesn't re-fire the style effect, and vice versa. This is why styling never has to know about structure and components don't have to know about backend differences.
