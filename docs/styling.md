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
values, no theme tokens, no closures. Every field is `Option<T>`
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

A `StyleSheet` is **a closure over the active theme**:

```rust
pub struct StyleSheet {
    base: Box<dyn Fn(&dyn Any) -> StyleRules>,
    variants: BTreeMap<VariantAxis, VariantAxisDef>,
    compounds: Vec<CompoundVariant>,
}
```

- `base`: the unconditional rules. Reads the theme, returns
  `StyleRules`.
- `variants`: per-axis overlay closures. Each axis (`size`, `kind`,
  `parity`, …) has one closure per declared value.
- `compounds`: overlay closures triggered when *all* `(axis, value)`
  pairs in `when` are simultaneously active.

The theme is type-erased at the `StyleSheet` level — the closure
internally downcasts to the app's typed theme. This means
`StyleSheet` itself is a single concrete type that can be held in a
`Rc<StyleSheet>`, even though different stylesheets target different
themes.

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

1. `base(theme)` runs and produces the unconditional rules.
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
    PerfRow<MyTheme> {
        base |theme| {
            padding: 8.0,
            background: theme.colors.surface.clone(),
        }
        variants {
            parity: Parity {
                #default Even => |theme| { background: theme.colors.surface.clone() },
                Odd          => |theme| { background: theme.colors.surface_alt.clone() },
            }
        }
    }
}
```

It produces:

1. A `pub fn PerfRow() -> PerfRowBuilder` constructor.
2. A typed variant enum `PerfRowParity { Even, Odd }` (plus
   `as_variant_str` etc.) per declared axis.
3. A builder with a method per axis: `PerfRow().parity(Parity::Odd)`.
4. The underlying `Rc<StyleSheet>` cached in a thread-local so
   repeated calls return the same `Rc` and the resolution cache
   stays hot.

Every variant method on the builder accepts **anything convertible
to a closure** that reads the value. The same setter works for
static values, enum values, and `Signal<T>`:

```rust
let scale = signal!(1.0_f32);
PerfRow().parity(Parity::Odd).override_padding(scale)
```

When the builder produces its `StyleApplication`, signal reads inside
the variant-source / override-source closures subscribe naturally to
the apply-style `Effect` — so the style re-applies when the signal
changes, with no additional ceremony.

---

## Themes

A theme is any `'static` struct. The framework doesn't dictate its
shape — stylesheets downcast to the typed reference inside their
closures.

```rust
struct MyTheme {
    colors: ColorPalette,
    spacing: Spacing,
}

install_theme(light_theme());                    // call once at startup
set_theme(dark_theme());                         // swap; everything re-applies
```

`install_theme` stores the theme in a thread-local `Signal<Rc<dyn
Any>>`. `set_theme` updates that signal, which:

1. Clears the resolution cache (old entries reference the old theme
   pointer and would never be reused).
2. Queues every currently-registered `(stylesheet, theme)` pair for
   `Backend::unregister_stylesheet`. The framework drains the queue
   on the next style-effect run, when the backend is in scope.
3. Sets the new theme — every styled `Effect` that read the theme
   re-fires and re-resolves.

Theme changes propagate through the existing reactivity system. No
re-render, no diff. The set of styled effects subscribed to the
theme is exactly the set that needs to re-apply, by construction.

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

### Stylesheets as closures, not tokens

Many systems define a "token enum" — `Color::Accent` — and look up
the concrete value at the call site against the active theme. We
don't. A stylesheet's `base(|theme| { background: theme.colors.accent })`
returns a `Color` immediately; the theme is just a normal Rust
struct, accessed with normal field syntax.

What this buys:

- **No special property language.** Whatever color, size, or
  computation rule you can express in Rust against `&Theme` is
  fair game in a stylesheet. There's no need for a "computed
  token" feature.
- **Type-checked theme access.** Forgetting to add a field to your
  theme is a compile error, not a runtime "unknown token."
- **Per-property indirection is at the same cost as direct access.**
  No HashMap lookup, no key parsing, no token registry.

The cost is that the theme type is fixed at stylesheet declaration
time. We type-erase via `&dyn Any` and downcast inside the closure
— the downcast happens once per resolution, not once per property.

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
