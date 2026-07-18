# Idealyst VS Code extension

DSL-vocabulary completion inside `ui! { … }` / `jsx! { … }` blocks:

- **Tag completion** in child position — every primitive (`view`, `text`,
  `button`, …) and every `#[component]` in the project + its
  dependencies, with docs.
- **Prop completion** inside a tag's parens — `Button(│` offers `label`,
  `on_click`, `tone`, … with types and doc comments; props already
  written are filtered out; accepting inserts `name = `.

Data comes from the live catalog: the extension shells out to
`idealyst catalog-json` once per workspace (first run compiles the
catalog wrapper — minutes cold, seconds warm) and caches in memory.
`Idealyst: Refresh Catalog` (command palette) re-reads after you add
components or dependencies.

This complements rust-analyzer, which owns types/expressions (including
inside the macros via `ui!`'s IDE-recovery expansion) but cannot know
the DSL vocabulary.

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
