# Idealyst VS Code extension

DSL-vocabulary completion inside `ui! { … }` / `jsx! { … }` blocks:

- **Tag completion** in child position — every primitive (`view`, `text`,
  `button`, …) and every `#[component]` in the project + its
  dependencies, with docs.
- **Prop completion** inside a tag's parens — `Button(│` offers `label`,
  `on_click`, `tone`, … with types and doc comments; props already
  written are filtered out; accepting inserts `name = `.
- **Prop value completion** after `name = ` — what the prop's type
  accepts, resolved from the catalog: open-set markers registered with
  `#[schema(value_of = …)]` (`tone::Primary`, `variant::Soft`,
  `typography_kind::H1`, `size::Md`, …, and an app's own `tone!`
  declarations), `IdealystSchema` enum variants (`ModalPresentation::Sheet`),
  `true`/`false`, a `Rc::new(move |…| { … })` snippet for callbacks with
  the right arity, and every icon constant for `IconData` props.
  `Reactive<…>` is transparent (the macro coerces); `Option<…>` props get
  `Some(…)`-wrapped values (with the `.into()` an `Option<Ref>` needs)
  plus `None`. Only a bare `name = <path>` counts as a value position —
  inside a nested expression rust-analyzer owns the completion.
  Accepting a value also adds the `use` it needs when the file lacks one
  (`use idea_ui::typography_kind;` after the last top-level `use`; the
  item's detail shows it): the catalog's `import` for registered values,
  `module::Type` for enum variants, `crate::…` inside the defining crate.
  A `use` naming the module, a parent glob, or a local `mod` of that name
  counts as already imported.

Hover inside `ui!` / `jsx!`:

- **A tag** (`Typography(`, `text {`) — the component's doc comment and
  every prop with its type and first doc paragraph.
- **A prop name** (`kind = …`) — that prop's type and full doc.
- **A value** (`typography_kind::Body`, also inside `Some(…)`) — the
  registered value's docs and the `use` that brings it in.

rust-analyzer's own hover (the props-struct type) still shows; this one
stacks above it with the catalog's documentation.

Authoring hints for the reactive/component vocabulary, outside the
markup macros:

- **In a fn body** — `signal`, `memo`, `memo_with`, `effect!`, `rx!`,
  `spawn_then`, `resource`, `mutation`, `reducer`, `watch`, `untrack`,
  `on_cleanup`, `on_scope_drop`, `provide`/`inject`, `after_ms_scoped`,
  `raf_loop_scoped`, `node_ref!`, `animated!`, `timeline!`, `ui!`, …
  each inserting the catalog's snippet with the arguments you supply as
  tab stops (`let ${name} = signal(${value});`) and the framework docs
  (when to use it, the sharp edges) as documentation. Handler bodies
  inside `ui!` (`on_click = Rc::new(move || { │ })`) get the same set.
  A snippet that declares its own binding drops it when you've already
  written one: `let count = sig│` completes to `let count = signal(…);`,
  not `let count = let name = signal(…);` (same for `reducer`'s tuple).
- **At item level** — `#[component]` (a whole component fn skeleton),
  `#[props]` (a props struct with a documented field), `stylesheet!`.

The snippets and docs are the catalog's `macros` / `utilities` slices
(`idealyst mcp`'s `describe_macro` / `describe_utility` return the same
`snippet`), so an agent and the editor propose one canonical spelling.

Theme-token completion inside `stylesheet! { … }` blocks:

- **Token completion** off the block's binding — `base(t) { padding: t.│ }`
  offers the namespaces (`color`, `intent`, `spacing`, `radius`,
  `typography`), and `t.spacing.│` offers that namespace's tokens.
  Accepting inserts the call (`md()`); each item shows the registry name
  it resolves under and the theme's base value (`spacing-md · 12px`).
- Nested vocabularies work to any depth — `t.intent.primary.│` offers the
  six intent slots.
- A binding the sheet opted out of (`base(_t)`) offers nothing: the macro
  doesn't bind `_t`, so suggesting tokens there would propose code that
  doesn't compile.

Data comes from the live catalog: the extension shells out to
`idealyst catalog-json` (first run compiles the catalog wrapper —
minutes cold, seconds warm) and caches in memory. `Idealyst: Refresh
Catalog` (command palette) re-reads after you add components or
dependencies. Everything the CLI prints while building — and every
load/failure — lands in **Output ▸ Idealyst**; look there first when a
popup is empty.

## Which catalog a file sees

The catalog is resolved **per file**, not per workspace, so monorepos
work, and every shape is warmed as soon as you open a file — it should
be loading before your first keystroke:

- Walking up from the file, the nearest crate whose `Cargo.toml` has
  `[package.metadata.idealyst]` is the project. Its catalog holds its
  own components plus every component library it depends on — exactly
  the vocabulary usable from that file. In `crates/app-main/src/*.rs`
  that's `crates/app-main`.
- A file in a plain library crate that depends on the framework
  (`runtime-core` / `idealyst` / `idea-ui` in its `Cargo.toml`) is handed
  to `catalog-json` as that crate, and the CLI catalogs the **lightest
  idealyst app that depends on it** — any app that pulls the library in
  sees the same library components, and one app keeps the build small
  even in a workspace with dozens of apps.
- Anything else — a crate with no framework dependency, a file outside
  any crate — gets nothing, silently, and never spawns the CLI. The
  extension activates for every Rust file; other people's Rust must
  stay untouched.

This complements rust-analyzer, which owns types/expressions (including
inside the macros via `ui!`'s IDE-recovery expansion) but cannot know
the DSL vocabulary.

For theme tokens the gap is total, and measured rather than assumed. A
headless LSP probe, run against this repo (so every crate in the probed
file compiles) with a plain-Rust control in the same file and session:

| cursor | completions |
| --- | --- |
| `s.│` on a `String` — plain Rust control | **123** |
| `padding: t.spacing.m│d()` — valid sheet body | **0** |
| `gap: t.│` — mid-typing | **0** |

The control rules out an unindexed server: RA was fully warmed and its
proc-macro server loaded, in the same session that returned 0 inside the
macro. So RA offers nothing for a token accessor
inside `stylesheet!` *at all* — not merely while the body is unparseable.
Expanding the macro isn't enough; RA also has to map the cursor's source
range onto a node in the expansion, and idealyst's own IDE work has
already established that RA's proc-macro server reports degenerate
zero-width spans for every token, which is what that mapping needs.

Hence this provider. It doesn't compete with RA inside `stylesheet!` —
there is nothing there to compete with.

## Install (no build step)

Plain dependency-free JS — symlink into the extensions dir and reload:

```bash
ln -s "$(pwd)/editors/vscode-idealyst" ~/.vscode/extensions/idealyst.vscode-idealyst-0.1.0
```

Then "Developer: Reload Window". If the `idealyst` on your PATH is older
than this checkout, point the extension at the fresh binary in the
project's `.vscode/settings.json`:

```json
{ "idealyst.cli": "/path/to/idealyst-native/target/debug/idealyst" }
```
