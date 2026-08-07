+++
title = "Styling and Theming"
order = 40
tags = ["style", "theme"]
+++

# Styling and Theming

Style in Idealyst is declared with the `stylesheet!` macro, theme-aware via **named tokens**, and applied to primitives via the `style` slot.

**Where the pieces live:** `stylesheet!` is re-exported from `runtime_core` — no extra dependency. The theme runtime (`install_theme`, `set_theme`, `install_themes`) lives in the **`idea-theme` crate** (`idea_theme::install_theme`), NOT in `runtime-core`. Add it the same way your project references `runtime-core` (same git rev / path / workspace source):

```toml
[dependencies]
idea-theme = { git = "...", rev = "<same rev as runtime-core>" }  # or path/workspace
```

`idea-theme` has no third-party dependencies — it depends only on `runtime-core` — so adding it never pulls anything new into the build.

## Anatomy of a stylesheet

```rust
use runtime_core::{stylesheet, Color, Tokenized};

stylesheet! {
    pub PrimaryButton<()> {
        base(_theme) {
            padding: 8,
            border_radius: 6,
            background: Tokenized::token("color-accent", Color("#0d6efd".into())),
        }
        variant size {
            #[default]
            small(_theme) { padding: 4 }
            large(_theme) { padding: 12 }
        }
        transitions {
            background: 200ms EaseOut,
        }
        state pressed(_theme) {
            background: Color("#0a53be".into()), // a darker accent for the pressed state
        }
        state disabled(_theme) {
            opacity: 0.5,
        }
    }
}
```

Key parts:
- `base(_theme)` — the always-applied baseline.
- `variant <axis>` — N-way orthogonal options; one arm per value. `#[default]` marks the implicit choice.
- `transitions { property: <duration> <easing> }` — per-property animated transitions.
- `state <name>(_theme)` — overlay for one of the four interaction states: [[hovered]], [[pressed]], [[focused]], [[disabled]]. Other names are rejected at compile time.
- Property names are `StyleRules` fields (see the appendix) plus shorthands the macro fans out: `padding` / `padding_horizontal` / `padding_vertical` (same for `margin`), `border_radius`, `border_width`, `border_color`. Note the color field is `background` / `color` — there is no `background_color`.

**The `<…>` type parameter names the sheet's token vocabulary, and the arm binding hands it to the block.** With a design system installed, declare its theme type and reference tokens through the binding — the name is a path the compiler checks, not a string:

```rust
stylesheet! {
    pub Panel<IdeaThemeRef> {
        base(t) {
            background: t.color.surface(),
            padding: t.spacing.md(),
        }
    }
}
```

The binding is *not* the theme's values — a theme swap still flows through the token registry at resolve time, which is what keeps it one write per token rather than a re-mint. So `background: theme.colors.primary` does not compile: there are no value fields to read.

`<()>` declares no vocabulary. Such a sheet still references tokens by name with `Tokenized::token("name", fallback)` — the right form for app-defined tokens that no vocabulary describes (see the light/dark app below) — and conventionally writes its binding `_theme` / `_t`.

## Applying styles

Inside `ui!`, pass the generated builder to the `style` prop:

```rust
ui! {
    button(label = "Save", style = PrimaryButton().size(PrimaryButtonSize::Large))
}
```

The macro generates, for `pub PrimaryButton<()>`: the `PrimaryButton()` builder fn, one enum per variant axis (`PrimaryButtonSize` here, arms from the arm names), one setter per axis, and a `pub fn primary_button_style() -> Rc<StyleSheet>` accessor for the raw cached sheet. Per [[native_first_layout_for_web]], use stylesheet bindings for cross-platform chrome; the `.layout(...)` builder is a web-only escape hatch.

## Theming: tokens + `install_theme`

A stylesheet references theme values by **token name** (`Tokenized::token("color-accent", fallback)`). A theme is any struct implementing `idea_theme::ThemeTokens` — its `tokens()` method maps names to concrete values. `install_theme(theme)` installs those tokens; `set_theme(other)` swaps them, and **every styled primitive referencing a token re-resolves automatically** — no per-node wiring.

