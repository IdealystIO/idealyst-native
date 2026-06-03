//! Phase 2: runtime name-resolution + adjacency.
//!
//! Composes edges emitted by `#[component]` (see `EdgeRef`) carry the
//! bare ident the author wrote at the `ui!` / `jsx!` call site. The
//! framework's dispatch is transform-free: a call site `Card()` lowers
//! to `Card!()` → `Card(...)`, so the call-site ident is *exactly* the
//! component's fn name (and thus its catalog entry name). We match
//! edges to entries by that exact name — no case folding. Case is
//! significant: `Card` and `card` are distinct components, and a
//! reference can only compile against a definition of the same name.
//!
//! Bare idents are still ambiguous in one way:
//!
//! - **Same short-name across modules**: two crates / submodules may
//!    each declare `#[component] fn Card`. We disambiguate by
//!    source-module proximity (spec §6): same module first, then the
//!    deepest ancestor module, then anywhere in the workspace.
//!    Anything still ambiguous after that is surfaced as
//!    [`EdgeStatus::Ambiguous`] with the candidate list intact — the
//!    runtime hands the choice back to the user / `mcp --check`.
//!
//! Resolution runs *once* over the global catalog via
//! [`ResolvedCatalog::build`]; the forward and reverse adjacency maps
//! are then constant-time lookups. Reverse adjacency gives
//! `find_uses(name)` — who composes me? — in O(1) after the one-pass
//! build.

use std::collections::HashMap;

use crate::slice::LeakFromJson;
use crate::{
    AnimationEntry, ComponentEntry, EdgeRef, GuideEntry, MethodEntry, ParamSpec,
    PrimitiveCategory, PrimitiveEntry, PropFieldSpec, RecipeEntry, ScopeEntry, StateEntry,
    ToolEntry, TypeEntry, TypeShape, UtilityCategory, UtilityEntry, VariantSpec,
};

/// A `(module_path, name)` pair, the canonical identity for a
/// `ComponentEntry`. Two entries with the same pair would be a
/// duplicate registration — we treat them as identical here.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub struct EntryRef {
    pub module_path: &'static str,
    pub name: &'static str,
}

impl EntryRef {
    pub fn of(entry: &ComponentEntry) -> Self {
        EntryRef { module_path: entry.module_path, name: entry.name }
    }

    /// Fully-qualified name as a heap string: `module_path::name`.
    pub fn fqn(&self) -> String {
        format!("{}::{}", self.module_path, self.name)
    }
}

/// One composition edge with its resolution result attached.
#[derive(Debug, Clone)]
pub struct ResolvedEdge {
    /// Bare ident as written at the `ui!` / `jsx!` call site.
    pub raw_name: &'static str,
    /// Source line the macro recorded (0 on stable Rust; see
    /// `mcp_catalog::EdgeRef` doc-comment).
    pub line: u32,
    pub status: EdgeStatus,
}

/// Outcome of attempting to resolve a composes edge.
#[derive(Debug, Clone)]
pub enum EdgeStatus {
    /// Exactly one match (possibly after proximity tie-break).
    Resolved { target: EntryRef },
    /// Zero matches anywhere in the workspace.
    NoMatch,
    /// More than one match at the same proximity level. Spec §6 says
    /// the runtime surfaces these to the user — `mcp --check` is the
    /// intended consumer.
    Ambiguous { candidates: Vec<EntryRef> },
}

/// The catalog slice an entity lives in — the discriminant
/// [`ResolvedCatalog::resolve_entity`] tags each match with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityKind {
    Component,
    Primitive,
    Utility,
    Tool,
    Type,
    Method,
}

/// One hit from the cross-kind name resolver. `module_path` is `""` for
/// primitives (they have no module).
#[derive(Debug, Clone, Copy)]
pub struct EntityMatch {
    pub kind: EntityKind,
    pub module_path: &'static str,
    pub name: &'static str,
}

/// Resolved view over the catalog.
///
/// Built once at startup. Holds the entries (drained from `inventory`),
/// the forward edges (host → resolved edges), and the reverse map
/// (target → hosts that compose it). All three are populated in a
/// single pass.
///
/// In v2 (catalog_version: 2), the model also carries the
/// framework-shipped locked slices and the open-author slices
/// (methods, animations, types) — populated from `inventory` in the
/// [`build`] path and from JSON in [`build_from_json`].
#[derive(Debug, Default)]
pub struct ResolvedCatalog {
    entries: Vec<&'static ComponentEntry>,
    forward: HashMap<EntryRef, Vec<ResolvedEdge>>,
    reverse: HashMap<EntryRef, Vec<EntryRef>>,
    primitives: Vec<&'static crate::PrimitiveEntry>,
    utilities: Vec<&'static crate::UtilityEntry>,
    states: Vec<&'static crate::StateEntry>,
    guides: Vec<&'static crate::GuideEntry>,
    methods: Vec<&'static crate::MethodEntry>,
    animations: Vec<&'static crate::AnimationEntry>,
    types: Vec<&'static crate::TypeEntry>,
    tools: Vec<&'static crate::ToolEntry>,
    recipes: Vec<&'static crate::RecipeEntry>,
    scopes: Vec<&'static crate::ScopeEntry>,
}

