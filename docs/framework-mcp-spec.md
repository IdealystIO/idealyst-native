# Framework MCP — Design Spec

> **Status: implemented.** `idealyst mcp` (`crates/tools/cli/src/cmd/mcp.rs`)
> serves the catalog over stdio from `mcp-server` (`crates/mcp/server`);
> the catalog model is `mcp-catalog` (`crates/mcp/catalog`). `#[component]` /
> `#[props]` / `#[idealyst_tool]` / `#[derive(IdealystSchema)]` / `doc_scope!` /
> `recipe!` emit `inventory` entries when the `catalog` cargo feature is on
> (`runtime-core/catalog`; `mcp` is an alias), so shipped app binaries carry
> none. Where the shipped system differs from the original design, the
> sections below describe what shipped: there is no per-app `bin/mcp.rs` or
> `mcp-runtime` crate, the workspace's own crates are read **from source**
> (§8.1) rather than compiled, and the `mcp(category = …, example = …,
> unstable)` sub-arguments on `#[component]` were not built. Author: Nicho.

## 1. Motivation

An Idealyst app is a tree of components written in Rust. As the surface grows, the set of available components becomes the de-facto API the team — and AI assistants working in the repo — need to understand. Today that knowledge lives only in source: an LLM has to read every component file to learn what's available, what props each takes, and what composes what.

**Framework MCP** turns the framework itself into an MCP server. The same Rust code that compiles to an iOS / Android / web app also compiles to a Model Context Protocol server whose tools and resources expose:

- Every component declared in the project (and its third-party deps): name, signature, prop types, doc comments, example, source location.
- A usage graph: "what composes `planet`?" / "what does `app` compose?"
- Standalone functions explicitly tagged for MCP discovery.

The MCP server is **derived from the components themselves**, not from a parallel doc tree someone has to keep in sync. Drift is impossible by construction.

## 2. Goals & non-goals

**Goals**

- Zero-maintenance catalog: a `#[component]` is automatically discoverable. No registration list to keep updated.
- Macro-extracted metadata: catalog entries are exactly what the catalog macros emit — whether rustc compiled them (dependencies) or `catalog-scan` ran the same expansion over workspace source (§8.1).
- Per-platform safety: extracting MCP data is host-only and doesn't bloat iOS / Android / wasm binaries.
- Distributed registration: no central list — each crate's macros submit their own entries.

**Non-goals**

- Not a `syn`/`rust-analyzer`-driven interpretation of Rust source. Even the source scan runs the catalog macros' own expansion and reads back what they emit.
- Not a runtime introspection of mounted instances ("the planet at index 2"). The MCP exposes the *declared* component catalog and its static composition graph.
- Not a substitute for inline documentation. A component shipped without doc comments produces a sparse MCP entry. The MCP is a mirror, not an authoring surface.

## 3. Architecture overview

Three pieces:

```
#[component] fn Foo(...) -> Element { ui! { Bar() Baz() } }
        │  the #[component] macro walks the body for ui! invocations
        ▼
inventory::submit!(ComponentEntry { name: "Foo", composes: ["Bar", "Baz"], … })
        │  collected per refresh by `idealyst catalog-json --scan`:
        │  dependencies from a compiled extractor (`--deps-only`),
        │  workspace crates by `crates/mcp/catalog-scan`
        ▼
mcp-server (`idealyst mcp`, stdio JSON-RPC)
  - resolves bare `composes` idents to FQNs
  - derives reverse adjacency
  - serves the catalog tools + the Robot tools
```

### 3.1 Distributed registration via `inventory`

