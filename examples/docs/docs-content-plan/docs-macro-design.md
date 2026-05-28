# `docs!` macro design

Concrete sketch of the structured-input macro that replaces hand-built
page components, with the dual goal of generating UI and emitting a
machine-readable metadata blob.

## Goals (recap)

1. **One Rust source per page.** No markdown parser, no JSON authoring.
2. **The macro emits the screen** (a `pub fn page() -> Element` that
   composes the shell's existing `PageHeader` / `Section` / `CodeBlock`
   components).
3. **The macro also emits structured metadata** (a `pub static
   PAGE_META: PageMeta`) that an MCP server (or text export, or a
   search index, or any future consumer) can walk without re-parsing
   anything.
4. **Demos are real function references.** Type-checked at compile
   time; no "missing demo" runtime errors.
5. **Cookbook entries opt out of the default page index** so the MCP
   server doesn't promote recipes as primary documentation.

## The `PageMeta` struct

Everything below is `&'static [T]` so the entire structure lives in
`.rodata` — no allocation at runtime, no init code needed.

```rust
pub struct PageMeta {
    /// Stable identifier. Used in cross-references and as the URL
    /// slug. Must be unique across the docs.
    pub slug: &'static str,

    /// Human-readable title shown in the page header.
    pub title: &'static str,

    /// Category for routing and MCP tool exposure.
    pub category: PageCategory,

    /// One-sentence summary. Shown under the title; also used by
    /// search/MCP for ranking.
    pub description: Option<&'static str>,

    /// Slugs of related pages. Used to render "see also" footers and
    /// to drive concept-based discovery.
    pub related: &'static [&'static str],

    /// Vocabulary terms this page is the **authoritative source** for.
    /// Many pages mention "signal" — only Reactivity teaches what one
    /// is, so only Reactivity lists `DocConcept::Signal` here.
    ///
    /// Drives the MCP server's `pages_about(concept)` reverse-index
    /// queries. The set of variants in `DocConcept` is the framework's
    /// canonical vocabulary list.
    pub concepts: &'static [DocConcept],

    /// Ordered sections.
    pub sections: &'static [SectionMeta],
}

pub enum PageCategory {
    /// The Overview page. Always first.
    Overview,
    /// Foundational concepts (Primitives, Reactivity, Styles,
    /// Components). The order matters here — pages are listed in
    /// declaration order within a category.
    Foundation,
    /// Tooling pages (Robot, Dev tools).
    Tools,
    /// Reference material (Backends, Refs, Lists, Icons, Navigation,
    /// Getting Started).
    Reference,
    /// Cookbook recipes. Default-hidden from the MCP page list; the
    /// MCP server exposes a separate `list_cookbook_recipes` tool.
    Cookbook,
}

pub struct SectionMeta {
    /// Heading text.
    pub heading: &'static str,
    /// Slug for anchored navigation. Derived from `heading` by
    /// kebab-casing if the author doesn't supply one.
    pub slug: &'static str,
    /// Blocks inside this section, in order.
    pub blocks: &'static [BlockMeta],
}

pub enum BlockMeta {
    /// A paragraph: a sequence of spans (plain text + inline code).
    Paragraph(&'static [Span]),

    /// A fenced code block.
    Code {
        language: &'static str,
        source: &'static str,
    },

    /// A bulleted list of items. Each item is its own paragraph.
    List(&'static [&'static [Span]]),

    /// A comparison callout for "From <framework>." readers.
    Comparison {
        from: ComparisonFramework,
        blocks: &'static [BlockMeta],
    },

    /// An embedded interactive demo. The `name` is the demo's stable
    /// identifier; the actual component invocation is emitted on the
    /// UI side of the macro and isn't part of the metadata blob.
    Demo {
        name: &'static str,
        description: Option<&'static str>,
    },

    /// A callout (note, warning, tip).
    Note {
        kind: NoteKind,
        blocks: &'static [BlockMeta],
    },
}

pub enum Span {
    Text(&'static str),
    Code(&'static str),
    /// Cross-reference to another page (or page#section).
    Link {
        target: &'static str,  // "reactivity" or "reactivity#signals"
        text: &'static str,
    },
}

pub enum ComparisonFramework {
    React,
    Solid,
    SvelteFive,
    VueThree,
}

pub enum NoteKind {
    Info,
    Tip,
    Warning,
}

/// Every named concept the framework documents. A page lists each
/// concept it is the authoritative source for in its `concepts`
/// field. Pages may reference concepts they don't *own* via inline
/// `link(...)` spans — being in someone else's `concepts` list is
/// what makes a concept "owned" by that page.
///
/// Adding a new concept = adding a variant here. Compile-time
/// reference-checked from every page's `concepts` field.
pub enum DocConcept {
    // Reactivity
    Signal,
    Effect,
    Scope,
    TrackedContext,
    Derived,
    Action,
    Untrack,

    // Components
    Component,
    ComponentMethods,    // the `methods!` block surface
    Bindable,
    Bound,
    Props,
    Defaults,            // #[component(default(...))]

    // UI DSLs
    UiMacro,             // ui! { ... }
    JsxMacro,            // jsx! { ... }
    InlineSpans,         // the docs! macro's span vocabulary (meta)

    // Primitives
    Element,
    Container,           // View, ScrollView, Pressable
    Content,             // Text, Image, Icon, Video, WebView
    Input,               // Button, TextInput, Toggle, Slider
    ReactiveControlFlow, // When, Switch (the primitive), Repeat

    // Styling
    Stylesheet,
    Theme,
    Token,
    Variant,
    Override,
    StyleState,          // hovered / pressed / focused / disabled
    Transition,

    // Refs and handles
    Ref,
    Handle,              // primitive handles + user-component handles

    // Navigation
    Route,
    RouteParams,
    Screen,
    StackNavigator,
    TabNavigator,
    DrawerNavigator,
    Link,
    AmbientNavigator,
    MountPolicy,

    // Lists
    Virtualizer,
    FlatList,
    ItemKey,
    ItemSize,

    // Icons
    IconData,
    IconRegistry,
    StrokeAnimation,

    // Overlays / animation
    Overlay,
    AnchoredOverlay,
    Presence,

    // Backends
    Backend,
    RuntimeBackend,
    GeneratorBackend,
    LazySlotCapture,

    // Dev tools
    Cli,
    HotReload,
    Aas,                 // app-as-server
    WireProtocol,
    McpServer,

    // Robot
    Robot,
    TestId,

    // Architecture / cross-cutting
    AppBackendSplit,
    BuildCache,
    SafeArea,
    DocsMacro,
}

impl DocConcept {
    /// Human-readable name for display in chips, search results, etc.
    pub const fn display(self) -> &'static str {
        match self {
            DocConcept::Signal => "Signal",
            DocConcept::Effect => "Effect",
            DocConcept::Scope => "Scope",
            DocConcept::TrackedContext => "Tracked context",
            DocConcept::ComponentMethods => "methods!",
            DocConcept::UiMacro => "ui!",
            DocConcept::JsxMacro => "jsx!",
            DocConcept::ReactiveControlFlow => "Reactive control flow",
            DocConcept::StyleState => "Style state",
            DocConcept::AmbientNavigator => "Ambient navigator",
            DocConcept::AppBackendSplit => "App ↔ Backend split",
            DocConcept::Aas => "runtime-server (app-as-server)",
            DocConcept::McpServer => "MCP server",
            DocConcept::TestId => "test_id",
            // ... etc., one arm per variant
            _ => stringify_default(self),  // placeholder for the bulk
        }
    }

    /// Slug form for URLs and search keys.
    pub const fn slug(self) -> &'static str {
        // Generated to match the display name, kebab-cased.
        // e.g. DocConcept::StackNavigator => "stack-navigator"
        // ...
        unimplemented!()
    }
}
```

