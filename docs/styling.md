# Styling

The framework owns the *data model* of styling — what a style is, what
variant axes exist, how the active theme propagates — but doesn't own
the *rendering strategy*. Each backend interprets a `StyleRules` value
however suits its platform: the web backend mints CSS classes, native
backends call view setters directly.

Implementation: `runtime_shared::style` (the data model, re-exported under
`runtime_core::…`) plus `runtime_vocabulary::style_attach` (the attach engine)
and `runtime_macros::stylesheet`.

---

## The shape of a style

### `StyleRules`

`StyleRules` is the bag of resolved style properties — concrete
values, no style tokens, no closures. Every field is `Option<T>`
because not setting a property is meaningful (vs setting it to a
"default" value), and because `StyleRules::merge` works by overlaying
`Some`s on top of `None`s.

```rust
pub struct StyleRules {
    pub background: Option<Color>,
    pub color: Option<Color>,
    pub padding_top: Option<Length>,
    pub padding_right: Option<Length>,
    // …layout, flex, typography, borders, shadows, transforms, transitions…
}
```

The style binding hands this to `StyleOps::apply_style(node, rules)` as
`Rc<StyleRules>`. The backend's job is to translate it into the
platform's native style format. Caching strategy is the backend's
problem.

### `StyleSheet`

A `StyleSheet` is **a set of rule-producing closures**, each keyed off
the active variant selection:

```rust
type RulesFn = Box<dyn Fn(&VariantSet) -> StyleRules>;

pub struct StyleSheet {
    base: RulesFn,
    variants: BTreeMap<VariantAxis, VariantAxisDef>,
    compounds: Vec<CompoundVariant>,
}
```

- `base`: the unconditional rules. Returns `StyleRules`.
- `variants`: per-axis overlay closures. Each axis (`size`, `kind`,
  `parity`, …) has one closure per declared value.
- `compounds`: overlay closures triggered when *all* `(axis, value)`
  pairs in `when` are simultaneously active.

