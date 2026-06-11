# `lint` — the idealyst source linter

Flags three idiom-drift patterns in idealyst projects, over the project's
**un-expanded** Rust source:

| Rule | Default | Flags | Use instead |
|------|---------|-------|-------------|
| `prefer-signal-macro` | warn | `Signal::new(v)` | `signal!(v)` |
| `prefer-effect-macro` | warn | `Effect::new(\|\| …)` | `effect! { … }` |
| `prefer-memo-macro` | warn | `memo(\|\| …)` | `memo!(…)` |
| `prefer-ui-macro` | warn | `builder::view(…)`, `BuildElement::build(…)`, `Element::View { … }` | `ui! { … }` / `jsx! { … }` |
| `component-pascal-case` | error | `#[component] fn icon_button` | `#[component] fn IconButton` |

> **Why un-expanded source?** After macro expansion, `signal!(0)` *is*
> `Signal::new(0)` and `ui! { … }` *is* `BuildElement::build(…)` — the idiom
> choice has vanished. A clippy/rust-analyzer lint pass runs post-expansion
> and can't see it. This linter parses with `syn` and walks the tree before
> expansion, which is the only place the question "did the author use the
> macro?" still has an answer. As a bonus, `syn` never descends into macro
> token streams, so anything *inside* `ui! { … }` / `signal!( … )` is
> invisible — legitimate macro use is never flagged.

## CLI

```sh
idealyst lint                  # lint ./ , human report
idealyst lint crates/ui        # lint a subtree
idealyst lint --rules          # list rules + default levels
idealyst lint --deny-warnings  # CI strict mode: warnings fail the exit code
idealyst lint --format json    # cargo-style JSON (for editors / CI tools)
```

Exit status mirrors `cargo check`: non-zero when any **error**-level
diagnostic fires (or a file fails to parse), or any warning under
`--deny-warnings`.

## Configuration — `idealyst-lint.toml`

Discovered by walking up from the lint target. Every rule is individually
settable to `off` / `warn` / `error` (the ESLint model):

```toml
# idealyst-lint.toml
[rules]
component-pascal-case = "error"   # keep the hard line on naming
prefer-signal-macro   = "warn"
prefer-effect-macro   = "warn"
prefer-ui-macro       = "off"     # e.g. a crate that hand-builds elements
```

### Inline suppression

```rust
// Whole file:
// idealyst-lint-disable-file
// Whole file, one rule:
// idealyst-lint-disable-file prefer-signal-macro

// Next line, all rules:
// idealyst-lint-disable-next-line
let s = Signal::new(0);

// Same line, specific rules (comma- or space-separated):
let s = Signal::new(0); // idealyst-lint-disable-line prefer-signal-macro
```

A directive with no rule ids after it suppresses **all** rules on its target
line/file.

## rust-analyzer integration (inline editor squiggles)

rust-analyzer has no lint-plugin API, but its flycheck runs an arbitrary
command and renders the cargo-JSON diagnostics it prints. Point it at
`idealyst lint --format json` and the lint findings show up as squiggles
next to `cargo check`'s.

`.vscode/settings.json` (or the equivalent RA client setting):

```jsonc
{
  // Run BOTH cargo check and the idealyst linter. RA merges the
  // diagnostics from each line of JSON the command prints.
  "rust-analyzer.check.overrideCommand": [
    "idealyst", "lint", "--format", "json", "."
  ]
}
```

To keep `cargo check`'s type errors *and* add lint diagnostics, run a small
wrapper script that emits both streams' JSON, or use
`rust-analyzer.check.extraArgs` strategies per your client — see the
"Combining with cargo check" note in the framework lint guide.

The JSON is the `cargo check --message-format=json` shape: one
`{"reason":"compiler-message", …}` per finding plus a trailing
`{"reason":"build-finished", …}`. Diagnostic codes are `idealyst::<rule>`
(e.g. `idealyst::prefer-signal-macro`), so they're easy to filter.

## The hard-stop companion: `strict-naming`

`component-pascal-case` is a *lint* (warns/errors in the linter). For a
build that must never compile a misnamed component, turn on the
`strict-naming` Cargo feature (forwarded `runtime-core/strict-naming` →
`runtime-macros/strict-naming`): `#[component]` then emits a
`compile_error!` on any non-PascalCase fn name. The lint catches it while
you type; the feature stops the build. Use the feature in CI, the lint
everywhere.