### Complete minimal light/dark app

```rust
use idea_theme::{install_theme, set_theme, ThemeTokens, TokenEntry, TokenValue};
use runtime_core::{signal, stylesheet, ui, Color, Element, Tokenized};

// 1. The theme is ONE struct. Fields are whatever your design needs.
#[derive(Clone, Copy)]
struct AppTheme {
    surface: &'static str,
    text: &'static str,
}

impl ThemeTokens for AppTheme {
    fn tokens(&self) -> Vec<TokenEntry> {
        vec![
            TokenEntry { name: "app-surface", value: TokenValue::Color(self.surface.into()) },
            TokenEntry { name: "app-text", value: TokenValue::Color(self.text.into()) },
        ]
    }
}

const LIGHT: AppTheme = AppTheme { surface: "#ffffff", text: "#111827" };
const DARK: AppTheme = AppTheme { surface: "#0b1220", text: "#e5e7eb" };

// 2. Stylesheets consume tokens BY NAME. The fallback is used only
//    when no token with that name has been installed.
stylesheet! {
    pub Panel<()> {
        base(_t) {
            background: Tokenized::token("app-surface", Color("#ffffff".into())),
            color: Tokenized::token("app-text", Color("#111827".into())),
            padding: 16,
        }
    }
}

// 3. Install once at startup, BEFORE the first render.
//    Swap at any time with set_theme — every token-referencing
//    style in the app re-resolves in place.
pub fn app() -> Element {
    install_theme(LIGHT);
    let dark = signal(false);
    ui! {
        view(style = Panel()) {
            text { "Hello" }
            toggle(
                value = dark,
                on_change = move |v| {
                    dark.set(v);
                    set_theme(if v { DARK } else { LIGHT });
                },
            )
        }
    }
}
```

This exact pattern is available as a compile-checked recipe: `describe_recipe("dark_mode_toggle")`.

For a fully signal-driven variant selection (no explicit `set_theme` calls), `idea_theme::install_themes(active, &[("light", LIGHT), ("dark", DARK)])` takes a `Signal<String>` naming the active variant and swaps automatically whenever the signal changes.

> **Anti-pattern — do NOT hand-roll per-node theme switching.** Don't define a palette struct and branch per node with `if dark.get() { palette.dark_bg } else { palette.light_bg }` inside style closures, and don't rebuild `StyleRules` per node per theme. Declare colors as token references in `stylesheet!` (typed via the block binding when a design system is installed, `Tokenized::token` for app-defined names) and swap the whole theme with `set_theme` / `install_themes` — one call re-flows every styled primitive, stays cache-friendly (theme swap changes token values, not class identities), and keeps the theme a single source of truth. For idea-ui apps the equivalents are `install_idea_theme` / `set_idea_theme` / `install_idea_theme_reactive` — see [[theming]].

**When is `install_theme` required?** For bare `stylesheet!` usage it's required only when styles reference tokens you expect a theme to supply (otherwise fallbacks apply). idea-ui **components** require `install_idea_theme(...)` before the first render — they read the active theme and panic without one (see [[theming]]).

## Reactive styles from signals

The `style` slot accepts anything implementing `IntoStyleSource`. The accepted forms, from most to least common:

1. **Generated builder, static variants** — `style = PrimaryButton().size(PrimaryButtonSize::Large)`. Resolved once; cheapest.
2. **Generated builder, signal-driven variants** — the blessed reactive form. Every variant setter also accepts a `Signal<E>` (or `derived(move || ...)`) of its enum: `style = NavStyle().active(is_active)` restyles the node whenever the signal changes, without touching the sheet.
3. **`Rc<StyleSheet>`** — e.g. the generated `primary_button_style()`. Static; resolves once and memoizes.
4. **Closure `Fn() -> StyleApplication`** — fully reactive: re-runs whenever any signal it reads changes, e.g. `style = move || StyleApplication::new(some_sheet()).with("size", if big.get() { "large" } else { "small" })`. Use when variant enums can't express the logic; prefer form 2 where they can.

