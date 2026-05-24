+++
title = "Styling and Theming"
order = 40
tags = ["style", "theme"]
+++

# Styling and Theming

Style in Idealyst is declared with the `stylesheet!` macro, theme-aware by construction, and applied to primitives via the `style` slot.

## Anatomy of a stylesheet

```rust
stylesheet! {
    pub primary_button<MyTheme> {
        base(theme) {
            padding: 8,
            border_radius: 6,
            background_color: theme.colors.primary,
        }
        variant size {
            default small(theme) { padding: 4 }
            large(theme) { padding: 12 }
        }
        transitions {
            background_color: 200ms EaseOut,
        }
        state pressed(theme) {
            background_color: darken(theme.colors.primary, 0.1),
        }
        state disabled(theme) {
            opacity: 0.5,
        }
    }
}
```

Key parts:
- `base(theme)` — the always-applied baseline.
- `variant <axis>` — N-way orthogonal options; one arm per value. `default` marks the implicit choice.
- `transitions { property: <duration> <easing> }` — per-property animated transitions.
- `state <name>(theme)` — overlay for one of the four interaction states: [[hovered]], [[pressed]], [[focused]], [[disabled]]. Other names are rejected at compile time.

## Theme

A theme is whatever struct you declare and pass to `install_theme(...)` before render. Reading `theme.colors.primary` inside `base(theme)` ties the stylesheet to the theme — switching themes reactively updates every styled primitive.

`install_theme` is **required before render** (see [[install_theme_required]]) — even a static, never-changing theme must be installed once.

## Applying styles

Inside `ui!`:

```rust
ui! {
    Button(label = "Save", style = primary_button(BUTTON_SIZE.with(Size::Large)))
}
```

The `style` prop accepts a stylesheet value (or any [[StyleSource]]). Per [[native_first_layout_for_web]], use stylesheet bindings for cross-platform chrome; the `.layout(...)` builder is a web-only escape hatch.

## Helpers

- [[parse_color]] — parse `#abc` / `rgba(…)` / named colors.
- [[lighten]], [[darken]] — adjust luminosity for hover/press overlays.
- [[current_breakpoint]] — read the active responsive breakpoint.
- [[active_theme]] — full theme handle.

## Per-platform notes

- iOS clamps `cornerRadius` against the layer's smaller dimension (see [[ios_cornerradius_unclamped]]). Don't over-specify.
- Gradients (`background_gradient`) work on every backend; radial gradient radius is closest-side scaled (`1.0` = edge midpoint).
