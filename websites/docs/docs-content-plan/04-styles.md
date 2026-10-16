# Styles and Themes

Styling in Idealyst is a layered system: a **theme** holds named
token values; **stylesheets** declare rule sets that reference those
tokens by name; the framework caches the rule sets and applies them
through the same reactive substrate everything else uses.

What makes the system interesting is what happens on a theme change.
On web, the framework updates a handful of CSS custom properties.
DOM elements aren't touched. Class names don't change. No node
re-renders. On native, one theme driver effect re-applies every
styled node's style; no component body re-runs.
This page explains how that works, then shows you how to write your
own theme and your own stylesheets.

## The pieces

There are four moving parts:

- **Theme** — a Rust struct you define. Holds whatever values your
  app needs: colors, spacing, typography, breakpoints, anything
  else.
- **Tokens** — named values inside the theme. A token reference is
  a `(name, fallback)` pair. Stylesheets reference tokens by name;
  the fallback is what backends use when no value is installed for
  that name.
- **Stylesheets** — declared with the `stylesheet!` macro. A
  stylesheet is a typed builder that produces a `StyleRules` (a flat
  bag of optional property values). Its `<…>` slot names a token
  vocabulary (a `TokenVocabulary` type), not the theme: blocks see
  token names, and values arrive from the installed tokens at
  resolve time.
- **`StyleRules`** — the concrete output. Every primitive's `style`
  slot eventually gets one of these.

Application code writes themes and stylesheets. The framework
handles caching, theme installation, resolution, and the backend
calls that put the result on screen.

## Themes

A theme is a struct you write. The framework doesn't care about its
shape; it just has to implement the `ThemeTokens` trait so the
framework knows what to install as runtime variables.

```rust
use runtime_core::{Color, Length, Tokenized, TokenEntry, TokenValue};
use idea_theme::ThemeTokens;

#[derive(Clone)]
pub struct MyTheme {
    pub background: Tokenized<Color>,
    pub text: Tokenized<Color>,
    pub primary: Tokenized<Color>,
    pub spacing_md: Tokenized<Length>,
}

impl MyTheme {
    pub fn light() -> Self {
        Self {
            background: Tokenized::token("bg",      Color::from("#ffffff")),
            text:       Tokenized::token("text",    Color::from("#111111")),
            primary:    Tokenized::token("primary", Color::from("#3b82f6")),
            spacing_md: Tokenized::token("space-md", Length::Px(16.0)),
        }
    }

    pub fn dark() -> Self {
        Self {
            background: Tokenized::token("bg",      Color::from("#0b0b0c")),
            text:       Tokenized::token("text",    Color::from("#f5f5f5")),
            primary:    Tokenized::token("primary", Color::from("#60a5fa")),
            spacing_md: Tokenized::token("space-md", Length::Px(16.0)),
        }
    }
}

impl ThemeTokens for MyTheme {
    fn tokens(&self) -> Vec<TokenEntry> {
        vec![
            TokenEntry { name: "bg",       value: TokenValue::Color(self.background.value().clone()) },
            TokenEntry { name: "text",     value: TokenValue::Color(self.text.value().clone()) },
            TokenEntry { name: "primary",  value: TokenValue::Color(self.primary.value().clone()) },
            TokenEntry { name: "space-md", value: TokenValue::Length(*self.spacing_md.value()) },
        ]
    }
}
```

Three things to notice:

1. **Token names are theme-independent.** `light()` and `dark()` use
   the same names (`"bg"`, `"text"`, `"primary"`, `"space-md"`) with
   different fallback values. That's deliberate — see "Why classes
   don't change on theme swap" below.
2. **Tokens declare a fallback.** The fallback is the value a
   token resolves to when no value is installed under its name. On
   web it is the second argument of `var(--name, fallback)`; native
   backends use it when `Tokenized::resolve()` finds no installed
   value.
3. **The `ThemeTokens` impl is mechanical.** It lists the tokens to
   install. Every backend uses them: web as CSS variables, native
   backends through `Tokenized::resolve()` when they apply a rule.

### Installing a theme

