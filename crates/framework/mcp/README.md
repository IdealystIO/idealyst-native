# framework-mcp

Phase 1 prototype of the framework's component catalog: the data layer that
backs tooling like `cargo idealyst mcp --json-catalog`, the stdio MCP server
in [`../../mcp-server`](../../mcp-server), and AI-assisted authoring.

## What this crate provides

- **`ComponentEntry`**: one record per `#[component]` in the workspace.
  Captures the component's name, file/line, prop list (name + pretty-printed
  type), and `composes` edges (other components called from its body).
- **`inventory` distributed slice**: `#[component]` (under the
  `framework-macros/mcp` feature) emits an `inventory::submit!` for each
  component. `entries()` walks them.
- **`dump_catalog_json`**: serialises every registered entry to JSON on
  stdout. The minimum surface for a CLI subcommand to wire up.
- **`resolve` module**: phase 2 work that turns the bare idents recorded
  in `composes` edges into fully-qualified `EntryRef`s
  (same-module-first → closest ancestor → workspace-wide).

## Status

Phase 1 emits the flat catalog with `composes` edges as bare idents. Phase 2
(resolution) is in `resolve.rs`. Phase 3+ (richer prop metadata, slots,
example bundles) is sketched in `docs/framework-mcp-spec.md`.

## What this crate is *not*

This crate is the data plane. It does not enforce naming conventions, style
guidelines, or any other policy. Convention enforcement lives in
`.claude/audits/` (see `feedback_mcp_no_style_policy` in memory). The
catalog mirrors what exists.

The real downstream consumer of this catalog is the [idea-ui](../../ui/idea-ui)
component library and the docs site, not this repo's examples (see
`project_framework_mcp_consumer` in memory).
