+++
title = "Migrating 0.4 → 0.5"
order = 904
tags = ["migration", "0.5.0", "bundle-size", "features", "primitives", "brotli", "deploy"]
+++

# Migrating 0.4 → 0.5

> **Status: 0.5.0 is in development.** This is the living record of its
> changes — each section carries a `Status:` line and fills in as the change
> lands. See [[migrations]] for the versioning policy.
>
> **Superseded in part by runtime v2.** The `prim-*` cargo features and
> `idealyst build --web --primitives=…` described below **no longer exist**.
> They gated pre-v2 walker dispatch arms, authoring builder fns, and
> `Backend` trait methods — none of which survive. Nothing in the
> primitive-gating sections is actionable on a current build; they are kept
> as the historical record. The structural successor is per-primitive handler
> registration (`runtime_vocabulary::handlers::register_builtins`) — see the
> [[sdks]] guide, "There are no `prim-*` features". The brotli /
> `dist/web` sections below are unaffected and still current.

0.5.0 is the bundle-size release. The web framework floor drops from ~591 KB
to ~392 KB raw (~133 KB over the wire with brotli) through per-family
primitive gating, and release bundles ship precompressed `.br` siblings. For
a typical app **nothing is breaking**: every new cargo feature defaults ON,
the default build is byte-for-byte equivalent, and the new flags are opt-in.
The sections below exist because two groups carry small contracts: **SDK
authors** (a feature-forwarding rule) and **deploy pipelines** (new files in
`dist/web`).

## Primitive families are cargo features (`prim-*`)

