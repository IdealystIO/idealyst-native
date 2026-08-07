# Idealyst VS Code extension

DSL-vocabulary completion inside `ui! { … }` / `jsx! { … }` blocks:

- **Tag completion** in child position — every primitive (`view`, `text`,
  `button`, …) and every `#[component]` in the project + its
  dependencies, with docs.
- **Prop completion** inside a tag's parens — `Button(│` offers `label`,
  `on_click`, `tone`, … with types and doc comments; props already
  written are filtered out; accepting inserts `name = `.

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
`idealyst catalog-json` once per workspace (first run compiles the
catalog wrapper — minutes cold, seconds warm) and caches in memory.
`Idealyst: Refresh Catalog` (command palette) re-reads after you add
components or dependencies.

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
