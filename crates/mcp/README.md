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
| `catalog-scan` | [`catalog-scan/`](./catalog-scan) | Reads a workspace's catalog entries from **source**, with no build: it runs the catalog macros' own expansion (`runtime-macros-expand`) over the items they are attached to and reads the `inventory::submit!` literals back. Entries are byte-identical to a compiled catalog's (pinned by `mcp-catalog`'s `source_scan_reads_exactly_what_the_compiled_macros_register`). Used by `idealyst mcp`'s refresh, `catalog-json --scan` and `catalog-scan`. |
| `mcp-design-server` | [`design-server/`](./design-server) | Stdio MCP server (`idealyst mcp design`) with **opinionated idea-ui design guidance**: design skills (`content/skills/*.md`), whole-screen recipes (`content/recipes/*.rs`) and four prompts (`design_screen`, `brand_theme`, `plan_navigation`, `design_review`). Static content shipped in the CLI — no project, no catalog. See [The design server](#the-design-server). |
| `mcp-design-recipes-check` | [`design-server/recipes-check/`](./design-server/recipes-check) | Not published. Compiles every design recipe as a module against idea-ui and server-renders it at the phone/tablet/desktop widths its `//! render:` lines declare. |
| `mcp-server` | [`server/`](./server) | Stdio MCP server (`idealyst mcp`). Surfaces catalog tools (`list_components`, `describe_component`, `find_uses`, …) and the `idealyst://catalog` resource, **plus** the Robot tools (`find_element`, `click`, `type_text`, `get_snapshot`, …) that drive a running app, **plus** the dev-session tools (`run_dev` / `list_dev_sessions` / `dev_status` / `rebuild_dev` / `stop_dev` / `read_dev_log` / `wait_for_app`) that launch, observe, rebuild, and tear down `idealyst dev` from MCP — its own sessions and a person's. Reaches the app's Robot bridge via discovery (`~/.idealyst/apps/`) or an explicit `--robot-port`. Consumed by Claude Code / IDE plugins. |

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

The CLI builds that catalog without any per-project setup: it generates
a small extractor crate that links the project with the `catalog`
feature on and prints `catalog_json()` (`crates/tools/cli/src/cmd/catalog_wrapper.rs`).
Drift between code and surface is structurally impossible — they come
from the same source.

### Keeping it current without building the app

`idealyst mcp` refreshes the catalog on every save, and linking the
whole project for that is a full host build of the app — on a large
workspace, minutes of several cores competing with `idealyst dev`. So
the refresh (`idealyst catalog-json --scan`) splits the catalog at the
workspace boundary:

- **Dependencies** (idea-ui, idea-theme, SDKs) come from the compiled
  extractor with none of the workspace's crates linked
  (`catalog-json --deps-only`). It only has to rebuild when the
  dependency graph changes, and when every dependency comes from a
  registry the result is cached against the lockfile and manifests.
- **The workspace's own crates** are read from source by
  [`catalog-scan`](./catalog-scan): the same macro expansion the compiler
  runs, applied to the items carrying catalog macros, with `cfg`s
  evaluated as a catalog build would (`catalog` features on). No guessing:
  a crate whose registrations only exist at run time (idea-theme's style
  tokens compute their defaults by calling the theme) is refused by name
  and compiled with the dependencies instead.

Measured on CrewForge (35 workspace crates): a warm refresh costs about
four CPU-seconds on one core, where the compiled extractor rebuilt the
app on every save.

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

## Tool definitions stay small

An MCP client loads every tool definition into the model's context and
keeps it there for the whole session, so `mcp-server` treats the size of
its tool list as a budget. Each tool carries a one-line description and
one-line parameter docs, and nothing else:

- **Long-form docs live in [`server/docs/`](./server/docs)**, one markdown
  topic per tool group (`catalog`, `robot`, `dev`, `project`, plus
  `overview` and `app`). A `## tool_name` heading is that tool's doc. The
  `get_docs` tool serves them on demand: `get_docs()` lists the topics,
  `get_docs(name)` returns a topic or a tool's page (summary, parameters
  read from its live schema, then its section), and `query` keeps only the
  matching sections, within one doc or across all of them.
