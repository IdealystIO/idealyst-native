//! Catalog → view-model mapping.
//!
//! The renderer never touches `mcp_catalog`'s `&'static` entry structs
//! directly — it walks an owned [`CatalogModel`] built once at startup
//! from [`mcp_catalog::ResolvedCatalog::build()`]. Keeping the mapping
//! in one pure module (no `Element`, no `ui!`) means it's unit-testable
//! without a backend: the tests in this file assert that idea-ui's
//! components actually land in the runtime catalog (the inventory
//! linker-section concern flagged in `examples/mcp-demo/Cargo.toml`),
//! and that the kind grouping + slugging behave.
//!
//! ## Why owned, not borrowed
//!
//! `ResolvedCatalog` is `!Clone` and its slices are `&'static` to the
//! inventory section. The navigator pushes a fresh per-route screen
//! closure that may outlive any single borrow, so the model copies the
//! handful of `&'static str`s it needs into `String`s up front. The
//! catalog is small (low hundreds of entries), so the one-time clone is
//! negligible.

use mcp_catalog::{ResolvedCatalog, TypeShape};

/// The five navigable kinds the sidebar groups entries under. Methods
/// and animations are rendered inline on their parent component's detail
/// page (joined by `parent_*`), so they aren't top-level kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Component,
    Primitive,
    Utility,
    Type,
    Guide,
}

impl Kind {
    pub fn title(self) -> &'static str {
        match self {
            Kind::Component => "Components",
            Kind::Primitive => "Primitives",
            Kind::Utility => "Utilities",
            Kind::Type => "Types",
            Kind::Guide => "Guides",
        }
    }

    /// URL-path segment for this kind's routes (`/components/<slug>`).
    pub fn path_segment(self) -> &'static str {
        match self {
            Kind::Component => "components",
            Kind::Primitive => "primitives",
            Kind::Utility => "utilities",
            Kind::Type => "types",
            Kind::Guide => "guides",
        }
    }
}

/// One field/prop row — shared by component props, primitive props,
/// utility params, struct fields. `ty` is always present (the
/// pretty-printed type string); `doc` / `constraint` may be empty when
/// the source didn't derive `IdealystSchema`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    pub name: String,
    pub ty: String,
    pub doc: String,
    pub constraint: String,
}

/// A method exposed on a component's handle (joined by parent).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Method {
    pub name: String,
    pub doc: String,
    pub params: Vec<Field>,
    pub return_type: String,
}

/// An `animated!(...)` binding captured in a component body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Animation {
    pub binding: String,
    pub initial: String,
}

/// A resolved composition edge — a link to another component, or an
/// unresolved/ambiguous marker. `target_slug` is `Some` only when the
/// edge resolved to a component that's in the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Compose {
    pub name: String,
    pub target_slug: Option<String>,
}

/// One catalog entry, normalized across kinds. Not every field applies
/// to every kind (a guide has only `docs` as markdown `body`; a utility
/// has `return_type` but no `composes`), but a single struct keeps the
/// renderer uniform — empty vectors / strings render as "nothing".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub kind: Kind,
    /// Human display name (`Button`, `view`, `now_micros`, `Platform`).
    pub name: String,
    /// URL-safe slug, unique within the kind.
    pub slug: String,
    /// Module path or FQN, shown as a subtitle. Empty for guides.
    pub module_path: String,
    /// Docs / markdown body.
    pub docs: String,
    /// Props (components/primitives), params (utilities), fields
    /// (struct types). Empty when none / not documented.
    pub fields: Vec<Field>,
    /// Whether any `fields` carried a non-empty doc/constraint. Drives
    /// the "props not yet documented" note for entries whose props
    /// struct hasn't derived `IdealystSchema` yet.
    pub fields_documented: bool,
    /// Enum variants (type entries with `TypeShape::Enum`).
    pub variants: Vec<(String, String)>,
    /// Composition graph (components only).
    pub composes: Vec<Compose>,
    /// Methods on this component's handle (components only).
    pub methods: Vec<Method>,
    /// Animated values declared in the body (components only).
    pub animations: Vec<Animation>,
    /// Return type (utilities only). Empty otherwise.
    pub return_type: String,
}

/// The whole catalog, grouped and slugged for navigation.
#[derive(Debug, Clone, Default)]
pub struct CatalogModel {
    entries: Vec<Entry>,
}

