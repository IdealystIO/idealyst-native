//! `CatalogSlice` — the per-slice (de)serialization seam.
//!
//! Each catalog entry type implements [`CatalogSlice`] (sort key + JSON
//! key + `to_json`) so [`crate::catalog_json`] assembles the document by
//! iterating slices instead of open-coding each one. The lenient v2
//! slices additionally implement [`LeakFromJson`] so
//! [`crate::ResolvedCatalog::build_from_json`] rebuilds them the same way.
//!
//! Why this exists: previously every slice was written out by hand in
//! three places — the struct, the `catalog_json()` writer, and the
//! `leak_*_from_json()` reader — which silently drift. Funnelling the
//! writer and reader through one trait per type collapses that to a
//! single site each.
//!
//! `ComponentEntry` deliberately does **not** implement [`LeakFromJson`]:
//! components carry *required-field* error semantics (a malformed
//! component must fail the rebuild, not be silently dropped), so their
//! reader stays the `Result`-returning path in [`crate::resolve`]. They
//! still implement [`CatalogSlice`] for the writer side.

use serde_json::{json, Value};

use crate::{
    AnimationEntry, ComponentEntry, GuideEntry, MacroEntry, MethodEntry, PrimitiveEntry,
    RecipeEntry, ScopeEntry, SdkEntry, StateEntry, ToolEntry, TypeEntry, TypeShape, UtilityEntry,
};

/// Writer side: a catalog entry type that knows its JSON array key, how
/// to enumerate itself in stable order, and how to serialize one entry.
pub trait CatalogSlice: Sized + 'static {
    /// The key this slice occupies in the catalog JSON object
    /// (`"components"`, `"primitives"`, …).
    const KEY: &'static str;

    /// Every entry of this slice, in the stable order `catalog_json`
    /// emits (so JSON diffs stay minimal).
    fn collect_sorted() -> Vec<&'static Self>;

    /// Serialize one entry to its JSON object.
    fn to_json(&self) -> Value;
}

/// Reader side (lenient): rebuild one entry from the JSON
/// [`CatalogSlice::to_json`] produced, leaking owned strings into
/// `&'static`. Returns `None` to skip a malformed entry — used for the
/// optional v2 slices where a missing/garbled entry should be dropped,
/// not fatal. Implemented in [`crate::resolve`] (next to the leak
/// helpers); `ComponentEntry` intentionally opts out (see module docs).
pub trait LeakFromJson: CatalogSlice {
    fn from_json(v: &Value) -> Option<&'static Self>;
}

/// `{ S::KEY: [ ...entries.to_json() ] }` — the array half of the
/// catalog document for one slice.
pub fn slice_array<S: CatalogSlice>() -> Value {
    Value::Array(S::collect_sorted().iter().map(|e| e.to_json()).collect())
}

// ---------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------

impl CatalogSlice for ComponentEntry {
    const KEY: &'static str = "components";

    fn collect_sorted() -> Vec<&'static Self> {
        let mut v: Vec<&'static ComponentEntry> = crate::entries().collect();
        v.sort_by_key(|e| (e.module_path, e.name));
        v
    }

    fn to_json(&self) -> Value {
        let composes: Vec<Value> = self
            .composes
            .iter()
            .map(|edge| json!({ "name": edge.name, "line": edge.line }))
            .collect();
        let params: Vec<Value> = self
            .params
            .iter()
            .map(|p| {
                // If the param's type resolves to a known props schema,
                // inline its fields. Otherwise the field is just absent —
                // consumers fall back to `type_str` alone.
                let schema = if p.type_short_name.is_empty() {
                    None
                } else {
                    crate::lookup_schema(p.type_short_name)
                };
                let mut obj = serde_json::Map::new();
                obj.insert("name".into(), p.name.into());
                obj.insert("type".into(), p.type_str.into());
                obj.insert("type_short_name".into(), p.type_short_name.into());
                if let Some(s) = schema {
                    let fields: Vec<Value> = s
                        .fields
                        .iter()
                        .map(|f| {
                            json!({
                                "name": f.name,
                                "type": f.type_str,
                                "doc": f.doc,
                                "constraint": f.constraint,
                            })
                        })
                        .collect();
                    obj.insert("schema".into(), json!(fields));
                }
                Value::Object(obj)
            })
            .collect();
        json!({
            "name": self.name,
            "module_path": self.module_path,
            "file": self.file,
            "line": self.line,
            "docs": self.docs,
            "composes": composes,
            "params": params,
        })
    }
}

// ---------------------------------------------------------------------
// Primitive
// ---------------------------------------------------------------------

impl CatalogSlice for PrimitiveEntry {
    const KEY: &'static str = "primitives";

