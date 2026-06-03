# Catalog Scopes & Unified Documentation — Design Spec

> Status: draft / pre-implementation. Author: Nicho. Date: 2026-06-02.
>
> Companion to [`framework-mcp-spec.md`](framework-mcp-spec.md). That spec
> defines *what* the catalog extracts (components, primitives, utilities,
> recipes, …) and how it's collected (`inventory`, compile-time, JSON
> round-trip). This spec defines how that content is **organized** — so the
> catalog is a navigable tree of documented features, not a flat pile of
> auto-extracted notes — and **unifies** scopes, recipes, and documentation
> targets behind one model that every doc surface (components and base
> framework utilities included) shares.

## 1. Motivation

The catalog today is a set of independent slices: components, primitives,
utilities, states, guides, methods, animations, types, tools, recipes. Each is
discoverable, but there is no **spine** — nothing says "these things belong to
the Authentication feature" or "this prose explains that utility." Two concrete
gaps:

1. **No organizing structure.** Adding a free-form prose slice would turn the
   catalog into a junk drawer. We need sections that are *structural and
   enforced*, not a convention authors are trusted to follow.
2. **Recipes are component-only.** A `recipe!` can only demonstrate a
   `#[component]`. Free functions / utilities — `parse_color`, server-fn
   helpers, etc. — have no compile-checked usage examples, even though they're
   exactly the API surface an LLM most often gets wrong.

The fix is one unified model: a flat set of **Scope** labels as the organizing
layer, every documentable thing is an **Entity** that lands in a scope, and
**attachments** (recipes, anchored prose) hang off entities. Components and
base-framework utilities use the *same* system — they don't get a bespoke path.

## 2. Goals & non-goals

**Goals**

- A flat set of **Scope** labels (sections) declared programmatically — no TOML manifest.
- Scope assignment is **ambient**: `#[component]` takes **no scope argument**.
  Authors don't repeat themselves; a component inherits the scope of its module.
- Works with **`strict-docs`**: under the strict gate, an undocumented /
  unscoped surface is a *compile error*, consistent with the existing
  missing-`///` behavior.
- **Recipes target any entity** — component, utility, free function, type — via
  one generalized resolver. `recipe!(Component, …)` stays valid as sugar.
- Base-framework primitives/utilities **pre-declare** their scopes; the macro
  just uses what exists.
- Kill the per-slice serialization triplication (struct ↔ `catalog_json` writer
  ↔ `leak_*_from_json` reader) while we're restructuring.

**Non-goals**

- Not a runtime builder model. Scopes and attachments are compile-time
  `inventory` registrations (see §4); a runtime `let scope = …` value cannot
  drive life-before-main collection and would break cross-crate auto-discovery.
- Not a replacement for inline `///` docs — those remain the source of prose for
  entities. Scopes organize; they don't author.
- Not changing the wire/JSON transport beyond adding the new slices.

## 3. The unified model

Three node kinds and one resolver:

- **Scope** — a **flat** label. Fields: stable `slug` (identity), `title`,
  `docs`, `module_path` (for the ambient join), `order`. **No hierarchy** —
  there is no `parent`/tree. Granularity comes from module nesting (a scope at
  `crate::ui::inputs` is "nearer" than one at `crate::ui`), not an explicit
  tree. Hierarchy was dropped deliberately: it added cycle/dangling-parent
  validation and a marker-type compile-time system for speculative benefit
  (navigation breadcrumbs), while the one valuable behavior — ambient
  assignment — never depended on it. See §13.
- **Entity** — anything documentable: component, primitive, utility, tool, type,
  method, animation. Each has a stable ref `(kind, module_path, name)`.
- **Attachment** — recipes and anchored prose notes, attached to an entity
  and/or a scope.
- **Resolver** — one name→ref resolver across **all** entity kinds. Today
  [`resolve.rs`](../crates/mcp/catalog/src/resolve.rs) only buckets components
  by name; it generalizes to bucket every entity kind, reusing the existing
  proximity logic (`resolve_one` / `is_ancestor_module`) verbatim.

Two distinct graphs coexist and must not be conflated:

- **`composes`** — the structural dependency graph (what a component renders).
  Already captured per `ComponentEntry`.
- **`scope`** — the editorial/feature grouping. Orthogonal to `composes` and to
  module structure-as-dependency.

## 4. Scope declaration — `doc_scope!` (compile-time item macro)

`doc_scope!` is an **item-position** macro, not a `let` binding. It expands to
an `inventory::submit!(ScopeEntry)` carrying its **stable slug** (the identity —
see §4.1) and `module_path!()` (used only for the ambient join in §5) at the
declaration site. No marker item is emitted — scopes are flat, so there's
nothing for a marker type to reference:

```rust
mod auth {
    doc_scope!(Auth = "Authentication");
    // optional refinement:
    // doc_scope!(Auth = "Authentication", slug = "auth", docs = "Login, sessions, tokens.", order = 10);

    mod forms {
        #[component] fn LoginForm(..) -> Element { .. }   // ← inherits `Auth` ambiently
    }
}
```

