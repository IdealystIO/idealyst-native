# `mcp/` — project-aware MCP and introspection

The same macros that emit a project's UI also emit a structured
catalog of what the project defines — components, prop schemas,
method signatures, animation declarations. The crates here turn
that catalog into surfaces external tools (LLMs, IDE plugins, test
runners) can query.

The key word is **project-aware**: the catalog is the *current*
project, not training-data approximations or source-parsed guesses.
The same source emits both the running UI and the catalog; they
cannot drift.

| Crate | Path | Role |
| --- | --- | --- |
| `mcp-catalog` | [`catalog/`](./catalog) | The catalog data model + resolution. Eight inventory slices (Component / Primitive / Utility / State / Guide / Method / Animation / Type). The `#[component]` macro submits to these slices at compile time. |
| `mcp-server` | [`server/`](./server) | Stdio MCP server (`idealyst mcp`). Surfaces catalog tools (`list_components`, `describe_component`, `find_uses`, …) and the `idealyst://catalog` resource, **plus** the Robot tools (`find_element`, `click`, `type_text`, `get_snapshot`, …) that drive a running app, **plus** the dev-session tools (`run_dev` / `list_dev_sessions` / `stop_dev` / `read_dev_log` / `wait_for_app`) that launch, observe, and tear down `idealyst dev` from MCP. Reaches the app's Robot bridge via discovery (`~/.idealyst/apps/`) or an explicit `--robot-port`. Consumed by Claude Code / IDE plugins. |

## How the catalog gets populated

```
  #[component] fn MyCard(...) { ... }
              │
              ▼
   runtime-macros emits a
   mcp_catalog::ComponentEntry
   into the `inventory` slice
              │
              ▼
   At project link time, every
   submission is collected into
   one ResolvedCatalog
              │
              ▼
   mcp-server reads the catalog
   and exposes it over stdio MCP
```

Each scaffolded project ships a `catalog` binary that exposes its
own catalog. Drift between code and surface is structurally
impossible — they come from the same source.

### Scope: the catalog is the linked dependency graph

Project-awareness has a corollary worth calling out: the catalog only
contains what the project **actually links**. An SDK you haven't added
to `Cargo.toml` yet contributes nothing to the `inventory` slices, so
`list_components` / `describe_component` / `list_icon_sets` return
nothing for it — you can't introspect an SDK's API *before* depending on
it. This is by construction (the catalog is collected at link time from
the crates in the build), not a bug. To explore a not-yet-added SDK,
add the dependency first (then rebuild the `catalog` binary), or read
the SDK crate's own docs/README. A registry that lets tools browse
*available-but-unlinked* SDKs would be a separate surface from this
link-time catalog.

## Robot is the other half

The catalog is the *static* view. Robot (in `runtime-core` under a
feature gate) is the *runtime* view — what's actually on screen
right now. `mcp-server` exposes Robot's introspection + control as
MCP tools on the same wire as the catalog, so one `idealyst mcp`
process gives external tools both halves. It connects to the running
app's Robot bridge over TCP — by `~/.idealyst/apps/` discovery, or an
explicit `--robot-port` when the address is known up front.

## Launching apps from MCP

A client that can't hold a foreground terminal can still run the app:
`run_dev` spawns `idealyst dev` detached (stdout/stderr → a per-session
log under `~/.idealyst/dev-logs/`) and tracks the process. It mirrors
the CLI flags worth driving from MCP — `platforms` (`web`/`ios`/
`android`/`macos`/`terminal`), `all`, `local`, `no_robot`,
`bridge_port`, `screenshot_dir`, `no_build`. `list_dev_sessions` shows
what's tracked + each session's status, and `stop_dev` (by `session_id`
or `all`) tears a session back down — on unix the whole `idealyst dev`
process **group** (its cargo builds, web servers, simulators) gets a
graceful SIGINT escalating to SIGKILL. The server also best-effort stops
any sessions it still owns when it shuts down.

`read_dev_log` tails a session's log (with a case-insensitive `filter`,
e.g. `"error"`, applied before the tail) so you can follow the build /
spot a compile failure — readable even after the session exits.
`wait_for_app` blocks until an app registers its Robot bridge, closing
the gap between `run_dev` (returns once the build starts) and the Robot
tools (need the app actually up). The loop: `run_dev` → `wait_for_app`
(or `read_dev_log` if it's slow / failing) → drive it with the Robot
tools → `stop_dev`.
