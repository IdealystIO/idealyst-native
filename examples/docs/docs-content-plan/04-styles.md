# Styles and Themes

Styling in Idealyst is a layered system: a **theme** holds named
values; **stylesheets** are functions that take the active theme and
produce concrete rule sets; the framework caches the rule sets and
applies them through the same reactive substrate everything else
uses.

What makes the system interesting is what happens on a theme change.
On web, the framework updates a handful of CSS custom properties.
DOM elements aren't touched. Class names don't change. No node
re-renders. On native, the same change re-fires only the per-node
style Effects whose values actually depended on the changed tokens.
This page explains how that works, then shows you how to write your
own theme and your own stylesheets.

## The pieces

There are four moving parts:

- **Theme** — a Rust struct you define. Holds whatever values your
  app needs: colors, spacing, typography, breakpoints, anything
  else.
- **Tokens** — named values inside the theme. A token is a
  `(name, fallback)` pair. Stylesheets reference tokens by name; the
  fallback is what backends use when no runtime variable system is
  available.
- **Stylesheets** — declared with the `stylesheet!` macro. A
  stylesheet is a typed builder that takes the active theme and
  produces a `StyleRules` (a flat bag of optional property values).
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
use framework_core::{Color, Length, Tokenized, TokenEntry, TokenValue};
use framework_theme::ThemeTokens;

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
2. **Tokens declare a fallback.** The fallback is the value the
   token resolves to when there's no runtime variable system (iOS,
   Android, server-rendered HTML). On web, the fallback also fills
   in if the CSS variable hasn't been written yet.
3. **The `ThemeTokens` impl is mechanical.** It just lists the
   tokens that should be installed as variables. For backends
   without runtime variables, this impl is effectively unused.

### Installing a theme

```rust
use framework_theme::{install_theme, set_theme};

#[component]
fn app() -> Primitive {
    install_theme(MyTheme::light());

    ui! {
        // ...
    }
}

// Later, from anywhere:
set_theme(MyTheme::dark());
```

`install_theme(t)` registers the theme at app boot and is what
stylesheet closures see when they run. `set_theme(t)` swaps it
later. Both write to the same arena slot — internally, the active
theme is stored as a `Signal<Rc<dyn Any>>`, so anything that reads
it participates in reactivity.

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

A stylesheet receiving `theme.primary` reads a `Tokenized<Color>` —
either a literal color, or a token reference like
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

iOS and Android don't have a runtime variable system, so they
ignore the token name and read the `.value()` (the fallback). When
the theme swaps, every styled node has an Effect wrapping its
apply-style call; that Effect re-fires with the new theme, the
stylesheet closure re-runs, the new rules go to the backend, the
backend mutates the native widget's properties.

This is more work per swap than the web's variable-update model,
but it's still proportional to the number of styled nodes — not
the size of the tree. And the rule-set cache deduplicates: if two
themes produce identical rules for the same `(sheet, variants)`,
the second swap is a refcount bump.

### What generator backends do

Roku and other generator backends can't ship closures to the
device. They get a different deal: the `Derived<T>` machinery and
the wire protocol install a device-side variable system that
mirrors the host's tokens. Theme swaps on the device are
device-side variable rewrites, conceptually the same as the web's
model.

## Stylesheets

The `stylesheet!` macro is how you declare a typed, themed
stylesheet. Grammar:

```rust
use framework_core::{stylesheet, Color, Length};

stylesheet! {
    pub Card<MyTheme> {
        base(theme) {
            background: theme.background.clone(),
            color: theme.text.clone(),
            padding: theme.spacing_md,
            border_radius: 8.0,
        }

        variant size {
            small(theme)  { padding: theme.spacing_md.value().clone() }
            #[default]
            medium(_)     {}
            large(theme)  { padding: 24.0 }
        }

        variant kind {
            #[default]
            elevated(theme) {
                background: theme.background.clone(),
                shadow: Shadow { x: 0.0, y: 2.0, blur: 8.0, color: Color::from("#0001") },
            }
            outlined(theme) {
                background: Color::from("transparent"),
                border: (2.0, theme.text.clone()),
            }
        }

        override padding: Length

        state hovered(theme) { opacity: 0.92 }
        state pressed(_)     { opacity: 0.85 }

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
- **`pub fn Card() -> CardBuilder`** — the entry point you call at
  use sites.
- **`pub enum CardSize { Small, Medium, Large }`** — one enum per
  variant axis, with a `Default` impl that picks the `#[default]`
  arm.
- **`pub enum CardKind { Elevated, Outlined }`** — same.
- **A `CardBuilder` struct** with one setter per variant axis and
  per override:
  ```rust
  Card()
      .size(CardSize::Large)
      .kind(CardKind::Outlined)
      .padding(20.0)
  ```
  Each setter accepts either a typed value or a `Signal<T>` — pass
  a signal and the variant axis becomes reactive.
- **`impl IntoStyleSource for CardBuilder`** — so the builder can
  be passed to `.with_style(...)` or as `style = ...` inside `ui!`.

### Using a stylesheet

```rust
ui! {
    View(style = Card().size(CardSize::Large).kind(CardKind::Outlined)) {
        Text { "..." }
    }
}
```

With a reactive variant axis:

```rust
let size = signal!(CardSize::Medium);

ui! {
    View(style = Card().size(size)) {
        Text { "..." }
    }

    Button(label = "Grow", on_click = move || size.set(CardSize::Large))
}
```

When `size` changes, the framework looks up the cached rule set for
the new variant tuple and calls `apply_style` on the backend for
the View's node only.

## How resolution works

