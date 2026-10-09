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
    AnimationEntry, ComponentEntry, PropsSchemaEntry, GuideEntry, IconSetEntry, MacroEntry, MethodEntry,
    PrimitiveEntry, RecipeEntry, ScopeEntry, SdkEntry, StateEntry, StyleTokenEntry, ToolEntry,
    TypeEntry, TypeShape,
    UtilityEntry, ValueEntry,
};

/// Writer side: a catalog entry type that knows its JSON array key, how
/// to enumerate itself in stable order, and how to serialize one entry.
pub trait CatalogSlice: Sized + 'static {
    /// The key this slice occupies in the catalog JSON object
    /// (`"components"`, `"primitives"`, …).
    const KEY: &'static str;

    /// Every entry of this slice registered in this process's inventory,
    /// in no particular order.
    fn registered() -> Vec<&'static Self>;

    /// Put entries in the stable order the catalog document lists them in
    /// (so JSON diffs stay minimal). Separate from [`registered`] because a
    /// catalog is not always this process's inventory: the scanner's
    /// catalog is a dependency extractor's entries plus entries read from
    /// source, and it has to come out in the same order.
    ///
    /// [`registered`]: CatalogSlice::registered
    fn sort(v: &mut [&'static Self]);

    /// [`registered`](CatalogSlice::registered), [`sort`](CatalogSlice::sort)ed.
    fn collect_sorted() -> Vec<&'static Self> {
        let mut v = Self::registered();
        Self::sort(&mut v);
        v
    }

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

    fn registered() -> Vec<&'static Self> {
        crate::entries().collect()
    }

    fn sort(v: &mut [&'static Self]) {
        v.sort_by_key(|e| (e.module_path, e.name));
    }

    fn to_json(&self) -> Value {
        self.to_json_with(&|short_name| crate::nearest_schema(short_name, self.module_path))
    }
}

impl ComponentEntry {
    /// [`CatalogSlice::to_json`] with the props-schema join made through
    /// `schemas` (param type's short name → schema) instead of this
    /// process's inventory — for a catalog assembled from more than one
    /// source (see [`crate::CatalogParts`]), where the schema a param names
    /// may have come from either. Pick the schema nearest this component
    /// ([`crate::nearest_by_module`]), as `to_json` does.
    pub fn to_json_with(&self, schemas: &dyn Fn(&str) -> Option<&'static PropsSchemaEntry>) -> Value {
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
                    schemas(p.type_short_name)
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

    fn registered() -> Vec<&'static Self> {
        crate::primitives().collect()
    }

    fn sort(v: &mut [&'static Self]) {
        v.sort_by_key(|p| p.name);
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

    fn registered() -> Vec<&'static Self> {
        crate::utilities().collect()
    }

    fn sort(v: &mut [&'static Self]) {
        v.sort_by_key(|u| u.name);
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
            "snippet": self.snippet,
            "category": self.category.as_str(),
        })
    }
}

// ---------------------------------------------------------------------
// Macro
// ---------------------------------------------------------------------

impl CatalogSlice for MacroEntry {
    const KEY: &'static str = "macros";

    fn registered() -> Vec<&'static Self> {
        crate::macros().collect()
    }

    fn sort(v: &mut [&'static Self]) {
        v.sort_by_key(|m| m.name);
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
            "snippet": self.snippet,
        })
    }
}

// ---------------------------------------------------------------------
// State
// ---------------------------------------------------------------------

impl CatalogSlice for StateEntry {
    const KEY: &'static str = "states";

