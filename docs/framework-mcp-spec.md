# Framework MCP — Design Spec

> Status: draft / pre-implementation. Author: Nicho. Date: 2026-05-22.

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
- Compile-time-extracted metadata: the MCP runtime does **not** parse Rust source.
- Per-platform safety: extracting MCP data is host-only and doesn't bloat iOS / Android / wasm binaries.
- Mirrors the project's existing distributed-registration pattern (`Element::External` / `ExternalRegistry`).

**Non-goals**

- Not a `syn`/`rust-analyzer`-driven source walker at MCP serve time. The catalog is what the proc-macros emitted during the user's normal compile.
- Not a runtime introspection of mounted instances ("the planet at index 2"). The MCP exposes the *declared* component catalog and its static composition graph.
- Not a substitute for inline documentation. A component shipped without doc comments produces a sparse MCP entry. The MCP is a mirror, not an authoring surface.

## 3. Architecture overview

Three pieces:

```
┌──────────────────────────────────────────────┐
│ #[component] fn foo(...) {                   │
│     ui! { bar() baz() }                      │
│ }                                            │
│                                              │
│   ↓ #[component] attribute macro walks the   │
│     function body for ui! invocations and    │
│     pulls the child idents directly.         │
│                                              │
│ inventory::submit!(ComponentEntry {          │
│     name: "foo",                             │
│     composes: ["bar", "baz"],  // ← embedded │
│     ...                                      │
│ })                                           │
└──────────────────────────────────────────────┘
                ↓
        ┌───────────────────────────────────────────────┐
        │ idealyst-mcp-runtime                          │
        │   - drains ComponentEntry + ToolEntry slices  │
        │   - resolves bare composes idents to FQNs     │
        │   - derives reverse adjacency in O(n)         │
        │   - serves MCP JSON-RPC over stdio            │
        └───────────────────────────────────────────────┘
                          ↑
                ┌───────────────────────────────────────┐
                │ src/bin/mcp.rs in the user's app      │
                │   fn main() { idealyst_mcp_runtime::  │
                │       serve(); }                      │
                └───────────────────────────────────────┘
```

### 3.1 Distributed registration via `inventory`

