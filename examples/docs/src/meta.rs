//! Structured documentation metadata.
//!
//! Each page in the docs site is one Rust module containing a
//! single `docs! { ... }` invocation. The macro (defined in the
//! sibling `docs-macro` crate, imported by the docs site) emits
//! two items into the calling module:
//!
//! 1. `pub fn page() -> Primitive` — the renderable screen.
//! 2. `pub static PAGE_META: PageMeta` — the structured metadata,
//!    consumable by MCP servers, text exporters, search indexers,
//!    and other introspection tools.
//!
//! This module defines the types those emissions reference. The
//! macro hard-codes `::docs::meta::*` paths because it's
//! opinionated to this docs site — not a reusable framework
//! concern.
//!
//! See [`docs-macro-design.md`](../docs-content-plan/docs-macro-design.md)
//! for the input grammar and design rationale.

// =============================================================================
// PageMeta — top-level page descriptor
// =============================================================================

/// Top-level structured form of one documentation page. Stored as a
/// `pub static` per page; the docs registry walks every page's
/// `PAGE_META` to power MCP queries, search indexing, and
/// cross-reference resolution.
///
/// Every field is `&'static` so the whole structure lives in `.rodata`
/// — no allocation, no init code.
#[derive(Debug, Clone, Copy)]
pub struct PageMeta {
    /// Stable identifier. Used in cross-references and as the URL
    /// slug. Must be unique across the docs.
    pub slug: &'static str,

    /// Human-readable title shown in the page header.
    pub title: &'static str,

    /// Category for routing and MCP tool surface.
    pub category: PageCategory,

    /// One-sentence summary shown under the title; also used for
    /// search/MCP ranking.
    pub description: Option<&'static str>,

    /// Slugs of related pages. Used to render "see also" footers
    /// and to drive concept-based discovery.
    pub related: &'static [&'static str],

    /// Vocabulary terms this page is the **authoritative source**
    /// for. Many pages mention "signal" — only Reactivity teaches
    /// what one is, so only Reactivity lists [`DocConcept::Signal`]
    /// here.
    pub concepts: &'static [DocConcept],

    /// Ordered sections.
    pub sections: &'static [SectionMeta],
}

// =============================================================================
// PageCategory
// =============================================================================

/// Page category. Drives sidebar grouping, MCP tool surface, and
/// search defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageCategory {
    /// The Overview page. Always first in the sidebar.
    Overview,
    /// Foundational concepts (Primitives, Reactivity, Styles,
    /// Components). Order within the category matches declaration
    /// order in the registry.
    Foundation,
    /// Tooling pages (Robot, Dev tools).
    Tools,
    /// Reference material (Backends, Refs, Lists, Icons, Navigation,
    /// Getting Started).
    Reference,
    /// Deeper-cutting material that goes past the everyday surface:
    /// internal contracts, extension points, the public API of a
    /// specific backend, etc.
    Advanced,
    /// Cookbook recipes. Default-hidden from `list_doc_pages`; the
    /// MCP server exposes a separate `list_cookbook_recipes` tool.
    Cookbook,
}

// =============================================================================
// SectionMeta
// =============================================================================

/// A heading-delimited section within a page.
#[derive(Debug, Clone, Copy)]
pub struct SectionMeta {
    /// Heading text, as the author wrote it.
    pub heading: &'static str,
    /// Slug for anchored navigation. Derived from `heading` by
    /// kebab-casing if the author doesn't supply one explicitly.
    pub slug: &'static str,
    /// Blocks inside this section, in order.
    pub blocks: &'static [BlockMeta],
}

// =============================================================================
// BlockMeta — kinds of content blocks
// =============================================================================

/// Content blocks. The structured equivalent of "paragraph", "code
/// block", "list", etc.
#[derive(Debug, Clone, Copy)]
pub enum BlockMeta {
    /// A paragraph — a sequence of spans (plain text + inline code +
    /// cross-reference links).
    Paragraph(&'static [Span]),

    /// A fenced code block.
    Code {
        /// Language identifier (`"rust"`, `"bash"`, `"json"`, etc.).
        language: &'static str,
        /// Source text. Indentation is trimmed by the macro at
        /// authoring time.
        source: &'static str,
    },

    /// A bulleted list. Each item is its own paragraph (sequence of
    /// spans).
    List(&'static [&'static [Span]]),

