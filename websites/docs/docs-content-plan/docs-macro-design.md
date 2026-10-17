# `docs!` macro design

> **Status: implemented.** The macro is `websites/docs-macro` (`parse.rs`,
> `emit.rs`); `PageMeta` lives in `websites/docs/src/meta.rs` and the page
> registry in `websites/docs/src/registry.rs`. Every page under
> `websites/docs/src/pages/` is built with it except the hand-built
> `cli.rs`, `macros_page.rs`, `platforms.rs`, `simulator_demo.rs` and
> `ui_dsl.rs`. The MCP tool surface sketched below is not built.

The structured-input macro that replaces hand-built page components, with
the dual goal of generating UI and emitting a machine-readable metadata
blob.

## Goals (recap)

1. **One Rust source per page.** No markdown parser, no JSON authoring.
2. **The macro emits the screen** (a `pub fn page() -> Element` that
   composes the shell's existing `PageHeader` / `Card` / `CodeBlock`
   components).
3. **The macro also emits structured metadata** (a `pub static
   PAGE_META: PageMeta`) that an MCP server (or text export, or a
   search index, or any future consumer) can walk without re-parsing
   anything.
4. **Demos are real function references.** Type-checked at compile
   time; no "missing demo" runtime errors.
5. **Cookbook entries opt out of the default page index**
   (`registry::non_cookbook`) so recipes aren't promoted as primary
   documentation.

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
    /// Drives the `registry::pages_about(concept)` reverse index. The
    /// set of variants in `DocConcept` is the framework's canonical
    /// vocabulary list.
    pub concepts: &'static [DocConcept],

    /// Ordered sections.
    pub sections: &'static [SectionMeta],
}

