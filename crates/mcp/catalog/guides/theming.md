+++
title = "Theming and Light/Dark Mode"
order = 45
tags = ["theme", "style", "dark-mode", "light-dark"]
+++

# Theming and Light/Dark Mode

Light/dark mode and app-wide reskins work the same way at every level of the
stack: install a **theme** (one plain struct of design tokens) once at startup,
swap it at runtime, and every styled surface re-resolves automatically. This is
NOT an idea-ui-only facility:

- **Any app** — including one built on bare `runtime-core` primitives — themes
  through the `idea-theme` crate's generic runtime (`install_theme`,
  `set_theme`, `install_themes`) plus token-referencing `stylesheet!` rules.
  That path, with a complete light/dark example and the signal-driven swap
  pattern, is covered in the [[styling]] guide (and the `dark_mode_toggle`
  recipe — `describe_recipe("dark_mode_toggle")`).
- **idea-ui apps** get a richer, typed layer on top: every idea-ui component
  reads its colors, spacing, radii, and type scale from an installed
  `IdeaTheme`. This guide covers that concrete API (`install_idea_theme`,
  `light_theme`, `dark_theme`, `app_theme!`, …). Customizing the look of the
  whole component set is done by installing a theme, not by styling components
  one by one.

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

let dark = signal(false);
install_idea_theme_reactive(move || if dark.get() { dark_theme() } else { light_theme() });
// flipping `dark` now re-themes the whole app.
```

Because component stylesheets read tokens through the installed theme, a swap
re-flows every styled surface automatically — there is no per-component wiring.
Do NOT hand-roll a palette struct with per-node `if dark {...}` color switching —
that forfeits the automatic re-flow and scatters the theme across call sites.
See [[color_scheme]] for the platform's light/dark default, useful for picking the
initial theme without a flash.

Apps not using idea-ui components get the same swap semantics from the generic
layer directly: `idea_theme::set_theme(...)` for an event-driven swap, or
`idea_theme::install_themes(active, &[("light", LIGHT), ("dark", DARK)])` with a
`Signal<String>` for a fully signal-driven one. See [[styling]] and the
`dark_mode_toggle` recipe.

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

**Name the token through the binding in your block header.** Declare the theme in
the sheet's `<…>` slot and the block's `(t)` is the token vocabulary — every token
is a path, not a string:

```rust
use idea_ui::IdeaThemeRef;

stylesheet! {
    Sidebar<IdeaThemeRef> {
        base(t) {
            background: t.color.surface(),
            border_color: t.color.border(),
            color: t.intent.primary.fg(),
            padding: t.spacing.lg(),
            border_radius: t.radius.md(),
        }
    }
}
```

**The accessor path is the token name**: `t.color.surface()` is `color-surface`,
`t.intent.primary.fg()` is `intent-primary-fg`, `t.spacing.lg()` is `spacing-lg`.
So the vocabulary is the token table — reach for autocomplete on `t.`, and a typo
(`t.color.surfaze()`) fails the build. That matters more than it sounds: a
misspelled *string* used to compile fine and render its fallback forever, which is
exactly what two sheets in this repo were doing before the typed path landed.

The reference itself is unchanged — each accessor returns the same
`Tokenized::Token { name, fallback }` a hand-written `Tokenized::token(…)` builds,
with the fallback pulled from idea-theme's base palette rather than restated by
you. At runtime the installed theme's value wins over the fallback, so a reskin (or
a light/dark swap) re-flows the stylesheet with zero changes to it. The theme stays
the single source of truth.

The namespaces are `t.color.*` (neutrals), `t.intent.<intent>.<slot>()` (the seven
intents × six slots), `t.spacing.*`, `t.radius.*`, and `t.typography.*_size()`.

**Don't guess a token — ask.** `list_tokens` returns the whole vocabulary as data
(`name`, `path`, `accessor`, `value_type`, `default_value`); `describe_token`
takes either spelling (`spacing-md` or `spacing.md`) and hands back the accessor
to write. `search` matches both spellings too, so a token you know by its old
string name resolves to its accessor. The same slice drives token completion in
the VS Code extension.

Outside a `stylesheet!` — component code assembling `StyleRules` by hand — call
`tokens()` for the same namespace:

```rust
use idea_ui::tokens;

let rules = StyleRules {
    gap: Some(tokens().spacing.sm()),
    background: Some(tokens().color.surface()),
    ..Default::default()
};
```

For a token name only known at runtime (author-supplied, read from data), the
string-driven `theme_color(name)` / `theme_length(name)` functions remain. They're
runtime-checked: an unknown name yields a transparent/0px fallback and warns in
debug builds.
