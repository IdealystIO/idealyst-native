# Catalog tools

The catalog is generated from the project's own source: every `#[component]`, primitive, utility, macro, token, type, recipe, SDK and icon pack the project links. It is the current project, not a training-data guess.

The intended flow is top-down: a `list_*` tool returns a light summary of every item, narrowed with `filter`, then the matching `describe_*` tool returns one item in full. `search` cuts across every slice when you don't know which slice holds the answer.

## filter

Every `list_*` tool that takes `filter` matches it the same way:

- Case-insensitive.
- Whitespace-separated terms are AND'd. A term matches if it is a substring of the item's name or fully-qualified name, plus the extra fields that tool names (category, kind, …).
- `*` is a glob wildcard: `butt*`, `*view`, `*field*`.
- Omit, or pass an empty string, to list everything.

## name resolution

`describe_*` / `find_*` tools take a short name (`card`) or a fully-qualified name (`mcp_demo::components::card`). A short name resolves by module proximity. If it matches in several crates, the lookup is reported as ambiguous with its candidates; pass `app` (see `get_docs("app")`) or the fully-qualified name.

## catalog_status

Reports whether the served catalog is current, being rebuilt, or STALE because the last rebuild failed, plus how long a build takes, so you can judge whether waiting is worth it.

The catalog comes from compiling the project, so after an edit it lags, and a project that does not compile leaves the PREVIOUS catalog in place. Call this after changing a component when you need to know whether what you read reflects the change.

- `wait_for_current: true` blocks until an in-flight rebuild lands. One waiting call costs less than polling across several turns.
- `timeout_seconds` defaults to 120, capped at 600. On timeout the current status is returned. A timeout is an answer ("still building"), not an error.

## search

Fulltext search over EVERY catalog slice: components, primitives, utilities, macros, guides, types, methods, recipes, scopes, tools, states, animations, icon packs.

Multi-word queries are tokenized on whitespace and OR'd: an entry matches if ANY term appears in its name or docs, and results are ranked by how many distinct terms each entry hit. So `http fetch network request` surfaces the networking guide/utility even though no single field holds the whole phrase.

Returns `[{ kind, name, fqn, score, matched_terms, docs_excerpt }]`, tagged with the slice each match came from. For individual icons use `search_icons`; this searches pack-level metadata only.

## list_components

Returns `[{ app, name, fqn, summary }]` (summary = first doc line). Source: live apps over their Robot bridge, else the in-process / project catalog. Follow with `describe_component` for props, full docs and composes edges.

## describe_component

Full record: docs, params, file/line, and its composes edges with resolution status.

## find_uses

Reverse adjacency: every component that composes the target, as an array of FQNs.

## find_dependencies

Forward adjacency: `[{ raw_name, resolved_fqn, status }]` per edge the host composes.

## list_methods

Every imperative method declared via `#[method] fn foo(…) { … }` inside `#[component]` bodies, tagged with its parent component. `name` filters by the parent component. Live instances' methods are called with `invoke_method`.

## list_animations

Every `AnimatedValue` declared in a `#[component]` body (`let x = animated!(…)`), as `{ parent_fqn, binding, initial, line }`. `name` filters by parent component.

## list_primitives

The leaf nodes of `ui!` (`view`, `text`, `button`, `scroll_view`, …) as `{ name, pascal_name, category, summary }`. `filter` also matches `pascal_name` and `category`.

## describe_primitive

Docs, every prop (name/type/doc/constraint), backend support and category. Accepts `scroll_view` or `ScrollView`.

## list_utilities

Free helpers called outside `ui!` (`platform()`, `parse_color()`, `now_micros()`) as `{ name, fqn, category, return_type, summary }`.

## describe_utility

Docs, params and return type. Inlines the return type's record (enum variants, struct fields) when known, plus the LSP `snippet` used for authoring hints.

## list_macros

The authoring macros (`effect!`, `ui!`, `#[component]`, `stylesheet!`, `animated!`) as `{ name, invocation, kind, summary }`. Signal creation is the plain `signal(value)` FUNCTION (see `list_utilities`), not a macro.

## describe_macro

What the macro does, the crate exporting it, the canonical invocation, and a one-line sketch of what it expands to (`effect!` → `Effect::scoped(move || …)`). `effect` and `effect!` both resolve.

## list_states

The four interaction states (`hovered`, `pressed`, `focused`, `disabled`): the valid names for `state foo(theme) { … }` arms in `stylesheet!`. Returns `{ name, docs, backends }`.

## list_tokens

