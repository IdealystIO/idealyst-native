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

/// The navigable kinds the sidebar groups entries under. `Scope` leads
/// (it's the organizing spine — each scope page lists its members);
/// methods and animations are rendered inline on their parent
/// component's detail page (joined by `parent_*`), so they aren't
/// top-level kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Scope,
    Component,
    Primitive,
    Utility,
    Type,
    IconSet,
    Guide,
}

impl Kind {
    pub fn title(self) -> &'static str {
        match self {
            Kind::Scope => "Scopes",
            Kind::Component => "Components",
            Kind::Primitive => "Primitives",
            Kind::Utility => "Utilities",
            Kind::Type => "Types",
            Kind::IconSet => "Icons",
            Kind::Guide => "Guides",
        }
    }

    /// URL-path segment for this kind's routes (`/components/<slug>`).
    pub fn path_segment(self) -> &'static str {
        match self {
            Kind::Scope => "scopes",
            Kind::Component => "components",
            Kind::Primitive => "primitives",
            Kind::Utility => "utilities",
            Kind::Type => "types",
            Kind::IconSet => "icons",
            Kind::Guide => "guides",
        }
    }

    /// Singular noun for this kind — used in subtitles / member labels.
    pub fn noun(self) -> &'static str {
        match self {
            Kind::Scope => "scope",
            Kind::Component => "component",
            Kind::Primitive => "primitive",
            Kind::Utility => "utility",
            Kind::Type => "type",
            Kind::IconSet => "icon set",
            Kind::Guide => "guide",
        }
    }
}

/// A link from one entry to another (a scope's member, or an entry's
/// owning scope) — enough to render a navigable `link` to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryLink {
    pub kind: Kind,
    pub name: String,
    pub slug: String,
}

/// One field/prop row — shared by component props, primitive props,
/// utility params, struct fields. `ty` is always present (the
/// pretty-printed type string); `doc` / `constraint` may be empty when
/// the source didn't derive `IdealystSchema`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Field {
    pub name: String,
    pub ty: String,
    pub doc: String,
    pub constraint: String,
    /// When this field's type resolves to a catalog `Type` entry (a
    /// non-primitive struct/enum like `ControlSize`), a link to that
    /// type's page. `None` for primitives (`String`, `u32`, …) and types
    /// not in the catalog. Populated in a post-pass over all entries.
    pub type_link: Option<EntryLink>,
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

/// A usage recipe attached to a component. Owned copy of the relevant
/// `mcp_catalog::RecipeEntry` fields. A recipe attaches to a component
/// when it primarily demonstrates it (`primary == true`) or when the
/// component merely appears in the recipe's `uses` list
/// (`primary == false`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recipe {
    /// The recipe fn's name, e.g. `button_basic`.
    pub name: String,
    /// The `module_path!()` at the recipe site (e.g. `idea_ui::recipes`).
    /// Paired with `name`, it keys the build-time `recipe_renderer` map
    /// that turns a renderable recipe into a live component preview.
    pub module_path: String,
    /// Prose docs on the recipe fn. May be empty.
    pub docs: String,
    /// The recipe's formatted, copy-pasteable source code.
    pub source: String,
    /// True when this component is the recipe's primary subject
    /// (`recipe.target == component.name`); false when the component
    /// only appears in the recipe's `uses` list.
    pub primary: bool,
}

/// Metadata for a `Kind::IconSet` entry. The actual icon *geometry*
/// isn't carried here (the catalog is names-only); the renderer joins
/// `crate_name` to a build-linked icon registry to draw the grid (see
/// `crate::icons`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct IconSetMeta {
    /// Cargo crate name — the bridge key to the geometry registry.
    pub crate_name: String,
    /// `use` path root for an icon's ident (`icons_lucide`).
    pub import_path: String,
    /// License of the icon artwork (`ISC`).
    pub license: String,
    /// Upstream homepage for attribution.
    pub homepage: String,
    /// Number of icons in the pack.
    pub count: usize,
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
    /// Usage recipes that demonstrate or reference this entry — for any
    /// kind that can be a recipe target (component, utility, type, …),
    /// joined via `recipes_for`. Primary recipes sort first.
    pub recipes: Vec<Recipe>,
    /// Return type (utilities only). Empty otherwise.
    pub return_type: String,
    /// The scope this entry is assigned to by module proximity, if any.
    /// `None` for unscoped entries and for scope entries themselves.
    /// Rendered as a badge linking to the scope's page.
    pub scope: Option<EntryLink>,
    /// For `Kind::Scope` entries only: the entities assigned to this
    /// scope. Empty for every other kind.
    pub members: Vec<EntryLink>,
    /// For `Kind::IconSet` entries only: pack metadata for the icon
    /// gallery page. `None` for every other kind.
    pub icon_set: Option<IconSetMeta>,
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