impl ResolvedCatalog {
    /// Build over the global `inventory` slice.
    pub fn build() -> Self {
        let entries: Vec<&'static ComponentEntry> = crate::entries().collect();
        let mut cat = Self::build_from(entries);
        cat.primitives = crate::primitives().collect();
        cat.utilities = crate::utilities().collect();
        cat.states = crate::states().collect();
        cat.guides = crate::guides().collect();
        cat.methods = crate::methods().collect();
        cat.animations = crate::animations().collect();
        cat.types = crate::types().collect();
        cat.tools = crate::tools().collect();
        cat.recipes = crate::recipes().collect();
        cat.scopes = crate::scopes().collect();
        cat
    }

    /// Build from a JSON document in the shape `catalog_json()`
    /// produces (see `crates/framework/mcp/src/lib.rs`). Used by
    /// the MCP server's subprocess-reload pipeline: a child
    /// process prints `catalog_json()` to stdout, the server pipes
    /// it back, and this call leaks-and-builds the owned entries
    /// into a new catalog.
    ///
    /// **Allocation model**: each parsed string is `Box::leak`ed
    /// into `&'static str`, and each `composes` / `params` slice
    /// into `&'static [_]`. The bytes persist for the server's
    /// lifetime. For dev workflows reloading a handful of times
    /// per minute the leak is on the order of KB per reload —
    /// acceptable. A long-running deployment that reloads
    /// thousands of times should batch reloads or graduate to a
    /// per-reload arena.
    pub fn build_from_json(json: &str) -> Result<Self, BuildFromJsonError> {
        let value: serde_json::Value = serde_json::from_str(json)
            .map_err(BuildFromJsonError::Parse)?;
        let components = value["components"]
            .as_array()
            .ok_or(BuildFromJsonError::MissingComponents)?;
        let mut entries: Vec<&'static ComponentEntry> = Vec::with_capacity(components.len());
        for c in components {
            entries.push(leak_entry_from_json(c)?);
        }
        let mut cat = Self::build_from(entries);
        // v2 slices — all optional, each rebuilt through its
        // `LeakFromJson` impl. Missing keys yield empty Vecs (via
        // `slice_vec`) so v1 producers keep working unchanged.
        cat.primitives = slice_vec::<PrimitiveEntry>(&value);
        cat.utilities = slice_vec::<UtilityEntry>(&value);
        cat.states = slice_vec::<StateEntry>(&value);
        cat.guides = slice_vec::<GuideEntry>(&value);
        cat.methods = slice_vec::<MethodEntry>(&value);
        cat.animations = slice_vec::<AnimationEntry>(&value);
        cat.types = slice_vec::<TypeEntry>(&value);
        cat.tools = slice_vec::<ToolEntry>(&value);
        cat.recipes = slice_vec::<RecipeEntry>(&value);
        cat.scopes = slice_vec::<ScopeEntry>(&value);
        Ok(cat)
    }

    pub fn primitives(&self) -> &[&'static crate::PrimitiveEntry] {
        &self.primitives
    }
    pub fn utilities(&self) -> &[&'static crate::UtilityEntry] {
        &self.utilities
    }
    pub fn states(&self) -> &[&'static crate::StateEntry] {
        &self.states
    }
    pub fn guides(&self) -> &[&'static crate::GuideEntry] {
        &self.guides
    }
    pub fn methods(&self) -> &[&'static crate::MethodEntry] {
        &self.methods
    }
    pub fn animations(&self) -> &[&'static crate::AnimationEntry] {
        &self.animations
    }
    pub fn types(&self) -> &[&'static crate::TypeEntry] {
        &self.types
    }
    pub fn tools(&self) -> &[&'static crate::ToolEntry] {
        &self.tools
    }
    pub fn recipes(&self) -> &[&'static crate::RecipeEntry] {
        &self.recipes
    }
    pub fn scopes(&self) -> &[&'static crate::ScopeEntry] {
        &self.scopes
    }