### 4.1 Scope identity is a stable slug, not the module path — *decided*

A scope's identity is a **slug that is independent of module location**, so
moving or renaming a module never reorganizes the catalog or breaks saved
references / MCP `describe_scope(id)` calls. The slug defaults to the
`doc_scope!` marker ident (`Auth` → `"auth"`) — already location-independent —
and is overridable via an explicit `slug:` for when you want to rename the
marker without breaking the external key.

`module_path!()` is still recorded on `ScopeEntry`, but **only** to drive the
ambient proximity join in §5 (which components fall under this scope's subtree).
It is *not* the identity. Consequence: physically relocating a `doc_scope!`
changes which components it captures (membership follows code structure, as
intended), while its slug — and therefore every external reference to it — stays
fixed.

Rationale for compile-time (not a runtime value): catalog extraction never
*runs* user code — it links the crate and reads the linker section populated at
life-before-main. A `let MyScope = doc_scope!(…)` binding only exists when its
function executes, which is never during extraction, so any builder hung off
that value would register nothing. Making `doc_scope!` an item macro keeps the
ergonomic naming (`Auth` is a real path you reference) while preserving the
cross-crate auto-collection that makes third-party SDK docs "just work."

## 5. Ambient scope assignment

`#[component]` already emits `module_path!()` into its `ComponentEntry`.
`ScopeEntry` carries its own `module_path!()`. Scope assignment is therefore a
**build-time proximity join** — same module → closest ancestor → crate-root —
reusing the resolver's existing tie-break rules. A component at
`crate::auth::forms` inherits the `Auth` scope declared at `crate::auth`. **No
scope argument on the macro.**

Base-framework utilities are registered in hand-maintained tables
([`utilities.rs`](../crates/mcp/catalog/src/utilities.rs)) with `module_path`
values like `runtime_core` / `runtime_core::color`. The framework ships a `core`
scope at `module_path: "runtime_core"` ([`scopes.rs`](../crates/mcp/catalog/src/scopes.rs)),
so every such utility resolves to it by the same ambient proximity rule as
components — no per-entry scope field. (The scope is a table submit rather than a
`doc_scope!` because `runtime-core` can't self-reference `::runtime_core::__mcp`.)
User crates declare their own scopes via `doc_scope!`. All feed one flat scope
set.

## 6. Recipes target any entity

`recipe!` stops being component-specific. The first argument is an **entity
ref** resolved through the generalized resolver:

```rust
recipe!(in Auth, LoginForm,   fn login_basic()   { ui!{ .. } });   // → component
recipe!(in Auth, parse_token, fn token_example() { parse_token(..) }); // → free fn / utility
```

`recipe!(Component, fn …)` remains valid as sugar for `recipe!(target:
Component, fn …)`. A recipe in some module inherits that module's ambient scope
just like a component, so utility recipes organize identically. Cross-kind name
collisions (a component *and* a utility both named `foo`) reuse the resolver's
existing `EdgeStatus::Ambiguous` path — no new mechanism. `describe_utility`,
`describe_type`, etc. surface their recipes exactly the way `describe_component`
does today.

## 7. Enforcement

Every rule sorts by a single question: **does it need the whole graph?**

### 7.1 Compile-time (rustc) — local facts

- **Missing `///` docs** → `compile_error!` at the item (existing `strict-docs`
  behavior, unchanged).

That's the only scope-related compile-time fact. **Scopes have no other rustc
enforcement, by design.** Once hierarchy was dropped (§13) there is no parent
graph to validate, so the only remaining facts are "this crate has ≥1 scope"
(a marginal backstop `--check` already subsumes — a scope-less crate has *every*
component unscoped) and "this component is covered by a scope" (whole-graph,
inherently impossible as a rustc error — the macro is per-item and never sees
the assembled crate). Both are handled at build-time. There is no sentinel, no
marker-type system — those existed only to serve a hierarchy that no longer
exists.

### 7.2 Build-time (`mcp --check`) — whole-graph facts

The macro is per-item and never sees which scopes exist elsewhere, so this is
necessarily `--check`-time:

- **Unscoped component** — a first-party component resolves to no scope →
  **warning** by default, **error** under `--strict-scopes` (see §8). Only
  emitted once the project has declared ≥1 scope (a project not using scopes
  gets no noise).

## 8. Strictness gating — two modes, the right tier each

Whether "unscoped" is valid is the author's decision, expressed as a gate. The
decision splits across tiers, and only the maximal variant is a Cargo feature
(because Cargo feature unification leaks across the dependency graph):

| Decision | Mechanism | Scope of effect |
|---|---|---|
| unscoped valid (warns) | `mcp --check` default | first-party warnings |
| unscoped invalid (errors) | `mcp --check --strict-scopes` (CLI flag) | **first-party only**; deps stay lenient |

**Status:** both rows are **implemented** (`LintOptions { strict_scopes,
first_party_crates }` + `idealyst mcp --check [--strict-scopes]`). There is no
"won't-compile-without-a-scope" rustc tier — see §7.1.

The unscoped-component policy lives in `--check`, deliberately *not* as a Cargo
feature: a feature would unify across the build and make third-party SDKs with
unscoped components fail a check the consumer can't fix. `--check` applies
strictness **first-party only** — error on entries whose root crate is in the
workspace, warn on pulled-in deps. This is the *only* enforcement tier for
scopes; there is no compile-time variant (§7.1).

## 9. Serialization — kill the triplication

Adding `ScopeEntry` and generalized recipe shapes would otherwise mean three
new hand-mirrored sites (struct ↔ `catalog_json()` writer ↔
`leak_*_from_json()` reader) that silently drift. This restructure is the moment
to introduce a `CatalogSlice` trait with owned `#[derive(Serialize,
Deserialize)]` mirror types:

```rust
trait CatalogSlice {
    const KEY: &'static str;              // JSON object key, e.g. "scopes"
    fn to_json(&self) -> serde_json::Value;
    fn from_json(v: &serde_json::Value) -> Option<Self> where Self: Sized;
}
```

`catalog_json()` and `build_from_json()` iterate a registry of slices instead of
open-coding each. Adding a kind becomes ~2 sites: define the slice + (optionally)
add a tailored MCP tool. The `&'static str`-based inventory entries stay for the
linker-section requirement; the owned mirror types exist only for the JSON
boundary.

## 10. MCP surface

- `list_scopes(filter?)` → `{ slug, title, order, summary }`; the flat scope list.
- `describe_scope(slug)` → scope docs + the entities that resolve into it
  (components, utilities, …) by nearest-module proximity.
- **Anchored prose notes** get no list tool — they fold into `describe_*` of the
  entity they're `about`, the way recipes already do, so the LLM gets the note
  in context of the thing it asked about, never as a flat pile.
- Existing `describe_component` / `describe_utility` / `describe_type` gain a
  `scope:` field and surface their recipes regardless of entity kind.
- The raw `idealyst://catalog` resource gains `scopes` automatically once the
  slice is registered.

## 11. Migration / back-compat

- `recipe!(Component, fn …)` → sugar for the targeted form; no call-site change.
- Existing `#[component]`s with no ancestor `doc_scope!` resolve to the default
  scope and emit a `--check` **warning** (lenient default) — nothing breaks.
- `strict-docs` behavior is unchanged; `strict-scopes` is additive and opt-in.
- The framework's own crates declare their scopes + adopt `--strict-scopes`
  first, dogfooding the strict path.

## 12. Open questions

1. **Default scope identity** — one synthesized `Uncategorized` per crate, or
   the crate-root `doc_scope!` when present? (Affects the `--check` warning
   message and whether root-level placement is "real" or "fallback.")
2. ~~**Scope id stability**~~ — **decided (§4.1)**: identity is a stable slug
   (default = marker ident, overridable via `slug:`), independent of module
   path; `module_path` drives only the ambient join, not identity.
3. **Anchored-prose body source** — inline string literal vs
   `include_str!("notes/foo.md")` vs a doc-comment-carrying marker item. Long
   prose wants a file; short notes want inline.
4. ~~**`parent:` ergonomics**~~ — **moot:** hierarchy was dropped (§13), so
   scopes have no `parent`.

## 13. Phasing

1. ✅ `CatalogSlice` trait + migrate existing slices to it (no behavior change).
2. ✅ `ScopeEntry` + `doc_scope!` item macro + ambient module-proximity join.
3. ✅ Generalize the resolver to all entity kinds; generalize `recipe!` target
   (`component` → `target`, `recipes_for`, `resolve_entity`/`EntityKind`).
4. ✅ `--check` tier: unscoped-component warning + `--strict-scopes` first-party
   gate (`LintOptions` + CLI flag). *(Cycle/dangling-parent checks existed
   briefly, then were removed with hierarchy.)*
5. ⛔ **Dissolved — hierarchy dropped.** Phase 5 was the compile-time scope
   guarantee; with flat scopes there is no parent graph to enforce and the only
   remaining facts are handled at build-time (§7). No sentinel, no marker types.
6. ✅ MCP tools: `list_scopes` / `describe_scope`; fold `scope` + cross-kind
   recipes into `describe_component` / `describe_utility`.
7. ◐ Framework crates declare their scopes (flat). idea-ui declares a
   `components` scope (`components/mod.rs`) that every component resolves to by
   ambient proximity; mcp-catalog ships a `core` scope for the `runtime_core`
   utilities (a table there, not `doc_scope!`, since `runtime-core` can't
   self-reference `::runtime_core::__mcp`). Verified by `catalog-docs`'s
   `every_idea_ui_component_is_scoped` test — `--check --strict-scopes` passes
   for idea-ui. **Follow-up:** finer scopes (Inputs/Layout/Feedback/…) need the
   flat `components` modules grouped into category submodules — deferred
   (a ~36-file move + taxonomy decision).
