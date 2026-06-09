//! Framework MCP — phase 1 prototype.
//!
//! Defines the [`ComponentEntry`] record and the [`inventory`]
//! distributed slice that `#[component]` populates when the
//! `runtime-macros/mcp` feature is on. Provides [`entries`] to walk
//! every registered entry and [`dump_catalog_json`] to emit the catalog
//! as JSON on stdout — the minimum surface for `cargo idealyst mcp
//! --json-catalog` to wire up.
//!
//! See `docs/mcp-catalog-spec.md` for the full plan. Phase 1 emits
//! the flat catalog with `composes` edges as bare idents. Phase 2
//! resolves those idents into fully-qualified [`EntryRef`]s via the
//! [`resolve`] module — same-module-first, then closest ancestor, then
//! workspace-wide (spec §6).

pub use inventory;

pub mod resolve;
pub use resolve::{
    BuildFromJsonError, EdgeStatus, EntityKind, EntityMatch, EntryRef, ResolvedCatalog,
    ResolvedEdge,
};

pub mod slice;
pub use slice::{CatalogSlice, LeakFromJson};

mod primitives;
mod utilities;
mod macros;
mod states;
mod guides;
mod scopes;
mod sdks;

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

/// A `#[component]` the author tagged `external` — i.e. it should be
/// emitted as a framework-agnostic Web Component by `idealyst export`.
///
/// Registered only when the `catalog` feature is on (the export
/// pipeline builds the project with that feature, exactly like
/// `idealyst docs` / `idealyst mcp`). The prop *fields* themselves are
/// NOT duplicated here — they come from the component's props struct
/// via [`PropsSchemaEntry`] (which `#[derive(IdealystSchema)]` emits);
/// join on [`props_short_name`](Self::props_short_name).
#[derive(Debug)]
pub struct ExternalEntry {
    /// The component function's bare identifier — e.g. `"Greeter"`.
    pub name: &'static str,
    /// `module_path!()` at the registration site.
    pub module_path: &'static str,
    /// Short name of the component's props struct (`"GreeterProps"`) —
    /// the join key to [`PropsSchemaEntry`]/[`TypeEntry`] for the prop
    /// fields. Empty for a zero-prop component.
    pub props_short_name: &'static str,
    /// The custom-element tag the export emits, e.g. `"idl-greeter"`.
    /// Defaults to `idl-<kebab(name)>`; overridable via
    /// `#[component(external(tag = "..."))]`.
    pub tag: &'static str,
}

inventory::collect!(ComponentEntry);
inventory::collect!(ExternalEntry);
inventory::collect!(PropsSchemaEntry);
inventory::collect!(ToolEntry);
inventory::collect!(PrimitiveEntry);
inventory::collect!(UtilityEntry);
inventory::collect!(MacroEntry);
inventory::collect!(StateEntry);
inventory::collect!(GuideEntry);
inventory::collect!(MethodEntry);
inventory::collect!(AnimationEntry);
inventory::collect!(TypeEntry);
inventory::collect!(RecipeEntry);
inventory::collect!(ScopeEntry);
inventory::collect!(SdkEntry);
inventory::collect!(IconSetEntry);

