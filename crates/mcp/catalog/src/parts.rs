//! [`CatalogParts`] — a catalog as plain lists of entries, from any
//! source, written out by the one serializer.
//!
//! [`crate::catalog_json`] used to read this process's inventory slice by
//! slice and write each one. That stays the common case, but it is no
//! longer the only one: the CLI's catalog scanner assembles a catalog from
//! TWO sources — a compiled dependency extractor (its JSON) and entries it
//! read from the workspace's source — and its output has to be
//! indistinguishable from what one compiled extractor linking everything
//! would have printed. Same slices, same order, same props-schema join.
//!
//! So the document is written from a `CatalogParts`, and
//! `catalog_json()` is just [`CatalogParts::registered`] written out.
//! Reading JSON back ([`CatalogParts::from_json`]) goes through the same
//! per-slice readers [`crate::ResolvedCatalog::build_from_json`] uses,
//! and [`CatalogParts::extend`] merges two sources before the write.

use serde_json::Value;

use crate::resolve::{leak_entry_from_json, slice_vec as read, BuildFromJsonError};
use crate::slice::CatalogSlice;
use crate::{
    AnimationEntry, ComponentEntry, GuideEntry, IconSetEntry, MacroEntry, MethodEntry,
    PrimitiveEntry, PropFieldSpec, PropsSchemaEntry, RecipeEntry, ScopeEntry, SdkEntry, StateEntry,
    StyleTokenEntry, ToolEntry, TypeEntry, TypeShape, UtilityEntry, ValueEntry,
};

/// Every catalog slice as a list of `&'static` entries. See the module
/// docs.
#[derive(Debug, Default)]
pub struct CatalogParts {
    pub components: Vec<&'static ComponentEntry>,
    /// The props schemas component params join against when written
    /// (each param whose type names one gets that schema's fields
    /// inlined). Not a slice of the document itself.
    pub props_schemas: Vec<&'static PropsSchemaEntry>,
    pub primitives: Vec<&'static PrimitiveEntry>,
    pub utilities: Vec<&'static UtilityEntry>,
    pub macros: Vec<&'static MacroEntry>,
    pub states: Vec<&'static StateEntry>,
    pub style_tokens: Vec<&'static StyleTokenEntry>,
    pub guides: Vec<&'static GuideEntry>,
    pub methods: Vec<&'static MethodEntry>,
    pub animations: Vec<&'static AnimationEntry>,
    pub types: Vec<&'static TypeEntry>,
    pub values: Vec<&'static ValueEntry>,
    pub tools: Vec<&'static ToolEntry>,
    pub recipes: Vec<&'static RecipeEntry>,
    pub scopes: Vec<&'static ScopeEntry>,
    pub sdks: Vec<&'static SdkEntry>,
    pub icon_sets: Vec<&'static IconSetEntry>,
}

impl CatalogParts {
    /// Everything registered in this process's inventory — the catalog a
    /// compiled extractor prints.
    pub fn registered() -> Self {
        Self {
            components: ComponentEntry::registered(),
            props_schemas: crate::schemas().collect(),
            primitives: PrimitiveEntry::registered(),
            utilities: UtilityEntry::registered(),
            macros: MacroEntry::registered(),
            states: StateEntry::registered(),
            style_tokens: StyleTokenEntry::registered(),
            guides: GuideEntry::registered(),
            methods: MethodEntry::registered(),
            animations: AnimationEntry::registered(),
            types: TypeEntry::registered(),
            values: ValueEntry::registered(),
            tools: ToolEntry::registered(),
            recipes: RecipeEntry::registered(),
            scopes: ScopeEntry::registered(),
            sdks: SdkEntry::registered(),
            icon_sets: IconSetEntry::registered(),
        }
    }