The [`inventory`](https://crates.io/crates/inventory) crate provides distributed slice registration. Each `#[component]`-attributed function emits one `inventory::submit!` placing a `ComponentEntry` into a project-global slice. **The `composes: Vec<EdgeRef>` field on the entry holds the list of components this one renders** — emitted *by the same `#[component]` macro* after it walks the function body looking for `ui! { ... }` invocations. One slice, one macro, no separate joiner step beyond name resolution.

The MCP runtime iterates the slice once at startup; cost on non-MCP builds is zero when feature-gated (see §9.2).

This mirrors the `Element::External` design already in the framework: typed, distributed, no central registry to keep in sync. See [[project_third_party_extension]] for the prior art the team is already comfortable with.

### 3.2 How the composition list is built

The `#[component]` attribute macro has full access to the annotated function's body. After expanding the normal component code, it walks the body's AST for `ui! { ... }` invocations and pulls each call-shaped child in JSX position. For `app()` in [examples/welcome/src/app.rs](examples/welcome/src/app.rs), that produces:

```text
composes: [
    { name: "dark_layer",   site: "app.rs:21" },
    { name: "vignette",     site: "app.rs:22" },
    { name: "sun_glare",    site: "app.rs:23" },
    { name: "planets",      site: "app.rs:24" },
    { name: "content_layer", site: "app.rs:25" },
]
```

The `ui!` macro itself stays simple — it has no need to know the enclosing function context. Reverse adjacency ("what uses `planet`?") is a one-pass scan at runtime: `O(n × avg-composes-per-entry)`, trivial at any realistic project size.

### 3.3 MCP runtime

A small crate `idealyst-mcp-runtime`:

1. Drains `inventory::iter::<ComponentEntry>` and `inventory::iter::<ToolEntry>`.
2. Resolves bare `composes` idents to fully-qualified names via module proximity (§6).
3. Derives reverse adjacency once at startup.
4. Implements the JSON-RPC subset of MCP over stdio.

The user's `bin/mcp.rs` is a one-liner that calls `idealyst_mcp_runtime::serve()`.

## 4. Macro surface

### 4.1 Extending the existing `#[component]`

The current `#[component(default(...), children)]` in [component_attr.rs:30-72](crates/framework/macros/src/component_attr.rs#L30-L72) parses defaults and a children flag. We extend it with optional MCP metadata:

```rust
#[component(
    default(size_dp = 24.0, color = "#888888"),
    children,
    mcp(
        category = "decoration",
        example = "planet(0, &refs)",
        unstable,
    ),
)]
pub fn planet(idx: usize, refs: &WelcomeRefs) -> Element { ... }
```

| Sub-arg              | Meaning                                                                       |
|----------------------|-------------------------------------------------------------------------------|
| `category = "..."`   | Free-form bucket — `"layout"`, `"input"`, `"decoration"`, etc.                |
| `example = "..."`    | Single-line snippet shown alongside the component in MCP responses.            |
| `unstable`           | Marks the entry as unstable; LLMs see a warning in its description.            |
| `skip`               | Do not register this component for MCP.                                       |
| `uses = ["..."]`     | Explicit override for the usage graph (covers helpers that escape `ui!`).      |

Doc comments (`///`) on the function and on each prop-struct field are already visible to the macro — no new syntax needed for prose. Free-form text on the user's part remains optional; the macro must produce a useful entry without any of the `mcp(...)` sub-args.

The prop-type story:

Both single-struct and free-form positional signatures are first-class. The macro extracts a structured `ParamSpec { name, type_str, doc }` per parameter in either shape:

- **Single-struct signature** (`fn planet(props: PlanetProps) -> Element`): the macro records the struct's `TypeId` + `type_name`. The runtime expands each field via the `IdealystSchema` derive (§4.3) so per-field doc comments and `#[schema(...)]` hints flow through.
- **Free-form positional signature** (`fn planet(idx: usize, refs: &WelcomeRefs) -> Element`): the macro records each parameter's name and a pretty-printed type string. Per-arg docs are pulled from a Rustdoc-style `# Arguments` block in the function-level `///` if present, or from an optional `#[mcp(doc = "...")]` attribute on individual args.

Both shapes produce the same `ParamSpec` payload in the catalog and the same MCP schema output. The choice between them is purely a style call — exactly as it is in React with destructured props vs. a single `props` object.

### 4.2 New: `#[idealyst_tool]`

For standalone functions the developer wants to expose:

```rust
#[idealyst_tool(category = "color", example = "darken(\"#abc\", 0.2)")]
/// Returns a hex color darkened by `amount` in linear-light space.
pub fn darken(hex: &str, amount: f32) -> String { ... }
```

Same metadata vocabulary as `mcp(...)`. Submits a `ToolEntry` to its own inventory slice. Usage edges for tool calls inside `ui!` are **not** recorded by default — `ui!` cannot distinguish a tool call from a component call without name resolution. A later opt-in `mark_tool_call!(name)` macro can populate tool-usage edges deliberately.

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

## 5. MCP surface

Exposed tools (initial set, subject to revision after a prototype):

| Tool                            | Purpose                                                        |
|---------------------------------|----------------------------------------------------------------|
| `list_components`               | All components, with category / unstable filters.              |
| `describe_component(name)`      | Full record: signature, props, docs, example, source location. |
| `find_uses(name)`               | Who composes `name`? (reverse edges)                           |
| `find_dependencies(name)`       | What does `name` compose? (forward edges)                      |
| `list_tools`                    | All `#[idealyst_tool]` functions.                              |
| `describe_tool(name)`           | Tool record.                                                   |
| `search(query)`                 | Fulltext over names + doc comments.                            |

Exposed resources:

- `idealyst://catalog` — denormalized JSON catalog.
- `idealyst://graph` — adjacency list of the usage graph (DOT-friendly).

Notifications (server → client):

- `notifications/resources/list_changed` — sent when the catalog has been rebuilt and entries have been added or removed.
- `notifications/resources/updated` — sent per-resource when a specific component or tool changes (more granular; clients can choose to refetch just the affected entry).

See §8.1 for how rebuilds are triggered and §10 for which phase delivers this.

## 6. Name resolution

The `composes` field holds bare idents (`"dark_layer"`, `"planet"`) because proc-macros have no name resolution. The runtime maps them to fully-qualified names once at startup. Three issues to be explicit about:

1. **Proximity-based resolution.** `dark_layer(&refs)` is just an ident; the macro can't know it resolves to `crate::components::dark_layer::dark_layer`. The runtime matches against `ComponentEntry.short_name`, breaking ties by source-module proximity: a reference originating in `crate::a::b` resolves to a same-module match first, then ancestor-module, then crate root, then workspace-wide. Ambiguous matches are surfaced as `unresolved` with a candidate list.

2. **False positives we deliberately avoid.** `ui!` contains many non-component calls (`(0..3).map(|i| planet(i, refs)).collect()`). The `#[component]` macro only captures idents that appear *as the head of a child block in the JSX-ish position* of `ui!`, not arbitrary expression-position calls. That matches author intent: a "component" is something you place in a `ui!` slot.

3. **Indirection through helpers.** Today, [planets()](examples/welcome/src/components/planet.rs#L72-L74) returns `Vec<Element>` and is called from `app()` as `planets(&refs)`. So `app.composes` contains `"planets"`, and `planets` itself — if annotated `#[component]` — has `composes: ["planet"]` from its own `ui!`. A pure helper that *never* runs through a `ui!` and isn't itself a `#[component]` is invisible. That's acceptable: the abstraction layer that matters in the catalog is the component layer. The `mcp(uses = [...])` override covers escape cases.

The runtime distinguishes resolved entries from unresolved ones in MCP output. LLMs see "composes `dark_layer` (resolved to `crate::components::dark_layer::dark_layer`)" vs "composes `unknown_thing` (unresolved)" and can act accordingly.

## 7. Build / feature wiring

```toml
# idealyst-mcp-runtime/Cargo.toml
[features]
default = []
```

```toml
# user app Cargo.toml
[features]
mcp = ["idealyst-mcp-runtime", "runtime-core/mcp"]

[[bin]]
name = "mcp"
required-features = ["mcp"]
```

The `mcp` feature on `runtime-core` activates `cfg(idealyst_mcp)`, which gates every `inventory::submit!` emission in the proc-macros. Native release builds with the feature off carry zero MCP data.

Per [CLAUDE.md §3], MCP is a peripheral feature — the runtime and registry live outside `runtime-core`. `runtime-core` only exposes the `cfg` flag and the macro-emission helper functions.

## 8. CLI integration

Add `crates/cli/src/cmd/mcp.rs`:

- `cargo idealyst mcp` — build the project with `--features mcp` and launch the MCP server (stdio). Live-updates the catalog as files change (§8.1).
- `cargo idealyst mcp --json-catalog` — print the catalog as JSON and exit. Useful for testing and for piping into other tools.
- `cargo idealyst mcp --check` — lint pass: warn on unresolved usage edges, components without doc comments, props without docs. Suitable for CI.
- `cargo idealyst mcp --no-watch` — disable live updates (one-shot serve; rebuild the catalog manually by restarting).

`mcp --check` is the build-time hook teams should run to keep the MCP catalog clean.

### 8.1 Live updates as the codebase changes

The MCP server runs as a long-lived process under `cargo idealyst mcp` and updates its catalog in real time as the user edits code. Three pieces make this work:

**a) Protocol support is already there.** MCP defines `notifications/resources/list_changed` and `notifications/resources/updated` — the server pushes these to connected clients (Claude Desktop, claude.ai/code, IDE extensions) when its catalog changes. The client side handles them; we don't need a custom transport.

**b) File-watch and rebuild via the existing `dev-reload` infrastructure.** The repo already has `crates/dev/reload/` driving hot-reload for the running app. The MCP server hooks into the same file-watch loop — when a source file under the project changes, `dev-reload` triggers a rebuild; the MCP server intercepts the catalog-rebuild step. We do **not** stand up a separate file watcher; that would invite drift between what the running app sees and what the MCP catalog sees.

**c) A thin catalog-extraction binary that compiles fast.** This is the critical optimisation. The full `--features mcp` user-app binary is heavy because it links every platform backend. But the catalog binary doesn't need iOS UIKit, Android JNI, wgpu, or any other backend — it just enumerates `inventory::iter` and prints JSON. We give the catalog binary its own Cargo profile (`[profile.mcp-catalog]`) and feature surface that excludes platform-backend deps. Result: incremental rebuilds in the low-second range, even when only a comment changed.

