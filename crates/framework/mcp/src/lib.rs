//! Framework MCP — phase 1 prototype.
//!
//! Defines the [`ComponentEntry`] record and the [`inventory`]
//! distributed slice that `#[component]` populates when the
//! `framework-macros/mcp` feature is on. Provides [`entries`] to walk
//! every registered entry and [`dump_catalog_json`] to emit the catalog
//! as JSON on stdout — the minimum surface for `cargo idealyst mcp
//! --json-catalog` to wire up.
//!
//! See `docs/framework-mcp-spec.md` for the full plan. Phase 1 emits
//! the flat catalog with `composes` edges as bare idents. Phase 2
//! resolves those idents into fully-qualified [`EntryRef`]s via the
//! [`resolve`] module — same-module-first, then closest ancestor, then
//! workspace-wide (spec §6).

pub use inventory;

pub mod resolve;
pub use resolve::{BuildFromJsonError, EdgeStatus, EntryRef, ResolvedCatalog, ResolvedEdge};

mod primitives;
mod utilities;
mod states;
mod guides;

/// A single composition edge emitted by `#[component]` when it walked
/// the function body. `name` is the bare ident as written at the call
/// site — proc-macros run before name resolution, so the runtime is
/// responsible for resolving these to fully-qualified `ComponentEntry`
/// references (phase 2, see spec §6).
///
/// `line` is the source line of the ident *within the same file as
/// the parent entry* — the parent's `file` field plus this `line` is
/// enough to locate the edge in the user's editor.
#[derive(Debug)]
pub struct EdgeRef {
    pub name: &'static str,
    pub line: u32,
}

/// One parameter of a `#[component]` function's signature, as
/// extracted by the proc-macro at definition time.
///
/// Phase 3a records the surface-level info available directly from
/// `syn::Signature`: parameter name and pretty-printed type. Per-field
/// information about a props struct (when the signature is
/// `fn foo(props: &FooProps)`) is the job of `#[derive(IdealystSchema)]`
/// — a future addition in this phase. For now, a single-struct
/// signature surfaces as one `ParamSpec` whose `type_str` names the
/// struct; consumers can cross-reference the struct's own catalog
/// entry once `IdealystSchema` lands.
///
/// `type_str` is `quote!`-stringified, so a borrow shows as `"& Foo"`
/// (space between `&` and the type) — the catalog is for tooling /
/// AI consumption, which normalize trivially.
#[derive(Debug)]
pub struct ParamSpec {
    pub name: &'static str,
    pub type_str: &'static str,
    /// Last path segment of the type, with reference / lifetime /
    /// generic wrappers stripped. `&PlanetProps` → `"PlanetProps"`,
    /// `&'a Foo<T>` → `"Foo"`. Empty when the type isn't a path
    /// (tuples, primitives, function types, …). The MCP runtime
    /// uses this to join against [`PropsSchemaEntry`] when
    /// expanding a component's prop fields.
    pub type_short_name: &'static str,
}

/// One field of a props struct, captured by
/// `#[derive(IdealystSchema)]`. Each named field of the derived
/// struct becomes one of these.
#[derive(Debug)]
pub struct PropFieldSpec {
    pub name: &'static str,
    pub type_str: &'static str,
    /// Joined `///` doc comments on the field.
    pub doc: &'static str,
    /// Free-form constraint hint from `#[schema(constraint = "...")]`.
    /// Empty when the attribute is absent.
    pub constraint: &'static str,
}

/// A whole props struct's schema. `inventory::submit!`'d by the
/// `#[derive(IdealystSchema)]` macro. `short_name` is the struct's
/// bare ident; `module_path` is `module_path!()` at the derive site.
#[derive(Debug)]
pub struct PropsSchemaEntry {
    pub short_name: &'static str,
    pub module_path: &'static str,
    pub fields: &'static [PropFieldSpec],
}

