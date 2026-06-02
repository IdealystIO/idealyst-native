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

The fix is one unified model: a **Scope** tree as the spine, every documentable
thing is an **Entity** that lands in a scope, and **attachments** (recipes,
anchored prose) hang off entities. Components and base-framework utilities use
the *same* system — they don't get a bespoke path.

## 2. Goals & non-goals

**Goals**

- A **Scope** tree (sections) declared programmatically — no TOML manifest.
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

- **Scope** — the organizing node. Fields: stable `id`, `title`, `docs`,
  optional `parent` (→ forms a tree, the spine), `order`. Guides, project
  topics, and feature groupings are *all* scopes.
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

`doc_scope!` is an **item-position** macro, not a `let` binding. It expands to a
module-level marker item plus an `inventory::submit!(ScopeEntry)` carrying
`module_path!()` at the declaration site:

```rust
mod auth {
    doc_scope!(Auth = "Authentication");
    // optional refinement:
    // doc_scope!(Auth = "Authentication", docs: "Login, sessions, tokens.", parent: Features);

    mod forms {
        #[component] fn LoginForm(..) -> Element { .. }   // ← inherits `Auth` ambiently
    }
}
```

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

Base-framework primitives/utilities are registered in hand-maintained tables
([`primitives.rs`](../crates/mcp/catalog/src/primitives.rs), `utilities.rs`), so
they carry an explicit `scope:` id in the table — the scope genuinely
pre-exists, with zero per-call burden. The framework declares its own scope tree
(`Layout`, `Color`, `Platform`, …) once in its crate roots; user crates declare
theirs. Both feed one Scope tree.

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

## 7. Enforcement tiers

Every rule sorts by a single question: **does it need the whole graph?**

### 7.1 Compile-time (rustc) — local / absolute-path facts

- **Missing `///` docs** → `compile_error!` at the item (existing `strict-docs`
  behavior, unchanged).
- **Crate has *no* scope at all** → hard error, under the strict gate, via a
  sentinel the macro emits:

  ```rust
  // emitted only when the strict-scopes compile gate is active
  const _: () = { let _ = crate::__IDEALYST_DOC_SCOPE; };
  ```

  If no `doc_scope!` declared the sentinel at the crate root, `crate::…` fails
  to resolve → `E0433`. This is the only scope fact that can be a true rustc
  error, because it's an absolute path, not a whole-graph query.

### 7.2 Build-time (`mcp --check`) — whole-graph facts

The macro is per-item and never sees the full scope graph, so these are
necessarily `--check`-time:

- **Scope cycles** → error, with a readable message (`scope cycle: Auth →
  Features → Auth`). One DFS/topo-sort over the merged `ScopeEntry` set.
  *Cross-crate cycles are impossible by construction* — Cargo's dep graph is a
  DAG and `parent:` can only reference an in-scope (this-crate-or-upstream)
  scope, so a back-edge would require a crate cycle. Only intra-crate cycles can
  occur, and `--check` holds the whole crate's scopes.
- **Dangling `parent`** (references a scope id that doesn't resolve) → error.
- **Default-scope fallback** (an entity that reached only the synthesized
  default/root scope) → **warning** by default, error under `--strict-scopes`
  (see §8).

A type-level const-eval depth trick *can* force rustc to catch cycles, but it
produces cryptic errors (points at a const, not the cycle), forces `parent` to
be a type reference, and is the kind of too-clever construct the repo's
"proven, documented" rule discourages. Rejected in favor of the `--check` DFS.

## 8. Strictness gating — two modes, the right tier each

Whether "unscoped" is valid is the author's decision, expressed as a gate. The
decision splits across tiers, and only the maximal variant is a Cargo feature
(because Cargo feature unification leaks across the dependency graph):

| Decision | Mechanism | Scope of effect |
|---|---|---|
| unscoped valid (warns) | `mcp --check` default | first-party warnings |
| unscoped invalid (errors) | `mcp --check --strict-scopes` (or project config) | **first-party only**; deps stay lenient |
| won't-compile-without-a-scope | Cargo feature `strict-scopes` + §7.1 sentinel | **whole graph** (unification) |

The per-component default-fallback policy lives in `--check`, deliberately *not*
as a Cargo feature: a feature would unify across the build and make third-party
SDKs with unscoped components fail a check the consumer can't fix. `--check`
applies strictness **first-party only** — error on entries whose root crate is
in the workspace, warn on pulled-in deps. The compile-time sentinel remains
available as a Cargo feature for "I control my whole graph" shops and for the
framework's own crates, inheriting the same global-unification property
`strict-docs` already has (consistent, not a new hazard).

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

- `list_scopes(filter?)` → `{ id, title, parent, order, summary }`; the spine
  for navigation.
- `describe_scope(id)` → scope docs + its child scopes + the entities that
  resolve into it (components, utilities, recipes, …).
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
- The framework's own crates declare their scope tree + adopt
  `--strict-scopes`/the sentinel first, dogfooding the strict path.

## 12. Open questions

1. **Default scope identity** — one synthesized `Uncategorized` per crate, or
   the crate-root `doc_scope!` when present? (Affects the `--check` warning
   message and whether root-level placement is "real" or "fallback.")
2. **Scope id stability** — `module_path::MarkerName` is the obvious id, but it
   ties the id to module location; renaming a module reorganizes the catalog.
   Acceptable? Or allow an explicit stable slug on `doc_scope!`?
3. **Anchored-prose body source** — inline string literal vs
   `include_str!("notes/foo.md")` vs a doc-comment-carrying marker item. Long
   prose wants a file; short notes want inline.
4. **`parent:` ergonomics** — type-path reference (rustc-checked existence, but
   must be in scope) vs string id (resolved at `--check`, more flexible). The
   §7.1 cross-crate-DAG argument leans toward allowing a path *or* an upstream
   id.

## 13. Phasing

1. `CatalogSlice` trait + migrate existing slices to it (no behavior change).
2. `ScopeEntry` + `doc_scope!` item macro + ambient module-proximity join.
3. Generalize the resolver to all entity kinds; generalize `recipe!` target.
4. `--check` tier: cycles, dangling parents, default-fallback warning,
   `--strict-scopes` first-party gate.
5. Compile-time sentinel + `strict-scopes` Cargo feature.
6. MCP tools: `list_scopes` / `describe_scope`; fold scope + cross-kind recipes
   into existing `describe_*`.
7. Framework crates declare their scope tree and adopt the strict path.
