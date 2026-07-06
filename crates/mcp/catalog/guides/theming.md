+++
title = "Theming idea-ui"
order = 45
tags = ["theme", "style"]
+++

# Theming idea-ui

Every idea-ui component reads its colors, spacing, radii, and type scale from a
**theme** — one plain struct of design tokens. Customizing the look of the whole
component set is done by installing a theme, not by styling components one by one.
This guide is the idea-ui counterpart to the framework-level [[styling]] guide:
`styling` covers the `stylesheet!` macro and the generic `install_theme`; this one
covers the concrete idea-ui theme API (`install_idea_theme`, `light_theme`,
`app_theme!`, …).

## The theme is one struct

`IdeaThemeDefaults` holds every token idea-ui components consume, as public fields:

- `colors` — non-intent neutrals: `background`, `surface`, `surface_alt`, `text`,
  `text_muted`, `text_inverse`, `border`, `border_hover`, `border_strong`,
  `focus_ring`, `overlay`.
- `intents` — the seven semantic palettes (`primary`, `secondary`, `neutral`,
  `success`, `danger`, `warning`, `info`), each an `IntentColors` with six slots
  (`solid_bg`, `solid_text`, `soft_bg`, `soft_text`, `fg`, `border`).
- `spacing` — `xs`/`sm`/`md`/`lg`/`xl`/`xxl` (`f32`).
- `radius` — `sm`/`md`/`lg`/`pill` (`f32`).
- `typography` — per-variant font sizes (`body_size`, `h1_size`, …), all `f32`.
- `font` — the default body `FontFamily`.

`light_theme()` and `dark_theme()` each return a fully populated
`IdeaThemeDefaults` you can use as-is or as a starting point.

## Install a theme — required before render

A theme must be installed **once, before the first render**, even if it never
changes. Nothing renders correctly until it is.

```rust
use idea_ui::{install_idea_theme, light_theme};

install_idea_theme(light_theme());
```

`install_idea_theme` also installs the default component stylesheets, so this one
call is the entire setup for an app that doesn't need custom modifiers.

## Tweak individual tokens

Mutate the fields you care about on a built-in theme, then install it. Color
fields are `Tokenized<Color>`; scales are plain `f32`.

```rust
use idea_ui::{install_idea_theme, light_theme, Color, Tokenized};

let mut theme = light_theme();
theme.intents.primary.solid_bg = Tokenized::Literal(Color("#0066ff".into()));
theme.radius.md = 10.0;
theme.spacing.lg = 20.0;
theme.typography.body_size = 15.0;
install_idea_theme(theme);
```

Note: the string name in `Tokenized::token("…", …)` is **cosmetic**. On install,
each field's value is registered under its own fixed canonical name
(`intent-primary-solid-bg`, `spacing-lg`, …) regardless of the name you pass — so
a plain `Tokenized::Literal(...)` override is enough and a typo'd token name can't
silently no-op your change.

## A fully custom theme

For a distinct brand you have two options.

**Bundle a base with `app_theme!`** — the simplest path. It wraps a base theme and
lets you attach custom modifier tones:

```rust
use idea_ui::app_theme;

app_theme! {
    pub BrandTheme {
        idea: IdeaThemeDefaults,
    }
}

install_idea_theme(BrandTheme { idea: my_customized_defaults });
```

**Implement `IdeaTheme` directly** on any `'static` struct when you want full
control over how tokens are produced. The trait's required getters are `colors`,
`intents`, `spacing`, `radius`, `typography`; `font_family`, `hover_overlay`, and
`pressed_overlay` have defaults you can override to ship a brand face or tuned
state layers.

## Light / dark and runtime swaps

Swap the active theme at any time with `set_idea_theme(...)`. For a toggle driven
by a signal, `install_idea_theme_reactive` re-runs its selector whenever a signal
it reads changes — no hand-rolled effect required:

```rust
use idea_ui::{dark_theme, install_idea_theme_reactive, light_theme};
use runtime_core::signal;

let dark = signal!(false);
install_idea_theme_reactive(move || if dark.get() { dark_theme() } else { light_theme() });
// flipping `dark` now re-themes the whole app.
```

Because component stylesheets read tokens through the installed theme, a swap
re-flows every styled surface automatically — there is no per-component wiring.
See [[color_scheme]] for the platform's light/dark default, useful for picking the
initial theme without a flash.

## Custom tones and variants

The seven built-in intents cover most needs, but you can add your own semantic
palette (a "tone") with the `tone!` macro and register it via `app_theme! { tones:
[Brand] }`. A component then accepts it directly, e.g. `Button(tone = Brand)`.
Apps that only need to retune existing colors never touch this — it's for adding
*new* palettes, not editing existing ones.

## Overriding a single component's sheet

`install_idea_theme` installs a default stylesheet per component. To customize one
component's sheet (e.g. add a custom tone to Button) call its installer *after*
`install_idea_theme` returns:

```rust
install_button_sheet(ButtonSheetBuilder::new().add_tone(Brand.into()).build());
```

## Reference theme tokens from your own stylesheet

The sections above customize the *theme*. This one is the other direction: you're
writing your **own** `stylesheet!` for an app-specific layout (a custom sidebar,
say) and you want it to track the installed theme's colors — no hardcoded palette.

At the framework level a token reference is `Tokenized::token("color-surface",
fallback)`, and the `fallback` is a **required literal** (see [[styling]]). Supplying
the design's concrete hex there duplicates a value the theme already defines under
the same name — a second source of truth that silently drifts if the theme's value
later changes.

idea-theme closes that gap with `theme_token!` (colors) and `theme_length!`
(spacing / radius / typography sizes). They reference a canonical token **by name
only** — the fallback is pulled from idea-theme's base palette, so you restate no
hex, and the name is validated **at compile time**:

```rust
use idea_ui::{theme_token, theme_length};

stylesheet! {
    Sidebar<()> {
        base(_theme) {
            background: theme_token!("color-surface"),
            border_color: theme_token!("color-border"),
            color: theme_token!("intent-primary-fg"),
            padding: theme_length!("spacing-lg"),
            border_radius: theme_length!("radius-md"),
        }
    }
}
```

A typo — `theme_token!("color-surfaze")` — fails the build instead of silently
rendering a transparent fallback. At runtime the installed theme's value wins over
the fallback, exactly as with a hand-written `Tokenized::token`, so a reskin (or a
light/dark swap) re-flows the stylesheet with zero changes to it. The theme stays
the single source of truth.

Use the canonical names from the token table: the neutral colors
(`CANONICAL_NEUTRAL_TOKENS` — `color-background`, `color-surface`, `color-text`,
`color-border`, …), the intent slots (`intent-<intent>-<slot>`, e.g.
`intent-primary-soft-bg`), and the length tokens (`CANONICAL_LENGTH_TOKENS` —
`spacing-*`, `radius-*`, `typography-*-size`). For a token name computed at runtime,
the `theme_color(name)` / `theme_length(name)` functions are the string-driven
(runtime-checked) equivalents the macros delegate to.
