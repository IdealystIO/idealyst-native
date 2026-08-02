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
  `Tokenized::Literal` via `From`. The `fallback` is mandatory at this
  layer — runtime-core doesn't know any palette, so a token reference
  must carry its own default. If you're styling on top of the **idea-ui
  design system**, reach for its `theme_token!("color-surface")` /
  `theme_length!("spacing-md")` macros instead: they pull the fallback
  from idea-theme's canonical palette (so you restate no hex) and check
  the name at compile time. See the idea-ui theming guide.

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
UIKit's `userInteractionEnabled = NO` is deliberately NOT used: it
disables the whole subtree with no way for a descendant `Auto` to
re-enable. Android and the GPU backend do not implement
`pointer_events` yet — an always-mounted `None` overlay will swallow
input there.

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
   interaction, are still not seen.
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
- **Source order mirrors the resolver's merge order**, which iterates
  variant axes alphabetically (`BTreeMap`): the `__bp_*` < `__cq_*` <
  `__state_*` overlay prefixes sort before every lowercase author
  axis, so emission is base → breakpoints (rank ascending) →
  containers (threshold ascending) → states → author axes. This also
  reproduces the live web backend's cross-rule outcomes (a variant arm
  beats a state overlay on conflicting properties; a state overlay
  beats a breakpoint overlay) — verified by the A/B computed-style
  harness against the live engine on the full website.

Compound variants (`StyleSheet::compound`, a runtime-only API the
`stylesheet!` grammar cannot express) have no delta encoding; the dump
rejects a sheet carrying them, which cannot occur for macro-registered
sheets.

### What stays on the live engine

A sheet (or application) falls back to live minting when the build
can't prove the CSS at build time:

- **Reactive inputs** — a setter received a `Signal`/`derived`.
- **Runtime overrides** — `.override_*` values at the call site, or
  `with_style_overrides` layers.
- **Shadow-carrying sheets** — `shadow` lowers as `text-shadow` on
  text and `box-shadow` on boxes, and a class name carries no
  node-kind information.
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
build-time rule can't reproduce, so preminted rule bodies whose sheet
sets no `font_family` carry
`font-family: var(--iy-default-font, inherit)` and the driver defines
that variable from the installed theme
(`StyleOps::apply_default_text_font`; web sets it on the document
element, SSR emits it in the head CSS). With no default font
installed the `inherit` fallback reproduces the plain cascade.

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

### Dropping the engine — not available, and the old lever is removed

Preminting changes *when* rules are made, not the bundle size: the
engine still ships for the fallback paths.

The lever that used to remove it was the `style-dynamic` cargo feature,
which gated the old walker's dynamic style arms so an all-preminted app
could compile them out. **That feature has been deleted.** It was the
last remnant of the `prim-*` bundle-size gating model that runtime v2
removed by decision, and it had already stopped doing anything:

- `runtime_vocabulary::style_attach::attach_style` matches all six
  `StyleProp` arms unconditionally, with no feature gate, so the engine
  stays reachable and dead-code elimination cannot drop it;
- `runtime-shared` contained **zero** `cfg(feature = "style-dynamic")`
  blocks — only a dead-code lint `allow`;
- `backend-web` had already dropped its forward, so the documented
  instruction ("set `default-features = false` on BOTH runtime-core and
  your backend crate") was an unknown-feature error, not a size win;
- feature unification made it unturnoffable regardless: the workspace
  dep spec for `runtime-shared` does not set
  `default-features = false`, so any graph containing the vocabulary
  force-enabled it.

`style-dump` (the CLI's ephemeral premint-dump build) no longer implies
it. Nothing in the tree declares or forwards it any more.

The structural successor, if the size lever is ever wanted back, is a
gate inside `attach_style` itself, next to the arms it would remove —
one home, one decision, instead of a feature threaded through every
backend crate. Until that lands, treat preminting as a resolution-cost
optimization rather than a size lever.

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
list. On the idea-ui catalog it is 218 entries across 47 routes, of which
99 are `with_computed` layers and 43 carry overrides.

### Current limits

- `--premint` refuses to combine with `--ssg`/`--ssr`: server-rendered
  HTML would carry live-minted classes while the hydrating client
  stamps preminted ones, so adoption would diverge. SSR premint needs
  its own wiring.
- Only `stylesheet!` *builder* applications premint. A plain
  `Rc<StyleSheet>` passed directly (e.g. `card_style()`) stays live.
- A component that COMPOSES or INTROSPECTS a builder's styles (merging
  an inherited color onto a label sheet, re-deriving a cell's
  application to layer a hover) must hand the style over as an explicit
  **`StyleProp::Sheet`**:

  ```rust
  bound.with_style(StyleProp::Sheet(Box::new(TableBodyCell().into_style_application())))
  ```

  `into_style_application()` alone is NOT enough, despite reading like
  it should be. `IntoStyleProp for StyleApplication` has a preminted
  fast path, so a bare application premints anyway and the introspection
  silently finds nothing. That shipped: `Table`'s clickable rows lost
  their pointer cursor and their hover highlight in every `--premint`
  build, because `cell_base_application` returned `None` and the overlay
  was skipped without a word. Naming the variant is what actually pins
  it (idea-ui's `Tag`/`Alert` label coloring and `Table`'s cells).
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

`StyleRules::shadow` is a single `Shadow { x, y, blur, color }`. What it
*renders* as depends on the node it lands on:

- On a **box** element (`view`, `image`, `pressable`, …) it's a box
  shadow — the shadow of the element's rectangle.
- On the **text** primitive it's a *glyph* shadow — the shadow hugs the
  letter outlines, not the inline box. There's no separate `text_shadow`
  field; the text primitive reinterprets the one `shadow` field.

Each backend converges on that output through its own mechanism
(CLAUDE.md §7):

| Backend | Box element | Text primitive |
| --- | --- | --- |
| Web / SSR | `box-shadow` | `text-shadow` (`css::rules_to_css_text`) |
| iOS / macOS | CALayer shadow | CALayer shadow on the label (layer content is the glyphs) |
| Android | *(elevation, n/a in v1)* | `TextView.setShadowLayer` |

`text-shadow` and `box-shadow` share the exact `<x> <y> <blur> <color>`
grammar (neither takes a spread), so the mapping is lossless. The `y`
offset is positive-**down** on every backend — the coordinate flips
(AppKit's y-up layer, UIKit's y-down) are absorbed by the backend so a
`shadow { y: 2 }` lands below the glyphs everywhere.

Because a shadowed text node and a box element with an otherwise
identical `StyleRules` must render different CSS, the web/SSR backends
mint the text node a distinct class (`css::text_shadow_class_key`) so the
two never collide in the content-keyed style cache. This only diverges
when a shadow is actually present — unshadowed text still shares classes
with views.

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
See [`breakpoint.rs`](../crates/runtime/shared/src/breakpoint.rs) for the
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