    /// Recipes that demonstrate `name` — either as their primary
    /// [`target`](crate::RecipeEntry::target) or merely referenced in
    /// their body ([`uses`](crate::RecipeEntry::uses)). Kind-agnostic:
    /// works for a component, utility, free function, or type name
    /// alike. This is the join every `describe_*` uses to surface
    /// "here's how to use it" examples — the phase-3 generalization of
    /// what was previously inlined for components only.
    pub fn recipes_for(&self, name: &str) -> Vec<&'static RecipeEntry> {
        self.recipes
            .iter()
            .copied()
            .filter(|r| r.target == name || r.uses.iter().any(|u| *u == name))
            .collect()
    }

    /// Cross-kind name resolver: every catalog entity matching `name`
    /// exactly, tagged with its [`EntityKind`]. Empty when nothing
    /// matches; more than one hit means the name is ambiguous across (or
    /// within) kinds — a `--check` concern, surfaced here so callers can
    /// report it. The generalized counterpart to the component-only
    /// `composes` edge resolver: a recipe target or a future "what is
    /// X?" query resolves through this.
    pub fn resolve_entity(&self, name: &str) -> Vec<EntityMatch> {
        let mut out = Vec::new();
        for e in &self.entries {
            if e.name == name {
                out.push(EntityMatch {
                    kind: EntityKind::Component,
                    module_path: e.module_path,
                    name: e.name,
                });
            }
        }
        for p in &self.primitives {
            if p.name == name || p.pascal_name == name {
                out.push(EntityMatch {
                    kind: EntityKind::Primitive,
                    module_path: "",
                    name: p.name,
                });
            }
        }
        for u in &self.utilities {
            if u.name == name {
                out.push(EntityMatch {
                    kind: EntityKind::Utility,
                    module_path: u.module_path,
                    name: u.name,
                });
            }
        }
        for t in &self.tools {
            if t.name == name {
                out.push(EntityMatch {
                    kind: EntityKind::Tool,
                    module_path: t.module_path,
                    name: t.name,
                });
            }
        }
        for t in &self.types {
            if t.short_name == name {
                out.push(EntityMatch {
                    kind: EntityKind::Type,
                    module_path: t.module_path,
                    name: t.short_name,
                });
            }
        }
        for m in &self.methods {
            if m.name == name {
                out.push(EntityMatch {
                    kind: EntityKind::Method,
                    module_path: m.parent_module_path,
                    name: m.name,
                });
            }
        }
        out
    }

    /// Ambient scope assignment: the scope an entity at `module_path`
    /// belongs to, by module proximity — same-module declaration first,
    /// then the closest ancestor module that declared a `doc_scope!`,
    /// else `None` (falls back to the default/root scope at a higher
    /// layer). Mirrors the component edge resolver's proximity rules
    /// (see [`resolve_one`]), so scope membership follows the same
    /// "nearest wins" semantics authors already rely on.
    ///
    /// Ties (two scopes declared in the same module, or two equidistant
    /// ancestors) resolve to the lowest `slug` for determinism — a
    /// genuinely ambiguous scoping is a `--check` concern, not a panic.
    pub fn scope_for(&self, module_path: &str) -> Option<&'static ScopeEntry> {
        // 1. Same-module declaration wins outright.
        if let Some(s) = self
            .scopes
            .iter()
            .filter(|s| s.module_path == module_path)
            .min_by_key(|s| s.slug)
        {
            return Some(s);
        }
        // 2. Closest ancestor module (longest matching module prefix).
        self.scopes
            .iter()
            .filter(|s| is_ancestor_module(s.module_path, module_path))
            .max_by(|a, b| {
                a.module_path
                    .len()
                    .cmp(&b.module_path.len())
                    // Same depth → lowest slug for a deterministic pick.
                    .then_with(|| b.slug.cmp(a.slug))
            })
            .copied()
    }

    /// Build from an explicit entry list — the path tests use to
    /// exercise resolution without touching the global slice.
    pub fn build_from(entries: Vec<&'static ComponentEntry>) -> Self {
        // Bucket every entry under its exact name so the resolution
        // path is a single hash lookup. The framework's transform-free
        // dispatch (see module doc-comment) guarantees a call-site
        // ident equals the component's fn/entry name verbatim, so an
        // exact-name match is correct — no case folding.
        let mut by_name: HashMap<&'static str, Vec<&'static ComponentEntry>> =
            HashMap::new();
        for e in &entries {
            by_name.entry(e.name).or_default().push(e);
        }

        let mut forward: HashMap<EntryRef, Vec<ResolvedEdge>> = HashMap::new();
        let mut reverse: HashMap<EntryRef, Vec<EntryRef>> = HashMap::new();

        for host in &entries {
            let host_ref = EntryRef::of(host);
            let mut edges = Vec::with_capacity(host.composes.len());
            for edge in host.composes {
                let candidates = by_name
                    .get(edge.name)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]);
                let status = resolve_one(host.module_path, candidates);
                if let EdgeStatus::Resolved { target } = &status {
                    reverse.entry(*target).or_default().push(host_ref);
                }
                edges.push(ResolvedEdge {
                    raw_name: edge.name,
                    line: edge.line,
                    status,
                });
            }
            forward.insert(host_ref, edges);
        }

        // Stable order in `uses()` output makes tests + diffs sane.
        for v in reverse.values_mut() {
            v.sort_by_key(|r| (r.module_path, r.name));
            v.dedup();
        }

        ResolvedCatalog {
            entries,
            forward,
            reverse,
            ..Default::default()
        }
    }

    pub fn entries(&self) -> &[&'static ComponentEntry] {
        &self.entries
    }

    /// Forward edges: what does `host` compose?
    pub fn dependencies(&self, host: &EntryRef) -> &[ResolvedEdge] {
        self.forward.get(host).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Reverse edges: who composes `target`?
    pub fn uses(&self, target: &EntryRef) -> &[EntryRef] {
        self.reverse.get(target).map(Vec::as_slice).unwrap_or(&[])
    }
}