/// A built-in framework primitive — the leaf nodes of the `ui!` /
/// `jsx!` grammar (`View`, `Text`, `Button`, `ScrollView`, ...).
///
/// **Locked**: only `mcp-catalog`'s own `primitives.rs` table can
/// construct one. The `_seal` field is private, so external crates
/// can't write `PrimitiveEntry { name: ..., _seal: () }` — `#[non_exhaustive]`
/// further blocks struct-literal construction at any call site. The
/// open extension point for third-party "primitive-like" things is
/// `Element::External` + the per-backend `ExternalRegistry`, not
/// this slice.
///
/// Read-only consumption: every metadata field is `pub`, so callers
/// iterate the inventory slice and project entries normally.
#[derive(Debug)]
#[non_exhaustive]
pub struct PrimitiveEntry {
    /// snake_case identifier — the stable catalog key for this
    /// primitive (the snake_case form of the `pascal_name` tag).
    /// Stable across versions.
    pub name: &'static str,
    /// PascalCase tag — what authors actually type inside `ui!` /
    /// `jsx!`. Mirrors the variant ident on `Element`.
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
/// `runtime_core` (or a sibling) that authors call from regular
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

/// A framework authoring macro — `signal!`, `effect!`, `ui!`,
/// `#[component]`, `stylesheet!`, etc. These are the *verbs* of
/// writing an idealyst app: the `macro_rules!` and proc-macros an
/// author types directly, as opposed to the [`UtilityEntry`] free
/// functions they call. The slice exists because the original macro
/// surface was undocumented in the catalog — agents reached for the
/// lower-level primitive (`Effect::new`) because nothing told them
/// `effect!` existed or what it expanded to.
///
/// **Locked** — same `_seal: ()` pattern as [`PrimitiveEntry`]. The
/// macro surface ships with the framework version; third-party crates
/// extend behavior through `#[idealyst_tool]` / `Element::External`,
/// not by registering new entries here.
#[derive(Debug)]
#[non_exhaustive]
pub struct MacroEntry {
    /// Bare identifier with no `!` / `#[…]` decoration — `"effect"`,
    /// `"ui"`, `"component"`. Doubles as the `describe_macro` lookup
    /// key.
    pub name: &'static str,
    /// Canonical call syntax as an author writes it: `"effect!({ … })"`,
    /// `"ui! { … }"`, `"#[component]"`, `"#[derive(IdealystSchema)]"`.
    /// Carries the `!` / attribute shape the bare `name` drops.
    pub invocation: &'static str,
    pub kind: MacroKind,
    /// Crate the macro is exported from — `"runtime_core"` for the
    /// `macro_rules!` set, `"runtime_macros"` for the proc-macros.
    pub module_path: &'static str,
    pub docs: &'static str,
    /// One-line sketch of what the macro expands to, so a reader sees
    /// the primitive underneath — e.g. `effect!` →
    /// `"let _effect = Effect::new(move || { … });"`. Empty when the
    /// expansion is codegen too large to usefully summarize (`ui!`,
    /// `jsx!`, `stylesheet!`).
    pub expansion: &'static str,
    #[doc(hidden)]
    pub _seal: (),
}

/// Classification for [`MacroEntry`] — the role a macro plays in
/// authoring. Mirrors the `as_str()` lowercase tags used in the
/// catalog JSON and the `list_macros` filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacroKind {
    /// State + reactivity: `signal!`, `effect!`, `rx!`, `bind!`.
    Reactive,
    /// Element-tree construction: `ui!`, `jsx!`, `text_fmt!`, `lazy!`,
    /// `node_ref!`, `children!`.
    Markup,
    /// Motion: `animated!`, `animate_at!`, `timeline!`.
    Animation,
    /// Typed styles: `stylesheet!`.
    Styling,
    /// Component declaration: `#[component]`.
    Component,
    /// Documentation + introspection tooling: `recipe!`, `doc_scope!`,
    /// `#[derive(IdealystSchema)]`, `#[idealyst_tool]`.
    Catalog,
}

impl MacroKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Reactive => "reactive",
            Self::Markup => "markup",
            Self::Animation => "animation",
            Self::Styling => "styling",
            Self::Component => "component",
            Self::Catalog => "catalog",
        }
    }
}

/// A framework-defined interaction state — `hovered`, `pressed`,
/// `focused`, `disabled`. These are the four well-known states the
/// `stylesheet!` macro accepts in `state foo(theme) { … }` arms
/// (see [`runtime_macros::stylesheet`]).
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
/// block (see [`runtime_macros::methods_block`]). Methods are
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

/// A usage **recipe** — a compile-checked example, captured by the
/// `recipe!(Target, fn ...)` macro. The recipe's fn is real code built
/// against the target's live API, so if it changes and the recipe isn't
/// updated it FAILS TO COMPILE (whenever the catalog is built). That
/// makes recipes self-verifying docs + trustworthy LLM context: "here is
/// how to use this", proven to still type-check.
///
/// The target is **any** documentable entity — a component, a utility, a
/// free function, a type — not just a component (phase 3 generalization,
/// see `docs/catalog-scopes-spec.md` §6). Consumers join a recipe to an
/// entity by name via [`ResolvedCatalog::recipes_for`].
///
/// Open slice — anyone can write recipes for anything, anywhere (the
/// macro is location-agnostic and emits nothing without the `catalog`
/// feature, so recipes cost zero in production).
#[derive(Debug)]
pub struct RecipeEntry {
    /// The recipe fn's name, e.g. `"select_basic"`.
    pub name: &'static str,
    /// The entity this recipe primarily demonstrates — the `recipe!`
    /// first argument's last path segment, e.g. `"Select"` or
    /// `"parse_color"`. May name a component, utility, function, or type.
    pub target: &'static str,
    /// `module_path!()` at the recipe site.
    pub module_path: &'static str,
    /// `file!()` at the recipe site.
    pub file: &'static str,
    /// `line!()` at the recipe site.
    pub line: u32,
    /// Concatenated `///` docs on the recipe fn — prose context for
    /// humans and the LLM. Empty when undocumented.
    pub docs: &'static str,
    /// The recipe's source code (the whole fn), formatted for display.
    /// This is the copy-pasteable, compile-verified example shown in
    /// docs and handed to the LLM.
    pub source: &'static str,
    /// Every component the recipe's `ui!` / `jsx!` body references (the
    /// composes walk). Lets `describe_component` surface recipes that
    /// merely *use* a component, not just the primary one.
    pub uses: &'static [&'static str],
}