There is no theme parameter. Closures take `&VariantSet`, never a
theme reference. Theme-dependent values (colors, spacings, radii) enter
through **named tokens** (`Tokenized::Token { name, fallback }`), not by
reading a theme struct inside the closure — see [Themes](#themes)
below. This keeps `StyleSheet` a single concrete type holdable in an
`Rc<StyleSheet>`, with the theme decoupled entirely from the sheet's
shape.

### `StyleApplication`

What the call site builds:

```rust
pub struct StyleApplication {
    pub sheet: Rc<StyleSheet>,
    pub variants: VariantSet,        // active axis selections
    pub overrides: StyleRules,       // per-call-site fine-tuning
}
```

`StyleApplication` is what `.with_style(...)` accepts. It carries
the chosen variants (discrete selections) and any per-instance
overrides (continuous values that don't fit the variant model — say,
a user-controlled font scale).

### Resolution

```rust
pub fn resolve(app: &StyleApplication) -> Rc<StyleRules>
```

Walks the layers and produces concrete rules:

1. `base` runs and produces the unconditional rules.
2. For each active variant `(axis, value)`, the axis's `value` closure
   runs and the result is **merged** into the accumulator.
3. For each compound variant whose `when` clause is fully active,
   its closure runs and merges into the accumulator.
4. `app.overrides` merges on top.

`merge` is simple property-wise: for each `Option<T>` field, the
right-hand side wins if it's `Some`, otherwise the left-hand side
is preserved. So variants don't have to set every property — they
only overlay what they care about.

Resolution is memoized: a `(stylesheet pointer, variants, theme
pointer, overrides content key)` tuple keys a `Weak<StyleRules>` map.
Cache entries with no live strong refs are opportunistically swept.

---

## The `stylesheet!` macro

`stylesheet!` is the declarative front-end:

```rust
stylesheet! {
    pub PerfRow<AppTheme> {
        base(t) {
            padding: 8.0,
            background: t.surface(),
        }
        variant parity {
            #[default]
            even(t) {
                background: t.surface(),
            }
            odd(t) {
                background: t.surface_alt(),
            }
        }
    }
}
```

(`AppTheme` is the app's own theme struct — see
[Your own tokens](#your-own-tokens-derivetokens).)

A few things about the grammar:

- The `<…>` slot names the sheet's **token vocabulary**, and each block's
  binding (`base(t)`) is that vocabulary. `<()>` declares none — its
  bindings are conventionally written `_theme` / `_t`. The binding carries
  token *names*, not the theme's values: values still arrive at resolve
  time from the token registry, which is what makes a theme swap one write
  per token. Reading `theme.colors.primary` therefore doesn't compile —
  there is no such field.
- A vocabulary is either your app's own `#[derive(Tokens)]` theme
  (`<AppTheme>` → `t.surface()`, `t.accent.soft_bg()`) or a design
  system's (`<IdeaThemeRef>` → `t.color.surface()`, `t.spacing.md()`; see
  the idea-ui theming guide). Either way the accessor path *is* the token
  name, the fallback comes from the theme's own values (you restate no
  hex), and a name that doesn't exist fails the build rather than
  silently rendering its fallback.
- A bare literal (`padding: 8.0`, `background: "#fff"`) becomes
  `Tokenized::Literal` via `From`. `Tokenized::token("name", fallback)`
  is the untyped form underneath every accessor — a string name plus a
  restated fallback, checked by nothing. Reach for it only when the name
  is genuinely computed at run time.

It produces:

1. A `pub fn PerfRow() -> PerfRow` builder constructor, plus a
   `pub fn perf_row_style() -> Rc<StyleSheet>` (snake_case + `_style`)
   that returns the cached sheet.
2. A typed variant enum `PerfRowParity { Even, Odd }` per declared axis,
   with a `Default` impl picking the `#[default]` arm.
3. A builder with a method per axis: `PerfRow().parity(PerfRowParity::Odd)`.
4. The underlying `Rc<StyleSheet>` cached in a thread-local so
   repeated calls return the same `Rc` and the resolution cache
   stays hot.

Every variant method on the builder accepts **anything convertible
to a closure** that reads the value. The same setter works for
static values, enum values, and `Signal<T>`:

```rust
let scale = signal(1.0_f32);
PerfRow().parity(PerfRowParity::Odd).override_padding(scale)
```

When the builder produces its `StyleApplication`, signal reads inside
the variant-source / override-source closures subscribe naturally to
the apply-style `Effect` — so the style re-applies when the signal
changes, with no additional ceremony.

---

## Component styles: `component_styles!`

A component library declares each component's styled sections once:

```rust
component_styles! {
    /// The styled sections of [`Field`].
    pub Field<IdeaThemeRef> {
        axis appearance { #[default] outline, contained, bare }
        slot input {
            extend(input_tone_arms)          // optional: computed arms
            base(t) { border_radius: t.radius.md() }
            variant appearance { outline(_t) {} contained(t) { background: t.color.surface_alt() } bare(_t) {} }
            state focused(t) { border_color: t.color.focus_ring() }
        }
        slot label { base(t) { color: t.color.text() } }
    }
}
```

Each `slot` body is the `stylesheet!` grammar (no `override` lines). The
macro generates `FieldStyles` (the registry key, implementing
`ComponentStyles`), one key newtype per axis (`FieldAppearance`, with an
associated const per value and a `const fn define(&'static str)` that
rejects a non-class-safe key at compile time), `FieldRestyle`, and
`FieldSlotStyles` (per-instance overrides, added to a props struct by
`#[props(styles = FieldStyles)]`).

**App values.** `axis_values! { impl FieldAppearance { Pill } }` (in the
axis type's crate) or `axis_values! { pub trait AppAppearances for
FieldAppearance { Pill } }` (from any other crate) adds named values whose
keys derive from the names (`Pill` → `"pill"`, `PillShape` →
`"pill_shape"`): each is `FieldAppearance::define(key)`, so it reads like
a built-in at call sites, in restyle arms and in `match` patterns (an open
axis needs a `_` arm). See the design-systems guide.

**Composition.** `FieldStyles::input()` returns a `SlotStyle` over the
slot's *composed* sheet, cached per component in a thread-local registry
keyed by the styles type's `TypeId` (two libraries can both ship a
"Field" without colliding). Composing a slot runs, in order: the
declaration, the slot's `extend` hook, every app restyle in registration
order (a re-registration from the same call site replaces its entry), the
component-level axis defaults for axes a slot only meets through a
compound leg, and app axis defaults (`set_default`). Within the sheet,
resolution is the stylesheet's: base, axis arms in axis-name order,
states, then compounds — compounds outrank states.

**Premint.** An unrestyled slot premints under the class the macro hashed
from its declaration. A restyled one premints under a deterministic
identity — the declaration's class plus a descriptor of each layer (its
target — base, an axis value or a compound — and the content key of the
rules it produced) — so
the same restyle yields the same class in the dump build and in the
shipped bundle. Under `--cfg idealyst_premint_dump` every linked
component registers a composition hook in `PREMINT_COMPONENT_STYLES`;
`premint::assembled_sheets()` runs them after `app()`, so a component no
crawled route rendered still ships its CSS.

**Restyle timing.** Restyles belong at boot. A restyle registered after
one of the component's slots was handed to an element recomposes for new
elements only, and debug builds warn once naming the component. Theme
installs and swaps do not touch the registry (`invalidate_all` recomposes
every component when a registered tone set changes).

**Per-instance overrides.** `SlotStyle::overridden(Option<&SlotOverride>)`
layers plain rules — plus optional per-value arms matched against the
slot's current variant set — as the application's inline layer: it wins
over every class layer and keeps the application premintable. It is data,
not a sheet, because it resolves in Rust at apply time and a
`--premint-only` bundle has no rule closures to call.

**Unknown values.** Selecting an axis value no slot styles (an app value
with no restyle arm) renders the base; debug builds warn once
(`check_axis_value`).

---

## Style tokens

A **style token** is a named value that stylesheets read by name —
`Tokenized::token("color-accent", fallback)`. Its job is cheap runtime
restyling: each token name owns a `Signal<TokenValue>`, so calling
`update_tokens(["color-accent"])` re-applies style only on the
components that read `color-accent`, with no stylesheet recomputation
anywhere else. A token behaves like a signal scoped to "every node
using this style value."

A **theme** sits one level up: a named collection of style-token values
(light, dark, a brand palette). Assembling tokens into a theme is a
separate concern from the token mechanism — the framework core only
holds the flat `(name → value)` table; a component library curates
which tokens exist and what each theme sets them to (see the closing
note).

A token reference is a `Tokenized<T>`:

```rust
pub enum Tokenized<T> {
    Literal(T),
    Token { name: &'static str, fallback: T },
}
```

- `Tokenized::token("color-accent", fallback)` (or `Tokenized::Token {
  name, fallback }`) references a token by name. The `fallback` is used
  on backends with no runtime-variable system, before the token is
  installed, or if the installed value is the wrong variant.
- A plain value (`8.0`, `"#fff"`) is `Tokenized::Literal` via `From`.

The token *table* is installed once at startup and swapped reactively:

```rust
install_tokens(&[
    TokenEntry { name: "color-accent",  value: TokenValue::Color(Color("#5b6cff".into())) },
    TokenEntry { name: "color-surface", value: TokenValue::Color(Color("#ffffff".into())) },
    TokenEntry { name: "spacing-md",    value: TokenValue::Length(Length::Px(12.0)) },
]);

// later — e.g. a light → dark swap. Only nodes that read these
// token names re-apply:
update_tokens(&[
    TokenEntry { name: "color-surface", value: TokenValue::Color(Color("#111317".into())) },
]);
```

`install_tokens` may be called more than once — a base theme, then an
extension's own token block. Every install made before the backend next
receives tokens is delivered together: the union of the sets, with a
name installed twice carrying its later value.

`TokenValue` is `Color`, `Length`, or `Number(f32)` — the variant must
match the `Tokenized<T>` reading it (a mismatch warns in debug and
falls back).

Mechanically, each token name owns a thread-local `Signal<TokenValue>`
in a registry. `Tokenized::<T>::resolve()` (called inside the
apply-style `Effect`) reads that signal, so the effect subscribes
**only** to the tokens it actually reads. `install_tokens` seeds the
registry; `update_tokens` calls `.set(..)` on the named entries (inside
a `batch`, so an effect reading several changed tokens re-runs once)
and clears the resolution cache.

Token updates propagate through the existing reactivity system. No
re-render, no diff. The set of styled effects subscribed to a changed
token is exactly the set that needs to re-apply, by construction.

> **Web vs native.** On web, tokens become CSS custom properties —
> `var(--color-accent, #5b6cff)` — so a theme swap is one variable
> write per token, no class regeneration. On native backends (iOS,
> Android) there's no variable system, so `resolve()` yields the
> concrete value and the swap re-applies the affected nodes directly.
> The observable result is identical.

> **Typed themes sit on top.** The flat `(name → TokenValue)` table is
> all the core knows. A `#[derive(Tokens)]` theme (next section) is a
> typed layer that *emits* token entries into it — nothing below it
> changes. idea-ui's theme is one too: `IdeaThemeDefaults` derives
> `Tokens`, and `install_idea_theme` installs through it.

### Your own tokens: `#[derive(Tokens)]`

An app declares its tokens as ordinary structs. The struct *is* the
vocabulary; a value of it *is* a theme.

```rust
use runtime_core::{stylesheet, Color, Length, Tokens};

/// A colour role. A SHAPE: reusable under any field name.
#[derive(Tokens, Clone)]
pub struct Swatch {
    pub solid_bg: Color,
    pub solid_text: Color,
    pub soft_bg: Color,
}

#[derive(Tokens, Clone)]
pub struct Spacing { pub sm: Length, pub md: Length, pub lg: Length }

#[derive(Tokens, Clone)]
#[tokens(default = light)]          // a THEME: installable, and where fallbacks come from
pub struct AppTheme {
    #[tokens(app_background)]       // also themes the window/body behind the app
    pub background: Color,
    pub surface: Color,
    pub surface_alt: Color,
    pub text: Color,
    pub accent: Swatch,             // nested set = namespace
    pub danger: Swatch,             // same shape, second prefix
    pub space: Spacing,
    pub radius: Length,
}

fn light() -> AppTheme {
    AppTheme {
        background: "#f3f4f6".into(),
        surface: "#ffffff".into(),
        surface_alt: "#f9fafb".into(),
        text: "#111827".into(),
        accent: Swatch { solid_bg: "#4f46e5".into(), solid_text: "#ffffff".into(), soft_bg: "#eef2ff".into() },
        danger: Swatch { solid_bg: "#dc2626".into(), solid_text: "#ffffff".into(), soft_bg: "#fef2f2".into() },
        space: Spacing { sm: 4.0.into(), md: 8.0.into(), lg: 16.0.into() },
        radius: 8.0.into(),
    }
}

/// A variant theme is "the base, except …".
fn dark() -> AppTheme {
    light().with(|t| {
        t.background = "#030712".into();
        t.surface = "#111827".into();
        t.text = "#e5e7eb".into();
        t.accent.soft_bg = "#1e1b4b".into();
    })
}

stylesheet! {
    pub Card<AppTheme> {
        base(t) {
            background: t.surface(),
            border_color: t.accent.soft_bg(),
            padding: t.space.md(),
            border_radius: t.radius(),
        }
    }
}
```

Installing, inside `app()` like every theme install:

```rust
AppTheme::install(light());                     // once, before first render
AppTheme::set(dark());                          // swap: same names, new values (event-handler safe)
AppTheme::follow_system(light(), dark());       // active = platform preference; BOTH declared as palettes
AppTheme::install_reactive(move || if dark_mode.get() { dark() } else { light() });
```

`follow_system` reads `color_scheme()` once (it is not a live OS
subscription) and calls `install_theme_palettes` with `"light"` /
`"dark"` palettes, so a static render paints the right one for each
reader (see below). `install_reactive` installs on its first run and
`set`s on every later one; calling it again replaces the previous
selector. Outside a sheet, `AppTheme::tokens()` gives the same
accessors — `AppTheme::tokens().accent.solid_bg().resolve()` for a
component that resolves a token itself — and `theme.token_entries()` /
`theme.palette(name, scheme)` give the raw entries.

**Names.** A leaf's token name is its field path in kebab case:
`surface` → `surface`, `accent.soft_bg` → `accent-soft-bg`,
`space.md` → `space-md`. A leaf is read by calling it (`t.surface()`);
a namespace is walked through its field (`t.accent.soft_bg()`). The
names are `&'static str`s built at compile time — the theme flattens
its field tree by const evaluation — so the same shape under `accent`
and `danger` yields two disjoint sets. Token names share ONE global
registry with every other vocabulary in the app, so a theme that sits
beside idea-ui should avoid idea's prefixes (`color-`, `intent-`,
`palette-`, `effect-`, `spacing-`, `radius-`, `typography-`) — or take
`#[tokens(prefix = "app")]`, which turns every name into `app-…`
while leaving the accessor paths alone.

**Leaves.** `Color`, `Length` and `f32` (a `TokenValue::Number`) are
leaves. A domain newtype becomes one by implementing `TokenLeaf`
(`type Value = Color; fn token_value(&self) -> Color`); its accessor
returns `Tokenized<Color>`. A field that is not a token at all — a
`FontFamily`, a label — takes `#[tokens(skip)]`: it stays on the struct
(and moves with `.with`) but has no accessor and installs nothing.

**Fallbacks come from the `default` theme.** A token's fallback is
observed only before a theme installs, as the second argument of the
web backend's `var(--name, fallback)` (including build-time premint
CSS, which bakes it with no theme in sight), and as the default the MCP
catalog shows — a catalog extracted from a binary that never runs the
app. None of those can ask "what is installed", so a theme names its
default instance with `#[tokens(default = light)]` and every accessor's
fallback is read from it (built once per thread). You still write each
value exactly once. The *installed* value always wins at resolve time;
installing `dark()` does not change the fallbacks. A struct without
`default` is a shape: it nests inside a theme, but installing it or
naming it as a stylesheet vocabulary is a compile error that says to
add `default`.

**What fails to compile**: a misspelled accessor (`t.surfce()` — with a
"did you mean `surface`" hint), a theme function that leaves a field
out, a field that is neither a leaf nor a token set, installing a
shape, two fields that kebab to one name (`accent_bg` beside
`accent.bg`), and `app_background` on a non-`Color` field.

**Tooling.** Each theme registers its whole vocabulary with the MCP
catalog (`list_tokens` shows every token with its accessor path, kind
and default). The catalog's source scan (`idealyst mcp --watch`) cannot
read const-evaluated names, so a crate defining a theme is compiled for
the catalog instead of scanned.

**Reading the active theme.** `AppTheme::current()` returns the theme
last installed or set (`Rc<AppTheme>`; `try_current()` is `None` before
an install) for code outside a stylesheet — a navigator header colour, a
canvas fill. It is a reactive read: an effect or reactive `ui!`
expression that calls it re-runs on every swap, and a swap is visible to
a read in the same turn. Prefer tokens in stylesheets: a token
reference re-tints through the registry without re-running anything.

**One owner per name.** Token names share one registry, so two token
sets naming the same token would silently overwrite each other —
whichever installed last would repaint the other's components. Each set
therefore OWNS its names in a world: installing a set that names a token
another, unrelated set already installed panics (in release builds too),
naming both sets and the token. Re-installing or swapping the same set
is fine, and so is a set and a set it embeds with `flatten` (below).
Untyped `install_tokens` / `update_tokens` writes — a theme editor's
live edit, a tone's extension tokens — claim nothing and are not
checked. idea-ui's theme is such a set (`IdeaThemeDefaults`), so an app
token named `spacing-md` or `color-surface` is refused rather than
re-spacing or re-tinting idea-ui; use `#[tokens(prefix = "app")]` or
build on idea's set instead.

#### Building on another set: `flatten`, `rename`, `as`, computed tokens

```rust
use idea_ui::{light_theme, IdeaThemeDefaults};

#[derive(Tokens, Clone)]
#[tokens(default = app_light)]
pub struct AppTheme {
    pub brand: Color,                 // the app's own token: `brand`
    #[tokens(flatten)]
    pub idea: IdeaThemeDefaults,      // idea's tokens, under idea's own names
}

fn app_light() -> AppTheme { AppTheme { brand: "#0ea5e9".into(), idea: light_theme() } }
fn app_dark() -> AppTheme {
    app_light().with(|t| { t.idea = idea_ui::dark_theme(); t.brand = "#38bdf8".into(); })
}

stylesheet! {
    pub Hero<AppTheme> {
        base(t) { background: t.color.surface(), border_color: t.brand(), padding: t.spacing.lg() }
    }
}
```

A flattened set's tokens keep their names (`color-surface`) and its
accessors are the embedding set's too (`t.color.surface()`, through
`Deref`). One field per struct may be flattened. A token name the two
sets share, or an embedding-set accessor that would hide one of the
base's (`pub color: Color` beside a base with a `color` namespace), is a
compile error. An app that embeds idea's set still calls
`install_idea_theme` (it registers the theme's tones and publishes its
default font) — the embedding is what lets both installs write idea's names.