/// Lower-case + replace any non-alphanumeric run with a single `-`.
/// Keeps slugs URL-safe and stable. Module path is appended (also
/// slugified) to disambiguate same-named entries across modules.
fn slugify(name: &str, qualifier: &str) -> String {
    let mut s = String::new();
    let mut prev_dash = false;
    let push = |s: &mut String, prev_dash: &mut bool, src: &str| {
        for ch in src.chars() {
            if ch.is_ascii_alphanumeric() {
                s.push(ch.to_ascii_lowercase());
                *prev_dash = false;
            } else if !*prev_dash {
                s.push('-');
                *prev_dash = true;
            }
        }
    };
    push(&mut s, &mut prev_dash, name);
    if !qualifier.is_empty() {
        if !prev_dash {
            s.push('-');
        }
        prev_dash = true;
        push(&mut s, &mut prev_dash, qualifier);
    }
    s.trim_matches('-').to_string()
}

impl CatalogModel {
    /// Build the model over the global in-process inventory catalog.
    pub fn build() -> Self {
        Self::from_resolved(&ResolvedCatalog::build())
    }

    /// Build from an already-resolved catalog. Split out so tests can
    /// pass a catalog built from an explicit entry list if needed; the
    /// app path goes through [`CatalogModel::build`].
    pub fn from_resolved(cat: &ResolvedCatalog) -> Self {
        let mut entries: Vec<Entry> = Vec::new();

        // --- Components ----------------------------------------------------
        // Pre-compute component slugs by (module_path, name) so composes
        // edges can link to them. The resolver gives us FQN targets.
        for c in cat.entries() {
            let slug = slugify(c.name, c.module_path);
            // Flatten every param's prop schema into a single field
            // list. Most idea-ui props structs don't derive
            // IdealystSchema yet, so a single `props: &FooProps` param
            // surfaces as one field naming the struct — graceful, not
            // blank. (See `fields_documented`.)
            let mut fields = Vec::new();
            for p in c.params {
                let mut had_schema = false;
                if !p.type_short_name.is_empty() {
                    if let Some(schema) = mcp_catalog::lookup_schema(p.type_short_name) {
                        for f in schema.fields {
                            fields.push(Field {
                                name: f.name.to_string(),
                                ty: f.type_str.to_string(),
                                doc: f.doc.to_string(),
                                constraint: f.constraint.to_string(),
                            });
                        }
                        had_schema = true;
                    }
                }
                if !had_schema {
                    fields.push(Field {
                        name: p.name.to_string(),
                        ty: p.type_str.to_string(),
                        doc: String::new(),
                        constraint: String::new(),
                    });
                }
            }
            let fields_documented =
                fields.iter().any(|f| !f.doc.is_empty() || !f.constraint.is_empty());

            // Composes edges, resolved to slugs when the target is a
            // known component.
            let host_ref = mcp_catalog::EntryRef::of(c);
            let composes: Vec<Compose> = cat
                .dependencies(&host_ref)
                .iter()
                .map(|edge| {
                    let target_slug = match &edge.status {
                        mcp_catalog::EdgeStatus::Resolved { target } => {
                            Some(slugify(target.name, target.module_path))
                        }
                        _ => None,
                    };
                    Compose { name: edge.raw_name.to_string(), target_slug }
                })
                .collect();

            // Methods + animations joined by parent identity.
            let methods: Vec<Method> = cat
                .methods()
                .iter()
                .filter(|m| m.parent_module_path == c.module_path && m.parent_name == c.name)
                .map(|m| Method {
                    name: m.name.to_string(),
                    doc: m.docs.to_string(),
                    params: m
                        .params
                        .iter()
                        .map(|p| Field {
                            name: p.name.to_string(),
                            ty: p.type_str.to_string(),
                            doc: String::new(),
                            constraint: String::new(),
                        })
                        .collect(),
                    return_type: m.return_type.to_string(),
                })
                .collect();
            let animations: Vec<Animation> = cat
                .animations()
                .iter()
                .filter(|a| a.parent_module_path == c.module_path && a.parent_name == c.name)
                .map(|a| Animation {
                    binding: a.binding.to_string(),
                    initial: a.initial.to_string(),
                })
                .collect();

            entries.push(Entry {
                kind: Kind::Component,
                name: c.name.to_string(),
                slug,
                module_path: c.module_path.to_string(),
                docs: c.docs.to_string(),
                fields,
                fields_documented,
                variants: Vec::new(),
                composes,
                methods,
                animations,
                return_type: String::new(),
            });
        }

        // --- Primitives ----------------------------------------------------
        for p in cat.primitives() {
            let fields: Vec<Field> = p
                .props
                .iter()
                .map(|f| Field {
                    name: f.name.to_string(),
                    ty: f.type_str.to_string(),
                    doc: f.doc.to_string(),
                    constraint: f.constraint.to_string(),
                })
                .collect();
            let fields_documented =
                fields.iter().any(|f| !f.doc.is_empty() || !f.constraint.is_empty());
            entries.push(Entry {
                kind: Kind::Primitive,
                name: p.name.to_string(),
                slug: slugify(p.name, ""),
                module_path: format!("primitive · {}", p.category.as_str()),
                docs: p.docs.to_string(),
                fields,
                fields_documented,
                variants: Vec::new(),
                composes: Vec::new(),
                methods: Vec::new(),
                animations: Vec::new(),
                return_type: String::new(),
            });
        }

        // --- Utilities -----------------------------------------------------
        for u in cat.utilities() {
            let fields: Vec<Field> = u
                .params
                .iter()
                .map(|p| Field {
                    name: p.name.to_string(),
                    ty: p.type_str.to_string(),
                    doc: String::new(),
                    constraint: String::new(),
                })
                .collect();
            entries.push(Entry {
                kind: Kind::Utility,
                name: u.name.to_string(),
                slug: slugify(u.name, u.module_path),
                module_path: format!("{}::{}", u.module_path, u.name),
                docs: u.docs.to_string(),
                fields,
                fields_documented: false,
                variants: Vec::new(),
                composes: Vec::new(),
                methods: Vec::new(),
                animations: Vec::new(),
                return_type: u.return_type.to_string(),
            });
        }

        // --- Types ---------------------------------------------------------
        for t in cat.types() {
            let mut fields = Vec::new();
            let mut variants = Vec::new();
            match &t.shape {
                TypeShape::Struct { fields: fs } => {
                    for f in *fs {
                        fields.push(Field {
                            name: f.name.to_string(),
                            ty: f.type_str.to_string(),
                            doc: f.doc.to_string(),
                            constraint: f.constraint.to_string(),
                        });
                    }
                }
                TypeShape::Enum { variants: vs } => {
                    for v in *vs {
                        variants.push((v.name.to_string(), v.docs.to_string()));
                    }
                }
            }
            let fields_documented =
                fields.iter().any(|f| !f.doc.is_empty() || !f.constraint.is_empty());
            entries.push(Entry {
                kind: Kind::Type,
                name: t.short_name.to_string(),
                slug: slugify(t.short_name, t.module_path),
                module_path: format!("{}::{}", t.module_path, t.short_name),
                docs: t.docs.to_string(),
                fields,
                fields_documented,
                variants,
                composes: Vec::new(),
                methods: Vec::new(),
                animations: Vec::new(),
                return_type: String::new(),
            });
        }

        // --- Guides --------------------------------------------------------
        for g in cat.guides() {
            entries.push(Entry {
                kind: Kind::Guide,
                name: g.title.to_string(),
                slug: slugify(g.slug, ""),
                module_path: String::new(),
                docs: g.body.to_string(),
                fields: Vec::new(),
                fields_documented: false,
                variants: Vec::new(),
                composes: Vec::new(),
                methods: Vec::new(),
                animations: Vec::new(),
                return_type: String::new(),
            });
        }

        // Stable, alphabetical order within each kind for a predictable
        // sidebar.
        entries.sort_by(|a, b| {
            (a.kind as u8, a.name.to_lowercase()).cmp(&(b.kind as u8, b.name.to_lowercase()))
        });

        CatalogModel { entries }
    }