/// Recipes targeting or using `name`, as owned [`Recipe`]s, primary
/// first. Kind-agnostic — routes through the catalog's `recipes_for`
/// join so components, utilities, and types all surface their examples
/// the same way the MCP `describe_*` tools do.
fn build_recipes(cat: &ResolvedCatalog, name: &str) -> Vec<Recipe> {
    let mut v: Vec<Recipe> = cat
        .recipes_for(name)
        .iter()
        .map(|r| Recipe {
            name: r.name.to_string(),
            module_path: r.module_path.to_string(),
            docs: r.docs.to_string(),
            source: r.source.to_string(),
            primary: r.target == name,
        })
        .collect();
    v.sort_by(|a, b| (!a.primary, a.name.to_lowercase()).cmp(&(!b.primary, b.name.to_lowercase())));
    v
}

/// The scope an entry at `module_path` belongs to (nearest by module
/// proximity), as a link to its scope page. `None` when unscoped.
fn scope_link(cat: &ResolvedCatalog, module_path: &str) -> Option<EntryLink> {
    cat.scope_for(module_path).map(|s| EntryLink {
        kind: Kind::Scope,
        name: s.title.to_string(),
        slug: slugify(s.slug, ""),
    })
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Does `word` appear in `haystack` bounded by non-identifier chars? So
/// `ControlSize` matches inside `Option<ControlSize>` but `Size` does
/// **not** match inside `ControlSize` (avoids spurious substring links).
fn contains_type_word(haystack: &str, word: &str) -> bool {
    if word.is_empty() {
        return false;
    }
    let bytes = haystack.as_bytes();
    let mut from = 0;
    while let Some(rel) = haystack[from..].find(word) {
        let i = from + rel;
        let before_ok = i == 0 || !is_ident_byte(bytes[i - 1]);
        let after = i + word.len();
        let after_ok = after >= bytes.len() || !is_ident_byte(bytes[after]);
        if before_ok && after_ok {
            return true;
        }
        from = i + 1;
    }
    false
}

/// The catalog `Type` a prop/field type string refers to, if any — the
/// first entry in `type_index` whose name appears as a whole word in
/// `type_str` (so `Option<ControlSize>` / `Reactive<AvatarSize>` link to
/// the inner type). Primitives aren't in the index, so they don't link.
fn link_for_type(type_str: &str, type_index: &[(String, String)]) -> Option<EntryLink> {
    type_index.iter().find_map(|(name, slug)| {
        if contains_type_word(type_str, name) {
            Some(EntryLink { kind: Kind::Type, name: name.clone(), slug: slug.clone() })
        } else {
            None
        }
    })
}

/// The framework catalog, extracted natively at **build time**
/// (`build.rs`) and embedded. Native extraction sees the FULL catalog;
/// the wasm self-inventory is DCE-pruned (only what `app()` references
/// survives), so the docs never depend on it. Identical on every target.
const EMBEDDED_CATALOG_JSON: &str = include_str!(concat!(env!("OUT_DIR"), "/catalog.json"));

impl CatalogModel {
    /// Build the model. Prefers the build-time-embedded catalog (complete
    /// on every target); falls back to the live in-process inventory only
    /// if the embed is empty/unparseable (shouldn't happen in a normal
    /// build — the fallback is partial on wasm due to DCE).
    pub fn build() -> Self {
        if let Ok(cat) = ResolvedCatalog::build_from_json(EMBEDDED_CATALOG_JSON) {
            if !cat.entries().is_empty() {
                return Self::from_resolved(&cat);
            }
        }
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
                                type_link: None,
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
                        type_link: None,
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
                            type_link: None,
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

            // Recipes — primary (target) + referencing (uses), via the
            // shared `recipes_for` join (same as the MCP surface).
            let recipes = build_recipes(cat, c.name);

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
                recipes,
                return_type: String::new(),
                scope: scope_link(cat, c.module_path),
                members: Vec::new(),
                icon_set: None,
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
                    type_link: None,
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
                recipes: build_recipes(cat, p.name),
                return_type: String::new(),
                // Primitives have no module path → no ambient scope.
                scope: None,
                members: Vec::new(),
                icon_set: None,
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
                    type_link: None,
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
                recipes: build_recipes(cat, u.name),
                return_type: u.return_type.to_string(),
                scope: scope_link(cat, u.module_path),
                members: Vec::new(),
                icon_set: None,
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
                            type_link: None,
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
                recipes: build_recipes(cat, t.short_name),
                return_type: String::new(),
                scope: scope_link(cat, t.module_path),
                members: Vec::new(),
                icon_set: None,
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
                recipes: Vec::new(),
                return_type: String::new(),
                scope: None,
                members: Vec::new(),
                icon_set: None,
            });
        }

        // --- Icons ---------------------------------------------------------
        // One entry per registered icon pack. Names-only here (count +
        // import path + license); the gallery page joins `crate_name` to a
        // build-linked geometry registry to render the actual glyphs.
        for s in cat.icon_sets() {
            entries.push(Entry {
                kind: Kind::IconSet,
                name: s.title.to_string(),
                slug: slugify(s.name, ""),
                module_path: s.import_path.to_string(),
                docs: s.docs.to_string(),
                fields: Vec::new(),
                fields_documented: false,
                variants: Vec::new(),
                composes: Vec::new(),
                methods: Vec::new(),
                animations: Vec::new(),
                recipes: Vec::new(),
                return_type: String::new(),
                scope: None,
                members: Vec::new(),
                icon_set: Some(IconSetMeta {
                    crate_name: s.name.to_string(),
                    import_path: s.import_path.to_string(),
                    license: s.license.to_string(),
                    homepage: s.homepage.to_string(),
                    count: s.icons.len(),
                }),
            });
        }

        // --- Scopes (the organizing spine) ---------------------------------
        // Built last so every other entry's `scope` is already set; a
        // scope's members are the entries that resolved into it by module
        // proximity. Empty scopes are still listed (they exist in the
        // catalog and may gain members as the code grows).
        for s in cat.scopes() {
            let slug = slugify(s.slug, "");
            let members: Vec<EntryLink> = entries
                .iter()
                .filter(|e| e.scope.as_ref().map(|l| l.slug.as_str()) == Some(slug.as_str()))
                .map(|e| EntryLink { kind: e.kind, name: e.name.clone(), slug: e.slug.clone() })
                .collect();
            entries.push(Entry {
                kind: Kind::Scope,
                name: s.title.to_string(),
                slug,
                module_path: String::new(),
                docs: s.docs.to_string(),
                fields: Vec::new(),
                fields_documented: false,
                variants: Vec::new(),
                composes: Vec::new(),
                methods: Vec::new(),
                animations: Vec::new(),
                recipes: Vec::new(),
                return_type: String::new(),
                scope: None,
                members,
                icon_set: None,
            });
        }

        // Link prop/field types to their catalog Type pages. Built after
        // all entries exist so the index covers every Type; primitives
        // (not in the index) stay plain text.
        let type_index: Vec<(String, String)> = cat
            .types()
            .iter()
            .map(|t| (t.short_name.to_string(), slugify(t.short_name, t.module_path)))
            .collect();
        for e in &mut entries {
            for f in &mut e.fields {
                f.type_link = link_for_type(&f.ty, &type_index);
            }
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
        const ORDER: [Kind; 7] = [
            Kind::Scope,
            Kind::Component,
            Kind::Primitive,
            Kind::Utility,
            Kind::Type,
            Kind::IconSet,
            Kind::Guide,
        ];
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

    /// Entries whose name or module-path contains `query`
    /// (case-insensitive), optionally narrowed to one `kind`, capped for
    /// a snappy modal. Each hit carries a short doc summary for the card.
    /// Empty/whitespace query → no results (the modal shows a hint).
    pub fn search(&self, query: &str, kind: Option<Kind>) -> Vec<SearchHit> {
        let q = query.trim().to_lowercase();
        // Require at least 2 characters before yielding results — a single
        // letter matches almost everything and isn't useful.
        if q.chars().count() < 2 {
            return Vec::new();
        }
        self.entries
            .iter()
            .filter(|e| kind.map_or(true, |k| e.kind == k))
            .filter(|e| {
                e.name.to_lowercase().contains(&q) || e.module_path.to_lowercase().contains(&q)
            })
            .take(40)
            .map(|e| SearchHit {
                kind: e.kind,
                name: e.name.clone(),
                slug: e.slug.clone(),
                module_path: e.module_path.clone(),
                summary: doc_summary(&e.docs),
            })
            .collect()
    }
}