/// Apply spec §6's proximity rules to a candidate set whose lowercased
/// names all match the edge's lowercased ident. Returns:
/// - `NoMatch` if `candidates` is empty.
/// - `Resolved` if exactly one candidate wins after tie-break.
/// - `Ambiguous` if multiple candidates remain at the winning level.
fn resolve_one(
    host_module: &'static str,
    candidates: &[&'static ComponentEntry],
) -> EdgeStatus {
    if candidates.is_empty() {
        return EdgeStatus::NoMatch;
    }

    // 1. Same-module match wins outright.
    let same: Vec<&ComponentEntry> = candidates
        .iter()
        .copied()
        .filter(|c| c.module_path == host_module)
        .collect();
    if let Some(unique) = single(&same) {
        return EdgeStatus::Resolved { target: EntryRef::of(unique) };
    }
    if same.len() > 1 {
        return EdgeStatus::Ambiguous {
            candidates: same.iter().map(|e| EntryRef::of(e)).collect(),
        };
    }

    // 2. Closest ancestor module wins next. "Closest" = the candidate
    //    whose module_path is the longest strict prefix of the host's.
    let mut best_len: usize = 0;
    let mut best: Vec<&ComponentEntry> = Vec::new();
    for c in candidates {
        if is_ancestor_module(c.module_path, host_module) {
            let len = c.module_path.len();
            #[allow(clippy::comparison_chain)]
            if len > best_len {
                best_len = len;
                best.clear();
                best.push(c);
            } else if len == best_len {
                best.push(c);
            }
        }
    }
    if let Some(unique) = single(&best) {
        return EdgeStatus::Resolved { target: EntryRef::of(unique) };
    }
    if best.len() > 1 {
        return EdgeStatus::Ambiguous {
            candidates: best.iter().map(|e| EntryRef::of(e)).collect(),
        };
    }

    // 3. Anywhere else in the workspace. One match → resolved; many →
    //    ambiguous with the full candidate list.
    if let Some(unique) = single(candidates) {
        return EdgeStatus::Resolved { target: EntryRef::of(unique) };
    }
    EdgeStatus::Ambiguous {
        candidates: candidates.iter().map(|e| EntryRef::of(e)).collect(),
    }
}

fn single<'a, T: Copy>(slice: &'a [T]) -> Option<T> {
    if slice.len() == 1 { Some(slice[0]) } else { None }
}

/// Error type for [`ResolvedCatalog::build_from_json`].
#[derive(Debug)]
pub enum BuildFromJsonError {
    Parse(serde_json::Error),
    MissingComponents,
    MissingField(&'static str),
}

impl std::fmt::Display for BuildFromJsonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parse(e) => write!(f, "catalog JSON parse error: {}", e),
            Self::MissingComponents => write!(f, "catalog JSON missing `components` array"),
            Self::MissingField(name) => write!(f, "catalog entry missing required field {:?}", name),
        }
    }
}

impl std::error::Error for BuildFromJsonError {}

fn leak_str(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

fn leak_entry_from_json(c: &serde_json::Value) -> Result<&'static ComponentEntry, BuildFromJsonError> {
    fn req_str<'a>(v: &'a serde_json::Value, field: &'static str) -> Result<&'a str, BuildFromJsonError> {
        v[field].as_str().ok_or(BuildFromJsonError::MissingField(field))
    }

    let composes_raw = c["composes"].as_array().cloned().unwrap_or_default();
    let composes: Vec<EdgeRef> = composes_raw
        .iter()
        .filter_map(|e| {
            let name = e["name"].as_str()?.to_string();
            let line = e["line"].as_u64().unwrap_or(0) as u32;
            Some(EdgeRef { name: leak_str(name), line })
        })
        .collect();
    let composes_leaked: &'static [EdgeRef] = Box::leak(composes.into_boxed_slice());

    let params_raw = c["params"].as_array().cloned().unwrap_or_default();
    let params: Vec<ParamSpec> = params_raw
        .iter()
        .filter_map(|p| {
            let name = p["name"].as_str()?.to_string();
            let type_str = p["type"].as_str()?.to_string();
            let short = p["type_short_name"].as_str().unwrap_or("").to_string();
            Some(ParamSpec {
                name: leak_str(name),
                type_str: leak_str(type_str),
                type_short_name: leak_str(short),
            })
        })
        .collect();
    let params_leaked: &'static [ParamSpec] = Box::leak(params.into_boxed_slice());

    let entry = ComponentEntry {
        name: leak_str(req_str(c, "name")?.to_string()),
        module_path: leak_str(req_str(c, "module_path")?.to_string()),
        file: leak_str(req_str(c, "file")?.to_string()),
        line: c["line"].as_u64().unwrap_or(0) as u32,
        docs: leak_str(c["docs"].as_str().unwrap_or("").to_string()),
        composes: composes_leaked,
        params: params_leaked,
    };
    Ok(Box::leak(Box::new(entry)))
}

fn leak_param_specs(arr: &serde_json::Value) -> &'static [ParamSpec] {
    let Some(arr) = arr.as_array() else { return &[] };
    let v: Vec<ParamSpec> = arr
        .iter()
        .filter_map(|p| {
            let name = p["name"].as_str()?.to_string();
            let type_str = p["type"].as_str()?.to_string();
            let short = p["type_short_name"].as_str().unwrap_or("").to_string();
            Some(ParamSpec {
                name: leak_str(name),
                type_str: leak_str(type_str),
                type_short_name: leak_str(short),
            })
        })
        .collect();
    Box::leak(v.into_boxed_slice())
}

