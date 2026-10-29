+++
title = "App stylesheets with theme tokens"
summary = "When an app writes its own stylesheet! (and when it shouldn't), the <IdeaThemeRef> token vocabulary, variants/states/breakpoints for app UI, and the premint-safe forms to use."
when = "Styling app-specific UI that idea-ui has no component for — a shell bar, a pane, a custom tile — or reviewing app styles."
order = 65
tags = ["stylesheets", "tokens", "style", "variants", "breakpoints", "premint"]
+++
# App stylesheets with theme tokens

## When to write one

1. An idea-ui component does it → use the component (no sheet).
2. A component does it but looks wrong for the brand → restyle it in
   `brand.rs` (read_design_skill("brand")).
3. It's layout glue or app-specific chrome (a top bar, a split-view pane,
   a dashboard tile frame) → a `stylesheet!` in the module that uses it.

## The shape

```rust
use idea_ui::IdeaThemeRef;
use runtime_core::{stylesheet, FlexDirection, Length};

stylesheet! {
    /// What the sheet styles, in one line.
    TopBar<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            gap: t.spacing.sm(),
            padding_horizontal: t.spacing.md(),
            background: t.color.surface(),
            border_bottom_width: 1.0,
            border_bottom_color: t.color.border(),
        }
        variant elevated {
            #[default]
            off(_t) {}
            on(t) { shadow: runtime_core::Shadow { x: 0.0, y: 2.0, blur: 8.0, color: t.effect.shadow() } }
        }
        breakpoint md(t) { padding_horizontal: t.spacing.xl() }
    }
}

ui! { view(style = TopBar().elevated(rx!(if scrolled.get() { TopBarElevated::On } else { TopBarElevated::Off }))) { … } }
```

- **`<IdeaThemeRef>` makes `t` the typed token vocabulary**:
  `t.color.*()`, `t.intent.<intent>.<slot>()`, `t.palette.<hue>.<slot>()`,
  `t.spacing.*()`, `t.radius.{sm,md,lg,pill}()`, `t.effect.*()`,
  `t.typography.*_size()`. A typo fails to compile; a string token name
  would silently render its fallback.
- **State that changes the look is a `variant`**, driven by a signal at
  the call site (`.axis(rx!(…))`) — not a closure that rebuilds rules.
- **Size changes are `breakpoint` blocks**, not `current_breakpoint()`
  branches, when only style changes.
- **`state hovered` / `pressed` / `focused`** for interaction feedback on
  custom pressables — and remember hover never fires on touch.

## Rules

- **Tokens only** — no hex, no font sizes. Fixed `Length::Px` only for
  layout constraints (pane widths, icon boxes), never for spacing.
- **Name sheets after what they are** (`ListPane`, `TopBar`), not what they
  look like (`GrayBox`).
- **No `StyleApplication::with_computed(…)` / per-render `StyleRules`** in
  app code: they can't premint and panic in `--premint-only` builds.
  Everything expressible as base/variant/state/breakpoint goes in the
  sheet; a truly continuous value (a measured width) uses the slot's
  inline layer.
- **Color on text nodes**, not on containers — native backends don't
  inherit text color (read_design_skill("platforms")).
- **One sheet per role**, reused; don't clone a sheet to change one field —
  add a variant.