/// A component the `#[component]` proc-macro registered at compile time.
/// Fields are all `&'static str` so the entry can live in a linker
/// section without any heap allocation. `line` is a `u32` because
/// `line!()` returns that.
#[derive(Debug)]
pub struct ComponentEntry {
    /// The component function's bare identifier — e.g. `"planet"`.
    pub name: &'static str,
    /// `module_path!()` at the registration site.
    pub module_path: &'static str,
    /// `file!()` at the registration site.
    pub file: &'static str,
    /// `line!()` at the registration site.
    pub line: u32,
    /// Concatenated `///` doc comments on the function, or the empty
    /// string when none. Newlines preserved.
    pub docs: &'static str,
    /// Components this one composes — every ident captured from
    /// `ui!` / `jsx!` invocations in the function body, in source
    /// order. Bare names; unresolved. See spec §3.2 / §6.
    pub composes: &'static [EdgeRef],
    /// Function parameters in declaration order. Empty for zero-arg
    /// components. See [`ParamSpec`].
    pub params: &'static [ParamSpec],
}

inventory::collect!(ComponentEntry);
inventory::collect!(PropsSchemaEntry);
inventory::collect!(ToolEntry);
inventory::collect!(PrimitiveEntry);
inventory::collect!(UtilityEntry);
inventory::collect!(StateEntry);
inventory::collect!(GuideEntry);
inventory::collect!(MethodEntry);
inventory::collect!(AnimationEntry);
inventory::collect!(TypeEntry);

/// A built-in framework primitive — the leaf nodes of the `ui!` /
/// `jsx!` grammar (`View`, `Text`, `Button`, `ScrollView`, ...).
///
/// **Locked**: only `framework-mcp`'s own `primitives.rs` table can
/// construct one. The `_seal` field is private, so external crates
/// can't write `PrimitiveEntry { name: ..., _seal: () }` — `#[non_exhaustive]`
/// further blocks struct-literal construction at any call site. The
/// open extension point for third-party "primitive-like" things is
/// `Primitive::External` + the per-backend `ExternalRegistry`, not
/// this slice.
///
/// Read-only consumption: every metadata field is `pub`, so callers
/// iterate the inventory slice and project entries normally.
#[derive(Debug)]
#[non_exhaustive]
pub struct PrimitiveEntry {
    /// snake_case identifier — what `pascal_to_snake` produces for
    /// the PascalCase tag authors write in `ui!`. Stable across
    /// versions; serves as the catalog key.
    pub name: &'static str,
    /// PascalCase tag — what authors actually type inside `ui!` /
    /// `jsx!`. Mirrors the variant ident on `Primitive`.
    pub pascal_name: &'static str,
    /// Concatenated `///` doc comments describing the primitive.
    /// Same shape/empty-string rules as [`ComponentEntry::docs`].
    pub docs: &'static str,
    /// Author-facing prop slots — name, type-string, doc, optional
    /// constraint hint. Reuses [`PropFieldSpec`] for symmetry with
    /// `#[derive(IdealystSchema)]` consumers.
    pub props: &'static [PropFieldSpec],
    /// Broad classification for catalog grouping. Not part of the
    /// resolver / runtime — purely for organizing MCP/doc-site
    /// output.
    pub category: PrimitiveCategory,
    /// Backends that fully support this primitive. The MCP server
    /// uses this to warn (not block) when a composes graph mixes
    /// primitives with the targeted backend's support set. Use
    /// `"all"` as a shorthand entry when every backend supports it.
    pub backends: &'static [&'static str],
    #[doc(hidden)]
    pub _seal: (),
}

/// Broad classification for [`PrimitiveEntry`]. Purely organizational
/// — not consulted by the renderer or resolver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrimitiveCategory {
    /// Layout/structure: `View`, `ScrollView`, ...
    Structural,
    /// User input: `Button`, `TextInput`, `Toggle`, `Slider`, ...
    Input,
    /// Display-only: `Text`, `Image`, `Icon`, `ActivityIndicator`, ...
    Display,
    /// Media: `Video`, ...
    Media,
    /// Control flow: `When`, `Switch`, `Repeat`, ...
    ControlFlow,
    /// Composition / overlay: `Portal`, `Presence`, `Link`, `Overlay`, ...
    Composition,
    /// Advanced / framework-internal escape hatch: `External`,
    /// `Graphics`, `Virtualizer`.
    Advanced,
}