fn leak_str_slice(arr: &serde_json::Value) -> &'static [&'static str] {
    let Some(arr) = arr.as_array() else { return &[] };
    let v: Vec<&'static str> = arr
        .iter()
        .filter_map(|x| x.as_str().map(|s| leak_str(s.to_string()) as &'static str))
        .collect();
    Box::leak(v.into_boxed_slice())
}

fn leak_prop_fields(arr: &serde_json::Value) -> &'static [PropFieldSpec] {
    let Some(arr) = arr.as_array() else { return &[] };
    let v: Vec<PropFieldSpec> = arr
        .iter()
        .filter_map(|p| {
            let name = p["name"].as_str()?.to_string();
            let type_str = p["type"].as_str()?.to_string();
            let doc = p["doc"].as_str().unwrap_or("").to_string();
            let constraint = p["constraint"].as_str().unwrap_or("").to_string();
            Some(PropFieldSpec {
                name: leak_str(name),
                type_str: leak_str(type_str),
                doc: leak_str(doc),
                constraint: leak_str(constraint),
            })
        })
        .collect();
    Box::leak(v.into_boxed_slice())
}

fn primitive_category_from_str(s: &str) -> PrimitiveCategory {
    match s {
        "structural" => PrimitiveCategory::Structural,
        "input" => PrimitiveCategory::Input,
        "display" => PrimitiveCategory::Display,
        "media" => PrimitiveCategory::Media,
        "control-flow" => PrimitiveCategory::ControlFlow,
        "composition" => PrimitiveCategory::Composition,
        _ => PrimitiveCategory::Advanced,
    }
}

fn utility_category_from_str(s: &str) -> UtilityCategory {
    match s {
        "color" => UtilityCategory::Color,
        "time" => UtilityCategory::Time,
        "theme" => UtilityCategory::Theme,
        "layout" => UtilityCategory::Layout,
        "math" => UtilityCategory::Math,
        _ => UtilityCategory::Platform,
    }
}

fn leak_primitive_from_json(v: &serde_json::Value) -> Option<&'static PrimitiveEntry> {
    let name = v["name"].as_str()?.to_string();
    let pascal_name = v["pascal_name"].as_str()?.to_string();
    let docs = v["docs"].as_str().unwrap_or("").to_string();
    let category =
        primitive_category_from_str(v["category"].as_str().unwrap_or("advanced"));
    let backends = leak_str_slice(&v["backends"]);
    let props = leak_prop_fields(&v["props"]);
    Some(Box::leak(Box::new(PrimitiveEntry {
        name: leak_str(name),
        pascal_name: leak_str(pascal_name),
        docs: leak_str(docs),
        props,
        category,
        backends,
        _seal: (),
    })))
}

fn leak_utility_from_json(v: &serde_json::Value) -> Option<&'static UtilityEntry> {
    let name = v["name"].as_str()?.to_string();
    let module_path = v["module_path"].as_str().unwrap_or("").to_string();
    let docs = v["docs"].as_str().unwrap_or("").to_string();
    let return_type = v["return_type"].as_str().unwrap_or("").to_string();
    let return_type_short = v["return_type_short"].as_str().unwrap_or("").to_string();
    let category =
        utility_category_from_str(v["category"].as_str().unwrap_or("platform"));
    let params = leak_param_specs(&v["params"]);
    Some(Box::leak(Box::new(UtilityEntry {
        name: leak_str(name),
        module_path: leak_str(module_path),
        docs: leak_str(docs),
        params,
        return_type: leak_str(return_type),
        return_type_short: leak_str(return_type_short),
        category,
        _seal: (),
    })))
}

fn leak_state_from_json(v: &serde_json::Value) -> Option<&'static StateEntry> {
    let name = v["name"].as_str()?.to_string();
    let docs = v["docs"].as_str().unwrap_or("").to_string();
    let backends = leak_str_slice(&v["backends"]);
    Some(Box::leak(Box::new(StateEntry {
        name: leak_str(name),
        docs: leak_str(docs),
        backends,
        _seal: (),
    })))
}

fn leak_guide_from_json(v: &serde_json::Value) -> Option<&'static GuideEntry> {
    let slug = v["slug"].as_str()?.to_string();
    let title = v["title"].as_str().unwrap_or(&slug).to_string();
    let order = v["order"].as_u64().unwrap_or(999) as u32;
    let body = v["body"].as_str().unwrap_or("").to_string();
    let tags = leak_str_slice(&v["tags"]);
    Some(Box::leak(Box::new(GuideEntry {
        slug: leak_str(slug),
        title: leak_str(title),
        order,
        tags,
        body: leak_str(body),
        _seal: (),
    })))
}

fn leak_method_from_json(v: &serde_json::Value) -> Option<&'static MethodEntry> {
    let parent_module_path = v["parent_module_path"].as_str()?.to_string();
    let parent_name = v["parent_name"].as_str()?.to_string();
    let name = v["name"].as_str()?.to_string();
    let docs = v["docs"].as_str().unwrap_or("").to_string();
    let return_type = v["return_type"].as_str().unwrap_or("").to_string();
    let params = leak_param_specs(&v["params"]);
    Some(Box::leak(Box::new(MethodEntry {
        parent_module_path: leak_str(parent_module_path),
        parent_name: leak_str(parent_name),
        name: leak_str(name),
        docs: leak_str(docs),
        params,
        return_type: leak_str(return_type),
    })))
}