    /// Read a catalog document (the shape [`Self::to_json`] writes) back
    /// into entries, leaking its strings to `&'static` exactly as
    /// [`crate::ResolvedCatalog::build_from_json`] does. A malformed
    /// component fails the read; a malformed entry of any other slice is
    /// dropped (the same leniency that reader has).
    ///
    /// The document carries no props-schema slice — a schema only
    /// appears inlined into the component params that name it — so the
    /// schemas are rebuilt from two places:
    ///
    /// 1. the `types` slice: `#[derive(IdealystSchema)]` on a named struct
    ///    registers a `TypeEntry` with the very same field list beside its
    ///    `PropsSchemaEntry`, module path included — which the join
    ///    ([`crate::nearest_by_module`]) needs to tell same-named structs
    ///    apart. A struct type with NO fields is skipped, because a unit
    ///    or tuple struct registers that same shape without any schema;
    /// 2. for a name (1) does not have, a param's inlined `"schema"` —
    ///    which is how an empty named props struct (`struct Foo {}`,
    ///    joined as `"schema": []`) comes back.
    pub fn from_json(value: &Value) -> Result<Self, BuildFromJsonError> {
        let components = value["components"]
            .as_array()
            .ok_or(BuildFromJsonError::MissingComponents)?
            .iter()
            .map(leak_entry_from_json)
            .collect::<Result<Vec<_>, _>>()?;
        let types: Vec<&'static TypeEntry> = read::<TypeEntry>(value);
        let mut props_schemas: Vec<&'static PropsSchemaEntry> = Vec::new();
        for t in &types {
            if let TypeShape::Struct { fields } = &t.shape {
                if !fields.is_empty() {
                    props_schemas.push(Box::leak(Box::new(PropsSchemaEntry {
                        short_name: t.short_name,
                        module_path: t.module_path,
                        fields,
                    })));
                }
            }
        }
        for (entry, json) in components.iter().zip(value["components"].as_array().into_iter().flatten()) {
            for (param, param_json) in entry.params.iter().zip(json["params"].as_array().into_iter().flatten()) {
                let Some(fields) = param_json["schema"].as_array() else { continue };
                if props_schemas.iter().any(|s| s.short_name == param.type_short_name) {
                    continue;
                }
                // Only an empty named struct gets here; its module path is
                // not in the document, so it joins wherever its name is
                // the only one.
                props_schemas.push(Box::leak(Box::new(PropsSchemaEntry {
                    short_name: param.type_short_name,
                    module_path: "",
                    fields: leak_fields(fields),
                })));
            }
        }
        Ok(Self {
            components,
            props_schemas,
            primitives: read(value),
            utilities: read(value),
            macros: read(value),
            states: read(value),
            style_tokens: read(value),
            guides: read(value),
            methods: read(value),
            animations: read(value),
            types,
            values: read(value),
            tools: read(value),
            recipes: read(value),
            scopes: read(value),
            sdks: read(value),
            icon_sets: read(value),
        })
    }

    /// Append every entry of `other`. No de-duplication: the two sources
    /// are expected to be disjoint (the scanner's are — the dependency
    /// extractor links no workspace member, the scan reads only them).
    pub fn extend(&mut self, other: CatalogParts) {
        let CatalogParts {
            components,
            props_schemas,
            primitives,
            utilities,
            macros,
            states,
            style_tokens,
            guides,
            methods,
            animations,
            types,
            values,
            tools,
            recipes,
            scopes,
            sdks,
            icon_sets,
        } = other;
        self.components.extend(components);
        self.props_schemas.extend(props_schemas);
        self.primitives.extend(primitives);
        self.utilities.extend(utilities);
        self.macros.extend(macros);
        self.states.extend(states);
        self.style_tokens.extend(style_tokens);
        self.guides.extend(guides);
        self.methods.extend(methods);
        self.animations.extend(animations);
        self.types.extend(types);
        self.values.extend(values);
        self.tools.extend(tools);
        self.recipes.extend(recipes);
        self.scopes.extend(scopes);
        self.sdks.extend(sdks);
        self.icon_sets.extend(icon_sets);
    }