/// A documentation **scope** — a flat label that groups documentable
/// entities (components, utilities, …), declared with the `doc_scope!`
/// item macro. Every entity is assigned to the nearest enclosing scope
/// by module proximity (see [`ResolvedCatalog::scope_for`]).
///
/// Scopes are **flat** — there is no parent/child hierarchy. Granularity
/// comes from module nesting (a scope at `crate::ui::inputs` is "nearer"
/// than one at `crate::ui`), not from an explicit tree. Open slice: any
/// crate declares its own scopes. Identity is the
/// [`slug`](ScopeEntry::slug), *independent of module location* so
/// moving/renaming a module never reorganizes the catalog or breaks
/// saved references. See `docs/catalog-scopes-spec.md` §4.1.
#[derive(Debug)]
pub struct ScopeEntry {
    /// Stable, location-independent identity + lookup key. Defaults to
    /// the `doc_scope!` marker ident (lowercased); overridable via
    /// `slug = "..."`.
    pub slug: &'static str,
    /// Human-facing title for tables-of-contents / doc headings.
    pub title: &'static str,
    /// Prose describing the scope. Empty when none was given.
    pub docs: &'static str,
    /// `module_path!()` at the declaration site — drives the ambient
    /// proximity join in [`ResolvedCatalog::scope_for`]. NOT the
    /// identity (that's [`slug`](ScopeEntry::slug)).
    pub module_path: &'static str,
    /// Display ordering — lowest first.
    pub order: u32,
}

/// An opt-in **SDK crate** — a peripheral capability that ships outside
/// `runtime-core` (networking, persistence, camera, the component
/// library, …) and that an author adds to `Cargo.toml` as a dependency.
///
/// These are invisible to the component/primitive/utility slices because
/// they expose plain Rust functions and types (`net::Client`,
/// `storage::platform_storage()`) or `Element::External` primitives, not
/// `#[component]`s the inventory walker can see. This slice is the
/// discovery surface an agent uses to learn "which crate makes a network
/// request / persists data / renders a map" — backed by the
/// [`sdks`](crate::sdks) guide for the prose.
///
/// **Locked** — same `_seal: ()` pattern as [`PrimitiveEntry`]. The SDK
/// roster ships with the framework version; a third-party crate that
/// wants to be discoverable documents itself, it doesn't register here.
#[derive(Debug)]
#[non_exhaustive]
pub struct SdkEntry {
    /// Crate name as written in `Cargo.toml` (no `idealyst-` prefix) —
    /// `"net"`, `"storage"`, `"idea-ui"`. Doubles as the `describe_sdk`
    /// lookup key.
    pub name: &'static str,
    /// One-line summary of what the crate gives you.
    pub summary: &'static str,
    /// The `Cargo.toml` dependency line to add — e.g.
    /// `"net = { workspace = true }"`. A copy-pasteable starting point;
    /// the source (git/rev/path) mirrors the project's `runtime-core`
    /// line.
    pub dep_line: &'static str,
    /// Broad classification for grouping in `list_sdks`.
    pub category: SdkCategory,
    /// Whether the crate's surface is plain functions/types (`Api`) or a
    /// `ui!` primitive wired through `Element::External` (`External`).
    pub kind: SdkKind,
    /// Slug of the guide (this slice's prose home) for cross-reference —
    /// always `"sdks"` today; present so a future per-SDK guide can
    /// override it.
    pub guide: &'static str,
    #[doc(hidden)]
    pub _seal: (),
}

/// Broad classification for [`SdkEntry`] — the capability area.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SdkCategory {
    /// HTTP / sockets / server relay / persistence: `net`, `server`,
    /// `storage`, `credentials`, `files`, `file-export`, `i18n`.
    Data,
    /// Capture / playback / drawing: `camera`, `microphone`,
    /// `screen-recorder`, `media-writer`, `media-stream`, `video`,
    /// `canvas`.
    Media,
    /// UI components / `Element::External` primitives: `idea-ui`,
    /// `idea-theme`, `icons-lucide`, `webview`, `maps`, `svg`,
    /// `markdown`, `codeblock`, `table`, `form`, `toolbar`, `menu`.
    Ui,
    /// Device capabilities that don't fit the above: `biometrics`.
    Device,
}