```rust
use idea_theme::{install_theme, set_theme};

#[component]
fn app() -> Element {
    install_theme(MyTheme::light());

    ui! {
        // ...
    }
}

// Later, from anywhere:
set_theme(MyTheme::dark());
```

`install_theme(t)` registers the theme at app boot and installs its
tokens. `set_theme(t)` swaps it later and updates the tokens. Both
write the same active-theme slot — a signal holding the theme as
`Rc<dyn Any>` — so code that reads `active_theme()` inside an effect
re-runs on a swap.

You can swap themes at any time, from any code path that runs in
the main thread. A dark-mode toggle is a one-liner:

```rust
Button(
    label = "Toggle theme",
    on_click = move || set_theme(
        if is_dark.get() { MyTheme::light() } else { MyTheme::dark() }
    ),
)
```

## Tokens

A token is a value that may resolve through a named runtime variable
instead of being baked into rules. The type is `Tokenized<T>`:

```rust
pub enum Tokenized<T> {
    Literal(T),
    Token { name: &'static str, fallback: T },
}
```

A stylesheet property is a `Tokenized<Color>` (or `Length`, `f32`) —
either a literal value, or a token reference like
`Token { name: "primary", fallback: Color("#3b82f6") }`. The
backend decides what to do with it.

### What the web backend does

The web backend installs tokens as CSS custom properties on the
document root:

```css
:root {
    --bg: #ffffff;
    --text: #111111;
    --primary: #3b82f6;
    --space-md: 16px;
}
```

When the backend emits CSS for a stylesheet rule, every tokenized
property turns into a `var(--name, fallback)` reference:

```css
.idealyst-card-12 {
    background: var(--bg, #ffffff);
    color: var(--text, #111111);
    padding: var(--space-md, 16px);
}
```

That's the whole trick. When the theme swaps to dark, the backend
writes four new values to `:root`'s style block. Every CSS rule
that referenced `var(--bg)` now resolves to the dark color
automatically — by the browser, in one paint pass. The framework
doesn't iterate over DOM elements. It doesn't change class names.
It doesn't touch any rule body.

### What native backends do

iOS, Android and macOS don't have a runtime variable system, so
they resolve each token when they apply a rule
(`Tokenized::resolve()`): the installed value if there is one, the
fallback if not. When the theme swaps, `update_tokens` bumps one
theme version signal and clears the resolution cache. A single
theme driver effect then re-applies every styled node (the
static-style cohort); each re-apply resolves against the new
values and the backend mutates the native widget's properties.
The re-apply is not filtered by which token changed.

This is more work per swap than the web's variable-update model,
but it's proportional to the number of styled nodes, runs through
one effect, and never re-runs component bodies.

### What generator backends do

Roku and other generator backends can't ship closures to the
device. They get a different deal: the `Derived<T>` machinery and
the wire protocol install a device-side variable system that
mirrors the host's tokens. Theme swaps on the device are
device-side variable rewrites, conceptually the same as the web's
model.

## Stylesheets

The `stylesheet!` macro is how you declare a typed stylesheet. The
`<…>` slot names a token vocabulary — a type implementing
`TokenVocabulary` (see "Building your own theme system" below) — and
each block's binding (`base(t)`) is that vocabulary's accessor
namespace, so `t.space_md()` names the `space-md` token. Bindings
must be identifiers: write `_t`, not `_`, for a block that reads no
tokens. Grammar:

```rust
use runtime_core::{stylesheet, Color, Length, Shadow};

stylesheet! {
    pub Card<MyTokens> {
        base(t) {
            background: t.bg(),
            color: t.text(),
            padding: t.space_md(),
            border_radius: 8.0,
        }

        variant size {
            small(_t)  { padding: Length::Px(8.0) }
            #[default]
            medium(_t) {}
            large(_t)  { padding: Length::Px(24.0) }
        }

        variant kind {
            #[default]
            elevated(t) {
                background: t.bg(),
                shadow: Shadow { x: 0.0, y: 2.0, blur: 8.0, color: Color::from("#0001").into() },
            }
            outlined(t) {
                background: Color::from("transparent"),
                border_width: 2.0,
                border_color: t.text(),
            }
        }

        override padding: Length

        state hovered(_t) { opacity: 0.92 }
        state pressed(_t) { opacity: 0.85 }

        transitions {
            background: 200ms EaseOut
            opacity:    150ms Linear
        }
    }
}
```