impl PrimitiveCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Structural => "structural",
            Self::Input => "input",
            Self::Display => "display",
            Self::Media => "media",
            Self::ControlFlow => "control-flow",
            Self::Composition => "composition",
            Self::Advanced => "advanced",
        }
    }
}

/// A framework-defined utility surface — a free function in
/// `framework_core` (or a sibling) that authors call from regular
/// Rust code (not inside `ui!`). Distinct from [`ToolEntry`]
/// (`#[idealyst_tool]`): tools are MCP-callable at chat-time;
/// utilities are author-time API documentation.
///
/// **Locked** — same `_seal: ()` pattern as [`PrimitiveEntry`].
/// Third parties wanting to expose chat-callable helpers use
/// `#[idealyst_tool]`.
#[derive(Debug)]
#[non_exhaustive]
pub struct UtilityEntry {
    pub name: &'static str,
    pub module_path: &'static str,
    pub docs: &'static str,
    pub params: &'static [ParamSpec],
    /// `quote!`-stringified return type, e.g. `"Platform"` or
    /// `"Option<Rgba>"`.
    pub return_type: &'static str,
    /// Last path segment of the return type — joins to
    /// [`TypeEntry::short_name`] so consumers can inline variant
    /// docs for enum returns.
    pub return_type_short: &'static str,
    pub category: UtilityCategory,
    #[doc(hidden)]
    pub _seal: (),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UtilityCategory {
    Platform,
    Color,
    Time,
    Theme,
    Layout,
    Math,
}

impl UtilityCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Platform => "platform",
            Self::Color => "color",
            Self::Time => "time",
            Self::Theme => "theme",
            Self::Layout => "layout",
            Self::Math => "math",
        }
    }
}

/// A framework-defined interaction state — `hovered`, `pressed`,
/// `focused`, `disabled`. These are the four well-known states the
/// `stylesheet!` macro accepts in `state foo(theme) { … }` arms
/// (see [`framework_macros::stylesheet`]).
///
/// **Locked** — the four states are fixed by the cross-platform
/// contract; new ones would silently never activate on every
/// backend that didn't learn about them. Same `_seal` pattern.
#[derive(Debug)]
#[non_exhaustive]
pub struct StateEntry {
    pub name: &'static str,
    pub docs: &'static str,
    /// Whether this state fires on every backend. `hovered` is
    /// mobile-silent (no pointer); `focused` is desktop/web only
    /// for keyboard-driven UIs.
    pub backends: &'static [&'static str],
    #[doc(hidden)]
    pub _seal: (),
}

/// A bundled framework usage guide — a markdown document shipped
/// inside the catalog so any MCP client gets the framework's
/// official authoring docs without an external download.
///
/// **Locked** — guides ship with the framework version. The user
/// extension point for project-level docs is the project's own
/// MCP layer / README, not this slice. Same `_seal` pattern.
#[derive(Debug)]
#[non_exhaustive]
pub struct GuideEntry {
    /// URL-safe slug; doubles as the lookup key. Mirrors the
    /// markdown filename in `crates/framework/mcp/guides/<slug>.md`.
    pub slug: &'static str,
    pub title: &'static str,
    /// Display ordering for table-of-contents — lowest first.
    pub order: u32,
    /// Free-form classification tags surfaced through `search_guides`.
    pub tags: &'static [&'static str],
    /// Raw markdown body — `include_str!`'d at build time. Cross-
    /// references between guides / catalog entries use the
    /// `[[name]]` convention (resolved by the MCP server at read
    /// time).
    pub body: &'static str,
    #[doc(hidden)]
    pub _seal: (),
}

