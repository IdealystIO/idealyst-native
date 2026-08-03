+++
title = "Migrating 1.1 → 1.2"
order = 907
tags = ["migration", "1.2.0", "breaking", "premint", "css", "shadow", "text_shadow", "bundle-size"]
+++

# Migrating 1.1 → 1.2

1.2.0 is the **build-time CSS release**. On web, styles can be extracted
from the wasm into a CSS asset at build time (`--premint`), and the
style engine itself can be compiled out of the bundle
(`--premint-only`). Native backends are untouched and render
identically; an app that never passes a `--premint*` flag behaves as
before. There is **one breaking change**, and its failure mode is a
silently wrong render, so do step 1 even though everything compiles.

The full-depth version of this guide lives in the repo at
`docs/migration-1.1-to-1.2.md`; the style-system mechanics are in
`docs/styling.md` and summarized in [[styling]].

## 1. BREAKING: `shadow` / `text_shadow` — one field per CSS property

`StyleRules` now has two shadow fields: `shadow` is **always** the box
shadow, and `text_shadow` is the glyph shadow on text nodes. Previously
one `shadow` field lowered per node kind. A `shadow:` on a **text**
node's stylesheet now produces a shadowed rectangle instead of shadowed
letter outlines — it compiles, it renders, it's wrong.

**Do:** grep your stylesheets for `shadow:` and rename the field to
`text_shadow:` on every rule that styles a text node. Same
`x / y / blur / color` grammar, `y` positive-down on every backend.

## 2. New: the build-time CSS ladder (opt-in, web only)

```
idealyst build --web --release --premint         # safe: engine ships as fallback
idealyst build --web --release --premint-report  # logs each live-engine fall-through + source line
idealyst build --web --release --premint-only    # engine compiled OUT of the wasm
```

Work down the ladder: build with `--premint-report`, walk your routes,
convert what it names (the common conversions: enumerable values →
variant axes; continuous per-instance values →
`StyleApplication::with_inline`; runtime overrides usually become one
of those two), and ship `--premint-only` when the list is empty.
Measured effect: −17% raw wasm on a small idea-ui app, −34% on the
idea-ui docs catalog, with the design system's CSS compressing to a few
KB of brotli that caches independently of code deploys.

Under `--premint-only` a style that still needs the engine panics
loudly at its source line — runtime slot overrides, `with_computed`
layers, identity-less closure sheets, and Rust code that READS resolved
`StyleRules` back (gate such reads with
`StyleApplication::attaches_preminted()`; on the preminting path the
value is already in CSS and web nodes inherit it).

## 3. Component-library authors

- `StyleSheet::r#static` **auto-premints** — the class derives from the
  rules' content, no `premint_as` needed. Explicit identities still
  override; layer-adding mutators (`variant`, `variant_default`,
  `compound`) retract the auto class.
- The explicit `StyleProp::Sheet(Box::new(app))` introspection handover
  premints under `--premint-only` when the application qualifies.
- Put continuous per-instance values on `with_inline`, never on
  overrides/computed — inline premints and never mints per-value
  classes.

## 4. Also in 1.2

- `runtime_core::on_scope_drop(f)` — cleanup hook, runs when the
  component's reactive scope drops.
- Compound variants premint as CSS compound selectors.
- Fixes: reactive-styled nodes inherit the theme/author font correctly
  (no more browser-serif fallback); clickable `Table` rows keep cursor
  + whole-row hover under premint; `with_inline` values reach `<svg>`
  nodes and the reactive preminted paths.

No wire-protocol change; dev clients and apps need no lockstep upgrade.