What the macro generates from this declaration:

- **`pub fn card_style() -> Rc<StyleSheet>`** — the registered
  stylesheet. Cached in a thread-local; repeat calls return the
  same `Rc`.
- **`pub fn Card() -> Card`** — the entry point you call at use
  sites.
- **`pub enum CardSize { Small, Medium, Large }`** — one enum per
  variant axis, with a `Default` impl that picks the `#[default]`
  arm.
- **`pub enum CardKind { Elevated, Outlined }`** — same.
- **A `pub struct Card` builder** with one setter per variant axis
  and per override:
  ```rust
  Card()
      .size(CardSize::Large)
      .kind(CardKind::Outlined)
      .padding(Length::Px(20.0))
  ```
  Each setter accepts either a typed value or a `Signal<T>` — pass
  a signal and the variant axis becomes reactive.
- **`impl IntoStyleProp for Card`** — so the builder can
  be passed to `.with_style(...)` or as `style = ...` inside `ui!`.

### Using a stylesheet

```rust
ui! {
    view(style = Card().size(CardSize::Large).kind(CardKind::Outlined)) {
        text { "..." }
    }
}
```

With a reactive variant axis:

```rust
let size = signal(CardSize::Medium);

ui! {
    view(style = Card().size(size)) {
        text { "..." }
    }

    Button(label = "Grow", on_click = move || size.set(CardSize::Large))
}
```

When `size` changes, the framework looks up the cached rule set for
the new variant tuple and calls `apply_style` on the backend for
the View's node only.

## How resolution works

Resolution is a function from
`(stylesheet, variants, overrides)` to a concrete
`Rc<StyleRules>`. The framework caches the result so the same
combination of inputs returns the same `Rc` instance.

The cache key is interesting:

- **Stylesheet** is identified by `Rc` pointer.
- **Variants** are an ordered set of `(axis, value)` strings.
- **Overrides** are serialized into a content key.

A theme swap clears the cache (`update_tokens`), so the next
resolution reads the new token values.

What makes the web backend's swap cheap is that the
**content key** that the web backend uses to mint a CSS class
hashes tokens by their **name**, not by their resolved value. So
`Card().size(Large).kind(Outlined)` produces the **same content
key** under `light` and `dark`. The web backend mints one class
and reuses it across themes. Theme swap turns into a refcount bump
on the existing class registration plus four CSS variable writes,
which is what makes the swap O(tokens) rather than O(styled
nodes).

> **From React.** Theming in React typically goes through Context;
> a context value change re-renders every consumer. Even with
> `React.memo` and selectors, every styled component participates
> in the dependency graph and pays some per-swap cost. Here a swap
> re-renders nothing: on native one theme driver effect re-applies
> styles, and on web even that is skipped because CSS variables do
> the work.

> **From styled-components / Emotion.** Each themed component
> usually generates a new class name when the theme changes, which
> forces every instance to re-attach. Idealyst's class names are
> theme-stable — the content key hashes token *names*, so a
> stylesheet's class is the same under any theme. Theme swap
> doesn't change className on a single element.

> **From Tailwind.** Closest analog is CSS-variable-based theming
> in Tailwind (the `darkMode: "class"` strategy with custom
> properties). Same payoff: a class flip on `:root` updates colors
> downstream without touching anything else. Idealyst's split
> between variable references (web) and apply-time token
> resolution (native) gives you the same payoff
> automatically — you don't choose between the two strategies, the
> backend chooses.

## Variants vs overrides vs states

Three different ways to vary a stylesheet at the point of use:

**Variants** are discrete enum-shaped axes. Each variant arm has a
fixed rule overlay. Use variants when there's a finite set of named
modes — `size: Small | Medium | Large`, `kind: Solid | Soft |
Outlined`. The macro generates enum types and `Default` impls.