- **Advertised schemas are minified** (`server/src/schema_minify.rs`):
  the `$schema` URI, `null` unions for `Option`, integer-width formats and
  the args struct's own title and Rust doc comment are stripped before a
  client sees them. Deserialization still goes through the same typed
  structs.
- **Tests hold the line** (`tool_surface_tests` in `catalog_service.rs`):
  a total size budget, per-description and per-parameter limits, exactly
  one docs section per registered tool, and no section naming a tool that
  doesn't exist.

Adding a tool means a short `#[tool(description = …)]` plus a
`## tool_name` section in the matching topic; the tests fail until both
exist.

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
`bridge_port`, `screenshot_dir`, `no_build`, `no_watch`,
`experimental_build`. `list_dev_sessions` shows every `idealyst dev`
on the machine — the ones this server started and the ones a person
started in a terminal (found through the session registry,
`~/.idealyst/dev-sessions/`) — and `stop_dev` (by `session_id`, `pid`,
or `all`) tears a session back down — on unix the whole `idealyst dev`
process **group** (its cargo builds, web servers, simulators) gets a
graceful SIGINT escalating to SIGKILL. A session this server did not
start is stopped only with `include_external: true`. The server also
best-effort stops any sessions it still owns when it shuts down.

`dev_status` reads a session's current state (the change, the decision,
the build or patch and its errors) and `rebuild_dev` does what the
panel's `r` does — for a `--no-watch` session, applies the saves it
queued. Both work on a person's session through its control endpoint,
so an agent can drive the rebuilds of a session it did not start.

`read_dev_log` tails a session's log (with a case-insensitive `filter`,
e.g. `"error"`, applied before the tail) so you can follow the build /
spot a compile failure — readable even after the session exits.
`wait_for_app` blocks until an app registers its Robot bridge, closing
the gap between `run_dev` (returns once the build starts) and the Robot
tools (need the app actually up). The loop: `run_dev` → `wait_for_app`
(or `read_dev_log` if it's slow / failing) → drive it with the Robot
tools → `stop_dev`.

## The design server

`idealyst mcp` is deliberately neutral: it describes what the framework
and the project's catalog *are*. `idealyst mcp design` is the opinionated
layer for apps built on idea-ui — how to choose a shell and navigator, lay
out a screen, handle loading/empty/error, write forms, adapt a layout from
phone to desktop (push/pop ⇄ split view, drawer/tabs ⇄ sidebar, sheet ⇄
dialog), respect platform conventions, and carry the user's brand through
theme tokens and component restyles. The look always comes from the user;
the guidance is about structure and use, so it can be specific to idea-ui
where the base server can't.

| Surface | What |
| --- | --- |
| `list_design_skills` / `read_design_skill(name, section?)` | The skills — `principles` (embedded in every prompt), `setup`, `adaptive`, `navigation`, `screens`, `layout`, `color-and-type`, `forms`, `states`, `overlays`, `brand`, `stylesheets`, `platforms`, `review`. |
| `list_design_recipes(tag?)` / `get_design_recipe(name)` | Whole app modules (`pub fn app() -> Element`) — shells, page frame, list→detail, settings, states, dashboard, sign-in, brand module. |
| `search_design(query)` | Ranked skill sections and recipes. |
| prompts | `design_screen`, `brand_theme`, `plan_navigation`, `design_review` — each opens with `principles` and its skill's full text. |
| resources | `idea-ui-design://skill/<name>`, `idea-ui-design://recipe/<name>`. |

**Recipes can't drift.** `mcp-design-recipes-check` compiles every
`content/recipes/*.rs` as a module (a `build.rs` lists them — no registry
to keep in step) and renders each one through `backend-ssr` at the
viewports its header declares (`//! render: /path @phone => text, !absent`).
Every recipe must declare a phone and a desktop check, so a layout that
breaks at one width fails CI. `CHECKED_AGAINST_IDEA_UI` is asserted
against the workspace idea-ui.

Register it next to the base server with `idealyst mcp install --design`
(server name `idealyst-design`). Its tool list is held to the same kind of
budget tests as `mcp-server`'s (`service.rs`), and content
cross-references (`read_design_skill("…")`, "the `x` recipe") are checked
to resolve.