**What changed.** Every heavy primitive family is now gated behind a
`prim-*` cargo feature on `runtime-core`, mirrored by each backend crate:
`prim-virtualizer`, `prim-icon`, `prim-image`, `prim-text-input` (TextInput
+ TextArea), `prim-toggle`, `prim-slider`, `prim-activity`, `prim-portal`
(overlay / anchored_overlay), `prim-presence`, `prim-graphics`,
`prim-navigator` (navigator + outlet + URL sync + Link's nav dispatch), and
`prim-lazy` (`#[lazy_component]` chunk mounting). All twelve are **ON by
default** — an app that does nothing sees an identical build.

Disabling a family removes its walker dispatch, backend implementation, and
embedded JS shims from the wasm. Authoring a gated-out primitive is a
compile error naming the feature; one arriving at runtime (wire-received, or
a feature mismatch between crates) renders a labeled "unsupported"
placeholder on every backend uniformly — never a panic.

**Migration (apps): none required.** To opt INTO smaller bundles, two edits
must land together, because cargo unifies features across the whole build
graph:

```toml
# 1. Your app crate's Cargo.toml — stop pulling the all-on default set:
runtime-core = { git = "…", default-features = false }
```

```sh
# 2. Build with exactly the families you use (or `none`):
idealyst build --web --release --primitives icon,text-input
```

The build warns if step 1 is missing (the flag would silently do nothing),
and an unknown family name is a hard error listing the valid set. SDKs you
depend on re-enable the families they render with automatically — you only
name the primitives *your own* code uses directly.

**Migration (SDK authors): forward what you render.** If your crate builds
a gated primitive (directly or through `ui!`), your `runtime-core` dep line
must forward that family, or consumers using `--primitives` get placeholders
where your UI should be:

```toml
# an SDK that renders icons and a text input:
runtime-core = { workspace = true, features = ["prim-icon", "prim-text-input"] }
```

First-party examples to copy: `virtualized` (prim-virtualizer),
`swap-navigator` / `stack-navigator` (prim-navigator, including the
backend-web forward on wasm), `idea-ui` (per-component gating — see the
next section).

**Migration (crates between an app and runtime-core).** Cargo silently
ignores `default-features = false` on `workspace = true` dependency lines.
Any intermediate crate (a backend, a utility crate) whose `runtime-core`
dep keeps default features re-enables **every** family for **every**
consumer. Declare runtime-core as a *path/git* dep with
`default-features = false` plus explicit forwards — `backend-web` and `css`
were converted in this release and are the model.

**Migration (custom Backend implementors).** The `Backend` trait's
`create_*` defaults for gated families no longer `unimplemented!`-panic;
they render the standard placeholder via `create_external`. If your backend
relied on the panic to surface missing implementations during bring-up,
watch for labeled placeholder boxes instead of aborts. Implemented methods
are unaffected.

**Status:** landed. Regression coverage in
`crates/runtime/core/tests/prim_gating.rs` (gated-off placeholder per
family, backend-mismatch placeholder, default-set anchors).

## idea-ui components are gated by their primitive families

**What changed.** idea-ui now declares the six `prim-*` families its
components render (`prim-icon`, `prim-image`, `prim-text-input`,
`prim-activity`, `prim-portal`, `prim-presence`) as its own cargo
features, each forwarding the same-named runtime-core family. They are
ALL ON by default — `idea-ui = { workspace = true }` keeps the complete
component set and existing apps see no change.

With a family disabled, the components that (transitively) render it are
**compiled out**: using one is a compile error naming the missing
feature, instead of the component silently rendering the runtime
"not supported" placeholder. The per-component map lives as `cfg` gates
in `idea-ui/src/components/mod.rs` (summary in idea-ui's `Cargo.toml`);
e.g. `prim-icon` alone gates Icon/IconButton/Breadcrumbs/Checkbox/
Switch/Slider/Pagination, while Button needs icon+activity, Modal needs
portal+presence, Field needs icon+activity+text-input, and Toast needs
all four of icon/activity/portal/presence.

Two deliberate fine-grained splits: **Textarea** needs only
`prim-text-input` (it shares the field module's stylesheets, not the
Field component), and **Autocomplete** needs text-input+portal (it
reuses `SelectOption`/`SelectSize` — data types — without the
icon-rendering Select component).

**Migration (restricted apps).** To build an idea-ui app with a reduced
primitive set, opt out of defaults on BOTH dep lines and re-enable the
families you use — the features are same-named on purpose:

```toml
runtime-core = { version = "…", default-features = false }
idea-ui = { version = "…", default-features = false, features = [
    "prim-icon", "prim-portal",   # e.g. buttons/menus, no text inputs
] }
```

then build with the matching `--primitives icon,portal`. The build
prints a warning if either dep line still carries default features
(unification would silently re-widen the set). `idea-theme` and the
`table` SDK were converted to `default-features = false` runtime-core
deps in this release, so idea-ui's whole subtree is unification-clean.

**Status:** landed. Regression coverage in
`crates/ui/idea-ui/tests/prim_gating.rs` (default-set pin, all-off core
survival, the Textarea and Autocomplete splits), exercised per feature
arm via `cargo test -p idea-ui --no-default-features [--features …]`.

## Release web bundles ship brotli `.br` siblings

**What changed.** `idealyst build --web --release` now writes a
precompressed sibling next to every compressible file in `dist/web`
(`baseline_bg.<hash>.wasm` → `baseline_bg.<hash>.wasm.br`), encoded with
brotli q11 at build time. Originals are untouched. Brotli beats gzip -9 by
~20% on wasm; a minimal app is ~133 KB over the wire.

**Migration: usually none.** Static hosts and CDNs that understand
precompressed siblings (nginx `brotli_static on;`, Caddy
`file_server { precompressed br }`, most CDN edges) start serving them with
`Content-Encoding: br` automatically; everything else keeps serving the
originals. Two cases to check:

- **Deploy scripts that enumerate `dist/web`** (globs, manifest generators,
  size budgets) will see the new `.br` files. Exclude `*.br` or account for
  them; they carry the same content hash as their original, so the
  immutable-cache advice is unchanged.
- **Hosts that do their own on-the-fly compression** — nothing breaks, but
  configure them to prefer static precompressed files to get the q11 win.

`--no-brotli` opts out entirely. The in-place `--gzip` mode is unchanged
and composes (the `.br` is always encoded from the original bytes).

**Status:** landed. Coverage in `build-web`
(`brotli_precompress_emits_siblings_and_skips_binaries`).

## `--data-prune` is now sound for multi-segment data

**What changed.** The wasm-split data pruner re-materializes zeroed
chunk-only symbols from **every** active data segment; previously only the
first segment was considered, which could corrupt bundles whose lazy chunks
carried data outside `.rodata` (symptom: a lazy chunk that silently failed
to load).

**Migration: none** — builds that avoided `--data-prune` because of the
instability can re-enable it. Verify the app still renders with pruning on,
as the guide has always advised.

**Status:** landed. Regression coverage in `wasm-split-cli`
(`regression_rematerializes_non_rodata_segments`,
`regression_prune_skips_unrematerializable_segments`).

## SDK self-registration behind a default-on feature

**What changed.** `canvas-native`'s `inventory::submit!` registration moved
behind a default-on `self-register` cargo feature so crates consuming it as
a *delegate* (e.g. `canvas-vello`'s Canvas2D fallback) can opt out and keep
the rasterizer + font stack out of `main.wasm` in lazy-loaded apps.

**Migration: none for apps** (the feature defaults on; direct dependents
keep zero-config registration). SDK authors whose crate both
self-registers and gets consumed as a delegate should adopt the same
pattern — see [[lazy-loading]] for the full transitive-anchor rule.

**Status:** landed.

## `lazy! { … }` is deprecated — use `#[component(lazy)]`

**What changed.** The anonymous-block `lazy!` macro is deprecated (a
compiler `deprecated` warning at every use site). Lazy components —
`#[component(lazy)]` / `#[lazy_component]` — are the one blessed
code-splitting surface: same wasm-split chunking mechanism, plus typed
props across the boundary, readable chunk filenames
(`…_lazy_Editor.wasm` instead of a content hash), and the standard
`loading` / `error` props instead of builder methods.

**Migration.** Hoist the block's body into a lazy component; anything the
block created internally (signals, closures) moves into the component fn,
and anything it *couldn't* capture becomes a prop:

```rust
// before
lazy! { heavy_sdk::register_lazy(); heavy_sdk::widget() }
    .placeholder(|| ui! { text { "loading…" } })

// after
#[component(lazy)]
fn HeavyCorner() -> Element {
    heavy_sdk::register_lazy();
    heavy_sdk::widget()
}
// call site:
ui! { HeavyCorner(loading = || ui! { text { "loading…" } }) }
```

`lazy!` keeps working (and splitting) while deprecated — this is a warning,
not a break.

**Status:** landed (deprecation + in-repo migration; the block form remains
covered by `tests/lazy-chunk-handoff`).

## Migration checklist

- ~~Apps: optionally adopt `--primitives` + the app-side
  `default-features = false` edit for smaller bundles.~~ **Removed by
  runtime v2** — the flag is now a hard CLI error. Do not do this.
- ~~SDK authors: forward `prim-*` for every gated family your crate
  renders; if your crate sits between apps and runtime-core, path-dep
  runtime-core with `default-features = false`.~~ **Removed by runtime
  v2** — there are no `prim-*` features to forward, and `runtime-core`
  itself is gone.
- ~~idea-ui apps using `--primitives`: also set
  `idea-ui = { default-features = false, features = ["prim-…"] }`.~~
  **Removed by runtime v2** — a stale `prim-*` entry is now a cargo
  resolve error. Delete it; the component set is unconditional.
- ~~Custom backends: expect placeholder rendering (not panics) for
  families you haven't implemented.~~ **Removed by runtime v2** — there
  is no `Backend` trait; backends implement capability traits and the
  scene `Registry` decides what a missing handler does.
- [ ] Deploy scripts: account for `*.br` siblings in `dist/web`, or pass
      `--no-brotli`.
- [ ] If you previously avoided `--data-prune`, it's safe to re-verify.
- [ ] Replace `lazy! { … }` blocks with `#[component(lazy)]` components
      (deprecation warning; the block form still works for now).