Three more attributes shape how a struct maps onto tokens:

- `#[tokens(rename = "color")]` on a field names its accessor and its
  token segment — idea's `colors` field is the `color-…` namespace,
  reached as `t.color`. One attribute moves both because the accessor
  path *is* the token name.
- `#[tokens(as = Length)]` (a field, or the whole struct) installs a
  leaf as another registry kind via `From` — idea stores spacing as bare
  `f32` px and installs `Length::Px`.
- `#[tokens(computed(pill: Length = Radius::pill))]` on a struct adds a
  token with no stored field, valued by `fn(&Self) -> Length` — for a
  value that is a property of the theme rather than a knob on it (idea's
  `radius-pill` is always `Length::Full`).

The derive's contract is documented in `runtime_vocabulary::tokens`.

### Declaring every palette (static rendering)

`install_tokens` publishes the palette that is active *right now*, which
is all a live app needs — it re-publishes on every swap. A statically
rendered page is different: its HTML is written at build time, before
the reader's system preference or stored choice exists. Ship only the
active palette and a dark-preferring reader gets the light one painted
first, corrected to dark only once the wasm bundle boots. That is the
white flash on nearly every SSG site.

`install_theme_palettes` fixes it by declaring the palettes the app can
switch between, so a static render can serialize them all:

```rust
install_theme_palettes(&[
    ThemePalette { name: "light", scheme: Some(ColorScheme::Light), tokens: light_tokens() },
    ThemePalette { name: "dark",  scheme: Some(ColorScheme::Dark),  tokens: dark_tokens() },
]);
```

SSR turns that into, after the active `:root` block:

```css
:root{color-scheme:light dark;}
@media (prefers-color-scheme: dark){:root{--color-background:#171512;/* …deltas… */}}
:root:where([data-theme="dark"]){--color-background:#171512;/* …deltas… */}
```

so the **first paint already matches the reader, with no JavaScript**.
Only tokens that differ from the active palette are repeated, and
`color-scheme` is declared so the browser paints its own canvas, form
controls and scrollbars to match — without it a dark page still flashes
a white canvas behind perfectly correct tokens.

A `#[derive(Tokens)]` theme does it in one call —
`AppTheme::follow_system(light(), dark())` installs whichever matches
`color_scheme()` and declares both (`theme.palette(name, scheme)` builds
one palette by hand). `idea-ui` wraps the same case as
`install_idea_theme_schemes(light_theme(), dark_theme())`. Apps driving the
theme from their own signal keep their existing install and call
`install_theme_palettes` alongside it (`idea_theme_palette(name, scheme,
theme)` converts a typed theme into a palette).

**Persisting a user's choice is the app's job**, deliberately. CSS
cannot read `localStorage`, so a stored override that must survive the
first paint needs a small blocking inline `<head>` script that stamps
`data-theme="dark"` on the document element; the emitted CSS already
answers to it. The framework does not inject that script, and an app
that stamps the attribute owns keeping it in sync when the user toggles.

The attribute rules are wrapped in `:where()` so they add no
specificity. That is load-bearing: it keeps them at `(0,1,0)`, the same
as the plain `:root` the web backend writes at boot, so the attribute
wins before boot (later in the sheet) and the running app's own theme
wins after (its sheet is appended later). The bare `[data-theme]` form
would be `(0,2,0)` and would permanently pin whatever the attribute
said, overriding every subsequent theme swap.

Backends that render live ignore all of this; they keep receiving the
active palette through `install_tokens` exactly as before, and an app
that declares no palettes emits byte-identical CSS.

---

## How styles reach the backend

`runtime_vocabulary::style_attach::attach_style(backend, node, prop)` is
the seam between author-facing style values and the backend. The author
value arrives as a `StyleProp` with six arms — `Static`, `Dynamic`,
`Sheet`, `SheetDynamic`, `SignalClass`, `Preminted` — and each primitive
handler calls `attach_style` once with whatever the builder collected.

For each styled node on the sheet paths:

1. Allocate a per-node `Signal<StateBits>` (initially `NONE`). This is
   the state machinery for `state hovered { ... }` overlays.
2. Build an effect that:
   - Calls the source closure to get a `StyleApplication`.
   - Calls `ensure_registered_with(sheet, register=…, unregister=…)`
     to lazily pre-generate this sheet's variants for the active
     theme (the first time the backend sees it).
   - Resolves to `Rc<StyleRules>` against the active theme.
   - If `StyleOps::handles_states_natively() == true`, calls
     `apply_styled_states(node, base, &overlays)`.
   - Else calls `apply_style(node, &resolved)` where `resolved` is
     the base merged with any active state overlay.
3. Wire the per-node state signal: it re-fires the apply-style
   effect when state bits change, which re-resolves with the new
   bits and re-applies.
4. If the backend supports it, call `attach_states(node, setter)` so
   the backend's native input listeners can flip the state bits.

Author code never touches any of this. The two paths — declarative
(CSS pseudo-classes) and event-driven (signal-flip) — both come out
of the same author-side stylesheet declaration, and the backend
opts into whichever it can support.

### `pointer_events` — hit-transparency