    /// [`extend`](Self::extend), but `other` wins: an entry of `self`
    /// with the same identity as one of `other`'s — a component's
    /// `(module_path, name)`, a scope's slug, … — is dropped first.
    ///
    /// The scanner's merge: a member read from source can also be
    /// compiled into the dependency extractor (as a dependency of a member
    /// the scan had to refuse, which the extractor then links), and its
    /// registrations must not appear twice. Both copies come from the same
    /// source at the same moment, so either would do; the scan's is
    /// complete, where the linker may have dropped some of the compiled
    /// ones.
    pub fn extend_replacing(&mut self, other: CatalogParts) {
        fn drop_same<T: ?Sized, K: PartialEq>(mine: &mut Vec<&'static T>, theirs: &[&'static T], key: impl Fn(&T) -> K) {
            let keys: Vec<K> = theirs.iter().map(|e| key(e)).collect();
            mine.retain(|e| !keys.contains(&key(e)));
        }
        drop_same(&mut self.components, &other.components, |e| (e.module_path, e.name));
        drop_same(&mut self.props_schemas, &other.props_schemas, |e| (e.module_path, e.short_name));
        drop_same(&mut self.methods, &other.methods, |e| (e.parent_module_path, e.parent_name, e.name));
        drop_same(&mut self.animations, &other.animations, |e| (e.parent_module_path, e.parent_name, e.binding, e.line));
        drop_same(&mut self.types, &other.types, |e| (e.module_path, e.short_name));
        drop_same(&mut self.values, &other.values, |e| (e.module_path, e.short_name, e.value_of));
        drop_same(&mut self.tools, &other.tools, |e| (e.module_path, e.name));
        drop_same(&mut self.recipes, &other.recipes, |e| (e.module_path, e.name));
        drop_same(&mut self.scopes, &other.scopes, |e| e.slug);
        self.extend(other);
    }

    /// The catalog document (schema version 2): every slice, each in its
    /// stable order, component params joined against
    /// [`props_schemas`](Self::props_schemas) — of the schemas with the
    /// param type's short name, the one nearest the component
    /// ([`crate::nearest_by_module`]), as `catalog_json()` picks.
    pub fn to_json(&self) -> Value {
        let mut components = self.components.clone();
        ComponentEntry::sort(&mut components);
        let components: Vec<Value> = components
            .iter()
            .map(|c| {
                c.to_json_with(&|short_name| {
                    crate::nearest_by_module(
                        self.props_schemas.iter().copied().filter(|s| s.short_name == short_name),
                        |s| s.module_path,
                        c.module_path,
                    )
                })
            })
            .collect();
        serde_json::json!({
            "catalog_version": 2,
            "components": components,
            "primitives": write(&self.primitives),
            "utilities": write(&self.utilities),
            "macros": write(&self.macros),
            "states": write(&self.states),
            "style_tokens": write(&self.style_tokens),
            "guides": write(&self.guides),
            "methods": write(&self.methods),
            "animations": write(&self.animations),
            "types": write(&self.types),
            "values": write(&self.values),
            "tools": write(&self.tools),
            "recipes": write(&self.recipes),
            "scopes": write(&self.scopes),
            "sdks": write(&self.sdks),
            "icon_sets": write(&self.icon_sets),
        })
    }
}

/// A param's inlined `"schema"` array, leaked — the inverse of what
/// [`ComponentEntry::to_json_with`] writes for it.
fn leak_fields(fields: &[Value]) -> &'static [PropFieldSpec] {
    let leak = |v: &Value| -> &'static str { Box::leak(v.as_str().unwrap_or_default().to_owned().into_boxed_str()) };
    let fields: Vec<PropFieldSpec> = fields
        .iter()
        .map(|f| PropFieldSpec {
            name: leak(&f["name"]),
            type_str: leak(&f["type"]),
            doc: leak(&f["doc"]),
            constraint: leak(&f["constraint"]),
        })
        .collect();
    Box::leak(fields.into_boxed_slice())
}