That's the whole metadata vocabulary. Everything else (typography
choices, layout, theme application) is the UI side and isn't visible
to the metadata consumer.

## The macro's input grammar

```rust
use runtime_core::docs;
use crate::demos::counter_demo;

docs! {
    slug = "reactivity",
    title = "Reactivity",
    category = Foundation,
    description = "The mechanism behind every change in an Idealyst app.",
    related = ["signals", "components", "styles"],
    concepts = [Signal, Effect, Scope, TrackedContext, Derived, Untrack],

    section(heading = "The model in one paragraph") {
        p("Idealyst's reactivity is one mechanism applied uniformly. A signal \
           holds a value. When a closure reads the signal inside a tracked \
           context, the framework records the dependency."),
        p("When the signal changes, the framework re-runs every tracked \
           context that read it — and only those. State, styles, themes, \
           conditional rendering, and list contents all use this same \
           mechanism underneath."),
    }

    section(heading = "Signals") {
        p("Make a signal with ", code("signal!(initial)"), ". Read with ",
          code(".get()"), " and write with ", code(".set(...)"),
          " or ", code(".update(|v| ...)"), "."),

        code(rust) = r#"
            let count = signal!(0);
            count.set(5);                  // replace
            count.update(|n| *n += 1);     // mutate in place
        "#,

        p("The ", code("Signal<T>"), " you hold back is a small Copy token \
           — a couple of u32s indexing into a thread-local arena. Pass it \
           into closures and child components freely; no manual clone()."),
    }

    section(heading = "What gets tracked") {
        p("A signal read inside any of these contexts subscribes the \
           context to the signal:"),

        list {
            ["Reactive text — ", code("Text { format!(\"...\", count.get()) }")],
            ["A reactive ", code("if"), " inside ", code("ui!")],
            ["A reactive ", code("for"), " over a signal-backed list"],
            ["Closure props that read a signal"],
            ["Stylesheets — reading the active theme is itself tracked"],
            ["A manual ", code("Effect::new(...)")],
        }

        p("A plain expression that doesn't read a signal is not tracked — \
           it's computed once when the tree is built and never re-runs."),
    }

    section(heading = "Comparisons") {
        compare(from = React) {
            p("A signal is not ", code("useState"), ". ", code("useState"),
              " triggers a re-render of the component; a signal notifies \
               only the specific reads that depend on it."),
            p("The closer React analog is ", code("useSyncExternalStore"),
              " over an observable, or a library like Jotai or Zustand."),
        }

        compare(from = Solid) {
            p("Identical to ", code("createSignal"), ". Track-on-read, \
               re-run on change, components run once."),
        }

        compare(from = SvelteFive) {
            p("Equivalent to ", code("$state"), ". The model is the same; \
               Svelte's runes are compiler-driven, Idealyst's signals are \
               a regular Rust API."),
        }
    }

    section(heading = "Try it") {
        p("This counter is mounted live in the page. Edits to ",
          code("counter_demo"), "'s source survive hot reload — the count \
           keeps its value across edits."),

        demo(counter_demo, description = "Counter with one signal."),
    }

    note(kind = Tip) {
        p("If you find an effect re-firing more often than expected, look \
           for an accidental ", code(".get()"), " in a place that didn't \
           need to subscribe. Wrap the read in ", code("untrack(|| ...)"),
          " to opt out."),
    }
}
```