fn leak_animation_from_json(v: &serde_json::Value) -> Option<&'static AnimationEntry> {
    let parent_module_path = v["parent_module_path"].as_str()?.to_string();
    let parent_name = v["parent_name"].as_str()?.to_string();
    let binding = v["binding"].as_str().unwrap_or("").to_string();
    let initial = v["initial"].as_str().unwrap_or("").to_string();
    let line = v["line"].as_u64().unwrap_or(0) as u32;
    Some(Box::leak(Box::new(AnimationEntry {
        parent_module_path: leak_str(parent_module_path),
        parent_name: leak_str(parent_name),
        binding: leak_str(binding),
        initial: leak_str(initial),
        line,
    })))
}

fn leak_recipe_from_json(v: &serde_json::Value) -> Option<&'static RecipeEntry> {
    let name = v["name"].as_str()?.to_string();
    let target = v["target"].as_str().unwrap_or("").to_string();
    let module_path = v["module_path"].as_str().unwrap_or("").to_string();
    let file = v["file"].as_str().unwrap_or("").to_string();
    let line = v["line"].as_u64().unwrap_or(0) as u32;
    let docs = v["docs"].as_str().unwrap_or("").to_string();
    let source = v["source"].as_str().unwrap_or("").to_string();
    let uses: Vec<&'static str> = v["uses"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|u| u.as_str().map(|s| leak_str(s.to_string())))
                .collect()
        })
        .unwrap_or_default();
    Some(Box::leak(Box::new(RecipeEntry {
        name: leak_str(name),
        target: leak_str(target),
        module_path: leak_str(module_path),
        file: leak_str(file),
        line,
        docs: leak_str(docs),
        source: leak_str(source),
        uses: Box::leak(uses.into_boxed_slice()),
    })))
}

fn leak_type_from_json(v: &serde_json::Value) -> Option<&'static TypeEntry> {
    let short_name = v["short_name"].as_str()?.to_string();
    let module_path = v["module_path"].as_str()?.to_string();
    let docs = v["docs"].as_str().unwrap_or("").to_string();
    let kind = v["shape"]["kind"].as_str().unwrap_or("struct");
    let shape = match kind {
        "enum" => {
            let variants_arr = v["shape"]["variants"].as_array().cloned().unwrap_or_default();
            let variants: Vec<VariantSpec> = variants_arr
                .iter()
                .filter_map(|vv| {
                    let name = vv["name"].as_str()?.to_string();
                    let docs = vv["docs"].as_str().unwrap_or("").to_string();
                    let payload = leak_prop_fields(&vv["payload"]);
                    Some(VariantSpec {
                        name: leak_str(name),
                        docs: leak_str(docs),
                        payload,
                    })
                })
                .collect();
            TypeShape::Enum { variants: Box::leak(variants.into_boxed_slice()) }
        }
        _ => {
            let fields = leak_prop_fields(&v["shape"]["fields"]);
            TypeShape::Struct { fields }
        }
    };
    Some(Box::leak(Box::new(TypeEntry {
        short_name: leak_str(short_name),
        module_path: leak_str(module_path),
        docs: leak_str(docs),
        shape,
    })))
}

fn leak_tool_from_json(v: &serde_json::Value) -> Option<&'static ToolEntry> {
    let name = v["name"].as_str()?.to_string();
    let module_path = v["module_path"].as_str().unwrap_or("").to_string();
    let file = v["file"].as_str().unwrap_or("").to_string();
    let line = v["line"].as_u64().unwrap_or(0) as u32;
    let docs = v["docs"].as_str().unwrap_or("").to_string();
    let return_type = v["return_type"].as_str().unwrap_or("").to_string();
    let params = leak_param_specs(&v["params"]);
    Some(Box::leak(Box::new(ToolEntry {
        name: leak_str(name),
        module_path: leak_str(module_path),
        file: leak_str(file),
        line,
        docs: leak_str(docs),
        params,
        return_type: leak_str(return_type),
    })))
}

fn leak_scope_from_json(v: &serde_json::Value) -> Option<&'static ScopeEntry> {
    let slug = v["slug"].as_str()?.to_string();
    let title = v["title"].as_str().unwrap_or(&slug).to_string();
    let docs = v["docs"].as_str().unwrap_or("").to_string();
    let module_path = v["module_path"].as_str().unwrap_or("").to_string();
    let order = v["order"].as_u64().unwrap_or(0) as u32;
    Some(Box::leak(Box::new(ScopeEntry {
        slug: leak_str(slug),
        title: leak_str(title),
        docs: leak_str(docs),
        module_path: leak_str(module_path),
        order,
    })))
}

/// `value[S::KEY]` as a Vec of leaked entries, or empty when the key is
/// absent / not an array. The generic reader half of `build_from_json`.
fn slice_vec<S: LeakFromJson>(value: &serde_json::Value) -> Vec<&'static S> {
    value[S::KEY]
        .as_array()
        .map(|a| a.iter().filter_map(|v| S::from_json(v)).collect())
        .unwrap_or_default()
}