    fn registered() -> Vec<&'static Self> {
        crate::states().collect()
    }

    fn sort(v: &mut [&'static Self]) {
        v.sort_by_key(|s| s.name);
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
// Style token
// ---------------------------------------------------------------------

impl CatalogSlice for StyleTokenEntry {
    const KEY: &'static str = "style_tokens";

    fn registered() -> Vec<&'static Self> {
        crate::style_tokens().collect()
    }

    fn sort(v: &mut [&'static Self]) {
        // By path, not name: consumers (editor completion) walk these in
        // the order an author types them.
        v.sort_by_key(|t| (t.vocabulary, t.namespace, t.path));
    }

    fn to_json(&self) -> Value {
        json!({
            "name": self.name,
            "path": self.path,
            "namespace": self.namespace,
            "value_type": self.value_type,
            // Resolved, not copied — see `TokenDefault`.
            "default_value": self.default_value.get(),
            "vocabulary": self.vocabulary,
        })
    }
}

// ---------------------------------------------------------------------
// Guide
// ---------------------------------------------------------------------

impl CatalogSlice for GuideEntry {
    const KEY: &'static str = "guides";

    fn registered() -> Vec<&'static Self> {
        crate::guides().collect()
    }

    fn sort(v: &mut [&'static Self]) {
        v.sort_by_key(|g| (g.order, g.slug));
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

    fn registered() -> Vec<&'static Self> {
        crate::methods().collect()
    }

    fn sort(v: &mut [&'static Self]) {
        v.sort_by_key(|m| (m.parent_module_path, m.parent_name, m.name));
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

    fn registered() -> Vec<&'static Self> {
        crate::animations().collect()
    }

    fn sort(v: &mut [&'static Self]) {
        v.sort_by_key(|a| (a.parent_module_path, a.parent_name, a.binding, a.line));
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

    fn registered() -> Vec<&'static Self> {
        crate::types().collect()
    }

    fn sort(v: &mut [&'static Self]) {
        v.sort_by_key(|t| (t.module_path, t.short_name));
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

    fn registered() -> Vec<&'static Self> {
        crate::tools().collect()
    }

    fn sort(v: &mut [&'static Self]) {
        v.sort_by_key(|t| (t.module_path, t.name));
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

    fn registered() -> Vec<&'static Self> {
        crate::recipes().collect()
    }

    fn sort(v: &mut [&'static Self]) {
        v.sort_by_key(|r| (r.target, r.module_path, r.name));
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

    fn registered() -> Vec<&'static Self> {
        crate::scopes().collect()
    }

    fn sort(v: &mut [&'static Self]) {
        v.sort_by_key(|s| (s.order, s.slug));
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

    fn registered() -> Vec<&'static Self> {
        crate::sdks().collect()
    }

    fn sort(v: &mut [&'static Self]) {
        v.sort_by_key(|s| s.name);
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

// ---------------------------------------------------------------------
// Value
// ---------------------------------------------------------------------

impl CatalogSlice for ValueEntry {
    const KEY: &'static str = "values";

    fn registered() -> Vec<&'static Self> {
        crate::values().collect()
    }

    fn sort(v: &mut [&'static Self]) {
        v.sort_by_key(|e| (e.value_of, e.module_path, e.short_name));
    }

    fn to_json(&self) -> Value {
        json!({
            "short_name": self.short_name,
            "module_path": self.module_path,
            "docs": self.docs,
            "value_of": self.value_of,
            "via": self.via,
            // Precomputed so a consumer that only reads JSON (the editor
            // extension) never has to know the `via` fallback rule.
            "spelled": self.spelled(),
            "import": self.import(),
        })
    }
}

// ---------------------------------------------------------------------
// IconSet
// ---------------------------------------------------------------------

impl CatalogSlice for IconSetEntry {
    const KEY: &'static str = "icon_sets";

    fn registered() -> Vec<&'static Self> {
        crate::icon_sets().collect()
    }

    fn sort(v: &mut [&'static Self]) {
        v.sort_by_key(|s| s.name);
    }

    fn to_json(&self) -> Value {
        // The full (name, ident) list is carried so `search_icons` works
        // off the JSON-reload path and the docs site can show the import
        // for any icon. It's the bulk of the document for a large pack —
        // but it's names only (no geometry), and the MCP `list_*` tools
        // never echo it back; only `search_icons` / paginated
        // `describe_icon_set` surface individual icons.
        let icons: Vec<Value> = self
            .icons
            .iter()
            .map(|i| json!({ "name": i.name, "ident": i.ident }))
            .collect();
        json!({
            "name": self.name,
            "title": self.title,
            "docs": self.docs,
            "import_path": self.import_path,
            "license": self.license,
            "homepage": self.homepage,
            "icon_count": self.icons.len(),
            "icons": icons,
        })
    }
}