The [`inventory`](https://crates.io/crates/inventory) crate provides distributed slice registration. Each `#[component]`-attributed function emits one `inventory::submit!` placing a `ComponentEntry` into a project-global slice. **The `composes: Vec<EdgeRef>` field on the entry holds the list of components this one renders** — emitted *by the same `#[component]` macro* after it walks the function body looking for `ui! { ... }` invocations. One slice, one macro, no separate joiner step beyond name resolution.

The catalog is collected once per refresh; cost on non-catalog builds is zero because emission is feature-gated (see §9.2).

### 3.2 How the composition list is built

The `#[component]` attribute macro has full access to the annotated function's body. After expanding the normal component code, it walks the body's AST for `ui! { ... }` invocations and pulls each call-shaped child in JSX position. For `app()` in [examples/welcome/src/app.rs](../examples/welcome/src/app.rs), that produces `composes: [Vignette, SunGlare, Planet, ContentLayer]`, each with its source site.

The `ui!` macro itself stays simple — it has no need to know the enclosing function context. Reverse adjacency ("what uses `planet`?") is a one-pass scan at runtime: `O(n × avg-composes-per-entry)`, trivial at any realistic project size.

### 3.3 MCP server

`mcp-server` (`crates/mcp/server`), launched by `idealyst mcp`:

1. Loads the catalog JSON produced by `idealyst catalog-json --scan` (or a pre-built catalog binary).
2. Resolves bare `composes` idents to fully-qualified names via module proximity (§6).
3. Derives reverse adjacency once per load.
4. Implements MCP over stdio (via `rmcp`), alongside the Robot tools that drive a running app.

## 4. Macro surface

### 4.1 `#[component]`

`#[component]` (expansion in [`crates/runtime/macros-expand/src/component_attr.rs`](../crates/runtime/macros-expand/src/component_attr.rs)) records the component's name, signature, docs, source location, and `composes` edges with no extra syntax. Doc comments (`///`) on the function and on each prop field carry the prose; `recipe!` carries compile-checked examples and `doc_scope!` the grouping (see [`catalog-scopes-spec.md`](catalog-scopes-spec.md)). (The original design added an `mcp(category, example, unstable, skip, uses)` sub-argument; it was not built.)

The prop-type story:

Both the explicit props struct and inline props are first-class. The macro extracts a structured `ParamSpec { name, type_str, doc }` per parameter in either shape:

- **Single-struct signature** (`fn Planet(props: &PlanetProps) -> Element`): the macro records the struct's `TypeId` + `type_name`. The runtime expands each field via the `IdealystSchema` derive (§4.3) so per-field doc comments and `#[schema(...)]` hints flow through.
- **Inline props** (`fn Planet(idx: usize, refs: WelcomeRefs) -> Element`): the macro generates the props struct and records each parameter's name and a pretty-printed type string. A `///` doc comment on a parameter moves to the generated field, so it documents the prop exactly as a struct field's doc would.

Both shapes produce the same `ParamSpec` payload in the catalog and the same MCP schema output. The choice between them is purely a style call — exactly as it is in React with destructured props vs. a single `props` object.

### 4.2 New: `#[idealyst_tool]`

For standalone functions the developer wants to expose:

```rust
#[idealyst_tool(category = "color", example = "darken(\"#abc\", 0.2)")]
/// Returns a hex color darkened by `amount` in linear-light space.
pub fn darken(hex: &str, amount: f32) -> String { ... }
```

Submits a `ToolEntry` to its own inventory slice. Usage edges for tool calls inside `ui!` are **not** recorded — `ui!` cannot distinguish a tool call from a component call without name resolution.

### 4.3 `#[derive(IdealystSchema)]` (optional refinement)

For props structs, a derive macro that emits a JSON Schema fragment using `///` field docs and optional `#[schema(...)]` attributes:

```rust
#[derive(IdealystSchema)]
pub struct PlanetProps {
    /// Orbit semi-axis as a fraction of viewport height.
    pub rx_frac: f32,
    pub period_ms: f64,
    #[schema(constraint = "valid CSS color")]
    pub color: String,
}
```

`IdealystSchema` is **purely opt-in**. Without it, the macro still records `ParamSpec` entries for the struct's fields by walking the struct definition at macro time. The derive exists for cases where the developer wants to fine-tune the schema beyond what the macro can infer (constraint annotations, custom JSON types, enum value lists, etc.).

`IdealystSchema` is no-op-able: on non-MCP builds the impl is `#[inline]` and never called; the linker drops it.

#### 4.3.1 Open-set prop values — `#[schema(value_of = …)]`

A closed enum answers "what can I write for this prop?" through its `TypeEntry` variants. idea-theme's extensible vocabularies do not: a prop takes `impl Into<ToneRef>`, every value is a zero-sized marker implementing the `Tone` trait, and an app adds its own by writing another impl. Nothing enumerates the implementors of an open trait, so each marker registers itself:

```rust
/// Built-in semantic tone.
#[derive(Copy, Clone, Default, IdealystSchema)]
#[schema(value_of = "ToneRef", via = "idea_ui::tone")]
pub struct Primary;
```

That emits one `ValueEntry { short_name, module_path, docs, value_of, via }` into the `values` slice instead of an empty `TypeEntry`. `value_of` is the short name of the prop type the value coerces into (the join key: a prop whose type — unwrapped of `Reactive<…>` / `Option<…>` — has that short name accepts every value registered under it). `via` is the `use` path call sites reach the value through: the catalog precomputes `spelled` (`tone::Primary`, the path's last segment plus the name) and `import` (`idea_ui::tone`). The brace form `via = "idea_ui::components::{card::variant}"` spells `card::variant::Flat` and imports `idea_ui::components::card` — Rust's own `use a::{b::c}` shape. Without `via` the defining module is the import and its last segment the spelling, which is what an app's own `tone!` / `variant!` marker gets (a consumer inside that crate rewrites the leading segment to `crate`). idea-theme's `tone!` / `variant!` declaration macros add the annotation for app-defined markers automatically, and the built-in tone / variant / typography-kind / size / shape markers (plus Card's local variants) ship annotated. The VS Code extension reads this slice to complete `tone = │`.

## 5. MCP surface

Exposed tools (the catalog's core set; `list_guides`, `list_primitives`, `list_scopes`, the SDK / token / type slices, and the Robot tools sit alongside them):

| Tool                            | Purpose                                                        |
|---------------------------------|----------------------------------------------------------------|
| `list_components`               | All components.                                                |
| `describe_component(name)`      | Full record: signature, props, docs, example, source location. |
| `find_uses(name)`               | Who composes `name`? (reverse edges)                           |
| `find_dependencies(name)`       | What does `name` compose? (forward edges)                      |
| `list_tools`                    | All `#[idealyst_tool]` functions.                              |
| `describe_tool(name)`           | Tool record.                                                   |
| `search(query)`                 | Fulltext over names + doc comments.                            |
| `list_values(filter)`           | Open-set prop values (§4.3.1), filterable by the prop type.    |
| `describe_macro` / `describe_utility` | Macro / utility record incl. its `snippet` (§5.1).       |

### 5.1 Snippets on the macro and utility tables

`MacroEntry.snippet` / `UtilityEntry.snippet` carry the invocation in LSP snippet syntax — `${n:name}` placeholders for the arguments an author supplies, `$0` for the final cursor, `\t` for one indent level (`"let ${1:name} = signal(${2:value});"`, `"effect!({\n\t$0\n});"`). The VS Code extension inserts them verbatim as authoring hints in `#[component]` bodies; `describe_macro` / `describe_utility` return the same string so an agent proposes the identical spelling. Empty for an attribute or derive that is complete as written (`#[derive(IdealystSchema)]`) and for catalog-only macros nobody types by hand (`recipe!`). The emission suite pins that every `Reactive` utility has a well-formed snippet that spells its own call, and that the utility table covers the reactive surface the guides describe.

Exposed resources:

- `idealyst://catalog` — denormalized JSON catalog (`catalog_version: 2`).

Notifications (server → client):

- `notifications/resources/list_changed` — sent when the catalog has been rebuilt and entries have been added or removed.
- `notifications/resources/updated` — sent per-resource when a specific component or tool changes (more granular; clients can choose to refetch just the affected entry).

See §8.1 for how rebuilds are triggered.

## 6. Name resolution

The `composes` field holds bare idents (`"Vignette"`, `"Planet"`) because proc-macros have no name resolution. The runtime maps them to fully-qualified names once at startup. Three issues to be explicit about:

1. **Proximity-based resolution.** `Vignette(refs = refs)` is just an ident; the macro can't know it resolves to `crate::components::vignette::Vignette`. The runtime matches against `ComponentEntry.short_name`, breaking ties by source-module proximity: a reference originating in `crate::a::b` resolves to a same-module match first, then ancestor-module, then crate root, then workspace-wide. Ambiguous matches are surfaced as `unresolved` with a candidate list.

2. **False positives we deliberately avoid.** `ui!` contains many non-component calls (`format!(…)`, closures, helper fns in expression position). The `#[component]` macro only captures idents that appear *as the head of a child block in the JSX-ish position* of `ui!`, not arbitrary expression-position calls. That matches author intent: a "component" is something you place in a `ui!` slot.

3. **Indirection through helpers.** A pure helper that *never* runs through a `ui!` and isn't itself a `#[component]` is invisible. That's acceptable: the abstraction layer that matters in the catalog is the component layer.

The runtime distinguishes resolved entries from unresolved ones in MCP output. LLMs see "composes `Vignette` (resolved to `crate::components::vignette::Vignette`)" vs "composes `unknown_thing` (unresolved)" and can act accordingly.

## 7. Build / feature wiring

Emission is gated by the `catalog` cargo feature on `runtime-core`
(`mcp` is an alias). It forwards to `runtime-macros/catalog` (the
macro emission gate) and `runtime-vocabulary/catalog` (the `glue::__mcp`
anchor the emissions name, re-exported as `runtime_core::__mcp`). Builds
with the feature off carry zero catalog data.

## 8. CLI integration

`idealyst mcp` (`crates/tools/cli/src/cmd/mcp.rs`):

- `idealyst mcp` — build the catalog for the project (or workspace) in the
  current directory and serve it over stdio, refreshing as files change
  (§8.1). Robot tools proxy to a running app discovered via
  `~/.idealyst/apps/` (`--robot-port` to name one; `--no-robot` for a
  catalog-only server).
- `idealyst mcp --check [--strict-scopes]` — lint pass over the catalog the
  server would serve: unresolved usage edges, missing docs, unscoped
  first-party components. Fails if the catalog cannot be built or is empty.
  Suitable for CI.
- `idealyst catalog-json --scan` — print the catalog as JSON (what the
  refresh runs).

### 8.1 Live updates as the codebase changes

The MCP server runs as a long-lived process under `idealyst mcp` and updates its catalog in real time as the user edits code. Three pieces make this work:

**a) Protocol support is already there.** MCP defines `notifications/resources/list_changed` and `notifications/resources/updated` — the server pushes these to connected clients (Claude Desktop, claude.ai/code, IDE extensions) when its catalog changes. The client side handles them; we don't need a custom transport.

**b) The server watches the workspace.** It watches `src/` and `Cargo.toml` of every workspace crate the catalog reads (`--watch <dir>` adds more) and refreshes when one changes, so adding a component or a dependency updates the catalog without restarting the server or building the app.

**c) The refresh reads the workspace from source.** Rebuilding a catalog binary on every save would be a full host build of the app (every crate the project links, with the `catalog` feature on) — on a large workspace, minutes of several cores competing with `idealyst dev`. The refresh is `idealyst catalog-json --scan`, which splits the catalog at the workspace boundary:

- everything outside the workspace comes from the compiled extractor with no workspace crate linked (`--deps-only`), which only rebuilds when the dependency graph changes and, with registry dependencies, is cached against the lockfile and manifests;
- the workspace's own crates are read from source by `crates/mcp/catalog-scan`, which runs the catalog macros' own expansion (`crates/runtime/macros-expand`, the library `runtime-macros` is a shim over) on the items carrying them and reads the emitted `inventory::submit!` literals back, `module_path!()` / `file!()` / `line!()` resolved as rustc would. The entries are identical to a compiled catalog's; a crate whose registrations cannot be read from source (one computed at run time) is compiled with the dependencies instead.

Flow on a file change:

```
file save → watcher (src/ + Cargo.toml of every workspace crate the catalog reads)
          → idealyst catalog-json --scan: cargo metadata, source scan, cached dependency catalog
          → JSON on stdout
          → mcp-server parses it, atomically swaps the in-memory catalog
          → emits notifications/resources/list_changed to subscribed clients
```

If the refresh fails, the MCP server keeps serving the previous good catalog and surfaces the error as a server-side log entry. A source file that does not tokenize (an edit in progress) is skipped with a warning rather than failing the refresh.

**Cost.** On CrewForge (35 workspace crates) a warm refresh is about four CPU-seconds on one core, nearly all of it the macro expansion. The dependency extractor builds once, then again only when dependencies change.

## 9. Open questions

### 9.1 `inventory` vs `linkme`

Both rely on linker-section magic and don't work in `no_std` / `wasm32-unknown-unknown` without effort. Since the catalog build is always host-targeted (it runs on the developer's machine, never on iOS / Android / wasm), only host triples matter. **Decision: `inventory`**, the more widely used of the two.

