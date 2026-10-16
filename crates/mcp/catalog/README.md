# mcp-catalog

The framework's catalog: the data layer behind `idealyst mcp` (the stdio MCP
server in [`../server`](../server)), `idealyst catalog-json`, the VS Code
extension's completions, and AI-assisted authoring. Design:
[`docs/framework-mcp-spec.md`](../../../docs/framework-mcp-spec.md) and
[`docs/catalog-scopes-spec.md`](../../../docs/catalog-scopes-spec.md).

## What this crate provides

- **Entry types** — `ComponentEntry` (one per `#[component]`: name,
  file/line, props, `composes` edges), plus the tool, type, value, recipe
  and scope entries the other catalog macros emit, and the locked
  framework tables (primitives, utilities, macros, states, guides, SDKs)
  only this crate can construct.
- **`inventory` distributed slices** — the catalog macros (under the
  `catalog` feature) emit an `inventory::submit!` per entry. `entries()`
  walks the components.
- **`catalog_json()` / `dump_catalog_json()`** — serialise everything
  registered (schema `catalog_version: 2`); `CatalogParts` reads a catalog
  back from JSON and merges catalogs from several sources (compiled
  dependencies + the source scan in [`../catalog-scan`](../catalog-scan)).
- **`resolve` module** — turns the bare idents recorded in `composes` edges
  into fully-qualified refs (same-module-first → closest ancestor →
  workspace-wide).
- **`guides/`** — the bundled framework guides `read_guide` serves.

## What this crate is *not*

This crate is the data plane. It does not enforce naming conventions, style
guidelines, or any other policy. Convention enforcement lives in
`.claude/audits/` (see `feedback_mcp_no_style_policy` in memory). The
catalog mirrors what exists.

The real downstream consumer of this catalog is the [idea-ui](../../ui/idea-ui)
component library and the docs site, not this repo's examples (see
`project_mcp_catalog_consumer` in memory).