    /// A "From X." comparison callout — content shown to readers
    /// who selected the corresponding framework in the comparison
    /// picker.
    Comparison {
        from: ComparisonFramework,
        blocks: &'static [BlockMeta],
    },

    /// An embedded interactive demo. The `name` is the demo's stable
    /// identifier (the function name passed to `demo(...)`); the
    /// actual component invocation is emitted on the UI side and
    /// isn't part of the metadata.
    Demo {
        name: &'static str,
        description: Option<&'static str>,
    },

    /// A callout (info / tip / warning).
    Note {
        kind: NoteKind,
        blocks: &'static [BlockMeta],
    },
}

// =============================================================================
// Span — inline text/code/link
// =============================================================================

/// Inline spans inside a paragraph.
#[derive(Debug, Clone, Copy)]
pub enum Span {
    /// Plain text.
    Text(&'static str),
    /// Inline code. Rendered in a monospace span.
    Code(&'static str),
    /// Cross-reference link. `target` is `"page-slug"` or
    /// `"page-slug#section-slug"`.
    Link {
        target: &'static str,
        text: &'static str,
    },
}

// =============================================================================
// ComparisonFramework
// =============================================================================

/// Frameworks the comparison-callout system understands.
///
/// Adding a new framework = adding a variant here. The site's
/// comparison-picker tab bar reads this set to know what tabs to
/// render.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComparisonFramework {
    React,
    Solid,
    SvelteFive,
    VueThree,
}

impl ComparisonFramework {
    /// Display name shown in the comparison-picker tab bar.
    pub const fn display(self) -> &'static str {
        match self {
            ComparisonFramework::React => "React",
            ComparisonFramework::Solid => "Solid",
            ComparisonFramework::SvelteFive => "Svelte 5",
            ComparisonFramework::VueThree => "Vue 3",
        }
    }

    /// Stable slug for URLs and persisted selection state.
    pub const fn slug(self) -> &'static str {
        match self {
            ComparisonFramework::React => "react",
            ComparisonFramework::Solid => "solid",
            ComparisonFramework::SvelteFive => "svelte-5",
            ComparisonFramework::VueThree => "vue-3",
        }
    }
}

// =============================================================================
// NoteKind
// =============================================================================

/// Callout kind. Drives the icon, color, and accessibility role of
/// `BlockMeta::Note`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteKind {
    /// Informational; neutral styling.
    Info,
    /// Tip / suggestion; affirmative styling.
    Tip,
    /// Warning; gets attention.
    Warning,
}

// =============================================================================
// DocConcept — the framework's named vocabulary
// =============================================================================

/// Every named concept the framework documents.
///
/// A page lists each concept it is the **authoritative source** for
/// in its [`PageMeta::concepts`] field. Pages may reference concepts
/// they don't *own* via inline `link(...)` spans — being in someone
/// else's `concepts` list is what makes a concept owned by that
/// page.
///
/// Adding a new concept = adding a variant here. The set of variants
/// is the framework's canonical vocabulary list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DocConcept {
    // ---- Reactivity ----
    Signal,
    Effect,
    Scope,
    TrackedContext,
    Derived,
    Action,
    Untrack,

    // ---- Components ----
    Component,
    /// The `methods!` block surface.
    ComponentMethods,
    Bindable,
    Bound,
    Props,
    /// `#[component(default(...))]` per-prop defaults.
    Defaults,

    // ---- UI DSLs ----
    /// The `ui!` macro.
    UiMacro,
    /// The `jsx!` macro.
    JsxMacro,

    // ---- Primitives ----
    Primitive,
    /// View / ScrollView / Pressable.
    Container,
    /// Text / Image / Icon / Video / WebView.
    Content,
    /// Button / TextInput / Toggle / Slider.
    Input,
    /// When / Switch (the primitive) / Repeat.
    ReactiveControlFlow,

    // ---- Styling ----
    Stylesheet,
    Theme,
    Token,
    Variant,
    Override,
    /// hovered / pressed / focused / disabled.
    StyleState,
    Transition,

    // ---- Refs and handles ----
    Ref,
    /// Primitive handles plus user-component handles.
    Handle,

    // ---- Navigation ----
    Route,
    RouteParams,
    Screen,
    StackNavigator,
    TabNavigator,
    DrawerNavigator,
    Link,
    AmbientNavigator,
    MountPolicy,

    // ---- Lists ----
    Virtualizer,
    FlatList,
    ItemKey,
    ItemSize,

    // ---- Icons ----
    IconData,
    IconRegistry,
    StrokeAnimation,

    // ---- Overlays / animation ----
    Overlay,
    AnchoredOverlay,
    Presence,

    // ---- Backends ----
    Backend,
    RuntimeBackend,
    GeneratorBackend,
    LazySlotCapture,

    // ---- Dev tools ----
    Cli,
    HotReload,
    /// App-as-server.
    Aas,
    WireProtocol,
    McpServer,

    // ---- Robot ----
    Robot,
    TestId,

    // ---- Architecture / cross-cutting ----
    /// The "you write one crate; backends render it" split.
    AppBackendSplit,
    BuildCache,
    SafeArea,
    /// This very macro.
    DocsMacro,
}