    fn collect_sorted() -> Vec<&'static Self> {
        let mut v: Vec<&'static PrimitiveEntry> = crate::primitives().collect();
        v.sort_by_key(|p| p.name);
        v
    }

    fn to_json(&self) -> Value {
        let props: Vec<Value> = self
            .props
            .iter()
            .map(|f| {
                json!({
                    "name": f.name,
                    "type": f.type_str,
                    "doc": f.doc,
                    "constraint": f.constraint,
                })
            })
            .collect();
        json!({
            "name": self.name,
            "pascal_name": self.pascal_name,
            "docs": self.docs,
            "category": self.category.as_str(),
            "backends": self.backends,
            "props": props,
        })
    }
}

// ---------------------------------------------------------------------
// Utility
// ---------------------------------------------------------------------

impl CatalogSlice for UtilityEntry {
    const KEY: &'static str = "utilities";

    fn collect_sorted() -> Vec<&'static Self> {
        let mut v: Vec<&'static UtilityEntry> = crate::utilities().collect();
        v.sort_by_key(|u| u.name);
        v
    }

    fn to_json(&self) -> Value {
        let params: Vec<Value> = self
            .params
            .iter()
            .map(|p| {
                json!({
                    "name": p.name,
                    "type": p.type_str,
                    "type_short_name": p.type_short_name,
                })
            })
            .collect();
        json!({
            "name": self.name,
            "module_path": self.module_path,
            "fqn": format!("{}::{}", self.module_path, self.name),
            "docs": self.docs,
            "params": params,
            "return_type": self.return_type,
            "return_type_short": self.return_type_short,
            "category": self.category.as_str(),
        })
    }
}

// ---------------------------------------------------------------------
// Macro
// ---------------------------------------------------------------------

impl CatalogSlice for MacroEntry {
    const KEY: &'static str = "macros";

    fn collect_sorted() -> Vec<&'static Self> {
        let mut v: Vec<&'static MacroEntry> = crate::macros().collect();
        v.sort_by_key(|m| m.name);
        v
    }

    fn to_json(&self) -> Value {
        json!({
            "name": self.name,
            "invocation": self.invocation,
            "kind": self.kind.as_str(),
            "module_path": self.module_path,
            "fqn": format!("{}::{}", self.module_path, self.name),
            "docs": self.docs,
            "expansion": self.expansion,
        })
    }
}

// ---------------------------------------------------------------------
// State
// ---------------------------------------------------------------------

impl CatalogSlice for StateEntry {
    const KEY: &'static str = "states";

    fn collect_sorted() -> Vec<&'static Self> {
        let mut v: Vec<&'static StateEntry> = crate::states().collect();
        v.sort_by_key(|s| s.name);
        v
    }

    fn to_json(&self) -> Value {
        json!({
            "name": self.name,
            "docs": self.docs,
            "backends": self.backends,
        })
    }
}

// ---------------------------------------------------------------------
// Guide
// ---------------------------------------------------------------------

impl CatalogSlice for GuideEntry {
    const KEY: &'static str = "guides";

    fn collect_sorted() -> Vec<&'static Self> {
        let mut v: Vec<&'static GuideEntry> = crate::guides().collect();
        v.sort_by_key(|g| (g.order, g.slug));
        v
    }

    fn to_json(&self) -> Value {
        json!({
            "slug": self.slug,
            "title": self.title,
            "order": self.order,
            "tags": self.tags,
            "body": self.body,
        })
    }
}

// ---------------------------------------------------------------------
// Method
// ---------------------------------------------------------------------

impl CatalogSlice for MethodEntry {
    const KEY: &'static str = "methods";

    fn collect_sorted() -> Vec<&'static Self> {
        let mut v: Vec<&'static MethodEntry> = crate::methods().collect();
        v.sort_by_key(|m| (m.parent_module_path, m.parent_name, m.name));
        v
    }

    fn to_json(&self) -> Value {
        let params: Vec<Value> = self
            .params
            .iter()
            .map(|p| {
                json!({
                    "name": p.name,
                    "type": p.type_str,
                    "type_short_name": p.type_short_name,
                })
            })
            .collect();
        json!({
            "parent_module_path": self.parent_module_path,
            "parent_name": self.parent_name,
            "parent_fqn": format!("{}::{}", self.parent_module_path, self.parent_name),
            "name": self.name,
            "docs": self.docs,
            "params": params,
            "return_type": self.return_type,
        })
    }
}

// ---------------------------------------------------------------------
// Animation
// ---------------------------------------------------------------------

impl CatalogSlice for AnimationEntry {
    const KEY: &'static str = "animations";

    fn collect_sorted() -> Vec<&'static Self> {
        let mut v: Vec<&'static AnimationEntry> = crate::animations().collect();
        v.sort_by_key(|a| (a.parent_module_path, a.parent_name, a.binding, a.line));
        v
    }

    fn to_json(&self) -> Value {
        json!({
            "parent_module_path": self.parent_module_path,
            "parent_name": self.parent_name,
            "parent_fqn": format!("{}::{}", self.parent_module_path, self.parent_name),
            "binding": self.binding,
            "initial": self.initial,
            "line": self.line,
        })
    }
}