Flow on a file change:

```
file save → dev-reload detects → cargo build --features mcp --bin mcp_catalog
          → catalog binary prints JSON to stdout
          → mcp-runtime parses, atomically swaps in-memory catalog
          → emits notifications/resources/list_changed to subscribed clients
```

If the rebuild fails (compile error), the MCP server keeps serving the previous good catalog and surfaces the build error as a server-side log entry. The client doesn't see a stale-vs-fresh distinction unless it specifically queries for build status (future tool: `last_build_status`).

**Latency expectations.** Sub-second for trivial edits (doc-comment-only changes hit incremental compilation hard); a few seconds when component bodies change; ten-plus seconds on the first cold rebuild or when something in `runtime-core` itself changes. Acceptable for the workflow — the user isn't blocked on it, the LLM consumer just sees a fresher catalog on the next query.

## 9. Open questions

### 9.1 `inventory` vs `linkme`

Both rely on linker-section magic and don't work in `no_std` / `wasm32-unknown-unknown` without effort. Since the MCP build is always host-targeted (it runs on the developer's machine, never on iOS / Android / wasm), this is fine — we only need it to work on host triples. `inventory` is more widely used in the ecosystem. Pick `inventory` for v1; revisit if cross-crate registration proves flaky.

### 9.2 Always-on vs feature-gated registration