**Overrides** are per-instance continuous values. The author passes
a specific value at the use site (`Card().padding(Length::Px(20.0))`), and that
value lands in the override slot of the resolved rule set. Use
overrides when the value isn't from a finite menu — a specific
size, color, or duration.

**States** are interaction states the backend flips automatically:
`hovered`, `pressed`, `focused`, `disabled`. Each state's rule
overlay applies when the backend's input layer says the relevant
state is active. You don't switch states from app code; the backend
listens for the native event and updates the state bits, and the
framework applies the overlay.

```rust
stylesheet! {
    pub Btn<MyTokens> {
        base(t) { background: t.primary() }
        state hovered(_t)  { opacity: 0.9 }
        state pressed(_t)  { opacity: 0.8 }
        state disabled(_t) { opacity: 0.5 }
    }
}
```

State overlays land in a reserved `__state` axis under the hood —
same machinery as variants, so resolution caching and
pre-generation work without special cases.

## Transitions

A `transitions { ... }` block declares which property changes
should animate, and how. The framework doesn't drive the animation
itself — backends use their native interpolators:

- Web: emits `transition: background 200ms ease-out`.
- iOS: wraps the property write in a `UIView.animate` block.
- Android: uses `ObjectAnimator`.

```rust
transitions {
    background: 200ms EaseOut
    opacity:    150ms Linear
    padding:    250ms cubic_bezier(0.2, 0.0, 0.0, 1.0)
}
```

Shorthand property names like `padding` fan out to all four sides
during macro expansion. Properties without a transition spec
change instantly.

## Property reference (short version)

`StyleRules` is a flat bag of optional properties. The shape is
mobile-first — what React Native's StyleSheet supports, plus a few
additions. The categories:

- **Color + text**: `background`, `color`, `font_size`, `font_family`,
  `font_weight`, `font_style`, `line_height`, `letter_spacing`,
  `text_align`, `underline`, `strikethrough`, `text_transform`,
  `max_lines` (truncate with "…" after that many lines; the text can
  then shrink in a row).
- **Flex container**: `flex_direction`, `flex_wrap`,
  `justify_content`, `align_items`, `align_content`, `gap`,
  `row_gap`, `column_gap`.
- **Flex item**: `flex_grow`, `flex_shrink`, `flex_basis`,
  `align_self`.
- **Sizing**: `width`, `height`, `min_width`, `min_height`,
  `max_width`, `max_height`.
- **Padding / margin / border radius / border width / border
  color** — per-side fields, with shorthand expansion in the macro.
- **Position**: `position`, `top`, `right`, `bottom`, `left`.
- **Visual**: `opacity`, `overflow`, `shadow` (box), `text_shadow`
  (glyphs), `transform`, `cursor`, `user_select`.
- **Per-property transitions** for every animatable property
  above.