impl SdkCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Data => "data",
            Self::Media => "media",
            Self::Ui => "ui",
            Self::Device => "device",
        }
    }
}

/// Whether an [`SdkEntry`]'s surface is plain API or a `ui!` primitive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SdkKind {
    /// Plain Rust functions/types called outside `ui!` (`net::Client`).
    Api,
    /// A `ui!` primitive wired through `Element::External`.
    External,
}

impl SdkKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Api => "api",
            Self::External => "external",
        }
    }
}

/// One icon within an [`IconSetEntry`] — the pair an author needs to go
/// from "I want the arrow-right icon" to a compiling `use`.
///
/// `name` is the icon's catalog/display name (the upstream kebab-case
/// name, e.g. `"arrow-right"`) — what you search for. `ident` is the
/// Rust constant it's exposed as (`"ARROW_RIGHT"`), so the paste-ready
/// import is `use {import_path}::{ident};`. The two are carried
/// separately rather than reconstructed from a rule because the
/// kebab→SCREAMING transform has edge cases (leading digits get an `_`
/// prefix; `a-arrow-down` → `A_ARROW_DOWN`) — the icon pack's build
/// script already knows the real ident, so it ships it verbatim.
#[derive(Debug)]
pub struct IconRef {
    pub name: &'static str,
    pub ident: &'static str,
}

/// An icon pack — a crate (`icons-lucide`, …) that exposes a set of
/// named icon `const`s an author drops into the `icon(...)` primitive.
///
/// Icon packs are invisible to the component / primitive slices: they
/// ship plain `IconData` constants, not `#[component]`s or `Element`s
/// the inventory walker sees. This slice is the discovery surface — an
/// agent learns "which packs exist, and what's the import for the X
/// icon" — and the data backing the docs site's icon gallery.
///
/// **Open** — a pack self-registers from its own `build.rs`-generated
/// table (the `icons` field is a `&'static` slice the pack emits), the
/// same way `#[component]` self-registers. A third-party icon crate
/// becomes discoverable by submitting one of these; nothing here is
/// hand-curated in `mcp-catalog`. The registration is feature-gated in
/// the pack (off by default) so normal apps that import three icons
/// don't pay for the whole name table.
#[derive(Debug)]
pub struct IconSetEntry {
    /// Crate name as written in `Cargo.toml` (`"icons-lucide"`).
    /// Doubles as the `describe_icon_set` lookup key.
    pub name: &'static str,
    /// Human-facing pack title — `"Lucide"`.
    pub title: &'static str,
    /// One-paragraph description of the pack.
    pub docs: &'static str,
    /// The `use` path root the `ident`s live under — `"icons_lucide"`.
    /// `use {import_path}::{icon.ident};` is the import for any icon.
    pub import_path: &'static str,
    /// SPDX-ish license id for the icon artwork — `"ISC"` for Lucide.
    pub license: &'static str,
    /// Upstream project homepage, for attribution / browsing.
    pub homepage: &'static str,
    /// Every icon in the pack as a `(name, ident)` pair, sorted by
    /// `name`. The pack's `build.rs` generates this slice. `len()` is
    /// the pack's icon count.
    pub icons: &'static [IconRef],
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

/// Iterate every `#[component(external)]` the build registered.
pub fn externals() -> impl Iterator<Item = &'static ExternalEntry> {
    inventory::iter::<ExternalEntry>()
}