// ---------------------------------------------------------------------
// Type
// ---------------------------------------------------------------------

impl CatalogSlice for TypeEntry {
    const KEY: &'static str = "types";

    fn collect_sorted() -> Vec<&'static Self> {
        let mut v: Vec<&'static TypeEntry> = crate::types().collect();
        v.sort_by_key(|t| (t.module_path, t.short_name));
        v
    }

    fn to_json(&self) -> Value {
        let shape_json = match &self.shape {
            TypeShape::Struct { fields } => {
                let fs: Vec<Value> = fields
                    .iter()
                    .map(|f| {
                        json!({
                            "name": f.name,
                            "type": f.type_str,
                            "doc": f.doc,
                            "constraint": f.constraint,
                        })
                    })
                    .collect();
                json!({ "kind": "struct", "fields": fs })
            }
            TypeShape::Enum { variants } => {
                let vs: Vec<Value> = variants
                    .iter()
                    .map(|v| {
                        let payload: Vec<Value> = v
                            .payload
                            .iter()
                            .map(|f| {
                                json!({
                                    "name": f.name,
                                    "type": f.type_str,
                                    "doc": f.doc,
                                    "constraint": f.constraint,
                                })
                            })
                            .collect();
                        json!({ "name": v.name, "docs": v.docs, "payload": payload })
                    })
                    .collect();
                json!({ "kind": "enum", "variants": vs })
            }
        };
        json!({
            "short_name": self.short_name,
            "module_path": self.module_path,
            "fqn": format!("{}::{}", self.module_path, self.short_name),
            "docs": self.docs,
            "shape": shape_json,
        })
    }
}

// ---------------------------------------------------------------------
// Tool
// ---------------------------------------------------------------------

impl CatalogSlice for ToolEntry {
    const KEY: &'static str = "tools";

    fn collect_sorted() -> Vec<&'static Self> {
        let mut v: Vec<&'static ToolEntry> = crate::tools().collect();
        v.sort_by_key(|t| (t.module_path, t.name));
        v
    }

    fn to_json(&self) -> Value {
        let params: Vec<Value> = self
            .params
            .iter()
            .map(|p| {
                json!({
                    "name": p.name,
                    "type": p.type_str,
                    "type_short_name": p.type_short_name,
                })
            })
            .collect();
        json!({
            "name": self.name,
            "module_path": self.module_path,
            "fqn": format!("{}::{}", self.module_path, self.name),
            "file": self.file,
            "line": self.line,
            "docs": self.docs,
            "params": params,
            "return_type": self.return_type,
        })
    }
}

// ---------------------------------------------------------------------
// Recipe
// ---------------------------------------------------------------------

impl CatalogSlice for RecipeEntry {
    const KEY: &'static str = "recipes";

    fn collect_sorted() -> Vec<&'static Self> {
        let mut v: Vec<&'static RecipeEntry> = crate::recipes().collect();
        v.sort_by_key(|r| (r.target, r.module_path, r.name));
        v
    }

    fn to_json(&self) -> Value {
        json!({
            "name": self.name,
            "target": self.target,
            "module_path": self.module_path,
            "fqn": format!("{}::{}", self.module_path, self.name),
            "file": self.file,
            "line": self.line,
            "docs": self.docs,
            "source": self.source,
            "uses": self.uses,
        })
    }
}

// ---------------------------------------------------------------------
// Scope
// ---------------------------------------------------------------------

impl CatalogSlice for ScopeEntry {
    const KEY: &'static str = "scopes";

    fn collect_sorted() -> Vec<&'static Self> {
        let mut v: Vec<&'static ScopeEntry> = crate::scopes().collect();
        v.sort_by_key(|s| (s.order, s.slug));
        v
    }

    fn to_json(&self) -> Value {
        json!({
            "slug": self.slug,
            "title": self.title,
            "docs": self.docs,
            "module_path": self.module_path,
            "order": self.order,
        })
    }
}

// ---------------------------------------------------------------------
// SDK
// ---------------------------------------------------------------------

impl CatalogSlice for SdkEntry {
    const KEY: &'static str = "sdks";

    fn collect_sorted() -> Vec<&'static Self> {
        let mut v: Vec<&'static SdkEntry> = crate::sdks().collect();
        v.sort_by_key(|s| s.name);
        v
    }

    fn to_json(&self) -> Value {
        json!({
            "name": self.name,
            "summary": self.summary,
            "dep_line": self.dep_line,
            "category": self.category.as_str(),
            "kind": self.kind.as_str(),
            "guide": self.guide,
        })
    }
}
