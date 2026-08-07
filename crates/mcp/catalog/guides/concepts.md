+++
title = "Concepts: Primitives, Components, Style"
order = 20
tags = ["intro", "core"]
+++

# Concepts

Idealyst is built on three orthogonal layers. Understanding the split is the single most useful piece of mental model:

## 1. Primitives — the structural skeleton

[[View]], [[Text]], [[Button]], [[ScrollView]], [[Image]], [[Icon]], [[TextInput]], [[Toggle]], [[Slider]], [[ActivityIndicator]], [[Video]], [[Link]], [[Portal]], [[Presence]], [[When]], [[Switch]], [[Repeat]], [[Graphics]], [[Virtualizer]].

These are leaf nodes. Each backend implements them natively — `View` is a `UIView` on iOS, a `FrameLayout` on Android, a `<div>` on web. **Authors never re-implement primitives** ([[backend_owns_rendering]]).

To extend the framework with new "primitive-like" things, register a payload handler on the scene `Registry` — the same mechanism the first-party primitives use (`runtime_vocabulary::register_builtins`). Registration happens at the boot seam (`register_scene_extensions`), and an unregistered payload panics at realize. The maps and webview SDKs are reference implementations; see the [[sdks]] guide.

## 2. Components — your reusable units

Mark a function with `#[component]` and you get a reusable unit you can drop into `ui!`/`jsx!`:

```rust
#[derive(Default, IdealystSchema)]
pub struct GreetingProps {
    pub name: String,
}

#[component]
pub fn Greeting(props: &GreetingProps) -> Element {
    let name = props.name.clone();
    ui! {
        // `{name}` interpolates by TYPE: a static prop bakes in, a
        // reactive one keeps the text live.
        text { "Hello, {name}!" }
    }
}

// elsewhere
ui! {
    Greeting(name = "Idealyst")
}
```

The `#[component]` macro:
- Rewrites the body for reactivity (signals capture into closures correctly).
- Emits the dispatch glue for use inside `ui!`: a `Greeting` props-type alias (same PascalCase name — dispatch is transform-free) plus a `BuildElement` impl.
- Registers a [[ComponentEntry]] into the MCP catalog so AI/tooling can discover it.

## 3. Style — orthogonal to structure

`stylesheet!` declares typed style descriptors with variants, transitions, and per-state overlays:

```rust
stylesheet! {
    pub MyButton<IdeaThemeRef> {
        base(t) {
            padding: t.spacing.sm(),
            background: t.intent.primary.solid_bg(),
        }
        state pressed(t) {
            background: t.intent.primary.fg(),
        }
    }
}
```

The `<…>` slot names the sheet's **token vocabulary** and the block binding (`base(t)`) *is* that vocabulary, so a theme token is a path the compiler checks — `t.spacing.sm()` references the `spacing-sm` token. The binding carries token *names*, not the theme's values: values arrive at resolve time from the token registry, which is what makes a theme swap one write per token instead of a re-mint. So there is no `theme.colors.primary` to read. A sheet with no vocabulary declares `<()>` and names tokens with `Tokenized::token("name", fallback)` — the right form for app-defined names. (The color field is `background` / `color`; there is no `background_color`.)

The four valid state names are [[hovered]], [[pressed]], [[focused]], and [[disabled]] (see `list_states`). Authors cannot add new state names — the cross-platform contract is fixed.

## Why the split matters

The renderer applies style via an independent `Effect` per primitive. A reactive content change doesn't re-fire the style effect, and vice versa. This is why styling never has to know about structure and components don't have to know about backend differences.
