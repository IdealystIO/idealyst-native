# Migrating from 1.1.x to 1.2

This is the front door for upgrading an app from **1.1.x** to **1.2.0**.

1.2 is the build-time CSS release: on web, styles can be extracted from
the wasm into a CSS asset at build time (`--premint`), and the style
engine itself can be compiled out of the bundle (`--premint-only`).
Almost all app code does **not** change — `ui!`, `#[component]`,
`stylesheet!`, the primitives and the component library keep their
spelling, and native backends are untouched. There is ONE breaking
change with a silent failure mode, so read
[shadow / text_shadow](#1-shadow--text_shadow-one-field-per-css-property)
even if everything compiles.

## How to use this document

- **[Breaking changes](#breaking-changes)** — the full inventory,
  ordered by how likely you are to hit it.
- **[Adopting build-time CSS](#adopting-build-time-css-optional)** —
  the new flags and the path to a style-engine-free web bundle.
  Optional: an app that never passes a `--premint*` flag behaves as
  before.
- **[`styling.md`](styling.md)** — the deep chapter on the style
  system, including the full premint mechanics (the delta model, what
  premints, the minted-class guard, the `--premint-only` contract).

Earlier upgrades: [0.5 → 1.0](migration-0.5-to-1.0.md) (the runtime-v2
core replacement), and before that
[0.2 → 0.3](migration-0.2-to-0.3.md) / [0.1 → 0.2](migration-0.1-to-0.2.md).

## What did NOT change

- **Native backends.** iOS / macOS / Android / wgpu never see a
  premint flag; they keep the full rule closures and render
  identically. Every change in this release is observable only in
  *when and where* the web's CSS is produced.
- **The wire protocol.** No wire declarations changed; dev clients and
  apps do not need a lockstep upgrade.
- **Authoring surface.** `ui!` / `jsx!` / `#[component]` /
  `stylesheet!` / `#[props]` — same syntax, same semantics. Theme
  swapping, tokens, state overlays, breakpoints, container queries —
  unchanged.

## Breaking changes

### 1. `shadow` / `text_shadow`: one field per CSS property

**What changed.** `StyleRules` now has two shadow fields:

- `shadow` — the **box** shadow, on every node kind.
- `text_shadow` — the **glyph** shadow on text nodes.

Previously there was one `shadow` field that lowered per node kind
(`box-shadow` on boxes, `text-shadow` on text). Now `shadow` is
*always* the box shadow.

**Who is affected.** Any stylesheet that puts `shadow:` on a **text**
node expecting a glyph shadow. After the upgrade that text node gets a
box shadow (a shadowed rectangle) instead of shadowed letter outlines —
it compiles and renders, just wrong. Box shadows on views/cards/buttons
are unaffected.

**What to do.** Rename the field on text-node styles:

```rust
// 1.1 — glyph shadow on a text node's stylesheet
stylesheet! {
    pub HeroTitle<MyTheme> {
        base(t) {
            shadow: Shadow { x: 0.0, y: 2.0, blur: 6.0, color: shadow_color },
        }
    }
}

// 1.2 — same shadow, now under the glyph-shadow field
stylesheet! {
    pub HeroTitle<MyTheme> {
        base(t) {
            text_shadow: Shadow { x: 0.0, y: 2.0, blur: 6.0, color: shadow_color },
        }
    }
}
```

`grep -rn "shadow" your stylesheets` and check each hit that styles a
`text` node. The grammar is identical (`x/y/blur/color`, no spread),
and `y` is positive-down on every backend.

**Why.** One field per CSS property is what lets shadowed sheets
participate in build-time CSS — a build-time rule body can't know what
kind of node will wear its class, so the old kind-dependent lowering
disqualified every shadowed sheet (and forced shadowed text onto
private class names). See the shadows section of
[`styling.md`](styling.md).

### 2. `--premint-only` has a contract (only if you opt in)

Nothing here affects a build without the flag. Under `--premint-only`
the style engine is compiled out, so two things become loud runtime
panics (each panic names its cause and the source line):

- **Styles that need runtime rule composition** — runtime slot
  overrides, `with_computed` layers, closure-built sheets with no
  premint identity. Run `--premint-report` and drive the list to zero
  first; `--premint` alone is always safe while offenders remain.
- **Reading resolved `StyleRules` back in Rust** — resolving a sheet to
  *use* a value (e.g. tinting an icon with the container's fill color)
  is running the engine. Component code can gate such reads with
  `StyleApplication::attaches_preminted()`; on the preminting path the
  value already ships in CSS and web nodes inherit it.

### 3. Framework/component-library authors only

App code can skip this section.

- `StyleSheet::r#static` now **auto-premints**: its class derives from
  the rules' content, registered for the dump automatically.
  `premint_as` still overrides it (stable names for parameterized
  sheets), and any layer-adding mutator (`variant`, `variant_default`,
  `compound`) retracts it. If you construct static sheets and compare
  `premint_class()` against `None`, that assumption is inverted.
- The explicit `StyleProp::Sheet(Box::new(app))` handover (the
  compose/introspect spelling) now premints under `--premint-only`
  when the application qualifies, instead of panicking.
- Continuous per-instance values (a slider thumb's `left`, a grid's
  track list, a skeleton's dimensions) belong on the **inline layer**
  (`StyleApplication::with_inline`), not on overrides or computed
  layers — inline stays premintable and never mints per-value classes.

## Adopting build-time CSS (optional)

The ladder, on web builds only:

1. **`idealyst build --web --premint`** — always safe. Static styles
   ship as a content-addressed `premint.<hash>.css`; anything the dump
   couldn't enumerate falls back to the live engine silently. Wasm size
   is roughly unchanged (the engine still ships as the fallback).
2. **`--premint-report`** — builds like `--premint` and logs one
   console warning per distinct style that reached the live engine,
   with the constructing source line. Walk your routes; the deduped
   list is the work list. Zero entries means step 3 is safe.
3. **`--premint-only`** — the production target: rule closures and the
   resolve machinery are compiled out of the wasm. Measured on this
   repo: a small idea-ui app drops ~17% of raw wasm vs 1.1; the
   idea-ui docs catalog drops ~34%. The whole design system's CSS
   compresses to a few KB of brotli and caches independently of code
   deploys.

A sheet constructed on a code path the build-time dump never executed
gets no CSS; at boot the bundle scans the loaded asset and such classes
fall back to the engine (`--premint`) or panic naming the uncrawled
construction (`--premint-only`). The dump mounts every literal route,
so this only bites styles reachable exclusively through interactions or
parameterized routes — the report and the panic both tell you which.

## Also in 1.2

- `runtime_core::on_scope_drop(f)` — a cleanup hook a component body
  can register; runs when the component's reactive scope is dropped.
- `--premint-report` diagnostics carry the author's source line
  (`#[track_caller]` on the sheet constructors).
- Compound variants premint as CSS compound selectors.
- Fix: reactive-styled nodes no longer fall back to the browser serif
  font when the theme declares a default text font (two related fixes:
  live builds now always publish the default font, and reactive
  preminted nodes inherit the author's font instead of the theme
  default).
- Fix: clickable `Table` rows keep their pointer cursor and whole-row
  hover highlight under `--premint` (and the highlight is now a
  preminted class swap).
- Fix: `with_inline` values reach the node on the reactive preminted
  paths, on `<svg>` icon nodes, and are properly replaced when the
  layer shrinks.

## Upgrade procedure

1. Bump the framework crates to 1.2.0. Build. Everything should
   compile without edits.
2. Sweep text-node shadows: rename `shadow:` → `text_shadow:` on text
   styles ([breaking change 1](#1-shadow--text_shadow-one-field-per-css-property)).
   This is the one silent visual change.
3. Run your test suite and your app once per platform. Native output
   is unchanged by construction; web output is unchanged for builds
   without `--premint*` flags.
4. Optionally, adopt build-time CSS via the
   [ladder above](#adopting-build-time-css-optional).