/// An imperative method exposed on a `#[component]`'s handle —
/// captured from the component's `methods! { fn name(&self, ...) { ... } }`
/// block (see [`framework_macros::methods_block`]). Methods are
/// open: any user component author can declare them.
#[derive(Debug)]
pub struct MethodEntry {
    /// The component the method belongs to. Same identity convention
    /// as [`ComponentEntry`] — `(module_path, name)`. Joining
    /// `MethodEntry` records by `(parent_module_path, parent_name)`
    /// yields the method list for one component.
    pub parent_module_path: &'static str,
    pub parent_name: &'static str,
    /// Method ident as written in the `methods!` block.
    pub name: &'static str,
    pub docs: &'static str,
    /// Method params (after `&self`), in declaration order.
    pub params: &'static [ParamSpec],
    /// Pretty-printed return type; empty for `()`. v1 of
    /// `methods!` forbids return types so this is always `""`
    /// today, but the field is present so a future relax is
    /// schema-compatible.
    pub return_type: &'static str,
}

/// An [`AnimatedValue<T>`]-backed animation declared in a
/// `#[component]` body — captured by the macro's walker when it
/// finds `animated!(…)` calls in the function body.
///
/// Open slice — every component author can declare animations.
/// Drift between body and catalog is tolerated: the walker is
/// best-effort and silently ignores expressions it can't parse.
#[derive(Debug)]
pub struct AnimationEntry {
    pub parent_module_path: &'static str,
    pub parent_name: &'static str,
    /// Local binding name from `let <name> = animated!(...);`.
    /// Empty when the `animated!` call wasn't bound to a `let`
    /// (e.g. an inline expression) — those still get a record so
    /// the catalog reflects every animation, but with no name to
    /// hang docs off.
    pub binding: &'static str,
    /// `quote!`-stringified initial-value expression — e.g.
    /// `"0.0_f32"` or `"(0.0_f32 , 0.0_f32 , 0.0_f32 , 1.0_f32)"`.
    /// Free-form; not parsed, just surfaced to consumers.
    pub initial: &'static str,
    /// Source line of the `animated!` call within the parent's
    /// file. 0 on stable when `span-locations` doesn't fire.
    pub line: u32,
}

/// Generalized type-catalog entry. Subsumes [`PropsSchemaEntry`]:
/// every props struct also produces a `TypeEntry` (shape `Struct`).
/// Enums get a `TypeEntry` with shape `Enum` listing their variants
/// and per-variant docs/payload.
///
/// Open — any author calling `#[derive(IdealystSchema)]` on a
/// struct or enum gets registered. The framework's own utility
/// surface (`Platform`, `SafeAreaSides`, ...) registers via this
/// slice too — locked construction isn't needed here because the
/// shape is informational, not policy.
#[derive(Debug)]
pub struct TypeEntry {
    pub short_name: &'static str,
    pub module_path: &'static str,
    pub docs: &'static str,
    pub shape: TypeShape,
}

#[derive(Debug)]
pub enum TypeShape {
    Struct { fields: &'static [PropFieldSpec] },
    Enum { variants: &'static [VariantSpec] },
}

/// One enum variant captured by `#[derive(IdealystSchema)]` on
/// an enum type.
#[derive(Debug)]
pub struct VariantSpec {
    pub name: &'static str,
    pub docs: &'static str,
    /// Empty for unit variants. Tuple variants get positional
    /// entries (`name = ""`); struct variants get named.
    pub payload: &'static [PropFieldSpec],
}

/// A standalone function the developer tagged with
/// `#[idealyst_tool]` to expose through MCP. Spec §4.2.
///
/// Unlike `ComponentEntry`, `ToolEntry` has no composes graph — tools
/// are leaf nodes. `params` records the function's parameter list in
/// the same shape as a `#[component]`'s params.
#[derive(Debug)]
pub struct ToolEntry {
    pub name: &'static str,
    pub module_path: &'static str,
    pub file: &'static str,
    pub line: u32,
    pub docs: &'static str,
    pub params: &'static [ParamSpec],
    /// The function's return type, pretty-printed. Empty for `()`
    /// / no-return functions.
    pub return_type: &'static str,
}

/// Iterate every `#[idealyst_tool]`-registered function.
pub fn tools() -> impl Iterator<Item = &'static ToolEntry> {
    inventory::iter::<ToolEntry>()
}

/// Iterate every props schema the `#[derive(IdealystSchema)]` macro
/// has registered. Empty if no struct in the build has the derive.
pub fn schemas() -> impl Iterator<Item = &'static PropsSchemaEntry> {
    inventory::iter::<PropsSchemaEntry>()
}