/// One slice of the document, sorted and serialized.
fn write<S: CatalogSlice>(entries: &[&'static S]) -> Value {
    let mut sorted = entries.to_vec();
    S::sort(&mut sorted);
    Value::Array(sorted.iter().map(|e| e.to_json()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EdgeRef, ParamSpec};

    static FIELDS: [PropFieldSpec; 1] = [PropFieldSpec { name: "label", type_str: "String", doc: "The label.", constraint: "" }];

    static EMPTY_PARAMS: [ParamSpec; 1] = [ParamSpec { name: "_props", type_str: "SeparatorProps", type_short_name: "SeparatorProps" }];
    static SEPARATOR: ComponentEntry = ComponentEntry {
        name: "Separator",
        module_path: "lib::menu",
        file: "src/menu.rs",
        line: 3,
        docs: "A divider.",
        composes: &[],
        params: &EMPTY_PARAMS,
    };
    static EMPTY_SCHEMA: PropsSchemaEntry = PropsSchemaEntry { short_name: "SeparatorProps", module_path: "lib::menu", fields: &[] };

    static BADGE_PARAMS: [ParamSpec; 1] = [ParamSpec { name: "props", type_str: "& BadgeProps", type_short_name: "BadgeProps" }];
    static BADGE_EDGES: [EdgeRef; 1] = [EdgeRef { name: "text", line: 9 }];
    static BADGE: ComponentEntry = ComponentEntry {
        name: "Badge",
        module_path: "app::badge",
        file: "/w/app/src/badge.rs",
        line: 7,
        docs: "",
        composes: &BADGE_EDGES,
        params: &BADGE_PARAMS,
    };
    static BADGE_TYPE: TypeEntry = TypeEntry {
        short_name: "BadgeProps",
        module_path: "ui::badge",
        docs: "",
        shape: TypeShape::Struct { fields: &FIELDS },
    };

    /// Regression: a named props struct with NO fields
    /// (`struct SeparatorProps {}`, idea-ui's `MenuSeparatorProps`) joins
    /// as `"schema": []`, but its `TypeEntry` is indistinguishable from a
    /// unit struct's, so rebuilding schemas from `types` alone dropped the
    /// `[]` and the re-written document differed from the original.
    #[test]
    fn regression_empty_named_props_schema_survives_a_json_round_trip() {
        let parts = CatalogParts { components: vec![&SEPARATOR], props_schemas: vec![&EMPTY_SCHEMA], ..Default::default() };
        let original = parts.to_json();
        assert_eq!(original["components"][0]["params"][0]["schema"], serde_json::json!([]));
        let back = CatalogParts::from_json(&original).unwrap().to_json();
        assert_eq!(back, original);
    }

    /// The scanner's merge: a component from one source naming a props
    /// struct whose schema only the OTHER source's document carries (as a
    /// `types` entry, since documents have no schema slice) still gets the
    /// fields inlined — the join a single compiled extractor would make.
    #[test]
    fn a_merged_component_joins_a_schema_from_the_other_source() {
        let deps = CatalogParts { types: vec![&BADGE_TYPE], ..Default::default() }.to_json();
        let mut merged = CatalogParts::from_json(&deps).unwrap();
        merged.extend(CatalogParts { components: vec![&BADGE], ..Default::default() });
        let doc = merged.to_json();
        assert_eq!(doc["components"][0]["params"][0]["schema"][0]["name"], "label");
        assert_eq!(doc["types"][0]["short_name"], "BadgeProps");
    }

    /// Entries from two sources come out in the document's stable order,
    /// not in source order.
    #[test]
    fn a_merge_writes_every_slice_in_its_stable_order() {
        let mut parts = CatalogParts { components: vec![&BADGE], ..Default::default() };
        parts.extend(CatalogParts { components: vec![&SEPARATOR], props_schemas: vec![&EMPTY_SCHEMA], ..Default::default() });
        let doc = parts.to_json();
        let names: Vec<&str> = doc["components"].as_array().unwrap().iter().map(|c| c["name"].as_str().unwrap()).collect();
        // (module_path, name): `app::badge` < `lib::menu`.
        assert_eq!(names, ["Badge", "Separator"]);
    }

    /// The scan's copy of an entry replaces the extractor's; everything
    /// else of both is kept — including an extractor entry whose
    /// `module_path` merely names a scanned crate (the framework's "Core"
    /// scope is registered by the catalog crate under `runtime_core`).
    #[test]
    fn extend_replacing_drops_only_true_duplicates() {
        static CORE: ScopeEntry = ScopeEntry { slug: "core", title: "Core", docs: "", module_path: "app", order: 0 };
        static BADGE_AGAIN: ComponentEntry = ComponentEntry {
            name: "Badge",
            module_path: "app::badge",
            file: "/w/app/src/badge.rs",
            line: 8,
            docs: "fresh",
            composes: &[],
            params: &[],
        };
        let mut compiled = CatalogParts { components: vec![&BADGE, &SEPARATOR], scopes: vec![&CORE], ..Default::default() };
        compiled.extend_replacing(CatalogParts { components: vec![&BADGE_AGAIN], ..Default::default() });
        let docs: Vec<(&str, &str)> = compiled.components.iter().map(|c| (c.name, c.docs)).collect();
        assert_eq!(docs, [("Separator", "A divider."), ("Badge", "fresh")]);
        assert_eq!(compiled.scopes.len(), 1);
    }

    /// Regression: two props structs sharing a short name (CrewForge's
    /// `BlockerRowProps`, in `ui-shared` and in `screens-accounting`)
    /// joined each component to whichever registered first — link order —
    /// so one of them was documented with the other's fields. The join
    /// takes the one nearest the component, whatever the order.
    #[test]
    fn regression_a_param_joins_the_props_struct_nearest_its_component() {
        static SHARED_FIELDS: [PropFieldSpec; 1] = [PropFieldSpec { name: "blocker", type_str: "Blocker", doc: "", constraint: "" }];
        static LOCAL_FIELDS: [PropFieldSpec; 1] = [PropFieldSpec { name: "chip", type_str: "String", doc: "", constraint: "" }];
        static SHARED: PropsSchemaEntry = PropsSchemaEntry { short_name: "RowProps", module_path: "ui_shared::wizard", fields: &SHARED_FIELDS };
        static LOCAL: PropsSchemaEntry = PropsSchemaEntry { short_name: "RowProps", module_path: "accounting::lifecycle", fields: &LOCAL_FIELDS };
        static PARAMS: [crate::ParamSpec; 1] = [crate::ParamSpec { name: "props", type_str: "& RowProps", type_short_name: "RowProps" }];
        static ROW: ComponentEntry = ComponentEntry {
            name: "Row",
            module_path: "accounting::lifecycle",
            file: "",
            line: 1,
            docs: "",
            composes: &[],
            params: &PARAMS,
        };
        for schemas in [vec![&SHARED, &LOCAL], vec![&LOCAL, &SHARED]] {
            let doc = CatalogParts { components: vec![&ROW], props_schemas: schemas, ..Default::default() }.to_json();
            assert_eq!(doc["components"][0]["params"][0]["schema"][0]["name"], "chip");
        }
    }

    /// Nearest means most shared leading segments; a tie goes to the
    /// first path in order, so the pick never depends on link order.
    #[test]
    fn nearest_by_module_prefers_shared_segments_then_the_first_path() {
        let near = |cands: &[&'static str], at: &str| crate::nearest_by_module(cands.iter().copied(), |m| m, at);
        assert_eq!(near(&["a::x", "b::y", "a::y::z"], "a::y::k"), Some("a::y::z"));
        assert_eq!(near(&["c::q", "c::p"], "a::b"), Some("c::p"));
        assert_eq!(near(&["c::p", "c::q"], "a::b"), Some("c::p"));
        assert_eq!(near(&[], "a"), None);
    }
}
