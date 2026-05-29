# Styling

The framework owns the *data model* of styling — what a style is, what
variant axes exist, how the active theme propagates — but doesn't own
the *rendering strategy*. Each backend interprets a `StyleRules` value
however suits its platform: the web backend mints CSS classes, native
backends call view setters directly.

Implementation: `runtime_core::style` plus `runtime_macros::stylesheet`.

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

The walker hands this to `Backend::apply_style(node, rules)` as
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
    pub PerfRow<()> {
        base(_theme) {
            padding: 8.0,
            background: Tokenized::token("color-surface", Color("#ffffff".into())),
        }
        variant parity {
            #[default]
            even(_theme) {
                background: Tokenized::token("color-surface", Color("#ffffff".into())),
            }
            odd(_theme) {
                background: Tokenized::token("color-surface-alt", Color("#f3f4f6".into())),
            }
        }
    }
}
```

A few things about the grammar:

- The `<()>` slot and the `(_theme)` bindings are **vestigial** —
  parsed for backward-compatibility but ignored. Closures don't receive
  a theme; reading `theme.*` inside a body is a compile error
  (`check_no_theme_refs`). Write `(_theme)` (or any `_`-prefixed name)
  and pull theme values from tokens instead.
- `Tokenized::token("name", fallback)` references a style token; a bare
  literal (`padding: 8.0`, `background: "#fff"`) becomes
  `Tokenized::Literal` via `From`.

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
let scale = signal!(1.0_f32);
PerfRow().parity(PerfRowParity::Odd).override_padding(scale)
```

When the builder produces its `StyleApplication`, signal reads inside
the variant-source / override-source closures subscribe naturally to
the apply-style `Effect` — so the style re-applies when the signal
changes, with no additional ceremony.

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

> **Building a typed theme on top.** Nothing stops a component library
> from offering a typed `Theme` struct as ergonomic sugar — `idea-ui`
> does exactly this. But that's a user-land convenience that *emits*
> token entries; the framework core only knows about the flat
> `(name → TokenValue)` table.

---

## How styles reach the backend

The render walker's `attach_style(backend, node, source)` is the
seam between author-facing style values and the backend.

For each styled node:

1. Allocate a per-node `Signal<StateBits>` (initially `NONE`). This is
   the state machinery for `state hovered { ... }` overlays.
2. Build an `Effect` that:
   - Calls the source closure to get a `StyleApplication`.
   - Calls `ensure_registered_with(sheet, register=…, unregister=…)`
     to lazily pre-generate this sheet's variants for the active
     theme (the first time the backend sees it).
   - Resolves to `Rc<StyleRules>` against the active theme.
   - If `Backend::handles_states_natively() == true`, calls
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

---

## Interaction states

`StateBits` is a 4-bit set: `HOVERED`, `PRESSED`, `FOCUSED`,
`DISABLED`. Stylesheets can declare per-state overlays that the
framework merges in when the bit is on.

There are two ways a backend can wire state activation:

### Native (`handles_states_natively() == true`)

The backend receives `apply_styled_states(base, overlays)` and emits
its own state-tracking mechanism. The web backend, for example,
mints CSS pseudo-class rules — `:hover`, `:active`, `:focus`,
`[disabled]` — so the browser handles state activation natively.
No Rust↔JS round trip per event.

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
  bucket.
- **Native** merges the active bucket's overlay reactively. The
  framework reads `current_breakpoint()` (a memo over `viewport_size()`
  that re-fires only when the *bucket* changes), and the apply-style
  effect re-resolves with the new overlay merged in.

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
See [`breakpoint.rs`](../crates/runtime/core/src/breakpoint.rs) for the
bucket definitions and thresholds.

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

The cost is indirection: a token is a name + fallback rather than a
type-checked field, so a typo is a runtime "unknown token → fallback"
(warned in debug) rather than a compile error. Component libraries that
want compile-time-checked theme access layer a typed façade on top
(see `idea-ui`) that emits token entries — the safety lives in the
library, the flat table lives in core.

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