/// Build the export manifest: every `external` component joined to its
/// prop schema. This is the single source `idealyst export` reads — the
/// join lives here (not in the CLI) because this crate already owns the
/// `ExternalEntry` ⇄ `PropsSchemaEntry` relationship.
///
/// Each prop carries its `name`, raw `type_str` (the codegen classifies
/// it into a TS type + a JS conversion), `doc`, and `constraint`. A
/// tagged component whose props struct lacks `#[derive(IdealystSchema)]`
/// surfaces with an empty `props` array — the CLI warns on that.
pub fn external_components_json() -> serde_json::Value {
    let mut components: Vec<serde_json::Value> = externals()
        .map(|e| {
            let props: Vec<serde_json::Value> = lookup_schema(e.props_short_name)
                .map(|s| {
                    s.fields
                        .iter()
                        .map(|f| {
                            serde_json::json!({
                                "name": f.name,
                                "type_str": f.type_str,
                                "doc": f.doc,
                                "constraint": f.constraint,
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            serde_json::json!({
                "name": e.name,
                "module_path": e.module_path,
                "props_struct": e.props_short_name,
                "tag": e.tag,
                "props": props,
            })
        })
        .collect();
    // Stable order so generated output diffs minimally.
    components.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    serde_json::json!({ "external_components": components })
}

/// Print the export manifest as pretty JSON on stdout — the entry
/// point the ephemeral export wrapper invokes.
pub fn dump_external_components_json() {
    println!(
        "{}",
        serde_json::to_string_pretty(&external_components_json()).unwrap()
    );
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

/// Iterate every [`RecipeEntry`] captured by `recipe!(...)`.
pub fn recipes() -> impl Iterator<Item = &'static RecipeEntry> {
    inventory::iter::<RecipeEntry>()
}

/// Iterate every [`TypeEntry`] (struct or enum) registered via
/// `#[derive(IdealystSchema)]`.
pub fn types() -> impl Iterator<Item = &'static TypeEntry> {
    inventory::iter::<TypeEntry>()
}

/// Iterate every [`ScopeEntry`] declared via `doc_scope!`.
pub fn scopes() -> impl Iterator<Item = &'static ScopeEntry> {
    inventory::iter::<ScopeEntry>()
}

/// Look up a scope by its [`slug`](ScopeEntry::slug).
pub fn lookup_scope(slug: &str) -> Option<&'static ScopeEntry> {
    scopes().find(|s| s.slug == slug)
}

/// Iterate every [`SdkEntry`] in the locked opt-in-crate table.
pub fn sdks() -> impl Iterator<Item = &'static SdkEntry> {
    inventory::iter::<SdkEntry>()
}

/// Look up an SDK crate by its `name` (the `Cargo.toml` crate name).
pub fn lookup_sdk(name: &str) -> Option<&'static SdkEntry> {
    sdks().find(|s| s.name == name)
}

/// Iterate every [`IconSetEntry`] an icon pack self-registered.
pub fn icon_sets() -> impl Iterator<Item = &'static IconSetEntry> {
    inventory::iter::<IconSetEntry>()
}

/// Look up an icon pack by its crate `name` (`"icons-lucide"`).
pub fn lookup_icon_set(name: &str) -> Option<&'static IconSetEntry> {
    icon_sets().find(|s| s.name == name)
}

/// Look up a primitive by its `name` (snake_case) or `pascal_name`.
pub fn lookup_primitive(needle: &str) -> Option<&'static PrimitiveEntry> {
    primitives().find(|p| p.name == needle || p.pascal_name == needle)
}

/// Look up a utility by its bare ident.
pub fn lookup_utility(needle: &str) -> Option<&'static UtilityEntry> {
    utilities().find(|u| u.name == needle)
}

/// Iterate every [`MacroEntry`] in the locked authoring-macro table.
pub fn macros() -> impl Iterator<Item = &'static MacroEntry> {
    inventory::iter::<MacroEntry>()
}

/// Look up an authoring macro by bare `name` (no `!`), tolerating a
/// trailing `!` the caller may have typed (`"effect"` and `"effect!"`
/// both resolve).
pub fn lookup_macro(needle: &str) -> Option<&'static MacroEntry> {
    let trimmed = needle.trim_end_matches('!');
    macros().find(|m| m.name == trimmed)
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
/// utilities, macros, states, guides, methods, animations, types, and
/// tools.
/// Entries within each slice are sorted by a stable key
/// (`module_path::name`, slug, etc.) so JSON diffs are minimal.
///
/// Each slice serializes through its [`CatalogSlice`] impl (see
/// `slice.rs`); this function just names the key → type mapping.
pub fn catalog_json() -> serde_json::Value {
    use slice::slice_array;
    serde_json::json!({
        "catalog_version": 2,
        "components": slice_array::<ComponentEntry>(),
        "primitives": slice_array::<PrimitiveEntry>(),
        "utilities": slice_array::<UtilityEntry>(),
        "macros": slice_array::<MacroEntry>(),
        "states": slice_array::<StateEntry>(),
        "guides": slice_array::<GuideEntry>(),
        "methods": slice_array::<MethodEntry>(),
        "animations": slice_array::<AnimationEntry>(),
        "types": slice_array::<TypeEntry>(),
        "tools": slice_array::<ToolEntry>(),
        "recipes": slice_array::<RecipeEntry>(),
        "scopes": slice_array::<ScopeEntry>(),
        "sdks": slice_array::<SdkEntry>(),
        "icon_sets": slice_array::<IconSetEntry>(),
    })
}

/// Print the catalog as pretty-formatted JSON on stdout. The shape
/// `cargo idealyst mcp --json-catalog` will eventually call.
pub fn dump_catalog_json() {
    let json = catalog_json();
    println!("{}", serde_json::to_string_pretty(&json).unwrap());
}