Note that **theme swaps never need a reactive style**: a static sheet whose values are token references re-resolves on `set_theme` anyway. Reach for forms 2/4 only when the *variant choice itself* is signal-driven (selection, active tab, validation state).

## Helpers

- [[parse]] — parse `#abc` / `rgba(…)` / named colors into `Rgba`.
- [[color_scheme]] — the platform light/dark default (useful for picking the initial theme).
- [[current_breakpoint]] — read the active responsive breakpoint.
- [[safe_area_insets]] — reactive per-side safe-area insets.

## Per-platform notes

- iOS clamps `cornerRadius` against the layer's smaller dimension (see [[ios_cornerradius_unclamped]]). Don't over-specify.
- Gradients (`background_gradient`) work on every backend; radial gradient radius is closest-side scaled (`1.0` = edge midpoint).

## Preminted styles (web bundle size)

`idealyst build --web --premint` emits stylesheet CSS at build time (a content-addressed `pkg/premint.<hash>.css`) and ships class references instead of running the style engine for them. What premints: every `stylesheet!`, every runtime-assembled sheet with a `premint_as` identity, and every plain `StyleSheet::r#static` (automatic — the class derives from the rules' content). Shadowed sheets premint (`shadow` = box, `text_shadow` = glyphs), and constant `Typeface` fonts premint with their `@font-face` rules emitted into the same asset. What stays on the live engine: `.override_*` values, `with_computed` layers, and closure-built sheets with no identity — run `--premint-report` to list every fall-through with the source line it was built at. Continuous per-instance values (a thumb `left`, a grid track list) ride `StyleApplication::with_inline`, which premints. The build step MOUNTS the app on every literal route to collect sheets — including `#[component(lazy)]` screens, whose bodies the dump resolves by pumping its executor to a fixed point per route (lazy bodies compile inline on native, so their styles mint like any other mount-time sheet); a sheet constructed on a path the dump never ran (an interaction-gated subtree, a parameterized route) gets no CSS, and at boot the bundle scans the loaded asset so such classes fall back to the engine instead of rendering unstyled. For the full size win, `--premint-only` compiles the style engine and the sheets' rule closures out of the wasm entirely (about a third of the raw wasm on the idea-ui docs catalog); a style that then needs the engine panics loudly, naming its source line. Theming still works: tokens are CSS variables, and a premint host driver delivers theme state without sheet registrations. Composes with `--ssg`/`--ssr`: the server binary builds with the same premint cfgs, stamps the same deterministic `iy-*` classes the hydrating client stamps, links `premint.css` from every rendered document, and arms the same minted-class guard — so hydration adopts cleanly. Details: `docs/styling.md` and [[migration-1-1-0-to-1-2-0]].

## Appendix: StyleRules properties

Every `stylesheet!` rule body sets fields of `runtime_core::StyleRules`. All fields are optional. Values wrapped `Into::into(...)` — so `padding: 16` works for `Length` fields, `"#fff"` strings need `Color(...)`.

**Color + text**: `background`, `color`, `caret_color`, `font_size`.

**Display mode**: `display` (`DisplayKind::{Flex, Grid}` — Flex is the default), `grid_template_columns` (`Vec<TrackSize>`; `TrackSize::{Auto, MinContent, MaxContent, Fr(f32), Px(f32), Minmax(..)}`).

**Flex container**: `flex_direction`, `flex_wrap`, `justify_content`, `align_items`, `align_content`, `gap`, `row_gap`, `column_gap`.

**Flex item**: `flex_grow`, `flex_shrink`, `flex_basis`, `align_self`.

**Sizing**: `width`, `height`, `min_width`, `min_height`, `max_width`, `max_height`, `aspect_ratio`.

**Padding / margin** (per-side; shorthands `padding`, `padding_horizontal`, `padding_vertical` etc. fan out in the macro): `padding_top`, `padding_right`, `padding_bottom`, `padding_left`; `margin_top`, `margin_right`, `margin_bottom`, `margin_left`.

**Border** (shorthands `border_radius`, `border_width`, `border_color`): `border_top_left_radius`, `border_top_right_radius`, `border_bottom_left_radius`, `border_bottom_right_radius`; `border_top_width`/`_right_`/`_bottom_`/`_left_` (f32); `border_top_color`/`_right_`/`_bottom_`/`_left_`.

**Position**: `position` (`Position::{Relative, Absolute, Sticky}`), `top`, `right`, `bottom`, `left`.

`Position::Sticky` pins per axis: `top` pins vertically, `left` pins horizontally, and they are independent — a frozen table column is `position: Sticky, left: Px(0.0)` and still scrolls vertically with its row. Sticky pins to the *nearest* enclosing scroll container, so an intervening `overflow: Hidden` becomes that container. Trailing edges (`bottom` / `right`) work on web only; native backends log a one-time `[unsupported]` warning in debug builds instead of failing silently.

**Typography**: `font_family` (`FontFamily::System(String)` or a registered `Typeface`; `"system-ui, sans-serif"` coerces), `font_weight`, `font_style`, `line_height`, `letter_spacing`, `text_align`, `underline` (bool), `strikethrough` (bool), `text_transform`.

**Visual**: `opacity`, `overflow`, `overscroll_behavior`, `object_fit`, `shadow`, `background_gradient`, `transform` (`Vec<Transform>`), `transform_origin`.

`overscroll_behavior` (`OverscrollBehavior::{Auto, Contain, None}`) governs what a scroll gesture does when it runs out of content, and only means anything on a scrolling surface. `Contain` stops the gesture chaining outward — the declarative fix for a horizontal scroller whose left-edge swipe becomes the browser's "back". `None` also suppresses the platform's edge effect (web: no chaining; iOS: `bounces = false`; macOS: no elasticity; Android: `OVER_SCROLL_NEVER`).

**Interaction** (desktop/web; touch backends no-op): `cursor`, `user_select`, `pointer_events`.

Key enums (defaults marked):

- `Length`: `Px(f32)`, `Percent(f32)`, `Auto` — bare `16` / `16.0` coerce to `Px`; `Length::pct(50.0)` for percent.
- `FlexDirection`: **Column**, Row, ColumnReverse, RowReverse (column default, like React Native).
- `FlexWrap`: **NoWrap**, Wrap, WrapReverse.
- `JustifyContent`: **FlexStart**, FlexEnd, Center, SpaceBetween, SpaceAround, SpaceEvenly.
- `AlignItems`: FlexStart, FlexEnd, Center, **Stretch**, Baseline.
- `AlignContent`: **FlexStart**, FlexEnd, Center, Stretch, SpaceBetween, SpaceAround.
- `AlignSelf`: **Auto**, FlexStart, FlexEnd, Center, Stretch, Baseline.
- `FontWeight`: Thin, ExtraLight, Light, **Normal**, Medium, SemiBold, Bold, ExtraBold, Black.
- `FontStyle`: **Normal**, Italic.
- `TextAlign`: **Left**, Right, Center, Justify.
- `TextTransform`: **None**, Uppercase, Lowercase, Capitalize.
- `Overflow`: **Visible**, Hidden (no `Scroll` — scrolling needs the `scroll_view` primitive).
- `ObjectFit`: Fill, **Contain**, Cover (image primitive only).
- `Cursor`: **Auto**, Default, Pointer, Text, Wait, Progress, Help, NotAllowed, Move, Grab, Grabbing, Crosshair, ColResize, RowResize, EwResize, NsResize.
- `UserSelect`: **Auto**, None, Text, All.
- `PointerEvents`: **Auto**, None.
- `Easing` (transitions): Linear, **Ease**, EaseIn, EaseOut, EaseInOut, CubicBezier(x1, y1, x2, y2).
- `Transform`: TranslateX(Length), TranslateY(Length), Scale(f32), ScaleXY { x, y }, Rotate(f32 deg), SkewX(f32), SkewY(f32).