// `LeakFromJson` impls — the reader side of each lenient v2 slice. Each
// delegates to the `leak_*_from_json` fn above, which stays the single
// implementation site. `ComponentEntry` intentionally has no impl: its
// reader (`leak_entry_from_json`) is `Result`-returning because a
// malformed component must fail the rebuild, not be silently skipped.
impl LeakFromJson for PrimitiveEntry {
    fn from_json(v: &serde_json::Value) -> Option<&'static Self> {
        leak_primitive_from_json(v)
    }
}
impl LeakFromJson for UtilityEntry {
    fn from_json(v: &serde_json::Value) -> Option<&'static Self> {
        leak_utility_from_json(v)
    }
}
impl LeakFromJson for StateEntry {
    fn from_json(v: &serde_json::Value) -> Option<&'static Self> {
        leak_state_from_json(v)
    }
}
impl LeakFromJson for GuideEntry {
    fn from_json(v: &serde_json::Value) -> Option<&'static Self> {
        leak_guide_from_json(v)
    }
}
impl LeakFromJson for MethodEntry {
    fn from_json(v: &serde_json::Value) -> Option<&'static Self> {
        leak_method_from_json(v)
    }
}
impl LeakFromJson for AnimationEntry {
    fn from_json(v: &serde_json::Value) -> Option<&'static Self> {
        leak_animation_from_json(v)
    }
}
impl LeakFromJson for TypeEntry {
    fn from_json(v: &serde_json::Value) -> Option<&'static Self> {
        leak_type_from_json(v)
    }
}
impl LeakFromJson for ToolEntry {
    fn from_json(v: &serde_json::Value) -> Option<&'static Self> {
        leak_tool_from_json(v)
    }
}
impl LeakFromJson for RecipeEntry {
    fn from_json(v: &serde_json::Value) -> Option<&'static Self> {
        leak_recipe_from_json(v)
    }
}
impl LeakFromJson for ScopeEntry {
    fn from_json(v: &serde_json::Value) -> Option<&'static Self> {
        leak_scope_from_json(v)
    }
}