Resolution is a function from
`(stylesheet, variants, theme, overrides)` to a concrete
`Rc<StyleRules>`. The framework caches the result so the same
combination of inputs returns the same `Rc` instance.

The cache key is interesting:

- **Stylesheet** is identified by `Rc` pointer.
- **Variants** are an ordered set of `(axis, value)` strings.
- **Theme** is identified by `Rc` pointer.
- **Overrides** are serialized into a content key.

So `Card().size(Large).kind(Outlined)` under `MyTheme::light()`
maps to one cached `Rc<StyleRules>`. The same builder under
`MyTheme::dark()` maps to a different one.

But — and this is what makes the web backend's swap cheap — the
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
> in the dependency graph and pays some per-swap cost. Here the
> graph is one signal (the active theme); only the per-node style
> Effects subscribe to it, and on web the Effects are bypassed
> entirely because CSS variables do the work.

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
> between literal fallbacks (for non-web backends) and
> variable references (for web) gives you the same payoff
> automatically — you don't choose between the two strategies, the
> backend chooses.

## Variants vs overrides vs states

Three different ways to vary a stylesheet at the point of use:

**Variants** are discrete enum-shaped axes. Each variant arm has a
fixed rule overlay. Use variants when there's a finite set of named
modes — `size: Small | Medium | Large`, `kind: Solid | Soft |
Outlined`. The macro generates enum types and `Default` impls.

**Overrides** are per-instance continuous values. The author passes
a specific value at the use site (`Card().padding(20.0)`), and that
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
    pub Btn<MyTheme> {
        base(theme) { background: theme.primary.clone() }
        state hovered(_)  { opacity: 0.9 }
        state pressed(_)  { opacity: 0.8 }
        state disabled(_) { opacity: 0.5 }
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
    padding:    250ms CubicBezier(0.2, 0.0, 0.0, 1.0)
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
  `text_align`, `underline`, `strikethrough`, `text_transform`.
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
- **Visual**: `opacity`, `overflow`, `shadow`, `transform`.
- **Per-property transitions** for every animatable property
  above.

There is no display/grid/float. Every node uses flexbox; the
framework relies on the browser (web) or Taffy (native) to do the
layout.

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
   the web backend will install as CSS variables.
4. **Install at boot** via `install_theme(MyTheme::light())`.
5. **Swap at runtime** via `set_theme(MyTheme::dark())`.

The theme type is generic over the stylesheets that consume it.
`stylesheet! { pub Foo<MyTheme> { ... } }` ties a stylesheet to a
specific theme type; the stylesheet's `base(theme)` closure
receives a `&MyTheme`, so you have full IDE completion and type
checking on theme access.

You can also write a stylesheet without a theme context by using
`<()>`. The closures don't receive a theme; instead the
stylesheet directly references token names with
`Tokenized::token("name", fallback)`. This is the lightest path
— no struct, no `ThemeTokens` impl — and works fine for app-local
styles that don't need typed theme access:

```rust
use framework_core::{stylesheet, Color, Length, Tokenized};

stylesheet! {
    pub Card<()> {
        base(_) {
            background: Tokenized::token("bg",       Color::from("#ffffff")),
            color:      Tokenized::token("text",     Color::from("#111111")),
            padding:    Tokenized::token("space-md", Length::Px(16.0)),
            border_radius: 8.0,
        }
    }
}
```

The `<MyTheme>` form is the right call when you want IDE
completion on a richly-typed theme; the `<()>` form is the right
call when bare token references are enough.

If you want multiple themes at once (light + dark + high-contrast
selectable from a menu), the pattern is to make `MyTheme` an enum
or a configurable struct whose constructors produce the variant
you want. The framework just sees one type.

idea-ui's `IdeaTheme` is one example of this pattern, organized
around intent palettes (Primary, Secondary, Neutral, Success,
Danger, Warning, Info) instead of role-named colors. You can read
its source as a reference, copy it, or ignore it entirely and roll
your own.

## Building your own stylesheets

The `stylesheet!` macro is part of `framework-core`. Nothing in it
is idea-ui-specific. To build your own component library or just
some app-local styles:

1. **Declare your theme** as above.
2. **Write `stylesheet!`** blocks for each styled surface (`Card`,
   `Btn`, `Heading`, …). Each one ties to your theme type via the
   `<MyTheme>` generic.
3. **Attach stylesheets** to primitives at the call site via the
   `style = ...` prop inside `ui!`.

You can mix styling sources freely. A primitive's `style` slot
takes any `IntoStyleSource` — a stylesheet builder, a raw
`StyleRules`, a closure that returns one. You can hand-write rule
sets for one-off cases and use the macro for the rest.

## Pre-generation

For backends that benefit from up-front rule emission (web), the
framework calls `Backend::register_stylesheet` once per
`(stylesheet, theme)` pair and hands it the pre-resolved rules for
every variant combination. The web backend mints CSS classes
eagerly; subsequent `apply_style` calls just set `className`. This
is what keeps the per-node apply path cheap on web — no string
building, no rule-text emission inside the hot path.

You don't write code that interacts with pre-generation. It's the
framework calling backends; mentioned here only so the next section
about backend internals is less surprising.

## Where to read more

- [Reactivity](#) — how the active theme being a signal makes the
  reactive substrate do most of the work for free.
- [idea-ui](#) — a complete theme + stylesheet system built on this
  page's primitives. Use it, fork it, or read it for ideas.
- [Backends](#) — what the `apply_style`, `register_stylesheet`,
  and token APIs look like from a backend's side.
- [Animations](#) — how transitions fold into the broader animation
  story (Presence, GPU effects, gesture-driven motion).