### 9.2 Always-on vs feature-gated registration

**Decision: always feature-gated.** The normal build must carry zero catalog overhead. `inventory::submit!` is only emitted when the `catalog` feature is on (§7). iOS / Android / wasm release binaries with the feature off contain no MCP data; the macro expansions are pure no-ops.

This is not a "binary size optimization" trade-off — it's a correctness requirement that the introspection layer doesn't leak into shipping native code.

### 9.3 Third-party component crates

A dependency that exposes `#[component]`s appears in the MCP catalog as long as it depends on `runtime-core` (which re-exports the macros) with the catalog emission on. The one subtlety is `inventory`'s cross-rlib behavior: its linker-section ctors only survive linking if the linker actually pulls that crate's object code into the catalog binary. Merely listing a component library in `[dependencies]` does **not** guarantee that — if the project doesn't yet reference any of the library's symbols, the linker can drop the whole object file and its registrations with it.

**Resolved by force-linking in the catalog wrapper.** `idealyst mcp`'s managed wrapper (`crates/tools/cli/src/cmd/catalog_wrapper.rs`) walks the project's `cargo metadata`, finds every direct dependency that itself depends on `runtime-core`, declares each as a direct wrapper dependency (sourced to match how the project resolves it — `path` in workspace mode, `git` in git mode — so cargo unifies them into one instance), and emits a `use <dep> as _;` for it. That pins each component library's object code, so its registrations are always present regardless of whether the project references the library.