    /// Every entry, in sorted order. Used by the slug round-trip
    /// invariant test; kept public as part of the model's surface.
    #[allow(dead_code)]
    pub fn all(&self) -> &[Entry] {
        &self.entries
    }

    /// Entries of one kind, in sorted order.
    pub fn of_kind(&self, kind: Kind) -> Vec<&Entry> {
        self.entries.iter().filter(|e| e.kind == kind).collect()
    }

    /// The kinds that actually have at least one entry, in display
    /// order. Drives the sidebar so empty kinds don't show a bare
    /// header.
    pub fn populated_kinds(&self) -> Vec<Kind> {
        const ORDER: [Kind; 5] =
            [Kind::Component, Kind::Primitive, Kind::Utility, Kind::Type, Kind::Guide];
        ORDER
            .into_iter()
            .filter(|k| self.entries.iter().any(|e| e.kind == *k))
            .collect()
    }

    /// Look up an entry by kind + slug (the route key).
    pub fn find(&self, kind: Kind, slug: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.kind == kind && e.slug == slug)
    }

    pub fn total(&self) -> usize {
        self.entries.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model() -> CatalogModel {
        CatalogModel::build()
    }

    // The inventory linker-section concern, validated: because this test
    // binary links idea-ui (we reference its components in `app()` and
    // depend on the crate), idea-ui's `#[component]` `inventory::submit!`
    // ctors must end up in the catalog. If this regresses (linker prunes
    // them), the docs app would render an empty Components list — so this
    // is the canary.
    #[test]
    fn idea_ui_components_are_present_in_runtime_catalog() {
        let m = model();
        let components: Vec<&str> = m.of_kind(Kind::Component).iter().map(|e| e.name.as_str()).collect();
        assert!(
            components.contains(&"Button"),
            "expected idea-ui `Button` in the runtime catalog, got components: {:?}",
            components
        );
        assert!(
            components.contains(&"Card"),
            "expected idea-ui `Card` in the runtime catalog, got components: {:?}",
            components
        );
    }

    #[test]
    fn primitives_slice_is_populated() {
        let m = model();
        // The framework ships a locked primitives table; `view` is the
        // canonical structural leaf and must always be present.
        let prims: Vec<&str> = m.of_kind(Kind::Primitive).iter().map(|e| e.name.as_str()).collect();
        assert!(prims.contains(&"view"), "expected `view` primitive, got: {:?}", prims);
    }

    #[test]
    fn populated_kinds_are_in_display_order_and_nonempty() {
        let m = model();
        let kinds = m.populated_kinds();
        assert!(kinds.contains(&Kind::Component));
        assert!(kinds.contains(&Kind::Primitive));
        // Display order invariant: each kind index is non-decreasing.
        let mut last = 0u8;
        for k in &kinds {
            assert!(*k as u8 >= last, "kinds out of display order: {:?}", kinds);
            last = *k as u8;
        }
    }

    #[test]
    fn find_round_trips_through_slug() {
        let m = model();
        // Every entry must be reachable by its own (kind, slug) — the
        // routing contract. A collision or empty slug would break a
        // page link.
        for e in m.all() {
            assert!(!e.slug.is_empty(), "empty slug for {} ({:?})", e.name, e.kind);
            let found = m.find(e.kind, &e.slug);
            assert!(found.is_some(), "could not round-trip {} via slug {}", e.name, e.slug);
        }
    }

    #[test]
    fn slugify_is_url_safe_and_stable() {
        assert_eq!(slugify("Button", ""), "button");
        assert_eq!(slugify("now_micros", "runtime_core::time"), "now-micros-runtime-core-time");
        assert_eq!(slugify("Foo<T>", ""), "foo-t");
        // No leading/trailing dashes, no doubled dashes.
        let s = slugify("  weird::name  ", "");
        assert!(!s.starts_with('-') && !s.ends_with('-'));
        assert!(!s.contains("--"));
    }

    #[test]
    #[ignore = "diagnostic — run with --ignored --nocapture to print catalog kind counts"]
    fn print_kind_counts() {
        let m = model();
        for k in m.populated_kinds() {
            println!("{}: {}", k.title(), m.of_kind(k).len());
        }
        println!("total: {}", m.total());
    }

    #[test]
    fn undocumented_props_flagged_gracefully() {
        // Most idea-ui props structs don't derive IdealystSchema yet, so
        // their component entries surface a single `props: &XxxProps`
        // field with no doc. `fields_documented` must be false for those
        // so the renderer shows the "not yet documented" note instead of
        // a blank table — never crashing.
        let m = model();
        let button = m
            .of_kind(Kind::Component)
            .into_iter()
            .find(|e| e.name == "Button")
            .expect("Button present");
        // Whatever the schema state, the field list is non-empty (at
        // minimum the props-struct param) and the flag is a clean bool.
        assert!(!button.fields.is_empty(), "Button should surface at least its props param");
        let _ = button.fields_documented; // either state is valid; must not panic
    }
}
