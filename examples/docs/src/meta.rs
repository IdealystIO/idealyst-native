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
    /// `framework_core::mount(backend, app_fn)` — the framework's
    /// entry point. Opens the root reactive scope, runs the user's
    /// tree constructor inside it, hands the result to the build
    /// walker. The lifetime boundary that turns `effect!` /
    /// `signal!` declared at the top of `app()` from leaks into
    /// owned arena slots cleaned up on `Owner` drop.
    Mount,
    TrackedContext,
    Derived,
    Action,
    Untrack,
    /// Cached derived value: `memo(|| expr)`. Recomputes only when its
    /// dependencies change; readers subscribe to the cache, not the
    /// underlying computation. Lives next to Effect/Derived in the
    /// reactivity vocabulary.
    Memo,
    /// `on_cleanup(callback)` — registers a callback that fires when
    /// the surrounding Effect / scope drops. The cleanup hook for
    /// resources (timers, sockets, native handles) created during a
    /// reactive run.
    OnCleanup,
    /// `reducer(initial, |state, action| ...)` — action-driven state.
    /// Returns a read-only signal + a dispatch function. Pairs with
    /// `Action` for round-tripping through generator backends.
    Reducer,
    /// `resource(deps, async closure)` — async data as a reactive
    /// primitive. Re-fetches when its deps change, exposes
    /// `data`/`error`/`loading`, supports cancellation. Feature-gated
    /// behind `async-driver`.
    Resource,
    /// `provide(value)` / `inject::<T>()` — context propagation. The
    /// "closest provider" model React introduced, adapted for
    /// fine-grained reactivity.
    Context,

    // ---- Reactive text bindings (web fast path) ----
    /// `TextSource::JsBinding` + `JsBindingSpec` — the structured
    /// reactive-text source variant. Carries `signal_ids`,
    /// `template_parts`, `initial_values`, and a `compute_fallback`
    /// closure. Backends that opt into JS-side bindings (web)
    /// process this variant without installing a per-leaf Rust
    /// Effect; non-opting backends use `compute_fallback` via the
    /// normal `Bound` Effect path. Authoritative explainer:
    /// `reactive-text-bindings` page.
    JsBinding,
    /// The `text_fmt!("template", args..)` proc macro. Sugar for
    /// constructing a `TextSource::JsBinding` from a format-style
    /// template + a mix of captured exprs and `bind!(signal)`
    /// args.
    TextFmtMacro,
    /// The `bind!(expr)` sentinel — marks a `text_fmt!` arg as a
    /// reactive signal. Has no behavior outside `text_fmt!`; the
    /// proc macro recognizes it at expansion time. Calling
    /// `bind!` standalone errors with `compile_error!`.
    BindSentinel,
    /// `WebBackend::register_signal_for_js(sid, stringifier)` —
    /// the one-call-per-signal setup that wires a signal's writes
    /// to the JS-side reactive layer via the framework's
    /// `signal_js_notifier` slot. Once registered, `Signal::set`
    /// ships value changes across the wasm→JS boundary for the
    /// backend's binding registry to fan out internally.
    RegisterSignalForJs,

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
    /// A user-space pattern of bundling tokens for app-wide swap.
    /// NOT a framework primitive — `framework-core` only ships
    /// `Tokenized<T>` + the token registry. The authoritative
    /// explainer is `building-a-theme-system` (Advanced).
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

    // ---- Floating UI / animation ----
    /// `Primitive::Portal` — the framework's one render-elsewhere
    /// primitive. Authoritative explainer: `portal` page.
    Portal,
    /// `overlay()` composition. Lowers to `Primitive::Portal` with a
    /// viewport target + backdrop child. Not a primitive itself.
    Overlay,
    /// `anchored_overlay()` composition. Lowers to `Primitive::Portal`
    /// with an anchor target. Not a primitive itself.
    AnchoredOverlay,
    Presence,

    // ---- Animation system ----
    /// `AnimatedValue<T>` — the user-facing value handle that drives
    /// per-frame animation. Authoritative explainer: `animation` page.
    AnimatedValue,
    /// `Animator<T>` — the per-frame motion source trait. One animator
    /// drives one value at a time.
    Animator,
    /// `AnimatorFactory<T>` — author-side builder that constructs an
    /// `Animator` given the value handle's current state. The seam
    /// that enables velocity-preserving handoff.
    AnimatorFactory,
    /// `TweenTo` — duration + curve interpolation factory.
    Tween,
    /// `SpringTo` — damped harmonic oscillator factory. Inherits
    /// current velocity on attach.
    Spring,
    /// `DecayFrom` — velocity-driven exponential settle factory. The
    /// flick/toss/fling primitive.
    Decay,
    /// `KeyframesTo` — multi-stop waypoint animation with per-segment
    /// or shared curves.
    Keyframes,
    /// `LoopFactory` + `Repeat` — replay an inner factory N times or
    /// forever.
    AnimationLoop,
    /// `SequenceFactory.then(...)` — back-to-back animator chaining
    /// with velocity flowing across boundaries.
    AnimationSequence,
    /// `AnimProp` — the cross-backend vocabulary of animatable
    /// properties.
    AnimProp,
    /// `AnimationClock` — per-thread tick registry, idles to zero
    /// per-frame work when no animation is live.
    AnimationClock,
    /// `stagger(values, step, factory_fn)` — per-index delayed
    /// animation across a collection.
    Stagger,

    // ---- Third-party extension ----
    /// `Primitive::External` — the framework's one extension hatch for
    /// third-party primitives. Authoritative explainer:
    /// `third-party-primitives` page.
    External,

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
            DocConcept::Mount => "mount()",
            DocConcept::TrackedContext => "Tracked context",
            DocConcept::Derived => "Derived",
            DocConcept::Action => "Action",
            DocConcept::Untrack => "untrack",
            DocConcept::Memo => "memo",
            DocConcept::OnCleanup => "on_cleanup",
            DocConcept::Reducer => "reducer",
            DocConcept::Resource => "resource",
            DocConcept::Context => "Context",

            DocConcept::JsBinding => "TextSource::JsBinding",
            DocConcept::TextFmtMacro => "text_fmt!",
            DocConcept::BindSentinel => "bind!",
            DocConcept::RegisterSignalForJs => "register_signal_for_js",

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

            DocConcept::Portal => "Portal",
            DocConcept::Overlay => "Overlay (composition)",
            DocConcept::AnchoredOverlay => "AnchoredOverlay (composition)",
            DocConcept::Presence => "Presence",

            DocConcept::AnimatedValue => "AnimatedValue",
            DocConcept::Animator => "Animator",
            DocConcept::AnimatorFactory => "AnimatorFactory",
            DocConcept::Tween => "Tween",
            DocConcept::Spring => "Spring",
            DocConcept::Decay => "Decay",
            DocConcept::Keyframes => "Keyframes",
            DocConcept::AnimationLoop => "Loop",
            DocConcept::AnimationSequence => "Sequence",
            DocConcept::AnimProp => "AnimProp",
            DocConcept::AnimationClock => "Animation clock",
            DocConcept::Stagger => "stagger",

            DocConcept::External => "External primitive",

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
            DocConcept::Mount => "mount",
            DocConcept::TrackedContext => "tracked-context",
            DocConcept::Derived => "derived",
            DocConcept::Action => "action",
            DocConcept::Untrack => "untrack",
            DocConcept::Memo => "memo",
            DocConcept::OnCleanup => "on-cleanup",
            DocConcept::Reducer => "reducer",
            DocConcept::Resource => "resource",
            DocConcept::Context => "context",

            DocConcept::JsBinding => "js-binding",
            DocConcept::TextFmtMacro => "text-fmt-macro",
            DocConcept::BindSentinel => "bind-sentinel",
            DocConcept::RegisterSignalForJs => "register-signal-for-js",

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

            DocConcept::Portal => "portal",
            DocConcept::Overlay => "overlay",
            DocConcept::AnchoredOverlay => "anchored-overlay",
            DocConcept::Presence => "presence",

            DocConcept::AnimatedValue => "animated-value",
            DocConcept::Animator => "animator",
            DocConcept::AnimatorFactory => "animator-factory",
            DocConcept::Tween => "tween",
            DocConcept::Spring => "spring",
            DocConcept::Decay => "decay",
            DocConcept::Keyframes => "keyframes",
            DocConcept::AnimationLoop => "animation-loop",
            DocConcept::AnimationSequence => "animation-sequence",
            DocConcept::AnimProp => "anim-prop",
            DocConcept::AnimationClock => "animation-clock",
            DocConcept::Stagger => "stagger",

            DocConcept::External => "external",

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
