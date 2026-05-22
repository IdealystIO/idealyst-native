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
pub use resolve::{EdgeStatus, EntryRef, ResolvedCatalog, ResolvedEdge};

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
}

inventory::collect!(ComponentEntry);

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
            serde_json::json!({
                "name": e.name,
                "module_path": e.module_path,
                "file": e.file,
                "line": e.line,
                "docs": e.docs,
                "composes": composes,
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
