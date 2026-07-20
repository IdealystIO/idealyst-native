# VS Code setup — `idealyst configure vscode`

Part of the [`idealyst configure`](./devcontainer.md) command group. This
subcommand sets up a project's `.vscode/` workspace so the editor understands an
idealyst project: it recommends the extensions you want and wires the idealyst
source linter into rust-analyzer so idiom-drift findings show up as inline
squiggles next to `cargo check`'s.

```
idealyst configure vscode                       # interactive wizard
idealyst configure vscode --non-interactive     # apply everything
idealyst configure vscode --non-interactive --lint=remove   # drop one aspect
```

## Aspects

The configuration is a registry of **aspects**, each contributing settings,
recommendations, and/or files. All are on by default.

| Aspect | What it does |
| --- | --- |
| `extensions` | Recommends `rust-lang.rust-analyzer` and the idealyst DSL extension (`idealyst.vscode-idealyst`) in `.vscode/extensions.json`. |
| `lint` | Points `rust-analyzer.check.overrideCommand` at a generated `.vscode/ra-check.sh` that emits both `cargo check` and `idealyst lint --format json`, so lint findings render as squiggles; also disables RA's built-in `non_snake_case`/`incorrect-case` diagnostics (they can't see the `#[allow(non_snake_case)]` that `#[component]` injects for PascalCase component fns). |

## Surgical, non-destructive merge

We never take ownership of your `settings.json` / `extensions.json`. The command
**sets only its own keys**, **unions only its own entries** into arrays
(`rust-analyzer.diagnostics.disabled`) and into `recommendations`, and generates
its own file (`ra-check.sh`). Your other settings are preserved untouched, and
removing an aspect pulls back out exactly what it added.

The `ra-check.sh` wrapper passes `"$PWD"` (the workspace root rust-analyzer runs
it from) as the lint path — rust-analyzer does not reliably expand
`${workspaceFolder}` inside `overrideCommand`. It's regenerated on each run.

## Interactive mode (default)

With a real terminal, the wizard shows a multi-select of aspects with the
currently-configured ones checked. Confirming applies the difference: newly
checked aspects are enabled, unchecked-but-previously-configured ones are
removed.

## Non-interactive mode

Pass `--non-interactive` (or run without a TTY). With no aspect flags it enables
every aspect. Otherwise:

| Flag | Effect |
| --- | --- |
| `--lint` / `--extensions` | Enable that aspect (no-op + warning if already configured) |
| `--lint=remove` | Remove that aspect (deletes its keys/recommendations/files) |
| `--aspect <id>[=remove]` | Same, for any registered aspect without a dedicated flag |
| `--remove` | Remove all idealyst-managed VS Code config |

Naming any aspect flag scopes the run to those aspects; unnamed aspects are left
untouched.

## Same engine from the MCP server

Like `configure devcontainer`, the logic lives in the shared `configure` crate
(`crates/tools/configure`), so an agent can do the same over MCP via the
**`configure_vscode`** tool — pass `dir` + optional `aspects: [{ id, action }]`
(omit to enable all, or `remove_all: true` to tear down) and get back the change
report.

## Extending — adding an aspect

1. Implement `VscodeAspect` in a new
   `crates/tools/configure/src/vscode/aspects/<id>.rs`, returning an
   `AspectContribution` (settings keys to set, arrays to union, extension
   recommendations, files to drop).
2. Add it to `registry()` in `crates/tools/configure/src/vscode/aspect.rs`.

The wizard, the generic `--aspect <id>` flag, and the MCP tool pick it up
automatically.
