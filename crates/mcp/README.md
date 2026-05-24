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
| `mcp-server` | [`server/`](./server) | Stdio MCP server. Surfaces tools (`list_components`, `describe_component`, `find_uses`, …) and the `idealyst://catalog` resource. Consumed by Claude Code / IDE plugins. |
| `robot-mcp-proxy` | [`robot-proxy/`](./robot-proxy) | MCP proxy in front of the Robot runtime control surface. Lets external processes drive a running app (list components on screen, find by props/path, click, type) through the MCP wire. |

## How the catalog gets populated

```
  #[component] fn my_card(...) { ... }
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

## Robot is the other half

The catalog is the *static* view. Robot (in `runtime-core` under a
feature gate) is the *runtime* view — what's actually on screen
right now. The `robot-mcp-proxy` crate sits in front of Robot's
introspection so external tools can query the running app through
the same MCP wire.