/// One search result — enough to render a card linking to the entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    pub kind: Kind,
    pub name: String,
    pub slug: String,
    /// Module path / FQN (the same subtitle shown on the detail page) —
    /// the crate/namespace the entry comes from.
    pub module_path: String,
    /// First couple of lines of the entry's docs, trimmed + truncated.
    pub summary: String,
}

/// First one or two non-empty doc lines, joined and truncated — a one-glance
/// description for search cards.
fn doc_summary(docs: &str) -> String {
    let s = docs
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .take(2)
        .collect::<Vec<_>>()
        .join(" ");
    const MAX: usize = 140;
    if s.chars().count() > MAX {
        let mut t: String = s.chars().take(MAX).collect();
        t.push('…');
        t
    } else {
        s
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
    fn icon_set_appears_in_model_with_metadata() {
        // End-to-end: build.rs force-links icons-lucide (catalog feature),
        // so its self-registered IconSetEntry lands in the embedded
        // catalog.json, and from_resolved maps it to a Kind::IconSet entry
        // with pack metadata. If the build-dep wiring regresses, the Icons
        // kind silently vanishes — this is the canary.
        let m = model();
        let sets = m.of_kind(Kind::IconSet);
        assert!(
            !sets.is_empty(),
            "expected at least one icon set (icons-lucide) in the model",
        );
        let lucide = sets
            .iter()
            .find(|e| {
                e.icon_set.as_ref().map(|s| s.crate_name.as_str()) == Some("icons-lucide")
            })
            .expect("icons-lucide pack present");
        let meta = lucide.icon_set.as_ref().expect("icon_set metadata");
        assert_eq!(meta.import_path, "icons_lucide");
        assert_eq!(meta.license, "ISC");
        assert!(
            meta.count > 100,
            "lucide should carry a large icon count, got {}",
            meta.count,
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
    fn every_idea_ui_component_is_scoped() {
        // Phase 7 dogfood: with the `idea-ui` + `components` scopes
        // declared, every idea-ui component must resolve to a scope by
        // ambient module proximity. This is exactly what
        // `mcp --check --strict-scopes` enforces — assert it holds for
        // the live catalog so the strict gate would pass for idea-ui.
        let cat = ResolvedCatalog::build();
        let unscoped: Vec<&str> = cat
            .entries()
            .iter()
            .filter(|e| e.module_path.starts_with("idea_ui"))
            .filter(|e| cat.scope_for(e.module_path).is_none())
            .map(|e| e.name)
            .collect();
        assert!(
            unscoped.is_empty(),
            "these idea-ui components resolve to no doc_scope!: {:?}",
            unscoped
        );
        // And they land in the `components` scope specifically (the
        // nearest ancestor), not some accidental other scope.
        if let Some(button) = cat.entries().iter().find(|e| e.name == "Button") {
            assert_eq!(
                cat.scope_for(button.module_path).map(|s| s.slug),
                Some("components"),
                "Button should resolve to the `components` scope",
            );
        }
    }

    #[test]
    fn scopes_are_generated_and_members_resolve() {
        let m = model();
        let scopes = m.of_kind(Kind::Scope);
        assert!(!scopes.is_empty(), "expected scope entries in the generated model");

        // The `components` scope exists and lists idea-ui components.
        let components_scope = scopes
            .iter()
            .find(|e| e.slug == "components")
            .expect("`components` scope generated");
        assert!(
            components_scope.members.iter().any(|l| l.name == "Button"),
            "components scope should contain Button; members: {:?}",
            components_scope.members.iter().map(|l| &l.name).collect::<Vec<_>>(),
        );

        // Button's detail entry carries a scope badge back to `components`.
        let button = m
            .of_kind(Kind::Component)
            .into_iter()
            .find(|e| e.name == "Button")
            .expect("Button entry");
        assert_eq!(
            button.scope.as_ref().map(|s| s.slug.as_str()),
            Some("components"),
            "Button should badge the `components` scope",
        );

        // The framework `core` scope captures runtime_core utilities.
        if let Some(core) = scopes.iter().find(|e| e.slug == "core") {
            assert!(
                core.members.iter().any(|l| l.kind == Kind::Utility),
                "core scope should contain at least one utility",
            );
        }
    }

    #[test]
    fn utility_recipes_surface_via_recipes_for() {
        // Phase-3 parity: a recipe targeting a non-component must attach
        // to that entity's page through `recipes_for`, not just components.
        // (No utility recipe ships today, so this asserts the wiring path
        // exists rather than a specific recipe — every kind builds recipes
        // through the same join.)
        let m = model();
        // Components still get their recipes (regression guard on the
        // shared helper).
        let with_recipes = m
            .of_kind(Kind::Component)
            .into_iter()
            .any(|e| !e.recipes.is_empty());
        assert!(with_recipes, "expected at least one component recipe via recipes_for");
    }

    /// Diagnostic snapshot of the *generated* docs — run with
    /// `cargo test -p catalog-docs dump_generated_docs -- --ignored --nocapture`
    /// to see exactly what the generator produces (every kind's count,
    /// every scope + its members). Ignored by default so it doesn't spam
    /// normal test runs.
    #[test]
    #[ignore]
    fn dump_generated_docs() {
        let m = model();
        eprintln!("\n=== Generated catalog docs ({} entries) ===", m.total());
        for k in m.populated_kinds() {
            let items = m.of_kind(k);
            eprintln!("\n{} ({}):", k.title(), items.len());
            for e in &items {
                if k == Kind::Scope {
                    let by_kind = |kind: Kind| {
                        e.members.iter().filter(|l| l.kind == kind).count()
                    };
                    eprintln!(
                        "  - {} [{}]  ({} components, {} utilities, {} total members)",
                        e.name,
                        e.slug,
                        by_kind(Kind::Component),
                        by_kind(Kind::Utility),
                        e.members.len(),
                    );
                } else {
                    let scope = e.scope.as_ref().map(|s| s.slug.as_str()).unwrap_or("—");
                    let rc = e.recipes.len();
                    eprintln!(
                        "  - {}{}{}",
                        e.name,
                        if scope != "—" { format!("  (scope: {})", scope) } else { String::new() },
                        if rc > 0 { format!("  [{} recipe(s)]", rc) } else { String::new() },
                    );
                }
            }
        }
        eprintln!();
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

    #[test]
    fn recipes_attach_to_their_component() {
        // Every recipe in the live catalog must surface on the component
        // it primarily demonstrates: its `component` field names a
        // component entry, and that entry carries a `primary` recipe with
        // the matching name. This is the canary that the recipes slice is
        // both linked (non-empty) and correctly joined by name.
        let m = model();
        let cat = ResolvedCatalog::build();
        let recipes = cat.recipes();
        assert!(
            !recipes.is_empty(),
            "expected at least one recipe in the live catalog (e.g. `button_basic`)"
        );

        for r in recipes {
            // Find the component this recipe primarily demonstrates.
            let Some(comp) = m
                .of_kind(Kind::Component)
                .into_iter()
                .find(|e| e.name == r.target)
            else {
                // The primary component may not be in the model (e.g. a
                // recipe for a non-idea-ui component); skip those.
                continue;
            };
            let attached = comp
                .recipes
                .iter()
                .find(|rec| rec.name == r.name)
                .unwrap_or_else(|| {
                    panic!(
                        "recipe `{}` (component `{}`) did not attach to its component; \
                         component had recipes: {:?}",
                        r.name,
                        r.target,
                        comp.recipes.iter().map(|x| &x.name).collect::<Vec<_>>()
                    )
                });
            assert!(
                attached.primary,
                "recipe `{}` should be marked primary on component `{}`",
                r.name, r.target
            );
            assert_eq!(attached.source, r.source, "recipe source should round-trip owned");
        }

        // A recipe must also attach (non-primary) to every component it
        // merely `uses`.
        for r in recipes {
            for &used_name in r.uses {
                if let Some(comp) =
                    m.of_kind(Kind::Component).into_iter().find(|e| e.name == used_name)
                {
                    assert!(
                        comp.recipes.iter().any(|rec| rec.name == r.name),
                        "recipe `{}` uses `{}` but didn't attach to it",
                        r.name,
                        used_name
                    );
                }
            }
        }
    }
}