/// Look up a props schema by its struct's bare ident. Returns the
/// first match — `(module_path, short_name)` is the canonical
/// identity, but in practice projects don't reuse the name across
/// modules. If no struct declared the derive, returns `None`.
pub fn lookup_schema(short_name: &str) -> Option<&'static PropsSchemaEntry> {
    schemas().find(|e| e.short_name == short_name)
}

/// Iterate every component the `#[component]` macro has registered. The
/// order is link-order; callers wanting stable ordering should sort by
/// `(module_path, name)`.
pub fn entries() -> impl Iterator<Item = &'static ComponentEntry> {
    inventory::iter::<ComponentEntry>()
}

/// Iterate every [`PrimitiveEntry`] in the framework table.
pub fn primitives() -> impl Iterator<Item = &'static PrimitiveEntry> {
    inventory::iter::<PrimitiveEntry>()
}

/// Iterate every [`UtilityEntry`] in the framework table.
pub fn utilities() -> impl Iterator<Item = &'static UtilityEntry> {
    inventory::iter::<UtilityEntry>()
}

/// Iterate every [`StateEntry`] in the framework table.
pub fn states() -> impl Iterator<Item = &'static StateEntry> {
    inventory::iter::<StateEntry>()
}

/// Iterate every bundled [`GuideEntry`].
pub fn guides() -> impl Iterator<Item = &'static GuideEntry> {
    inventory::iter::<GuideEntry>()
}

/// Iterate every [`MethodEntry`] registered from `methods!` blocks.
pub fn methods() -> impl Iterator<Item = &'static MethodEntry> {
    inventory::iter::<MethodEntry>()
}

/// Iterate every [`AnimationEntry`] captured from `#[component]`
/// bodies.
pub fn animations() -> impl Iterator<Item = &'static AnimationEntry> {
    inventory::iter::<AnimationEntry>()
}

/// Iterate every [`TypeEntry`] (struct or enum) registered via
/// `#[derive(IdealystSchema)]`.
pub fn types() -> impl Iterator<Item = &'static TypeEntry> {
    inventory::iter::<TypeEntry>()
}

/// Look up a primitive by its `name` (snake_case) or `pascal_name`.
pub fn lookup_primitive(needle: &str) -> Option<&'static PrimitiveEntry> {
    primitives().find(|p| p.name == needle || p.pascal_name == needle)
}

/// Look up a utility by its bare ident.
pub fn lookup_utility(needle: &str) -> Option<&'static UtilityEntry> {
    utilities().find(|u| u.name == needle)
}

/// Look up a guide by slug.
pub fn lookup_guide(slug: &str) -> Option<&'static GuideEntry> {
    guides().find(|g| g.slug == slug)
}

/// Look up a `TypeEntry` by its bare ident (last path segment of the
/// type). Mirrors [`lookup_schema`]'s contract but unified across
/// structs and enums.
pub fn lookup_type(short_name: &str) -> Option<&'static TypeEntry> {
    types().find(|t| t.short_name == short_name)
}