### Inline spans

Paragraphs and list items take a variadic sequence of spans. Three
kinds:

- **String literal** — plain text. `"foo"` lowers to `Span::Text("foo")`.
- **`code("...")`** — inline code span. `Span::Code("...")`.
- **`link("Reactivity", to = "reactivity")`** — cross-reference.
  Optional `#section` suffix on `to` for deep links.

The macro recognizes these by syntax shape, not by trait dispatch, so
the diagnostics are good ("expected string literal or `code(...)` /
`link(...)`").

### Sections

`section(heading = "Heading") { block, block, ... }`. Section slug is
auto-derived (`heading.to_lowercase().replace(' ', "-")`); override
with `section(heading = "Why no diff?", slug = "no-diff")`.

### Code blocks

`code(rust) = r#"..."#` — language tag is a bare identifier (`rust`,
`bash`, `typescript`, `json`, `text`), source is a string. The macro
extracts the indentation prefix from the raw string and trims so
authors can indent the source naturally inside the macro.

### Lists

`list { [span, span, ...], [span, span, ...], ... }` — each row is a
bracketed span sequence, same as a paragraph's contents.

### Comparisons

`compare(from = Framework) { p(...), p(...), ... }` — the
`Framework` is one of the `ComparisonFramework` enum variants. The
macro lowers to `BlockMeta::Comparison { from: ..., blocks: &[...] }`.

The UI side handles the tab-bar rendering; the metadata side just
records each comparison's framework and content.

### Demos

`demo(fn_name)` or `demo(fn_name, description = "...")` — the
identifier is a path to a `#[component] fn fn_name(props: &Props) ->
Element` (or a no-arg `() -> Element`). The macro emits both:

- **UI side**: `Demo { fn_name() }` — the component is invoked
  inline.
- **Metadata side**: `BlockMeta::Demo { name: "fn_name", description:
  ... }` — just the name. MCP consumers don't need to know how the
  demo is implemented; they just need to know it exists.

### Notes

`note(kind = Tip) { ... }` — wraps a sequence of blocks in an Info /
Tip / Warning callout. Kind matches the `NoteKind` enum.

## What the macro emits

For a page declared as above, the macro produces two items:

```rust
// 1. The UI function — called by the routes registry to render the screen.
pub fn page() -> Element {
    ui! {
        ScrollView {
            Stack(gap = StackGap::Xl) {
                PageHeader(
                    title = "Reactivity".to_string(),
                    description = "The mechanism behind every change in an Idealyst app.".to_string(),
                )

                Section(heading = "The model in one paragraph".to_string()) {
                    Body(content = "Idealyst's reactivity is one mechanism applied uniformly...".to_string())
                    Body(content = "When the signal changes, the framework re-runs...".to_string())
                }

                Section(heading = "Signals".to_string()) {
                    Body(/* spans → rendered with code-styling */)
                    CodeBlock(language = "rust".to_string(), source = "let count = signal!(0);\n...".to_string())
                    Body(...)
                }

                // ... sections continue ...

                Comparison { /* tab-bar component holding the per-framework blocks */ }

                Section(heading = "Try it".to_string()) {
                    Body(...)
                    Demo { counter_demo() }    // ← real fn call
                }

                Note(kind = NoteKind::Tip) { Body(...) }
            }
        }
    }
}

// 2. The metadata — collected by the docs-registry.
pub static PAGE_META: PageMeta = PageMeta {
    slug: "reactivity",
    title: "Reactivity",
    category: PageCategory::Foundation,
    description: Some("The mechanism behind every change in an Idealyst app."),
    related: &["signals", "components", "styles"],
    concepts: &["signal", "effect", "scope", "tracked-context"],
    sections: &[
        SectionMeta {
            heading: "The model in one paragraph",
            slug: "the-model-in-one-paragraph",
            blocks: &[
                BlockMeta::Paragraph(&[Span::Text("Idealyst's reactivity is one mechanism...")]),
                BlockMeta::Paragraph(&[Span::Text("When the signal changes...")]),
            ],
        },
        SectionMeta {
            heading: "Signals",
            slug: "signals",
            blocks: &[
                BlockMeta::Paragraph(&[
                    Span::Text("Make a signal with "),
                    Span::Code("signal!(initial)"),
                    Span::Text(". Read with "),
                    Span::Code(".get()"),
                    /* ... */
                ]),
                BlockMeta::Code { language: "rust", source: "let count = signal!(0);\n..." },
                /* ... */
            ],
        },
        /* ... */
    ],
};
```

The shell components (`PageHeader`, `Section`, `CodeBlock`,
`Comparison`, `Demo`, `Note`) don't change — the macro just calls them
with the right props. New ones we need to add:

- **`Comparison`** — the tab-bar card that shows one framework's
  blocks at a time, driven by the global `from_framework` signal.
- **`Demo`** — a wrapper that frames a live component (border, label
  "Live demo", maybe a "see source" affordance).
- **`Note`** — the info/tip/warning callout.

## The page registry

A small `docs-registry` module collects every `PAGE_META` exported by
the docs crate's modules. It can either be hand-maintained (one
`pub const PAGES: &[&PageMeta] = &[ &reactivity::PAGE_META, ... ];`)
or build-script-driven. Hand-maintained is fine for v1 — adding a
page is one line.