impl DocConcept {
    /// Human-readable display name. Used in chips, search results,
    /// and cross-reference UIs.
    pub const fn display(self) -> &'static str {
        match self {
            DocConcept::Signal => "Signal",
            DocConcept::Effect => "Effect",
            DocConcept::Scope => "Scope",
            DocConcept::TrackedContext => "Tracked context",
            DocConcept::Derived => "Derived",
            DocConcept::Action => "Action",
            DocConcept::Untrack => "untrack",

            DocConcept::Component => "Component",
            DocConcept::ComponentMethods => "methods!",
            DocConcept::Bindable => "Bindable",
            DocConcept::Bound => "Bound",
            DocConcept::Props => "Props",
            DocConcept::Defaults => "Component defaults",

            DocConcept::UiMacro => "ui!",
            DocConcept::JsxMacro => "jsx!",

            DocConcept::Primitive => "Primitive",
            DocConcept::Container => "Container primitives",
            DocConcept::Content => "Content primitives",
            DocConcept::Input => "Input primitives",
            DocConcept::ReactiveControlFlow => "Reactive control flow",

            DocConcept::Stylesheet => "Stylesheet",
            DocConcept::Theme => "Theme",
            DocConcept::Token => "Token",
            DocConcept::Variant => "Variant",
            DocConcept::Override => "Override",
            DocConcept::StyleState => "Style state",
            DocConcept::Transition => "Transition",

            DocConcept::Ref => "Ref",
            DocConcept::Handle => "Handle",

            DocConcept::Route => "Route",
            DocConcept::RouteParams => "RouteParams",
            DocConcept::Screen => "Screen",
            DocConcept::StackNavigator => "Stack navigator",
            DocConcept::TabNavigator => "Tab navigator",
            DocConcept::DrawerNavigator => "Drawer navigator",
            DocConcept::Link => "Link",
            DocConcept::AmbientNavigator => "Ambient navigator",
            DocConcept::MountPolicy => "Mount policy",

            DocConcept::Virtualizer => "Virtualizer",
            DocConcept::FlatList => "flat_list",
            DocConcept::ItemKey => "ItemKey",
            DocConcept::ItemSize => "ItemSize",

            DocConcept::IconData => "IconData",
            DocConcept::IconRegistry => "Icon registry",
            DocConcept::StrokeAnimation => "Stroke animation",

            DocConcept::Overlay => "Overlay",
            DocConcept::AnchoredOverlay => "AnchoredOverlay",
            DocConcept::Presence => "Presence",

            DocConcept::Backend => "Backend",
            DocConcept::RuntimeBackend => "Runtime backend",
            DocConcept::GeneratorBackend => "Generator backend",
            DocConcept::LazySlotCapture => "Lazy slot capture",

            DocConcept::Cli => "CLI",
            DocConcept::HotReload => "Hot reload",
            DocConcept::Aas => "AAS (app-as-server)",
            DocConcept::WireProtocol => "Wire protocol",
            DocConcept::McpServer => "MCP server",

            DocConcept::Robot => "Robot",
            DocConcept::TestId => "test_id",

            DocConcept::AppBackendSplit => "App ↔ Backend split",
            DocConcept::BuildCache => "Build cache",
            DocConcept::SafeArea => "Safe area",
            DocConcept::DocsMacro => "docs! macro",
        }
    }

    /// Kebab-cased slug for URLs and search keys. Stable.
    pub const fn slug(self) -> &'static str {
        match self {
            DocConcept::Signal => "signal",
            DocConcept::Effect => "effect",
            DocConcept::Scope => "scope",
            DocConcept::TrackedContext => "tracked-context",
            DocConcept::Derived => "derived",
            DocConcept::Action => "action",
            DocConcept::Untrack => "untrack",

            DocConcept::Component => "component",
            DocConcept::ComponentMethods => "component-methods",
            DocConcept::Bindable => "bindable",
            DocConcept::Bound => "bound",
            DocConcept::Props => "props",
            DocConcept::Defaults => "defaults",

            DocConcept::UiMacro => "ui-macro",
            DocConcept::JsxMacro => "jsx-macro",

            DocConcept::Primitive => "primitive",
            DocConcept::Container => "container",
            DocConcept::Content => "content",
            DocConcept::Input => "input",
            DocConcept::ReactiveControlFlow => "reactive-control-flow",

            DocConcept::Stylesheet => "stylesheet",
            DocConcept::Theme => "theme",
            DocConcept::Token => "token",
            DocConcept::Variant => "variant",
            DocConcept::Override => "override",
            DocConcept::StyleState => "style-state",
            DocConcept::Transition => "transition",

            DocConcept::Ref => "ref",
            DocConcept::Handle => "handle",

            DocConcept::Route => "route",
            DocConcept::RouteParams => "route-params",
            DocConcept::Screen => "screen",
            DocConcept::StackNavigator => "stack-navigator",
            DocConcept::TabNavigator => "tab-navigator",
            DocConcept::DrawerNavigator => "drawer-navigator",
            DocConcept::Link => "link",
            DocConcept::AmbientNavigator => "ambient-navigator",
            DocConcept::MountPolicy => "mount-policy",

            DocConcept::Virtualizer => "virtualizer",
            DocConcept::FlatList => "flat-list",
            DocConcept::ItemKey => "item-key",
            DocConcept::ItemSize => "item-size",

            DocConcept::IconData => "icon-data",
            DocConcept::IconRegistry => "icon-registry",
            DocConcept::StrokeAnimation => "stroke-animation",

            DocConcept::Overlay => "overlay",
            DocConcept::AnchoredOverlay => "anchored-overlay",
            DocConcept::Presence => "presence",

            DocConcept::Backend => "backend",
            DocConcept::RuntimeBackend => "runtime-backend",
            DocConcept::GeneratorBackend => "generator-backend",
            DocConcept::LazySlotCapture => "lazy-slot-capture",

            DocConcept::Cli => "cli",
            DocConcept::HotReload => "hot-reload",
            DocConcept::Aas => "aas",
            DocConcept::WireProtocol => "wire-protocol",
            DocConcept::McpServer => "mcp-server",

            DocConcept::Robot => "robot",
            DocConcept::TestId => "test-id",

            DocConcept::AppBackendSplit => "app-backend-split",
            DocConcept::BuildCache => "build-cache",
            DocConcept::SafeArea => "safe-area",
            DocConcept::DocsMacro => "docs-macro",
        }
    }
}

// =============================================================================
// Registry helpers
// =============================================================================

/// Find a page by slug in a static registry slice. Convenience for
/// docs-site code that wants to resolve cross-references.
pub fn find_page<'a>(
    registry: &'a [&'static PageMeta],
    slug: &str,
) -> Option<&'a &'static PageMeta> {
    registry.iter().find(|p| p.slug == slug)
}

/// Iterate over pages in a category. Cookbook recipes default-hidden
/// from MCP `list_doc_pages` would be excluded by filtering the
/// result here.
pub fn pages_in_category<'a>(
    registry: &'a [&'static PageMeta],
    cat: PageCategory,
) -> impl Iterator<Item = &'a &'static PageMeta> {
    registry.iter().filter(move |p| p.category == cat)
}

/// Pages where the given concept appears in `concepts` — i.e. pages
/// that are the authoritative source for the concept. Drives the MCP
/// `pages_about` reverse-index tool.
pub fn pages_about<'a>(
    registry: &'a [&'static PageMeta],
    concept: DocConcept,
) -> impl Iterator<Item = &'a &'static PageMeta> {
    registry.iter().filter(move |p| p.concepts.contains(&concept))
}
