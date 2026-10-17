# Project setup and linting

These tools run the same engines as the `idealyst lint` and `idealyst configure` CLI commands, against a project directory. Each takes an optional `dir` / `path`. Relative paths resolve against the server's working directory. When omitted, they use the single live app's project root, else the working directory.

## lint_project

Runs the idealyst source linter and returns its findings as JSON. Same engine and rules as `idealyst lint`. It flags idiom drift in un-expanded Rust source:

- Drift from the canonical reactive surface: `Signal::new` and the removed `signal!` macro → the `signal(…)` function; the removed `memo!` macro → `memo(move || …)`; `Effect::new` → `effect!`.
- Hand-built elements instead of `ui!` / `jsx!`: a primitive constructor called directly (`runtime_core::view(…)`, `builders::…`, or a bare `view(…)` imported from the framework), plus `BuildElement::build` and `Element::Variant {…}`.
- Non-PascalCase `#[component]` functions.

It respects `idealyst-lint.toml` (rule levels) and inline `// idealyst-lint-disable` directives. Directories are walked recursively, skipping `target/`, `.git/` and `node_modules/`.

- `deny_warnings`: set `failed` when any warning fires (CI strict mode), not only on errors / parse failures.
- `limit`: max entries in `diagnostics`, default 200, max 1000. The summary counts always cover the full run.

Returns `{ path, files_scanned, error_count, warn_count, total_diagnostics, failed, truncated, parse_errors, diagnostics[] }`, each diagnostic being `{ rule, severity, message, help, file, line, column, end_line, end_column, source_line }`.

## configure_devcontainer

Initializes or updates a project's devcontainer and toggles idealyst-managed add-ons. Same engine as `idealyst configure devcontainer` (non-interactive path). When no devcontainer exists, a minimal compose-based one is scaffolded.

### services

`services` is a list of `{ id, variant?, action? }`:

- `action`: `enable` (default; a no-op with a warning if already configured), `reconfigure` (reset to the given/default variant), or `remove`. Removing the last add-on deletes the managed file.
- An empty list just ensures a base devcontainer exists.

Service ids:

- `database` (variant `postgres` or `mysql`), `redis`, `minio`: sidecar services.
- `playwright`: the official Playwright MCP server image with headless Chromium. Agents inside the container reach it at `PLAYWRIGHT_MCP_URL` (streamable HTTP; e.g. `claude mcp add --transport http playwright "$PLAYWRIGHT_MCP_URL"`), so no browser is installed in the dev image.
- `claude` (Claude Code via its native installer; no Node.js needed), `codex` (OpenAI Codex via the node feature + npm): AI agent CLIs. Variant `host` (default) bind-mounts the host's `~/.claude` / `~/.codex` so an existing login carries over (`CLAUDE_CONFIG_DIR` / `CODEX_HOME` point the CLI at the mount); `volume` keeps an isolated login in a named volume.
- `idealyst-cli`: the `idealyst` CLI itself, built from the framework registry with `cargo install` and cached in a named volume so rebuilds skip the compile. `idealyst update` refreshes it.

### What it touches

Sidecar services live ENTIRELY in `.devcontainer/docker-compose.idealyst.yml`, which idealyst owns and regenerates and which is referenced from `devcontainer.json`'s `dockerComposeFile`. The user's own `docker-compose.yml` and its services are never touched. Agent CLIs install through `devcontainer.json` `features` / `postCreateCommand` entries; idealyst records which keys it added and only ever removes those.

`config` targets a named config (`.devcontainer/<config>/devcontainer.json`, which must already exist) instead of the default. Managed add-ons are shared across configs.

Returns `{ added, removed, reconfigured, unchanged, warnings, wrote }` (`wrote` = files created or modified).

## configure_vscode

Configures a project's VS Code workspace: recommends the extensions an idealyst project wants (rust-analyzer + the idealyst DSL extension) and wires the idealyst linter into rust-analyzer so findings show as inline squiggles. Same engine as `idealyst configure vscode`.

It merges surgically into `.vscode/settings.json` and `extensions.json`: only idealyst's keys and recommendations are set or removed, and the user's own settings are kept. It also writes a `.vscode/ra-check.sh` wrapper that emits both `cargo check` and `idealyst lint` JSON.

Aspects:

- `extensions`: the recommendations.
- `lint`: the rust-analyzer `overrideCommand` + `ra-check.sh`, and suppressing rust-analyzer's built-in case diagnostic so PascalCase `#[component]` fns aren't falsely flagged.

Pass `aspects` as `[{ id, action }]` (`enable` default, or `remove`) to act on specific aspects. Omit `aspects` to enable all, the usual setup. `remove_all: true` tears down everything idealyst manages.

Returns `{ added, removed, unchanged, warnings, wrote }`.