`StyleRules::pointer_events` follows CSS semantics: `None` makes the
node *and its subtree* hit-transparent (clicks/touches resolve to
whatever renders behind it), and a descendant with explicit `Auto`
re-enables itself and its own subtree. The nearest explicit value on
the ancestor chain wins, mirroring CSS inheritance.

This is what makes the always-mounted overlay pattern safe: a
full-viewport scrim or toast strip styled `pointer_events: None`
stays inert until it flips to `Auto` (idea-ui-nav's `AppShell`
drawer scrim, `overlay().click_through(true)` with interactive
children opting back in).

Per backend: web/SSR emit the CSS property verbatim; iOS and macOS
enforce it in the framework host view's hit-test override
(`hitTest:` returns `nil`), with the verdict logic shared —
host-testable — in `backend_apple_core::pointer_events_policy`.
On iOS that includes the portal container
(`IdealystOverlayPassthroughView`): it is the node `create_portal`
returns, so a click-through overlay's `None` lands on it, not on an
`IdealystTouchView`. UIKit's `userInteractionEnabled = NO` is
deliberately NOT used: it disables the whole subtree with no way for a
descendant `Auto` to re-enable. Linux models it in `IdealystView`'s
`contains` for the same reason (GTK `can-target` drops the whole
subtree); a leaf widget (a label) only honors its *own* explicit value
there. Windows applies the same rule in its hit walk
(`backend_windows::scene::hit_chain`), which every painted-view input
channel shares: pressable clicks, `on_touch`, `on_wheel` and `on_hover`.
Android and the GPU backend do not implement `pointer_events` yet — an
always-mounted `None` overlay will swallow input there.

A portal's own `style` (what `click_through` sets) is layered OVER the
container's placement layout on every native backend, never in place
of it — the placement's `justify_content` / size would otherwise be
reset and a `Bottom` overlay would land at the top.

### `z_index` — sibling stacking

`StyleRules::z_index: Option<i32>` orders a node among its SIBLINGS.
Siblings paint in ascending effective z, ties in tree order, so with no
`z_index` anywhere the last sibling is on top, as before. The effective
value is `StyleRules::stacking_z()`: the `z_index`, else `1` for
`position: Sticky` (a pinned element paints over what scrolls beneath
it), else `0`.

```rust
stylesheet! {
    pub Drawer<IdeaThemeRef> {
        base(_t) { position: Position::Absolute, z_index: 3 }
    }
}
```

The same rules hold on every backend:

- **No `position` needed.** A plain flex item raises like an absolute
  one. (CSS honours `z-index` on flex and grid items, which every
  framework node is on web, so the lowering never forces a position.)
- **A node with a `z_index` contains its subtree.** Its descendants
  paint with it; a descendant's own `z_index` orders it among *its*
  siblings only and never lifts it past an ancestor's siblings. On web
  that is the stacking context a non-`auto` `z-index` creates; native
  views always contain their subviews. Setting `z_index: 0` is the way
  to contain a subtree without raising it.
- **Hit-testing follows paint.** Where siblings overlap, the one painted
  on top receives the pointer.
- **Non-negative.** A negative value is treated as `0`, with a debug
  warning: CSS would paint it behind its own parent's background, which
  no native view can do. Raise the sibling that should be on top.

Animated `AnimProp::ZIndex` drives the same channel in the same units,
so a static `z_index: 2` and an animated `2.0` order identically. Once
an animation has written `ZIndex`, that value overrides the static
`z_index` (a style re-apply updates the static value underneath but
does not cancel the animated one), as animated opacity does.

Per backend: web/SSR emit CSS `z-index`. iOS and macOS write
`layer.zPosition`, and the framework host views' `hitTest:` walk their
subviews in the same order (UIKit and AppKit would otherwise hit-test by
subview index and ignore `zPosition`). A reactive anchor's layer is a
`CATransformLayer`, so `z_index` on its children stacks against the
anchor's siblings exactly as web's `display: contents` does; portal
containers are kept in paint order within the window / host view; macOS
robot screenshots render in Core Animation paint order. Android writes
`View.setTranslationZ`, which `ViewGroup` uses for both drawing and
touch dispatch. One z unit is 0.01 px of Z, so it casts no visible elevation
shadow (on API 28+ a raised view's outline shadow is disabled outright;
on API 21–27 a very large `z_index`, around 1000, can cast a faint one). GTK, Win32, the CPU and terminal renderers and the GPU
engine order each run of siblings with `runtime_shared::style::z_paint_order`.
Win32 and the CPU, terminal and GPU renderers paint in that order and walk
it in reverse for hit-testing and wheel routing. GTK reorders the actual
widget sibling list (and `IdealystView`'s paint list), because GTK picks
children from the last sibling backward; a scroll view's children are
reordered in its `GtkFixed` document. On Win32, native controls (buttons,
text fields, sliders) are child windows and always sit above painted
views, whatever their `z_index`.

---

## Preminted styles (build-time CSS)

`idealyst build --web --premint` moves static styles out of the wasm
entirely. The pipeline has two halves, coordinated by nothing but a
shared naming scheme:

1. **Dump build.** The CLI generates an ephemeral native binary that
   links the app with `--cfg idealyst_premint_dump` and the
   `style-dump` feature. The registry itself lives in `runtime-shared`
   (`runtime_shared::premint`); `runtime-core/style-dump` forwards it
   through the vocabulary so the macro's registration — which spells
   `::runtime_core::premint::…` and retargets to
   `::runtime_vocabulary::glue::premint::…` — resolves. Every
   `stylesheet!` in the app and its
   dependencies registers into a link-time distributed slice; the
   binary enumerates each sheet's full variant space, resolves it
   through the same layer resolution the live web engine uses, and
   emits the result as CSS. The output lands in the bundle as a
   content-addressed `pkg/premint.<hash>.css`, linked from
   `index.html`.

   The dump does not just BUILD the app — it **mounts it, on every
   literal route**, reusing the SSG crawl (the navigator's headless
   deep-link slot plus the shared route collector, one fresh `World`
   per route). Both halves of that are load-bearing, and each was
   learned from a silent breakage:

   - A sheet assembled at MOUNT rather than during `app()` — inside a
     component body, or behind a style closure that runs when a node
     attaches — is invisible to a build-only pass. It gets no CSS, and
     the shipped bundle then stamps a build-time class with nothing
     behind it: a silently unstyled node. AppShell's scrim/panel/content
     broke 144 of 400 catalog elements this way.
   - A sheet that first appears on a LATER route is invisible to a
     pass that only mounts `/`. Same failure, one level out.

   Parameterized routes (`/user/:id`) are reported and skipped — the
   crawl cannot invent a param value, the same limitation `--ssg` has.
   Styles reachable only through one of those, or only after an
   interaction, are still not seen — but no longer silently: at boot a
   premint build scans the loaded CSS asset for the classes it actually
   contains (`backend-web`'s minted-class guard →
   `runtime_shared::install_minted_classes`), and the attach paths
   consult that set per evaluation. A sheet whose class has no CSS
   falls back to the live engine on `--premint` (correct rendering,
   plus a once-per-class console warning naming the class — the
   dev-time tripwire, since the fallback would otherwise hide the bug
   until a `--premint-only` build hits it; `--premint-report` also
   names it), or panics on `--premint-only` with a message that says
   "constructed after the dump crawl" rather than the generic
   violation.

   **The crawl contract**, in authoring terms: the crawl mounts routes
   but never opens, pushes, or focuses anything, so a
   construction-registered sheet (`premint_as`, auto-preminted
   `r#static`) must be constructed by *mounting a route*. Anything
   that first constructs on interaction — a modal's body, a toast
   card, a drawer's open state — needs a construction-independent
   spelling:

   - a `stylesheet!` declaration (link-time registration) for the
     subtree's sheets;
   - a **variant axis** for enumerable state. Sheet *identity* must
     never depend on runtime state: two sheets keyed by
     `open`/`closed` mean the `open` one first constructs on the
     user's tap, after the crawl (AppShell's drawer panel had exactly
     this bug — the fix is one sheet with an `open` axis, both arms
     registered while the crawl constructs the closed shell);
   - the **inline layer** (`with_inline`) for continuous or open-set
     per-instance values (safe-area insets, a per-instance typeface).
     `with_computed` is a premint disqualifier and additionally has
     only one slot — prefer the other three spellings in library code.

   **Lazy boundaries are NOT interaction-gated.** A
   `#[component(lazy)]` screen is route-reachable, so the crawl
   resolves it: the dump installs host-mock's queue executor and, after
   each route's mount + flush, pumps the executor and re-flushes to a
   fixed point, which resolves every lazy load (native lazy bodies are
   compiled inline and their futures are ready on the first poll) and
   executes the bodies — one round per *nesting* generation, siblings
   together. Styles constructed inside a lazy body therefore mint like
   any other mount-time sheet. This was a real gap once: before the
   dump pumped, every lazily-split screen contributed only its loading
   placeholder, and a 50-page `--premint-only` catalog panicked on its
   first navigation. Sheets a lazy body constructs *on interaction*
   still follow the contract above.

   Both traps are also caught statically: `idealyst lint` flags a sheet
   identity or `cached_stylesheet` key selected by a runtime conditional
   (`premint-state-keyed-sheet`) and any `with_computed` layer
   (`premint-computed-layer`). The runtime warning only fires on paths a
   dev actually exercises; the lint covers the untested ones.

   The whole pipeline also runs in the dev loop: `idealyst dev --web
   --local --premint` (or `--premint-report` / `--premint-only`)
   re-runs the dump on every save, serves the fresh `pkg/premint.css`
   with the `<link>` spliced into the served HTML, and compiles the
   wasm with the same cfgs the deploy build gets — so guard warnings
   and po panics surface while iterating, not after a deploy. Requires
   `--local` (runtime-server mode resolves styles server-side).
   Composes with `dev --ssr`: the SSR wrapper builds with the same
   premint cfgs, stamps the same classes, and links `premint.css` from
   every served page.