**Decision: always feature-gated.** Confirmed with the author — the 99% use case ("normal native build") must carry zero MCP overhead. `inventory::submit!` is only emitted when `cfg(idealyst_mcp)` is on (driven by the `mcp` Cargo feature on `runtime-core`). iOS / Android / wasm release binaries with the feature off contain no MCP data; the macro expansions are pure no-ops.

This is not a "binary size optimization" trade-off — it's a correctness requirement that the introspection layer doesn't leak into shipping native code.

### 9.3 Third-party component crates

A dependency that exposes `#[component]`s appears in the MCP catalog as long as it depends on `runtime-core` (which re-exports the macros) with the catalog emission on. The one subtlety is `inventory`'s cross-rlib behavior: its linker-section ctors only survive linking if the linker actually pulls that crate's object code into the catalog binary. Merely listing a component library in `[dependencies]` does **not** guarantee that — if the project doesn't yet reference any of the library's symbols, the linker can drop the whole object file and its registrations with it.

**Resolved by force-linking in the catalog wrapper.** `idealyst mcp`'s managed wrapper (`crates/tools/cli/src/cmd/catalog_wrapper.rs`) walks the project's `cargo metadata`, finds every direct dependency that itself depends on `runtime-core`, declares each as a direct wrapper dependency (sourced to match how the project resolves it — `path` in workspace mode, `git` in git mode — so cargo unifies them into one instance), and emits a `use <dep> as _;` for it. That pins each component library's object code, so its registrations are always present regardless of whether the project references the library.

Validated against `examples/login-demo` (depends on `idea-ui`): the generated wrapper force-links `idea-ui`, and all 45 of its components surface in the emitted catalog. In git mode a force-linked dependency is required to originate from the framework's own git repo; a component library resolved from a *different* foreign source is skipped (re-declaring a foreign source would fork the crate graph), and its components still appear once the project references the crate.

### 9.4 Catalog schema versioning

The MCP catalog JSON itself should be versioned (`catalog_version: 1`). LLM-consumer tools and external scripts should be able to pin to a version. Bump on any breaking change to field names or shapes.

### 9.5 Naming the entry crate

`idealyst-mcp-runtime` follows the project convention of bare names (no `idealyst-` prefix on workspace crates — see [[feedback_no_idealyst_prefix]]). Actual workspace name will be `mcp-runtime` or `mcp` — to be decided when the crate is created.

