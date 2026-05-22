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

/// Build the catalog as a JSON value. Phase 1 schema is intentionally
/// flat — entries are an array sorted by `module_path::name`. A
/// `catalog_version` field is present from day 1 so consumers can pin
/// (see spec §9.4).
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
    serde_json::json!({
        "catalog_version": 1,
        "components": components,
    })
}

/// Print the catalog as pretty-formatted JSON on stdout. The shape
/// `cargo idealyst mcp --json-catalog` will eventually call.
pub fn dump_catalog_json() {
    let json = catalog_json();
    println!("{}", serde_json::to_string_pretty(&json).unwrap());
}