Layout is flexbox by default on every node, with real CSS grid
available via `display: DisplayKind::Grid` + `grid_template_columns`
(idea-ui's `Grid` component rides it). There is no float; the
browser (web) or Taffy (native) does the layout.

## Building your own theme system

The `MyTheme` walk-through above is the whole thing. To recap the
steps:

1. **Define a struct** with whatever fields you want. Use
   `Tokenized<T>` for fields that should resolve through a runtime
   variable on web; use plain `T` for fields that don't need to.
2. **Build instance constructors** (`light()`, `dark()`,
   `high_contrast()`). Each constructor returns a fully-populated
   instance.
3. **Implement `ThemeTokens`** to list the `(name, value)` pairs
   to install (CSS variables on web; resolved at apply time on
   native).
4. **Install at boot** via `install_theme(MyTheme::light())`.
5. **Swap at runtime** via `set_theme(MyTheme::dark())`.

6. **Define a token vocabulary** so stylesheets can name tokens as
   checked paths. A vocabulary is a zero-sized type whose accessors
   return token references, plus a `TokenVocabulary` impl:

```rust
use runtime_core::{Color, Length, TokenVocabulary, Tokenized};

#[derive(Default, Clone, Copy)]
pub struct MyTokens;

impl MyTokens {
    pub fn bg(&self) -> Tokenized<Color> { Tokenized::token("bg", Color::from("#ffffff")) }
    pub fn text(&self) -> Tokenized<Color> { Tokenized::token("text", Color::from("#111111")) }
    pub fn primary(&self) -> Tokenized<Color> { Tokenized::token("primary", Color::from("#3b82f6")) }
    pub fn space_md(&self) -> Tokenized<Length> { Tokenized::token("space-md", Length::Px(16.0)) }
}

impl TokenVocabulary for MyTokens {
    type Tokens = Self;
}
```

`stylesheet! { pub Foo<MyTokens> { ... } }` then binds `MyTokens` in
each block (`base(t)`), so you get IDE completion and a compile error
for a misspelled token. The stylesheet never sees the `MyTheme`
value; every theme that installs the same names works with it.

You can also write a stylesheet without a vocabulary by using
`<()>`. Bind `_t` and reference token names directly with
`Tokenized::token("name", fallback)`. This is the lightest path
— no vocabulary type — and works fine for app-local
styles that don't need typed theme access:

```rust
use runtime_core::{stylesheet, Color, Length, Tokenized};

stylesheet! {
    pub Card<()> {
        base(_t) {
            background: Tokenized::token("bg",       Color::from("#ffffff")),
            color:      Tokenized::token("text",     Color::from("#111111")),
            padding:    Tokenized::token("space-md", Length::Px(16.0)),
            border_radius: 8.0,
        }
    }
}
```

The vocabulary form is the right call when you want IDE completion
and checked token names; the `<()>` form is the right call when
bare token references are enough.

If you want multiple themes at once (light + dark + high-contrast
selectable from a menu), the pattern is to make `MyTheme` an enum
or a configurable struct whose constructors produce the variant
you want. The framework just sees one type.

idea-ui's theme is one example of this pattern (the `IdeaTheme`
trait; `light_theme()` / `dark_theme()` return an
`IdeaThemeDefaults`, and its sheets name `IdeaThemeRef` as their
vocabulary), organized around intent palettes (Primary, Secondary,
Neutral, Success, Danger, Warning, Info) instead of role-named
colors. Each intent's swatch has `solid_bg`, `solid_text`,
`soft_bg`, `soft_text`, `fg` and `border`, and component sheets
pick the intent from their `tone` axis. You can read
its source as a reference, copy it, or ignore it entirely and roll
your own.

## Building your own stylesheets

The `stylesheet!` macro is part of `runtime-core`. Nothing in it
is idea-ui-specific. To build your own component library or just
some app-local styles:

1. **Declare your theme** as above.
2. **Write `stylesheet!`** blocks for each styled surface (`Card`,
   `Btn`, `Heading`, …). Each one names your vocabulary in its
   `<MyTokens>` slot.
3. **Attach stylesheets** to primitives at the call site via the
   `style = ...` prop inside `ui!`.

You can mix styling sources freely. A primitive's `style` slot
takes any `IntoStyleProp` — a stylesheet builder, a raw
`StyleRules`, a closure that returns one. You can hand-write rule
sets for one-off cases and use the macro for the rest.

## Pre-generation

For backends that benefit from up-front rule emission (web), the
framework calls `StyleOps::register_stylesheet` once per stylesheet
and hands it the pre-resolved rules for every variant combination. The web backend mints CSS classes
eagerly; subsequent `apply_style` calls just set `className`. This
is what keeps the per-node apply path cheap on web — no string
building, no rule-text emission inside the hot path.

You don't write code that interacts with pre-generation. It's the
framework calling backends; mentioned here only so the next section
about backend internals is less surprising.

## Where to read more

- [Reactivity](reactivity) — how the theme version signal and the
  theme driver effect ride the same effect graph everything else
  uses.
- [Building a theme system](building-a-theme-system) — the full
  cookbook for themes and vocabularies.
- idea-ui — a complete theme + stylesheet system built on this
  page's primitives. Use it, fork it, or read it for ideas.
- [Backends](backends) — what the `apply_style`,
  `register_stylesheet`, and token APIs (the `StyleOps` capability)
  look like from a backend's side.
- [Animation](animation) — how transitions fold into the broader animation
  story (Presence, GPU effects, gesture-driven motion).
