---
name: mcp-catalog-drift
description: Check the locked MCP catalog tables (`PrimitiveEntry`, `UtilityEntry`, `StateEntry`, `GuideEntry`) against their underlying truth — the `Element` enum, the framework's utility surface, the `stylesheet!` parser's state whitelist, and the guides directory.
targets:
  - crates/mcp/catalog/src/primitives.rs
  - crates/mcp/catalog/src/utilities.rs
  - crates/mcp/catalog/src/states.rs
  - crates/mcp/catalog/guides
  - crates/runtime/core/src/element.rs
  - crates/runtime/macros/src/stylesheet.rs
severity: medium
---

# MCP catalog drift audit

## Background

The framework's MCP catalog ships four **locked** slices — only `mcp-catalog` can construct entries (via a private `_seal: ()` field on each entry type). The lock means third parties can't add their own primitives/utilities/states/guides, which is what we want, but it also means the framework itself has to keep the hand-curated tables in sync with their underlying truth.

Three drift surfaces matter:

1. **Primitive tables vs. the `Element` enum.** Adding a variant to `crates/runtime/core/src/element.rs` without a corresponding `inventory::submit!` in `crates/mcp/catalog/src/primitives.rs` means the new primitive is invisible to AI/idea-ui/the doc-site. The `tests/registers_component.rs::primitives_table_includes_core_set` test catches missing core entries but isn't exhaustive over every variant.

2. **State table vs. the `stylesheet!` parser whitelist.** `crates/runtime/macros/src/stylesheet.rs` hard-codes the four valid state names (`hovered`, `pressed`, `focused`, `disabled`). Changing either side without the other is a silent drift — the parser would accept a state the catalog doesn't document, or vice versa.

3. **Guide cross-references.** Each `guides/*.md` may reference catalog entries via `[[name]]`. If a primitive is renamed or removed, every dangling `[[name]]` becomes a broken link. Same for `[[memory]]` references that ship as part of in-repo memory-style cross-links.

## Checklist

### Primitive coverage

- [ ] Open `crates/runtime/core/src/element.rs`. List every `pub enum Element { … }` variant.
- [ ] For each variant, search `crates/mcp/catalog/src/primitives.rs` for an `inventory::submit!` whose `pascal_name` matches the variant ident, or `name` matches the snake_case form of the variant.
- [ ] Flag any variant with no matching entry. Suggest the matching `PrimitiveEntry { name, pascal_name, docs, props, category, backends, _seal: () }` block.

### Utility coverage

- [ ] Inspect `crates/mcp/catalog/src/utilities.rs`. For each entry, verify the named function actually exists in `runtime_core` (or the named module). Grep the codebase for `pub fn <name>` under `module_path`.
- [ ] Flag entries whose `module_path::name` resolves to nothing — those are stale catalog claims that will mislead AI authors.
- [ ] Flag *new* `pub fn`s in `runtime_core::{color, time, theme, layout}` that match the utility-shape contract (free function, returns a small framework type, not bound to a primitive) and are not yet in the table.

### State whitelist parity

- [ ] Read the allowlist in `crates/runtime/macros/src/stylesheet.rs` (search for `let allowed = [`).
- [ ] Compare with the four `inventory::submit!` calls in `crates/mcp/catalog/src/states.rs`. They must be set-equal.
- [ ] Flag any mismatch as `severity: high` — a parser-accepted state without a catalog entry (or vice versa) breaks the cross-platform contract.

### Guide link integrity

- [ ] For every `guides/*.md` file, grep for `[[name]]` references.
- [ ] Each reference's `name` should resolve to one of:
  - a primitive `name` or `pascal_name` (lower-cased) — e.g. `[[View]]`, `[[scroll_view]]`
  - a utility `name` (e.g. `[[platform]]`, `[[parse_color]]`)
  - a state name (`[[hovered]]`, `[[pressed]]`, `[[focused]]`, `[[disabled]]`)
  - a guide slug (e.g. `[[getting-started]]`) optionally followed by `|display text`
  - a known repo memory entry (e.g. `[[backend_owns_rendering]]`, `[[ios_scrollview_bounds_origin]]`).
- [ ] Flag broken links — fix the markdown or the catalog table to match.

### Build-graph dependencies

- [ ] Verify `crates/mcp/catalog/build.rs` still scans `guides/*.md` (`println!("cargo:rerun-if-changed=guides")` is present and the loop reads each `.md` file).
- [ ] Verify `crates/mcp/catalog/src/guides.rs` is the single-line `include!(concat!(env!("OUT_DIR"), "/guides_generated.rs"));`.

### Test coverage

- [ ] `cargo test -p mcp-catalog` should still pass. In particular: `catalog_json_v2_includes_every_new_slice`, `primitives_table_includes_core_set`, `states_table_has_exactly_the_four_interaction_states`, `utilities_table_includes_platform_accessor`, `guides_table_includes_getting_started`.

## Output format

Report findings as a Markdown list. For each finding include:

- **Severity**: low / medium / high (state-parity mismatches are always high)
- **Location**: `crate/src/file.rs:line` (or `crates/mcp/catalog/guides/<slug>.md:line` for guide links)
- **Issue**: one-line description (e.g. `Element variant Splat has no PrimitiveEntry`)
- **Why**: brief reasoning (1–3 sentences) — what feature regresses if this isn't fixed
- **Suggested fix**: actionable recommendation, or "needs design discussion"

End with a one-line summary: `Result: N high, M medium, K low findings.`