/// Build the catalog as a JSON value. Schema version 2 surfaces
/// every catalog slice in a single document: components, primitives,
/// utilities, states, guides, methods, animations, types, and tools.
/// Entries within each slice are sorted by a stable key
/// (`module_path::name`, slug, etc.) so JSON diffs are minimal.
pub fn catalog_json() -> serde_json::Value {
    let mut sorted: Vec<&ComponentEntry> = entries().collect();
    sorted.sort_by_key(|e| (e.module_path, e.name));
    let components: Vec<serde_json::Value> = sorted
        .into_iter()
        .map(|e| {
            let composes: Vec<serde_json::Value> = e
                .composes
                .iter()
                .map(|edge| {
                    serde_json::json!({
                        "name": edge.name,
                        "line": edge.line,
                    })
                })
                .collect();
            let params: Vec<serde_json::Value> = e
                .params
                .iter()
                .map(|p| {
                    // If the param's type resolves to a known props
                    // schema, inline its fields. Otherwise the field
                    // is just absent — consumers can fall back to
                    // `type_str` alone.
                    let schema = if p.type_short_name.is_empty() {
                        None
                    } else {
                        lookup_schema(p.type_short_name)
                    };
                    let mut obj = serde_json::Map::new();
                    obj.insert("name".into(), p.name.into());
                    obj.insert("type".into(), p.type_str.into());
                    obj.insert("type_short_name".into(), p.type_short_name.into());
                    if let Some(s) = schema {
                        let fields: Vec<serde_json::Value> = s
                            .fields
                            .iter()
                            .map(|f| {
                                serde_json::json!({
                                    "name": f.name,
                                    "type": f.type_str,
                                    "doc": f.doc,
                                    "constraint": f.constraint,
                                })
                            })
                            .collect();
                        obj.insert("schema".into(), serde_json::json!(fields));
                    }
                    serde_json::Value::Object(obj)
                })
                .collect();
            serde_json::json!({
                "name": e.name,
                "module_path": e.module_path,
                "file": e.file,
                "line": e.line,
                "docs": e.docs,
                "composes": composes,
                "params": params,
            })
        })
        .collect();
    // Primitives — sorted by name (the snake_case key) for stable
    // diffs.
    let mut sorted_prims: Vec<&PrimitiveEntry> = primitives().collect();
    sorted_prims.sort_by_key(|p| p.name);
    let primitives_json: Vec<serde_json::Value> = sorted_prims
        .into_iter()
        .map(|p| {
            let props: Vec<serde_json::Value> = p
                .props
                .iter()
                .map(|f| {
                    serde_json::json!({
                        "name": f.name,
                        "type": f.type_str,
                        "doc": f.doc,
                        "constraint": f.constraint,
                    })
                })
                .collect();
            serde_json::json!({
                "name": p.name,
                "pascal_name": p.pascal_name,
                "docs": p.docs,
                "category": p.category.as_str(),
                "backends": p.backends,
                "props": props,
            })
        })
        .collect();

    let mut sorted_utils: Vec<&UtilityEntry> = utilities().collect();
    sorted_utils.sort_by_key(|u| u.name);
    let utilities_json: Vec<serde_json::Value> = sorted_utils
        .into_iter()
        .map(|u| {
            let params: Vec<serde_json::Value> = u
                .params
                .iter()
                .map(|p| {
                    serde_json::json!({
                        "name": p.name,
                        "type": p.type_str,
                        "type_short_name": p.type_short_name,
                    })
                })
                .collect();
            serde_json::json!({
                "name": u.name,
                "module_path": u.module_path,
                "fqn": format!("{}::{}", u.module_path, u.name),
                "docs": u.docs,
                "params": params,
                "return_type": u.return_type,
                "return_type_short": u.return_type_short,
                "category": u.category.as_str(),
            })
        })
        .collect();

    let mut sorted_states: Vec<&StateEntry> = states().collect();
    sorted_states.sort_by_key(|s| s.name);
    let states_json: Vec<serde_json::Value> = sorted_states
        .into_iter()
        .map(|s| {
            serde_json::json!({
                "name": s.name,
                "docs": s.docs,
                "backends": s.backends,
            })
        })
        .collect();

    let mut sorted_guides: Vec<&GuideEntry> = guides().collect();
    sorted_guides.sort_by_key(|g| (g.order, g.slug));
    let guides_json: Vec<serde_json::Value> = sorted_guides
        .into_iter()
        .map(|g| {
            serde_json::json!({
                "slug": g.slug,
                "title": g.title,
                "order": g.order,
                "tags": g.tags,
                "body": g.body,
            })
        })
        .collect();

    let mut sorted_methods: Vec<&MethodEntry> = methods().collect();
    sorted_methods.sort_by_key(|m| (m.parent_module_path, m.parent_name, m.name));
    let methods_json: Vec<serde_json::Value> = sorted_methods
        .into_iter()
        .map(|m| {
            let params: Vec<serde_json::Value> = m
                .params
                .iter()
                .map(|p| {
                    serde_json::json!({
                        "name": p.name,
                        "type": p.type_str,
                        "type_short_name": p.type_short_name,
                    })
                })
                .collect();
            serde_json::json!({
                "parent_module_path": m.parent_module_path,
                "parent_name": m.parent_name,
                "parent_fqn": format!("{}::{}", m.parent_module_path, m.parent_name),
                "name": m.name,
                "docs": m.docs,
                "params": params,
                "return_type": m.return_type,
            })
        })
        .collect();

    let mut sorted_anim: Vec<&AnimationEntry> = animations().collect();
    sorted_anim.sort_by_key(|a| (a.parent_module_path, a.parent_name, a.binding, a.line));
    let animations_json: Vec<serde_json::Value> = sorted_anim
        .into_iter()
        .map(|a| {
            serde_json::json!({
                "parent_module_path": a.parent_module_path,
                "parent_name": a.parent_name,
                "parent_fqn": format!("{}::{}", a.parent_module_path, a.parent_name),
                "binding": a.binding,
                "initial": a.initial,
                "line": a.line,
            })
        })
        .collect();

    let mut sorted_types: Vec<&TypeEntry> = types().collect();
    sorted_types.sort_by_key(|t| (t.module_path, t.short_name));
    let types_json: Vec<serde_json::Value> = sorted_types
        .into_iter()
        .map(|t| {
            let shape_json = match &t.shape {
                TypeShape::Struct { fields } => {
                    let fs: Vec<serde_json::Value> = fields
                        .iter()
                        .map(|f| {
                            serde_json::json!({
                                "name": f.name,
                                "type": f.type_str,
                                "doc": f.doc,
                                "constraint": f.constraint,
                            })
                        })
                        .collect();
                    serde_json::json!({ "kind": "struct", "fields": fs })
                }
                TypeShape::Enum { variants } => {
                    let vs: Vec<serde_json::Value> = variants
                        .iter()
                        .map(|v| {
                            let payload: Vec<serde_json::Value> = v
                                .payload
                                .iter()
                                .map(|f| {
                                    serde_json::json!({
                                        "name": f.name,
                                        "type": f.type_str,
                                        "doc": f.doc,
                                        "constraint": f.constraint,
                                    })
                                })
                                .collect();
                            serde_json::json!({
                                "name": v.name,
                                "docs": v.docs,
                                "payload": payload,
                            })
                        })
                        .collect();
                    serde_json::json!({ "kind": "enum", "variants": vs })
                }
            };
            serde_json::json!({
                "short_name": t.short_name,
                "module_path": t.module_path,
                "fqn": format!("{}::{}", t.module_path, t.short_name),
                "docs": t.docs,
                "shape": shape_json,
            })
        })
        .collect();

    let mut sorted_tools: Vec<&ToolEntry> = tools().collect();
    sorted_tools.sort_by_key(|t| (t.module_path, t.name));
    let tools_json: Vec<serde_json::Value> = sorted_tools
        .into_iter()
        .map(|t| {
            let params: Vec<serde_json::Value> = t
                .params
                .iter()
                .map(|p| {
                    serde_json::json!({
                        "name": p.name,
                        "type": p.type_str,
                        "type_short_name": p.type_short_name,
                    })
                })
                .collect();
            serde_json::json!({
                "name": t.name,
                "module_path": t.module_path,
                "fqn": format!("{}::{}", t.module_path, t.name),
                "file": t.file,
                "line": t.line,
                "docs": t.docs,
                "params": params,
                "return_type": t.return_type,
            })
        })
        .collect();

    serde_json::json!({
        "catalog_version": 2,
        "components": components,
        "primitives": primitives_json,
        "utilities": utilities_json,
        "states": states_json,
        "guides": guides_json,
        "methods": methods_json,
        "animations": animations_json,
        "types": types_json,
        "tools": tools_json,
    })
}

/// Print the catalog as pretty-formatted JSON on stdout. The shape
/// `cargo idealyst mcp --json-catalog` will eventually call.
pub fn dump_catalog_json() {
    let json = catalog_json();
    println!("{}", serde_json::to_string_pretty(&json).unwrap());
}