## 10. Phasing

| Phase | Deliverable                                                                                                              |
|-------|--------------------------------------------------------------------------------------------------------------------------|
| 1     | `#[component]` records name + signature + doc + file/line into `inventory`, including `composes: Vec<EdgeRef>` extracted by walking the function body. `cargo idealyst mcp --json-catalog` emits the flat catalog. No schema, no live updates. |
| 2     | Runtime name-resolution: bare `composes` idents → fully-qualified names. `find_uses` / `find_dependencies` work; unresolved entries flagged. |
| 3     | `ParamSpec` extraction lands for both single-struct and positional-arg signatures. `#[derive(IdealystSchema)]` available as opt-in refinement. `#[idealyst_tool]` lands. |
| 4     | Full MCP JSON-RPC server (`mcp-runtime`). `cargo idealyst mcp` launches stdio MCP. Static catalog only.                  |
| 5     | **Live updates** — thin catalog-extraction binary with its own Cargo profile (no platform backends). `dev-reload` integration. `notifications/resources/list_changed` flows to connected clients. |
| 6     | `mcp --check` lint pass; CI integration; user-facing docs.                                                               |

Phases 1–2 are the minimum viable spike — they validate the registration + resolution model on a known-graph fixture. Phases 3–4 finish the static catalog. Phase 5 turns it into the live tool the workflow really wants. Phase 6 hardens for production use.

## 11. Testing

Per [CLAUDE.md §1], runtime-core-touching changes need tests. Specifically:

- A macros-crate integration test asserts a `#[component]` function produces a discoverable inventory entry with the expected `short_name`, `file`, `line`.
- A second test asserts `ui! { dark_layer() }` inside a host fn produces an edge with `to: "dark_layer"` and the host fn's path as `from`.
- A runtime test drives the joiner against a fixture project with a known component graph and asserts `find_uses` / `find_dependencies` results, including the proximity-resolution rules.
- A feature-gating test confirms that without `--features mcp`, no `submit!` is emitted (`inventory::iter` is empty). Use a tiny test binary built without the feature.

## 12. Risks

| Risk                                                       | Mitigation                                                                                                       |
|------------------------------------------------------------|------------------------------------------------------------------------------------------------------------------|
| Usage graph false positives / negatives.                   | Resolved vs unresolved distinction in MCP output. `mcp --check` surfaces unresolved cases. `mcp(uses = [...])` override for escape hatches. |
| Cross-crate inventory flakiness.                           | Validate in phase 1 with a workspace fixture. Fall back to `linkme` if `inventory` proves unreliable.            |
| Catalog schema churn breaks downstream tools.              | Explicit `catalog_version` from day 1; bump on any breaking change.                                              |
| Developer adoption — components without docs.              | `mcp --check` warns. The existing `audit` mechanism could grow an `mcp-coverage` audit (see [.claude/audits/](.claude/audits/)). |
| Macro complexity bloats compile times.                     | Submission is one static per component / per edge. Existing `#[component]` already does heavier work; this is additive but small. |

## 13. What this is *not* doing (and why)

To anchor the design against scope creep:

- **Not generating Rust code on the fly to add components.** An LLM consuming the MCP can describe what to write; it cannot mutate the catalog at serve time. The catalog is read-only — every change is a rebuild.
- **Not exposing internal helpers automatically.** A function without `#[component]` or `#[idealyst_tool]` is invisible. This is the policy by design: the developer chooses what's part of the public surface.
- **Not a chat-with-your-codebase server.** It's a structured catalog. Free-form Q&A over the catalog is the consumer's job (an LLM client), not the server's.
- **Not a style or convention enforcer.** The MCP records what's there; it does not have opinions about whether you should use a props struct vs. positional args, whether your component name is the right shape, whether your doc comments are thorough enough, etc. Stylistic / convention drift is the job of the existing audit system ([.claude/audits/](.claude/audits/) + `/audit`). The MCP `--check` lint (§8) is scoped narrowly to *catalog integrity* — unresolved usage edges, missing-doc warnings that affect catalog completeness — not project-wide style policy.