pub enum PageCategory {
    /// The Overview page. Always first.
    Overview,
    /// Architectural introduction, ahead of the onboarding path.
    Introduction,
    /// Foundational concepts (Primitives, Reactivity, Styles,
    /// Components). The order matters here — pages are listed in
    /// declaration order within a category.
    Foundation,
    /// Tooling pages (Robot, Dev tools).
    Tools,
    /// Reference material (Backends, Refs, Lists, Icons, Navigation,
    /// Getting Started).
    Reference,
    /// Deeper material: internal contracts, extension points, the
    /// public API of a specific backend.
    Advanced,
    /// Cookbook recipes. Kept out of `registry::non_cookbook()`;
    /// listed by `registry::cookbook()`.
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
    text(&'static str),
    Code(&'static str),
    /// Cross-reference to another page (or page#section).
    link {
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
/// field. Adding a new concept = adding a variant here; every page's
/// `concepts` list is checked against it at compile time.
pub enum DocConcept {
    // Reactivity
    Signal,
    Effect,
    Scope,
    Mount,
    Memo,
    Resource,
    // ...

    // Navigation
    Route,
    RouteParams,
    Screen,
    SwapNavigator,
    StackNavigator,
    NavigatorOutlet,
    Link,
    AmbientNavigator,
    MountPolicy,
    // ...one group per area: components, UI DSLs, primitives,
    // styling, refs, lists, icons, overlays, backends, dev tools,
    // robot, server functions, net, …
}

impl DocConcept {
    /// Human-readable name for chips and search results.
    pub const fn display(self) -> &'static str { /* one arm per variant */ }
    /// Kebab-case slug for URLs and search keys.
    pub const fn slug(self) -> &'static str { /* one arm per variant */ }
}
```

The variant list above is an excerpt. The full, current list —
with a doc comment per variant — is `DocConcept` in
`websites/docs/src/meta.rs`; `display()` and `slug()` there are
exhaustive `const` matches, so a new variant without both arms
fails to compile.

That's the whole metadata vocabulary. Everything else (typography
choices, layout, theme application) is the UI side and isn't visible
to the metadata consumer.

## The macro's input grammar

```rust
use docs_macro::docs;
use crate::demos::counter_demo;

docs! {
    slug = "reactivity",
    title = "Reactivity",
    category = Foundation,
    description = "The mechanism behind every change in an Idealyst app.",
    related = ["components", "styles"],
    concepts = [Signal, Effect, Scope, TrackedContext, Derived, Untrack],

    section(heading = "The model in one paragraph") {
        p("Idealyst's reactivity is one mechanism applied uniformly. A signal \
           holds a value. When a closure reads the signal inside a tracked \
           context, the framework records the dependency."),
        p("When the signal changes, the framework re-runs every tracked \
           context that read it — and only those. State, styles, themes, \
           conditional rendering, and list contents all use this same \
           mechanism underneath."),
    },

    section(heading = "Signals") {
        p("Make a signal with ", code("signal(initial)"), ". Read with ",
          code(".get()"), " and write with ", code(".set(...)"),
          " or ", code(".update(|v| ...)"), "."),

        code(rust, r##"
            let count = signal(0);
            count.set(5);                  // replace
            count.update(|n| n + 1);     // derive the next value from the current one
        "##),

        p("The ", code("Signal<T>"), " you hold back is a small Copy token \
           — a couple of u32s indexing into a thread-local arena. Pass it \
           into closures and child components freely; no manual clone()."),
    },

    section(heading = "What gets tracked") {
        p("A signal read inside any of these contexts subscribes the \
           context to the signal:"),

        list(
            ["Reactive text — ", code("text { \"count: {count}\" }")],
            ["A reactive ", code("if"), " inside ", code("ui!")],
            ["A reactive ", code("for"), " over a signal-backed list"],
            ["Closure props that read a signal"],
            ["Stylesheets — reading the active theme is itself tracked"],
            ["A manual ", code("effect!({ ... })"), " (or ", code("watch(...)"), " outside the tree)"],
        ),

        p("A plain expression that doesn't read a signal is not tracked — \
           it's computed once when the tree is built and never re-runs."),
    },

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
        },
    },

    section(heading = "Try it") {
        p("This counter is mounted live in the page. Edits to ",
          code("counter_demo"), "'s source survive hot reload — the count \
           keeps its value across edits."),

        demo(counter_demo, description = "Counter with one signal."),
    },

    note(kind = Tip) {
        p("If you find an effect re-firing more often than expected, look \
           for an accidental ", code(".get()"), " in a place that didn't \
           need to subscribe. Wrap the read in ", code("untrack(|| ...)"),
          " to opt out."),
    },
}
```

### Inline spans

Paragraphs and list items take a variadic sequence of spans. Three
kinds:

- **String literal** — plain text. `"foo"` lowers to `Span::text("foo")`.
- **`code("...")`** — inline code span. `Span::Code("...")`.
- **`link("Reactivity", to = "reactivity")`** — cross-reference,
  `Span::link { target, text }`. Optional `#section` suffix on `to`
  for deep links. The variant names really are `text` / `Code` /
  `link` — `meta.rs` spells them that way.

The macro recognizes these by syntax shape, not by trait dispatch, so
the diagnostics are good ("expected string literal or `code(...)` /
`link(...)`").

### Sections

`section(heading = "Heading") { block, block, ... }`. Section slug is
auto-derived (lowercased; each run of non-alphanumeric characters becomes one `-`); override
with `section(heading = "Why no diff?", slug = "no-diff")`.

### Code blocks

`code(rust, r##"..."##)` — language tag is a bare identifier (`rust`,
`bash`, `typescript`, `json`, `text`), then the source as a string
literal (raw strings with `##` let the source contain `"#`). The macro
extracts the indentation prefix from the raw string and trims so
authors can indent the source naturally inside the macro.

### Lists

`list([span, span, ...], [span, span, ...], ...)` — each row is a
bracketed span sequence, same as a paragraph's contents.

### Comparisons

`compare(from = Framework) { p(...), p(...), ... }` — the
`Framework` is one of the `ComparisonFramework` enum variants. The
macro lowers to `BlockMeta::Comparison { from: ..., blocks: &[...] }`.

The UI side renders it inline as a "From <Framework>" sub-heading
plus its blocks; the metadata side records each comparison's
framework and content.

### Demos

`demo(fn_name)` or `demo(fn_name, description = "...")` — the
identifier is a path to a no-arg `fn() -> Element`. The macro emits
both:

- **UI side**: `fn_name()` — called inline in the page tree, so the
  compiler checks the symbol at the `docs!` call site.
- **Metadata side**: `BlockMeta::Demo { name: "fn_name", description:
  ... }` — just the name. MCP consumers don't need to know how the
  demo is implemented; they just need to know it exists.

### Notes

`note(kind = Tip) { ... }` — wraps a sequence of blocks in an Info /
Tip / Warning callout. Kind matches the `NoteKind` enum.

## What the macro emits

For a page declared as above, the macro produces two items:

```rust
// 1. The metadata — collected by the registry.
pub static PAGE_META: crate::meta::PageMeta = crate::meta::PageMeta {
    slug: "reactivity",
    title: "Reactivity",
    category: crate::meta::PageCategory::Foundation,
    description: Some("The mechanism behind every change in an Idealyst app."),
    related: &["components", "styles"],
    concepts: &[
        crate::meta::DocConcept::Signal,
        crate::meta::DocConcept::Effect,
        // ...
    ],
    sections: &[
        crate::meta::SectionMeta {
            heading: "Signals",
            slug: "signals",
            blocks: &[
                crate::meta::BlockMeta::Paragraph(&[
                    crate::meta::Span::text("Make a signal with "),
                    crate::meta::Span::Code("signal(initial)"),
                    /* ... */
                ]),
                crate::meta::BlockMeta::Code { language: "rust", source: "let count = signal(0);\n..." },
                /* ... */
            ],
        },
        /* ... */
    ],
};

// 2. The UI function — the routes table calls it to render the screen.
pub fn page() -> ::runtime_core::Element {
    ::runtime_core::ui! {
        scroll_view {
            Stack(gap = ::idea_ui::StackGap::Xl) {
                PageHeader(
                    title = "Reactivity".to_string(),
                    description = "The mechanism behind every change in an Idealyst app.".to_string(),
                )
                // One Card per section.
                Card {
                    Typography(content = "Signals".to_string(), kind = ::idea_ui::typography_kind::H2)
                    Typography(content = "Make a signal with `signal(initial)`. ...".to_string(), muted = true)
                    CodeBlock(code = "let count = signal(0);\n...".to_string())
                }
                // ...
            }
        }
    }
}
```

The render side is deliberately plain:

- Each section is a `Card` headed by an H2 `Typography`.
- A paragraph is one muted `Typography`; its spans are flattened to
  plain text (code spans wrapped in backticks, links reduced to their
  text). Per-span styling exists only in the metadata.
- A list renders one `"• item"` `Typography` per row.
- A code block is a `CodeBlock` with its indentation trimmed.
- A comparison or note renders as an H3 label ("From React",
  "Tip:") followed by its blocks; a top-level one gets its own `Card`.
- A demo is a direct call to its function.

There are no `Comparison`, `Demo`, or `Note` shell components. A
page module must have `PageHeader`, `CodeBlock`, `Card`, `Stack` and
`Typography` in scope, which is why every page imports them.

## The page registry

`websites/docs/src/registry.rs` collects every `PAGE_META` by hand:
one `PAGES` slice, one line per page.

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

// Also: non_cookbook(), cookbook(), pages_about(concept), all_concepts().
```

The sidebar and cross-reference resolution build off this registry.
No MCP server reads it today.

## Cookbook — kept out of the default listing

Cookbook recipes are `PageCategory::Cookbook` entries. The site
treats them as normal pages (linked from a dedicated Cookbook
section). The registry splits them out:

- `registry::non_cookbook()` — every category **except** Cookbook.
  This is the default view of "what documentation exists."
- `registry::cookbook()` — Cookbook entries only.
- `registry::find(slug)` — works for any category. Recipes are
  addressable; they're just not promoted.

An MCP surface over the registry (below) should keep the same split,
so recipes-as-examples don't drown out concept pages when a model
asks how the framework works.

## MCP tool surface (not built)

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