```rust
// src/registry.rs
pub static PAGES: &[&'static PageMeta] = &[
    &crate::pages::overview::PAGE_META,
    &crate::pages::getting_started::PAGE_META,
    &crate::pages::reactivity::PAGE_META,
    // ...
];

pub fn find(slug: &str) -> Option<&'static PageMeta> {
    PAGES.iter().copied().find(|p| p.slug == slug)
}

pub fn by_category(cat: PageCategory) -> impl Iterator<Item = &'static PageMeta> {
    PAGES.iter().copied().filter(move |p| p.category == cat)
}
```

The routes table and the sidebar both build off this registry. The
MCP server queries it.

## Cookbook — "noop for MCP"

Cookbook recipes are `PageCategory::Cookbook` entries. The site
treats them as normal pages (linked from a dedicated Cookbook
section). The MCP server treats them specially:

- `list_doc_pages()` — returns all categories **except** Cookbook.
  This is the model's default view of "what documentation exists."
- `list_cookbook_recipes()` — returns Cookbook entries only.
- `get_page(slug)` — works for any category. Cookbook recipes are
  addressable; they're just not promoted.

The model can browse the cookbook if it explicitly wants to ("I want
example code for a form with validation"), but won't see recipes
when answering general questions about how the framework works.
Recipes-as-examples don't drown out concept pages.

This is the "noop for MCP" property: recipes participate in the
registry the same way every other page does, but the MCP tools'
default surface area excludes them.

## MCP tool surface (proposal)

Given the registry plus `PageMeta` walking, the MCP server can
expose:

| Tool | Returns |
| --- | --- |
| `list_doc_pages` | Title, slug, category, description per non-Cookbook page |
| `list_cookbook_recipes` | Same shape, Cookbook only |
| `get_page` | Full structured page (sections + blocks + spans) |
| `get_section` | Single section by `slug#section-slug` |
| `search_docs` | Ranked match across headings, descriptions, and paragraph text |
| `list_concepts` | Vocabulary index (union of every page's `concepts`) |
| `pages_about` | Pages that list a concept |
| `related_pages` | The page's `related` field, resolved to full metadata |
| `compare_for` | Every comparison block tagged with a given framework, across pages |

`compare_for` is interesting — a model asked "show me everything that
compares Idealyst to React" can get a single ranked list, since the
metadata tags every comparison with its target framework.

## Migration plan

13 existing markdown drafts to convert. For each page:

1. Extract slug + title + description from the existing intro.
2. Walk the markdown:
   - `#` → page title (already extracted).
   - `##` → `section(heading = ...)` block.
   - paragraphs → `p(...)` with `code()` spans for backticked code.
   - fenced code → `code(lang) = r#"..."#`.
   - bulleted lists → `list { ... }`.
   - `> **From X.** ...` blockquotes → `compare(from = X) { ... }`.
   - links to `[Page](#)` placeholders → `link("Page", to = "slug")`.
3. Curate `related` and `concepts` arrays based on cross-references.

A one-off script can do steps 2.a–2.d mechanically; the comparison
extraction and link resolution want human review. Estimate:
~30 minutes per page after the macro lands, mostly mechanical.

## Settled decisions

These were the open questions; the answers are now part of the design.

1. **Inline span helpers use strings.** `code("Signal<T>")`,
   `link("Reactivity", to = "reactivity")`. Bare-token syntax
   (`code(Signal<T>)`) was considered and dropped — it has too
   many parsing edge cases (commas in generics, leading dots,
   closure expressions) and the rendered output is identical.

2. **Duplicate section slugs are a compile error.** If two
   `section(heading = "Foo")` blocks on the same page auto-derive
   the same slug, the macro fails compilation. Author resolves by
   passing an explicit `slug` to one of them. Keeps URLs stable
   across re-ordering.

3. **Cookbook recipes are one page each.** No `subrecipes` field,
   no parent-child relationships. A recipe that wants more than
   one page is actually two recipes. Revisit if we hit a real
   case that can't be split.

4. **`description` stays `Option<&'static str>`.** Subtitles
   don't need inline code or spans; no existing draft wants it.