The theme tokens a `stylesheet!` can name through its block binding, as `{ name, path, namespace, value_type, default_value, vocabulary }`. `path` is what you type (`spacing.md` → `t.spacing.md()`); `name` is the registry key it resolves under (`spacing-md`). A token not listed here does not exist, and referencing one is a compile error.

## describe_token

One token by registry name (`spacing-md`) or accessor path (`spacing.md`): the accessor to write in a `stylesheet!`, its registry name, value type and base value.

## list_types

Types registered with `#[derive(IdealystSchema)]` as `{ short_name, fqn, kind }`.

## describe_type

Struct fields (name, type, doc, constraint) or enum variants (name, docs, payload).

## list_values

The VALUES an open-set prop type accepts: the answer to "what can I write for `tone = …`?" when the prop type is `ToneRef` / `VariantRef` / `TypographyKindRef` / `ButtonSizeRef` / `ShapeRef`, or an app's own. Each is a marker registered with `#[schema(value_of = …)]`.

Returns `{ value_of, spelled, import, short_name, fqn, docs }`. `spelled` is the paste-ready call-site form (`tone::Primary`) and `import` the `use` path that brings it into scope (`idea_ui::tone`). An `Option<Ref>` prop needs `Some(tone::Primary.into())`. Filter by the prop type, e.g. `ToneRef`. Closed enums are not here; use `describe_type` for their variants.

## list_tools

`#[idealyst_tool]`-registered functions as `{ name, fqn, return_type, summary }`.

## describe_tool

One `#[idealyst_tool]` function: docs, parameters, return type.

## list_recipes

Compile-checked usage examples for any catalog entity, as `{ name, target, fqn, summary }`. Recipes show the canonical, type-verified way to use something.

## describe_recipe

One recipe in full: target entity, docs, the compile-verified `source` (copy-pasteable, proven to type-check against the current API), and the entities it uses.

## list_scopes

Documentation scopes (declared with `doc_scope!`) that group components/utilities by feature area, as `{ slug, title, order, summary }`.

## describe_scope

One scope by slug: title, docs, and the entities assigned to it by module proximity. A component declared inside a nearer scope belongs to that nearer scope, so this lists what lives *directly* in the scope.

## list_sdks

The opt-in SDK crates: capabilities that ship OUTSIDE runtime-core (networking, persistence, camera, the component library, …). They are invisible to `list_components` / `list_primitives` / `list_utilities` because they expose plain functions/types or scene-registry extension primitives. This is how you find which crate makes a network request (`net`), persists data (`storage` / `credentials`) or renders a map (`maps`).

Returns `{ name, category, kind, dep_line, summary }`. The prose home is the `sdks` guide (`read_guide`).

The catalog only holds what the project links: an SDK not yet in `Cargo.toml` contributes nothing to the other slices until it is added.

## describe_sdk

Summary, the `Cargo.toml` dependency line, capability category, whether its surface is plain API or a `ui!` extension primitive, and the guide documenting it. Accepts the crate name (`net`, `storage`, `idea-ui`).

## list_icon_sets

Icon packs (crates like `icons-lucide` exposing named icon `const`s for the `icon(...)` primitive), as `{ name, title, icon_count, import_path, license, homepage }`. Per-icon names are deliberately NOT included (a pack has ~1600): use `search_icons` to find one, or page through `describe_icon_set`.

## describe_icon_set

Pages through one pack: `{ name, title, docs, import_path, license, icon_count, offset, limit, icons: [{ name, ident, import }] }`. `import` is the paste-ready `use` path (`icons_lucide::ARROW_RIGHT`). Icons are name-sorted; `limit` defaults to 100, max 500. The pack accepts a crate name (`icons-lucide`), title (`Lucide`), import path (`icons_lucide`) or bare family (`lucide`).

## search_icons

Searches icon names across every pack (case-insensitive substring). Returns up to `limit` (default 50, max 200) matches as `{ set, name, ident, import }`, where `import` is the paste-ready `use icons_lucide::ARROW_RIGHT;` path for the `icon(...)` primitive.

`-`, `_` and spaces are interchangeable, so `qr`, `qr_code` and `QR_CODE` all find `qr-code`. `set` restricts to one pack (same spellings as `describe_icon_set`); an unknown pack is an error, never an empty result.

## list_guides

Bundled framework guides (getting started, concepts, reactivity, styling, navigation, backends) as `{ slug, title, order, tags }`.

## read_guide

The full markdown of one guide by slug. Cross-references use `[[name]]`; resolve them with the `describe_*` tools. Framework guides are about writing apps; `get_docs` is about this server's tools.