2. **Shipped build.** The wasm compiles with `--cfg idealyst_premint`,
   which flips each `stylesheet!` builder's `into_style_prop` to a
   fast path: an all-constant application (plain variant values, no
   overrides, nothing reactive) returns
   `StyleProp::Preminted { class }`. `attach_style` stamps the classes
   on the node via `DocumentOps::attach_html_class` and stops — no
   `StyleRules`, no resolution, no rule minting. State overlays (`state hovered { … }`), breakpoints, and
   container queries ship as pseudo-class/`@media`/`@container` rules
   inside the same `.css`, so the browser drives them.

Class names derive from an FNV-1a hash of each sheet's source text,
computed inside the macro — the dump build and the shipped build agree
byte-for-byte with no manifest or build coordination between them.

Sheets with no macro expansion site register at runtime instead, and
there are two kinds:

- **Runtime-assembled builders** (idea-theme's component sheets) carry
  an explicit `premint_as("identity")` — the identity string hashes to
  the class, and the author owns keeping it content-descriptive.
- **Static sheets need no identity at all.** `StyleSheet::r#static`
  derives its class from the RULES' content key automatically — the
  dump and the shipped bundle compute the same name from the same
  rules independently, so "extract the CSS from the build" is the
  default for every plain-rules sheet in every app, with zero
  ceremony. Content-equal sheets share one class and one rule.
  `premint_as`/`premint_with_class` still REPLACE the auto class when
  a stable name matters (parameterized identities like per-px icon
  sizes), and any layer-adding mutator (`variant`, `variant_default`,
  `compound`) RETRACTS it — the auto class names exactly the base
  rules, so a sheet that grows layers falls back to explicit identity
  or the live engine rather than stamping a class whose CSS misses a
  layer.

### The delta model

Emission is **per layer, not per combination**: the dump writes the
sheet's base as `.iy-<hash>`, and each variant arm as its own DELTA
rule `.iy-<hash>-<axis>-<value>` containing only that arm's
properties. The runtime stamps one class per selected axis (an unset
axis contributes its `#[default]` arm's class, or nothing when the
axis declares none), and the browser's cascade performs the merge the
resolver does live. CSS size is therefore the *sum* of a sheet's arms
rather than their cartesian product — on the website this is the
difference between 3 MB / 5,068 rules (per-combo) and 116 KB / 430
rules.

The equivalence rests on two facts, both load-bearing:

- **Every emitted selector has specificity (0,1,0)** — single classes,
  state pseudo-classes wrapped in `:where()` (which contributes no
  specificity), and `@media`/`@container` preludes (which never do).
  Equal-specificity rules cascade by source order per property, which
  is exactly `StyleRules::merge`'s later-wins.
- **Source order mirrors the resolver's merge order** — the
  [layer order](#layer-order) every backend follows: base →
  breakpoints (rank ascending) → containers (threshold ascending) →
  author axes (alphabetical) → states (in
  [state precedence](#state-precedence) order, interaction pseudos
  guarded by `:not([disabled])`). States used to be emitted
  before the author axes, the resolver's order before states were
  made to merge last, so on a premint build a variant beat a state
  that set the same property.

A sheet whose base declares a non-flex `display` (a `display: grid`
container) is **display-locked**: its arm/overlay deltas lower with the
`display: flex` auto-promotion suppressed and emit no
`flex-direction: column` pin companions, because the merged set's
explicit display wins and the live engine never pins a grid. (A
gap-only arm of a grid sheet used to stomp the base's `grid` back to
`flex` from later source order.)

Compound variants premint as CSS compound selectors
(`.iy-abc-appearance-solid.iy-abc-size-md`, specificity (0,2,0) —
above the single-axis arms, reproducing "compounds merge after every
axis"). No extra stamped class is needed; the selector does the
matching.

### What stays on the live engine

A sheet (or application) falls back to live minting when the build
can't prove the CSS at build time:

- **Reactive inputs** — a setter received a `Signal`/`derived`.
- **Runtime overrides** — `.override_*` values at the call site, or
  `with_style_overrides` layers.
- **A `font_family` the build can't prove constant** — a call like
  `active_font_family()` can vary at runtime. String literals (system
  stacks) and path/`&`-reference expressions (`&INTER` — a `static`
  `Typeface`, constant by construction) DO premint: the dump emits the
  family's `@font-face` rules into the same `.css` with served-file
  URLs, standing in for the runtime `register_typeface` that sheet
  registration would have performed. The `<link>` carries a
  `data-iy-font-families` attribute listing the shipped families, and
  the web backend's runtime registration skips those — an attribute
  rather than a stylesheet/`FontFaceSet` probe because those race the
  link's async load against wasm boot and double-fetch the files.
  `Embedded`-bytes faces have no build-time URL and are skipped with a
  dump warning (use a bundled source for preminted fonts).

Fallback is per-application and silent: the same build can serve one
`Card()` preminted and another `Card().padding(sig)` live.

### Theme state without registrations

A fully-preminted app registers no sheets, so the host-state flush
that normally rides registration gets its own driver: the first
`Preminted` attach installs a per-thread Effect that drains queued
theme tokens (as `:root` CSS variables), app background, scrollbar
theme, and the app key handler into the backend, re-firing on every
token-version bump. Theme swap therefore works identically — preminted
rules reference `var(--token, fallback)` like live-minted ones.

The theme's *default text font* is the one apply-time fill a
build-time rule can't reproduce — and it is the one place the live
engine's behavior differs **per attach path**, so the dump encodes it
in cascade rank rather than in any rule body:

- A sheet whose base names no `font_family` gets a specificity-(0,0,0)
  companion — `:where(.iy-<hash>) { font-family:
  var(--iy-default-font, inherit) }` — and the driver defines that
  variable from the installed theme
  (`StyleOps::apply_default_text_font`; web sets it on the document
  element, SSR emits it in the head CSS). With no default font
  installed the `inherit` fallback reproduces the plain cascade.
- The asset's FIRST rule is `.iy-font-inherit { font-family: inherit }`
  at (0,1,0). The **reactive** preminted attach paths
  (`PremintedDynamic`, the runtime-assembled-sheet diversion, the
  `--premint-only` dynamic arm) stamp that class alongside the sheet
  classes; the static path never does.

That sandwich reproduces both live behaviors from one static file: a
STATIC application takes the hook — the theme-default fold
(`fill_default_text_font`) — while a REACTIVE one takes `inherit`,
because the live reactive engine never folds and under an author font
on an ancestor (a brand `font_family: &TYPEFACE` on the root
container) inheritance yields the *author's* font, not the theme
default. Any `font-family` a sheet actually declares (base, arm, or
overlay) beats both: arms are (0,1,0) after `.iy-font-inherit` in
source order, overlays are (0,2,0), and the hook is (0,0,0) under
everything. Measured before this encoding: every reactive-styled node
on a brand-fonted site rendered the theme default under `--premint`
while the live build rendered the brand font.

`apply_default_text_font` publishes the font **two** ways, and both are
load-bearing:

1. the `--iy-default-font` **variable**, which preminted rule bodies
   read as above; and
2. a real `font-family` declaration **on the document root**, which
   every other node inherits.

The second exists because the two live apply paths treat the default
font asymmetrically. A **static** style application folds the theme
font into the node's own resolved rules (`fill_default_text_font`). A
**reactive** one deliberately does not — folding there would change the
minted class hash for every reactive-styled node and break SSR/live
class-name parity. So a reactively-styled node has no `font-family` of
its own and inheritance is its only supply; with the variable alone it
inherited past the root into the browser's serif fallback while an
identically-styled static sibling rendered in the theme font. The two
halves are pinned by `dynamic_sheet_path_does_not_fold_default_font`
(runtime-vocabulary) and
`regression_reactive_styled_node_inherits_theme_font` (backend-ssr).

**Every world publishes this, not just preminted ones.** The delivery
was originally gated on premint use, on the reasoning that only
preminted rule bodies read the variable — which covered the variable
but not the inheritance half: a live (non-preminted) build published no
document font at all, so `<body>`, plain containers, and every
reactively-styled node rendered in the browser serif. The font rides
the pending host-state queue (like app background and scrollbar theme)
so it reaches the backend even in a purely static app that never
installs the theme driver. Pinned by
`regression_live_world_publishes_the_default_text_font`
(runtime-vocabulary).

### Dropping the engine: `--premint-only`

`--premint` alone changes *when* rules are made, not what ships: the
engine still compiles in for the fallback paths.

`idealyst build --web --premint-only` (implies `--premint`) is the
build that actually removes it — **the styles stop existing in the wasm
code**. Under `--cfg idealyst_premint_only` the `StyleSheet`
constructors drop the author's rule closures at construction, so their
bodies are never named and LLVM removes them, together with the resolve
machinery they fed. What replaces each closure is a stub that panics
with the full explanation if anything resolves it at runtime. Measured
on the website corpus: wire 762,361 → 737,386 br; on idea-ui-docs, raw
wasm −34.6%.

The contract: with no engine on board, two things become build errors
in spirit (loud runtime panics in practice, at the first offender):

1. **Runtime rule composition.** Any application that can't premint —
   runtime slot `overrides`, a `with_computed` layer, a plain
   `Rc<StyleSheet>` with no `premint_as` identity — has nothing to fall
   back to. Run `--premint-report` first and drive the fall-through
   list to zero; `--premint` alone is always safe while offenders
   remain.
2. **Reading resolved `StyleRules` back in Rust.** A component that
   resolves a sheet to *use a value* (tint an icon with the container's
   fill color) is running the engine, even though it never attaches the
   result.

Framework components satisfy (2) through
`StyleApplication::attaches_preminted()` — true exactly when the build
carries build-time CSS (`--cfg idealyst_premint`) and the application
premints, checked per evaluation like the attach paths. On that path
Button (icon + loading spinner), Checkbox (custom checkmark icon), and
Tabs/SegmentedControl (label color) skip their resolve-reads: the
foreground already ships in the box's preminted class, so the web node
inherits it as `currentColor`/cascade color. That is strictly *more*
correct on web — the tint follows `__state_hovered` compound arms,
which the Rust-side snapshot never did. Native builds never see the
cfg, so they keep the resolved reads (native nodes don't inherit
color). Author code holding a resolve-read can apply the same gate, or
drop `--premint-only`.

**Recommendation:** `--premint-only` is the web production build to
reach for once `--premint-report` is clean. Historical note: the old
`style-dynamic` cargo feature that claimed this role is deleted — it
had stopped gating anything, and feature unification made it
unturnoffable. The cfg-at-construction approach above is its
structural successor: one home (the sheet constructors), one decision.

### Finding what didn't premint: `--premint-report`

`idealyst build --web --premint-report` builds exactly like `--premint`
— the engine is present, everything still works — but logs one warning
per DISTINCT style that fell through to the live engine, deduped across
the session:

```
[premint-report] #7 SheetDynamic at crates/ui/idea-theme/src/extensible/sheets.rs:401:21 \
  css=iy-167c896b0df2 overrides=false computed=hug axes=[appearance=primary_soft] rules=bg=T:…
```

The **origin** is the field that makes it usable: `#[track_caller]` on
the `StyleSheet` constructors captures the author's line, so a
fall-through names itself. Everything after it says WHY — `css=NONE…`
means the sheet was never given a `premint_as` identity, `overrides=true`
and `computed=<key>` are the two disqualifiers above, and `SheetDynamic`
means the application is reactive.

Walk the app's routes with the console open; the deduped set is the work
list. (Historical scale: the idea-ui catalog started at 218 entries
across 47 routes; after the text-slot sheets, the shadow split, the
static auto-premint, and the Table axis conversion it reached zero.)

### Premint × SSR/SSG

`--premint`/`--premint-only` compose with `--ssg`/`--ssr`. The server
binary is compiled with the same `--cfg idealyst_premint*` posture as
the wasm bundle (build-ssr injects the cfgs; premint server builds get
an isolated `target-premint/` dir because the RUSTFLAGS change would
otherwise invalidate the project's shared native cache), so the generic
`Preminted` attach arm stamps the same deterministic `iy-*` classes on
the server that the hydrating client stamps — and since hydration's
preminted re-stamp is `classList.add`, adoption meets the classes
already present and no-ops. Three pieces make the agreement exact:

- **Same guard set.** The generated wrapper resolves the staged
  `premint.css` (`backend_ssr::resolve_premint_css`, the premint twin of
  `resolve_bundle_module`), scans its `iy-*` selectors
  (`runtime_shared::scan_minted_classes` — the text twin of the web
  boot's JS stylesheet scan) and arms the same minted-class guard per
  render thread, so server and client make identical
  premint-vs-engine-fallback decisions per sheet.
- **Same fall-through names.** Engine-rendered sheets (overrides,
  `with_computed`, identity-less closures under plain `--premint`) mint
  `css::hash_class_name(content_key)` `ui-*` classes — one pure function
  shared by web and SSR, so those agree byte-for-byte too, exactly as
  they always did for non-premint SSR.
- **Cascade order.** Every rendered document links `premint.css` BEFORE
  its inline `<style>`s, mirroring the live web document order, so
  `ui-*` override/fall-through rules beat premint rules on source order
  at their shared (0,1,0) specificity.

Under `--ssg-static` the link is the page's only CSS for preminted
classes — no wasm exists to repair a missing rule — which is why the
wrapper emits it independent of hydration mode. `--premint-report` on a
serve-mode server re-warns per request (each renders on a fresh thread
with a fresh dedup set); dev-only noise, by design. Known follow-up:
font preloads are extracted from the inline head CSS only, so a font
used exclusively by preminted rules loads without a preload hint
(correctness unaffected).

### Current limits
- What premints: `stylesheet!` builder applications (link-time
  registration), runtime-assembled sheets with a `premint_as` identity,
  and every plain `StyleSheet::r#static` (auto, by content key). A
  closure-built `StyleSheet::new` with no identity is the shape that
  stays live.
- A component that INTROSPECTS a built element's style (re-deriving a
  cell's application to select axes on it — `Table`'s clickable rows)
  must hand the style over as an explicit **`StyleProp::Sheet`**:

  ```rust
  bound.with_style(StyleProp::Sheet(Box::new(TableBodyCell().into_style_application())))
  ```

  `into_style_application()` alone is NOT enough, despite reading like
  it should be. `IntoStyleProp for StyleApplication` has a preminted
  fast path, so a bare application premints to an OPAQUE class and the
  introspection silently finds nothing. That shipped: `Table`'s
  clickable rows lost their pointer cursor and their hover highlight in
  every `--premint` build, because `map_cell_style` found no application
  to compose with and the overlay was skipped without a word.

  The explicit spelling costs the premint nothing, because what the
  introspector COMPOSES must itself be premintable: `Table`'s row
  overlay selects the cell sheet's `interactive`/`row_hovered` AXES
  (every arm has build-time CSS; the whole-row hover is a class swap
  through the reactive diversion), and the `--premint-only` attach
  premints an explicit `Sheet` whose application qualifies. Composing
  with `with_overrides` instead would drag the application back to the
  live engine — which is exactly what the old override-based row
  overlay did.
- Anything with **runtime slot overrides** or a **`with_computed`
  layer** falls through to the live engine by construction. Overrides
  are per-call-site rules and a computed key maps to an arbitrary
  closure, so neither is a layer the dump could have enumerated.
- Native backends never see either cfg — they keep the full rules
  closures and the apply-time default-font fill. The observable
  styling is identical everywhere; premint changes only *when* the
  web's rules are produced.

---

## Shadows: box vs. text

Two fields, one CSS property each, on every node kind:

- `StyleRules::shadow` — the **box** shadow (the element's rectangle).
- `StyleRules::text_shadow` — the **glyph** shadow on a text node (hugs
  the letter outlines). Ignored by non-text primitives.

They used to be one field that lowered per node kind (`box-shadow` on
boxes, `text-shadow` on text). That forced shadowed text nodes onto
distinct class keys AND disqualified every shadowed sheet from
preminting — a build-time rule body can't know what kind of node will
wear the class. One field per property removes both: shadowed sheets
premint like any other, and a text and box node with equal rules share
one class.

Each backend converges on the output through its own mechanism
(CLAUDE.md §7):

| Backend | `shadow` (box) | `text_shadow` (glyphs) |
| --- | --- | --- |
| Web / SSR | `box-shadow` | `text-shadow` |
| iOS / macOS | CALayer shadow | CALayer shadow on the label (layer content is the glyphs) |
| Android | *(elevation, n/a in v1)* | `TextView.setShadowLayer` |

`text-shadow` and `box-shadow` share the exact `<x> <y> <blur> <color>`
grammar (neither takes a spread), so the mapping is lossless. The `y`
offset is positive-**down** on every backend — the coordinate flips
(AppKit's y-up layer, UIKit's y-down) are absorbed by the backend so a
`shadow { y: 2 }` lands below the glyphs everywhere.

### Theme-driven shadow colors

`Shadow::color` (and a gradient's `GradientStop::color`) is a
`Tokenized<Color>`, like `background`. A token color emits
`var(--name, fallback)` on web and SSR, so a class minted at build time
(premint) still re-tints when the theme swaps; native backends resolve it
on each apply, and the theme cohort re-applies on every swap. The content
key hashes a token color by NAME, so a glow is one class under every
theme. A literal color keys and renders exactly as before.

### Animating a shadow: `shadow_transition`

`transitions { shadow: 180ms EaseOut }` sets `shadow_transition` and
animates the box shadow's color, offset and blur. A shadow that appears or
disappears fades from or to transparent, the way CSS interpolates
`box-shadow: none`, which is what lets a hover state grow a glow from
nothing. Like CSS, the first paint never animates.

| Backend | Mechanism |
| --- | --- |
| Web / SSR | `transition: box-shadow` |
| iOS / macOS | `CABasicAnimation` on `shadowColor` / `shadowRadius` / `shadowOffset`, or a `shadowOpacity` fade when the shadow appears or goes (`backend_apple_core::shadow_layer::apply_own_shadow`). A shadow on the clipped-view sibling layer (below) snaps. |
| GPU (wgpu) | A color tween; offset and blur snap |
| Android / Windows / Linux | No box shadow is rendered, so nothing to animate |

### `shadow` together with `overflow: hidden`

CSS clips an element's descendants with `overflow` but never clips the
element's own `box-shadow`, so on web the two compose freely. One
CALayer can't do that: `masksToBounds` clips the layer's whole
composite, and an outer drop shadow is painted outside the bounds.
(`layer.mask` clips it too — a mask defines the visible part of the
layer *and* affects the shadow it renders.)

The Apple backends therefore synthesize a second CALayer for that
combination, inserted into the **parent's** layer directly beneath the
view's own, carrying the shadow. It isn't a descendant of the masked
layer, so nothing clips it — and the view hierarchy is untouched, so
child indexing, hit-testing and introspection are unaffected. See
`backend_apple_core::shadow`.

One gap worth knowing: CoreAnimation scales a shadow by the alpha of
whatever casts it, so that second layer mirrors the view's background to
have something to cast from. It hides exactly behind an opaque
background. A **translucent** background plus `overflow: hidden` renders
with no shadow on iOS/macOS rather than double-painting the background —
a known gap, not a regression. Web is unaffected either way.


---

## Interaction states

`StateBits` is a 4-bit set: `HOVERED`, `PRESSED`, `FOCUSED`,
`DISABLED`. Stylesheets can declare per-state overlays that the
framework merges in when the bit is on.

There are two ways a backend can wire state activation:

### Native (`handles_states_natively() == true`)

The backend receives `apply_styled_states(base, overlays)` (or
`apply_styled_variants`, which adds the breakpoint and container
overlays) and emits its own state-tracking mechanism. The web backend,
for example, mints CSS pseudo-class rules —
`:hover:not([disabled])`, `:active:not([disabled])`,
`:focus-visible:not([disabled])`, `[disabled]` — so the browser handles state
activation natively. No Rust↔JS round trip per event. The
`:not([disabled])` guard is how CSS follows the
[state precedence](#state-precedence) rule that a disabled control shows
no hover, press or focus styling.

Each overlay the backend receives holds **only the properties its own
block sets** (with the values the full merge gives them, so a variant or
override above the block still wins). The rules therefore never repeat
the base:

```css
.ui-x { min-height: 44px }
@media (min-width: 640px) { .ui-x { min-height: 0px } }
.ui-x:hover:not([disabled]) { background: … }
```

A hovered element at 640px and wider keeps `min-height: 0`: the
`:hover` rule outranks the `@media` rule only on `background`, the one
property the state sets. Overlays used to be full resolutions, so the
`:hover` rule (specificity (0,2,0)) re-declared `min-height: 44px` and
the hovered element lost the breakpoint's value. See
[Layer order](#layer-order).

### Event-driven (`handles_states_natively() == false`)

The backend installs native event listeners via `attach_states(node,
setter)`. When the listener fires (touch down, focus change), the
backend calls `setter(StateBits::PRESSED, true)`. The framework's
per-node state signal flips, the apply-style effect re-fires with
the new bits merged into a fresh `StyleApplication`, and the
backend gets a regular `apply_style` call with the overlay merged
in.

The mobile backends (Android, iOS) use this path: state activation
flows through the framework's reactivity, not through any platform
native style-state system.

Both paths produce the same observable behavior on the resulting
widget. The choice is purely about where the state tracking lives.

### State precedence

When several states are on at once, they layer in one fixed order,
lowest to highest:

**hovered < focused < pressed < disabled**

and **`disabled` turns the other three off.** While a control is
disabled, its `state hovered`, `state pressed` and `state focused`
blocks don't apply, and neither does any compound that names one of
them. The order is the same on every backend and in the premint CSS,
and the order you declare `state` blocks in a sheet has no effect.

Why this order:

- **focused over hovered.** The focus ring is an accessibility signal.
  It shouldn't disappear while the pointer sits on the control, as it
  would on a `Select` whose hover border painted over its focus ring.
- **pressed over both.** Press feedback answers the input happening
  right now. A pressed button is always hovered too, so Button's 0.85
  press dim has to beat its 0.92 hover dim.
- **disabled turns them off.** A disabled control gives no interaction
  feedback. Ordering alone can't guarantee that: `SelectTrigger`'s
  `state disabled` block only sets `opacity`, so its `state hovered`
  border would still paint. The mechanism differs per path. On
  event-driven backends the resolver drops the interaction states from
  the variant set while `__state_disabled` is on. On CSS backends the
  interaction rules carry `:not([disabled])`. The state bits themselves
  still record what happened, so a control that is re-enabled while the
  pointer rests on it shows its hover straight away.

This is the CSS `:hover` → `:focus` → `:active` convention the idea-ui
sheets were written against. The order lives in one place,
`runtime_shared::StateBits::PRECEDENCE`. `StyleSheet::state_axes()` is
kept sorted by it, and the resolver's state pass, the overlay list CSS
backends emit in source order, and the premint dump all read that
slice. Before this was pinned down, the native resolver merged states
alphabetically (so hovered beat both focused and disabled), web
followed declaration order, and SSR followed selector text order. The
same sheet came out differently on each, and a hovered disabled
`Select` showed its hover border everywhere.

### Focus indicators

`state focused` means focus that should SHOW — the CSS `:focus-visible`
rule — on every backend. Keyboard focus (Tab) lights it, and so does
focus on a text field however it arrived (the ring marks where typing
goes). A mouse click or a tap on a button, switch or segment does not,
even when the control takes focus: a ring after every click read as a
stuck "selected" state. Web lowers the state to `:focus-visible`; macOS
skips a first-responder change made by a click; GTK follows the window's
`focus-visible` flag; Android pressables are not focusable in touch mode,
and iOS only focuses text fields.

A focusable element (a `pressable` is one: `tabindex="0"` on web, in the
key-view loop on macOS) stops being focusable while it is `disabled`: web
swaps in `tabindex="-1"` plus `aria-disabled` and puts its own value back
on re-enable, and macOS takes it out of the key-view loop. Either way it
also gives up focus if it had it.

A focusable element shows the platform's own focus ring until its
stylesheet takes the job over. Declaring a `state focused` block is what
takes it over: the sheet then owns the indicator, and the platform ring
is suppressed. On web the `:focus-visible` rule minted for the block starts with
`outline: none` (the live engine, SSR and premint all mint it that way);
macOS never draws its native ring on framework views.

```rust
stylesheet! {
    pub Row<IdeaThemeRef> {
        base(_t) {
            // Reserve the ring's width at rest, so focusing does not
            // move the layout.
            border_width: 1.0,
            border_color: "transparent",
        }
        state focused(t) {
            border_color: t.color.focus_ring(),
        }
    }
}
```

An element whose sheet declares no `state focused` keeps the browser's
default ring, on purpose. Without it, a keyboard user could not see where
focus is. That default ring is drawn outside the box, so an ancestor
with `overflow: hidden` clips it, and its colour ignores the theme. Both
are reasons to declare your own.

Draw the ring with a border, as above, or with `shadow`. If you use a
border, give `base` the same `border_width` and a transparent colour.
Otherwise focusing adds the border's width and shifts everything after
the element.

---

## Responsive breakpoints

A `breakpoint` block adds rules that apply only once the viewport is at
least a given width. You write the narrowest layout in `base`, then add
or change properties in `breakpoint` blocks as the screen widens; the
framework merges the blocks whose width threshold the current viewport
has crossed. The activation source is viewport width, and the merge
runs through the same apply-style effect that handles interaction
states. A stylesheet declares them with `breakpoint` blocks:

```rust
stylesheet! {
    pub Panel<()> {
        base(_t) {
            flex_direction: FlexDirection::Column,   // narrow / mobile
            padding: 12.0,
        }
        breakpoint md(_t) {
            flex_direction: FlexDirection::Row,      // ≥ 768 dp
            padding: 20.0,
        }
        breakpoint lg(_t) { padding: 32.0 }          // ≥ 1024 dp
    }
}
```

The model is **mobile-first and min-width only**:

- `base` is the `Xs` layout (the narrowest case). `xs` is therefore not
  a valid block name — it *is* the base.
- Valid blocks are `sm`, `md`, `lg`, `xl`. Each is a lower bound; at a
  given width every overlay whose threshold is `≤` the width applies,
  lowest first, so wider breakpoints win on conflicting properties
  (matching how stacked `@media (min-width)` rules cascade).
- There is intentionally **no `max-width`, no orientation, and no other
  media features.** Authors widen a narrow base rather than narrowing a
  wide one.

Thresholds come from [`runtime_core::breakpoints`] — Tailwind-style
defaults (`sm` 640, `md` 768, `lg` 1024, `xl` 1280 dp), overridable
once at startup via `install_breakpoints`.

Both backends key off the *same* thresholds, so a `breakpoint md` block
activates at exactly the same width everywhere:

- **Web** emits `@media (min-width: 768px) { .ui-… { … } }`. A static /
  SSR first paint is already responsive — no JS needed to pick the
  bucket. Each `@media` rule holds only what its block sets, so stacked
  rules cascade exactly like the native merge.
- **Native** turns every active breakpoint on and resolves once. The
  framework reads the viewport bucket (re-firing only when the *bucket*
  changes), and the apply-style effect re-resolves with the active
  blocks layered in.

For imperative layout switches that don't fit the overlay model, read
the bucket directly:

```rust
match current_breakpoint().get() {
    Breakpoint::Xs | Breakpoint::Sm => { /* stacked */ }
    _                               => { /* side-by-side */ }
}
```

Prefer declarative `breakpoint` blocks where you can — they keep web
and native in lockstep and survive SSR. The signal is the escape hatch.
See [`breakpoint.rs`](../crates/runtime/shared/src/breakpoint.rs) for the
bucket definitions and thresholds.

### Layer order

Every backend and the premint asset follow one precedence, defined by
`StyleSheet::resolve`:

**base < breakpoints (`sm` → `xl`) < containers (narrow → wide) <
variants < states < compounds**

(Among the states, see [State precedence](#state-precedence).)

Each layer contributes only the properties its own block sets, so a
higher layer overrides a lower one on the properties they share and
leaves the rest alone. With `sm { min_height: 0 }` and
`md { padding: 8 }`, an element at 768px gets both. A variant beats a
breakpoint, so a `size` variant's `padding` wins over a `breakpoint md`
padding at every width. A state beats a variant, so
`state pressed { background }` shows on top of any variant's
background.

Flipping a variant or entering a state re-resolves every layer, so an
active breakpoint or container arm stays applied. This holds for every
way a reactive style can arrive, including `signal_class`. On web,
`signal_class` normally swaps between one pre-minted class per value. A
class built that way holds only the base and the variants, so a sheet
with any `breakpoint`, `container` or `state` block uses the per-node
path that emits each layer as its own rule.

---

## Compound variants

Two axes that set the same property don't merge in the order you declared
them — `StyleSheet::variants` is a `BTreeMap`, so the **alphabetically
later axis name wins**. That is fine until it isn't: a `pinned` arm and a
`row_hovered` arm both setting `background` resolve to `row_hovered`,
because `"pinned" < "row_hovered"`, whatever order they appear in the
source.

A `compound` block states the combination outright. It applies only when
*every* named axis holds its named value, and `resolve` layers it after
all the per-axis overlays — so it decides the property regardless of how
the names sort:

```rust
stylesheet! {
    pub TableBodyCell<IdeaThemeRef> {
        base(_t) { /* … */ }
        variant pinned {
            #[default]
            none(_t) {}
            left(t) { background: t.color.surface() }
            right(t) { background: t.color.surface() }
        }
        variant row_hovered {
            #[default]
            off(_t) { background: Color("transparent".into()) }
            on(t) { background: t.color.surface_alt() }
        }
        // A frozen column stays opaque at rest, and still takes the row
        // tint with its row.
        compound (pinned: left, row_hovered: off)(t) { background: t.color.surface() }
        compound (pinned: left, row_hovered: on)(t)  { background: t.color.surface_alt() }
    }
}
```

The grammar mirrors `container (min_width: …)`: a parenthesized condition,
then the token-vocabulary binding, then the rules. The condition is checked
against the sheet's own axes **at macro time** — naming an axis the sheet
doesn't declare, an arm that doesn't exist, or the same axis twice is a
compile error, because each of those would otherwise fail silently by
simply never matching. A one-pair compound is rejected too: that is just
that axis's arm.

Compounds **premint**. The runtime stamps one class per axis, so the
condition is a CSS compound selector over those classes
(`.iy-abc-pinned-left.iy-abc-row_hovered-on`) at specificity (0,2,0) —
above the (0,1,0) single-axis arms, which reproduces the runtime's
"compounds merge after every axis". Reaching for one does not push the
sheet onto the live engine.

---

## Backend caching

Backends typically want to cache work that maps from `StyleRules`
to platform style state — minting a CSS class, building a
`Drawable`, setting up an animator. The trait provides a few hooks:

- `register_stylesheet(rules: &[Rc<StyleRules>])` — called once per
  `(sheet, theme)` pair, with the **pre-generated** rule sets (one
  for base, one per single-axis variant, one per compound). The
  web backend uses this to mint CSS classes up front so
  `apply_style` is a cache hit.
- `unregister_stylesheet(rules: &[Rc<StyleRules>])` — symmetric;
  called when the sheet is dropped or the theme changed. Backends
  free their per-rule state.
- `apply_style(node, &resolved)` — the per-node call. Backends look
  up cached state by content (a hash, a serialized form) or fall
  back to applying directly.
- `on_node_unstyled(node)` — fired when a styled node is being torn
  down. Lets backends free per-node bookkeeping (the web backend's
  dynamic CSS class slot, Android's animator state).

Pre-generation is opportunistic. Backends that can't profit from
it (most native backends — there's no "class" to mint) leave the
default no-op impl and just handle each `apply_style` call directly.

---

## Two design choices worth understanding

### Why style tokens

An earlier design had stylesheet closures read a typed theme struct
(`base(|theme| { background: theme.colors.accent })`). The framework
moved to style tokens (`Tokenized::token("color-accent", …)`) for two
concrete reasons: the struct approach made every `StyleSheet` generic
over a specific theme type, and a struct field read can't compile to a
CSS custom property the way a token name can.

What the token model buys:

- **Theme decoupled from sheet shape.** `StyleSheet` is one concrete
  type whose closures take `&VariantSet`. The theme is a flat
  `(name → TokenValue)` table installed separately — no generic theme
  parameter threading through every sheet.
- **Cheap web theme swaps.** Tokens map to CSS custom properties, so a
  light→dark swap is one `var(--…)` write per changed token. No class
  regeneration, no per-element restyle.
- **Reactive at token granularity.** Each token is its own signal;
  `update_tokens(["color-surface"])` wakes exactly the nodes that read
  `color-surface` and nothing else.

The cost is indirection: at the core, a token is a name + fallback
rather than a type-checked field, so a hand-written
`Tokenized::token("typo", …)` is a runtime "unknown token → fallback"
rather than a compile error. The typed layers put the check back without
touching the table: a `#[derive(Tokens)]` theme (or idea-ui's
vocabulary) generates one accessor per token, so a stylesheet names
tokens through methods the compiler checks, and each accessor's fallback
is read from the theme's own default values rather than restated.

### Variants vs overrides

Variants are **discrete** axes — `size: Small / Medium / Large`,
`kind: Filled / Outlined`. They're cacheable: the framework can
pre-generate every (axis, value) combination ahead of time, and a
backend like the web backend can mint a CSS class per combination.

Overrides are **continuous** — a user-controlled font scale, a
runtime-computed color. They can't be enumerated. They merge in
last, so they always win, but they're cache-unfriendly: each
distinct override value produces a unique resolution cache entry.

This split is the resolution of "do we let people pass arbitrary
runtime values into stylesheets?" Yes (overrides), but the
expensive cases are still cheap (variants enumerate and pre-bake).
Most styling fits in the variant model; overrides are an escape
hatch for the rest.