fn is_ancestor_module(maybe_ancestor: &str, descendant: &str) -> bool {
    if maybe_ancestor == descendant {
        return false;
    }
    if !descendant.starts_with(maybe_ancestor) {
        return false;
    }
    // The next chars after the prefix must be `::` — otherwise
    // `crate::foo` would falsely "ancestor" `crate::foobar`.
    descendant[maybe_ancestor.len()..].starts_with("::")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EdgeRef;

    // `params: &[]` is fine for the resolver tests — those care
    // about composes/proximity, not the per-prop schema surface.
    fn leak_entry(
        module_path: &'static str,
        name: &'static str,
        composes: &'static [EdgeRef],
    ) -> &'static ComponentEntry {
        Box::leak(Box::new(ComponentEntry {
            name,
            module_path,
            file: "synthetic.rs",
            line: 0,
            docs: "",
            composes,
            params: &[],
        }))
    }

    fn leak_edges(edges: Vec<EdgeRef>) -> &'static [EdgeRef] {
        Box::leak(edges.into_boxed_slice())
    }

    #[test]
    fn ancestor_check_is_strict_and_segment_aligned() {
        assert!(is_ancestor_module("crate::a", "crate::a::b"));
        assert!(!is_ancestor_module("crate::a", "crate::a"));
        assert!(!is_ancestor_module("crate::foo", "crate::foobar"));
        assert!(is_ancestor_module("crate", "crate::a"));
    }

    #[test]
    fn pascal_edge_matches_pascal_entry_exactly() {
        // Transform-free dispatch: a `PrimaryButton` call site needs a
        // `PrimaryButton` fn, so the entry is `PrimaryButton` too. The
        // resolver matches by exact name — no case folding.
        let target = leak_entry("crate", "PrimaryButton", &[]);
        let host = leak_entry(
            "crate",
            "host",
            leak_edges(vec![EdgeRef { name: "PrimaryButton", line: 0 }]),
        );
        let cat = ResolvedCatalog::build_from(vec![target, host]);
        let edges = cat.dependencies(&EntryRef::of(host));
        match &edges[0].status {
            EdgeStatus::Resolved { target } => assert_eq!(target.name, "PrimaryButton"),
            other => panic!("expected Resolved, got {:?}", other),
        }
    }

    #[test]
    fn case_is_significant_mismatched_casing_does_not_resolve() {
        // A `Vignette` edge against only a `vignette` entry must NOT
        // resolve — case is significant now. (Such a pairing can't
        // compile in real code; this guards against silent case-folding
        // creeping back into the resolver.)
        let target = leak_entry("crate::components::vignette", "vignette", &[]);
        let host = leak_entry(
            "crate",
            "app",
            leak_edges(vec![EdgeRef { name: "Vignette", line: 0 }]),
        );
        let cat = ResolvedCatalog::build_from(vec![target, host]);
        let edges = cat.dependencies(&EntryRef::of(host));
        assert_eq!(edges.len(), 1);
        assert!(
            matches!(edges[0].status, EdgeStatus::NoMatch),
            "expected NoMatch for case mismatch, got {:?}",
            edges[0].status
        );
    }

    #[test]
    fn same_module_beats_ancestor() {
        // Two `card` entries: one at `crate`, one at `crate::a::b`.
        // A host at `crate::a::b` composes `card` → should pick the
        // same-module `crate::a::b::card`, not the ancestor's.
        let root_card = leak_entry("crate", "card", &[]);
        let local_card = leak_entry("crate::a::b", "card", &[]);
        let host = leak_entry(
            "crate::a::b",
            "host",
            leak_edges(vec![EdgeRef { name: "card", line: 0 }]),
        );
        let cat = ResolvedCatalog::build_from(vec![root_card, local_card, host]);
        let edges = cat.dependencies(&EntryRef::of(host));
        match &edges[0].status {
            EdgeStatus::Resolved { target } => {
                assert_eq!(target.module_path, "crate::a::b");
            }
            other => panic!("expected same-module Resolved; got {:?}", other),
        }
    }

    #[test]
    fn closest_ancestor_wins() {
        // Two `card`s, both ancestors of `crate::a::b::c`. The deeper
        // ancestor (`crate::a`) should win over the shallower (`crate`).
        let deep_card = leak_entry("crate::a", "card", &[]);
        let shallow_card = leak_entry("crate", "card", &[]);
        let host = leak_entry(
            "crate::a::b::c",
            "host",
            leak_edges(vec![EdgeRef { name: "card", line: 0 }]),
        );
        let cat = ResolvedCatalog::build_from(vec![deep_card, shallow_card, host]);
        let edges = cat.dependencies(&EntryRef::of(host));
        match &edges[0].status {
            EdgeStatus::Resolved { target } => {
                assert_eq!(target.module_path, "crate::a");
            }
            other => panic!("expected closest-ancestor Resolved; got {:?}", other),
        }
    }

    #[test]
    fn ambiguous_when_two_candidates_at_same_depth() {
        // Two `card`s, neither in same module nor ancestor of the host.
        // No proximity preference → ambiguous, both surfaced.
        let card_x = leak_entry("crate::x", "card", &[]);
        let card_y = leak_entry("crate::y", "card", &[]);
        let host = leak_entry(
            "crate::host_mod",
            "host",
            leak_edges(vec![EdgeRef { name: "card", line: 0 }]),
        );
        let cat = ResolvedCatalog::build_from(vec![card_x, card_y, host]);
        let edges = cat.dependencies(&EntryRef::of(host));
        match &edges[0].status {
            EdgeStatus::Ambiguous { candidates } => {
                assert_eq!(candidates.len(), 2);
            }
            other => panic!("expected Ambiguous; got {:?}", other),
        }
    }

    #[test]
    fn no_match_when_name_absent() {
        let host = leak_entry(
            "crate",
            "host",
            leak_edges(vec![EdgeRef { name: "ghost", line: 0 }]),
        );
        let cat = ResolvedCatalog::build_from(vec![host]);
        let edges = cat.dependencies(&EntryRef::of(host));
        assert!(matches!(edges[0].status, EdgeStatus::NoMatch));
    }

    #[test]
    fn reverse_lookup_lists_all_hosts() {
        // Three hosts all composing the same target. `uses(target)`
        // should list every host, sorted for determinism.
        let target = leak_entry("crate::lib", "panel", &[]);
        let h1 = leak_entry(
            "crate::a",
            "host_a",
            leak_edges(vec![EdgeRef { name: "panel", line: 0 }]),
        );
        let h2 = leak_entry(
            "crate::b",
            "host_b",
            leak_edges(vec![EdgeRef { name: "panel", line: 0 }]),
        );
        let h3 = leak_entry(
            "crate::c",
            "host_c",
            leak_edges(vec![EdgeRef { name: "panel", line: 0 }]),
        );
        let cat = ResolvedCatalog::build_from(vec![target, h1, h2, h3]);
        let users = cat.uses(&EntryRef::of(target));
        assert_eq!(users.len(), 3);
        let names: Vec<&str> = users.iter().map(|r| r.name).collect();
        assert!(names.contains(&"host_a"));
        assert!(names.contains(&"host_b"));
        assert!(names.contains(&"host_c"));
    }

    #[test]
    fn ambiguous_in_same_module() {
        // Two entries with the same exact name in the same module —
        // a duplicate registration (possible via the inventory slice
        // even though it won't compile as two `fn Card`). The resolver
        // must surface it rather than silently picking one.
        let a = leak_entry("crate", "Card", &[]);
        let b = leak_entry("crate", "Card", &[]);
        let host = leak_entry(
            "crate",
            "host",
            leak_edges(vec![EdgeRef { name: "Card", line: 0 }]),
        );
        let cat = ResolvedCatalog::build_from(vec![a, b, host]);
        let edges = cat.dependencies(&EntryRef::of(host));
        match &edges[0].status {
            EdgeStatus::Ambiguous { candidates } => {
                assert_eq!(candidates.len(), 2);
            }
            other => panic!("expected Ambiguous, got {:?}", other),
        }
    }
}