Validated against `examples/login-demo` (depends on `idea-ui`): the generated wrapper force-links `idea-ui`, and all 45 of its components surface in the emitted catalog. In git mode a force-linked dependency is required to originate from the framework's own git repo; a component library resolved from a *different* foreign source is skipped (re-declaring a foreign source would fork the crate graph), and its components still appear once the project references the crate.

### 9.4 Catalog schema versioning

The catalog JSON is versioned (`catalog_version`, currently `2`) so consumers can pin to a shape. Bump on any breaking change to field names or shapes.

## 10. Testing

Coverage the catalog relies on:

- A macros-crate integration test asserts a `#[component]` function produces a discoverable inventory entry with the expected `short_name`, `file`, `line`.
- A second test asserts `ui! { Vignette() }` inside a host fn produces an edge with `to: "Vignette"` and the host fn's path as `from`.
- A runtime test drives the joiner against a fixture project with a known component graph and asserts `find_uses` / `find_dependencies` results, including the proximity-resolution rules.
- A feature-gating test confirms that without the `catalog` feature, no `submit!` is emitted (`inventory::iter` is empty).

## 11. Risks

| Risk                                                       | Mitigation                                                                                                       |
|------------------------------------------------------------|------------------------------------------------------------------------------------------------------------------|
| Usage graph false positives / negatives.                   | Resolved vs unresolved distinction in MCP output. `mcp --check` surfaces unresolved cases. |
| Cross-crate inventory flakiness.                           | The catalog wrapper force-links every component library (§9.3); `linkme` is the fallback if `inventory` proves unreliable. |
| Catalog schema churn breaks downstream tools.              | Explicit `catalog_version`; bump on any breaking change.                                                         |
| Developer adoption — components without docs.              | `mcp --check` warns. The existing `audit` mechanism could grow an `mcp-coverage` audit (see [`.claude/audits/`](../.claude/audits/)). |
| Macro complexity bloats compile times.                     | Submission is one static per component / per edge. Existing `#[component]` already does heavier work; this is additive but small. |

## 12. What this is *not* doing (and why)

To anchor the design against scope creep:

- **Not generating Rust code on the fly to add components.** An LLM consuming the MCP can describe what to write; it cannot mutate the catalog at serve time. The catalog is read-only — every change is a rebuild.
- **Not exposing internal helpers automatically.** A function without `#[component]` or `#[idealyst_tool]` is invisible. The developer chooses what's part of the public surface.
- **Not a chat-with-your-codebase server.** It's a structured catalog. Free-form Q&A over the catalog is the consumer's job (an LLM client), not the server's.
- **Not a style or convention enforcer.** The MCP records what's there; it does not have opinions about whether you should use a props struct vs. positional args, whether your component name is the right shape, whether your doc comments are thorough enough, etc. Stylistic / convention drift is the job of the existing audit system ([`.claude/audits/`](../.claude/audits/) + `/audit`). The MCP `--check` lint (§8) is scoped narrowly to *catalog integrity* — unresolved usage edges, missing-doc warnings that affect catalog completeness — not project-wide style policy.
