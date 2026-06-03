# `markdown`

Render a CommonMark/GFM document as a **single native styled-text node**
per backend. Built on `Element::External`, the same third-party-primitive
mechanism as [`codeblock`](../codeblock) — and for the same reason:
performance.

```rust
use markdown::{Markdown, MdTheme};

// At app bootstrap, once per backend:
markdown::register(&mut backend);

// In a component tree:
ui! { Markdown(source = "# Hello\n\nWorld **bold**".to_string()) }

// Or the low-level builder (mirrors `code_block`):
markdown::markdown("# Hi", MdTheme::dark()).with_style(my_panel_style())
```

## Why one node — the performance contract

A markdown document is a deep tree: blocks (headings, paragraphs, lists,
quotes, code) containing inline runs (bold, italic, code, links). The
naive lowering emits **one styled `text`/`view` node per inline run** —
the exact per-token explosion `codeblock` was carved out of `runtime-core`
to avoid (a measured 100–300× more backend ops per render). A paragraph
with twenty emphasis spans would be twenty-plus framework nodes, each with
its own reactive scope and layout entry.

Native rich-text engines already solve this: `NSAttributedString` and
`SpannableString` express the whole tree as **inline attribute ranges on
one widget**. So this SDK lowers the entire document to a single styled
string and hands it to one native text view. A 50-block document is **one
native node**, not thousands.

| Target | Mechanism |
| --- | --- |
| **iOS** | One `UILabel` whose `attributedText` is an `NSAttributedString` with per-range font (size + bold/italic/monospace), foreground color, background tint, underline, and strikethrough attributes. Wraps to the column width via a width-aware Taffy measure (`install_external_wrapping_measure`). |
| **Android** | One `android.widget.TextView` fed a `SpannableStringBuilder` carrying `AbsoluteSizeSpan` / `StyleSpan` / `TypefaceSpan` / `ForegroundColorSpan` / `BackgroundColorSpan` / `UnderlineSpan` / `StrikethroughSpan` ranges. No custom Kotlin class — a plain TextView gets width-aware wrapping measurement automatically. |
| **Web** | Semantic DOM (`<h1>`, `<p>`, `<pre>`, `<blockquote>`, `<hr>`, list rows) built through the `Backend` trait, with per-run inline styling. DOM layout is cheap and the semantic tree is accessible, so web keeps real elements. These are still raw backend nodes, **not** framework reactive `Element`s, so there's no per-node reactive overhead. |
| **Other** (macOS / terminal / gpu) | The framework's external-not-registered placeholder until a handler lands. |

The web/native split is deliberate and consistent with `codeblock`: native
collapses to one styled-text node (perf-critical, and the toolkits make it
trivial), web uses semantic elements (cheap layout, better accessibility).
Observable output converges (CLAUDE.md rule 7); the *mechanism* differs.

## Styling + theming

Parsing and theme resolution happen **author-side**, inside the `Markdown`
component's reactive scope, producing a fully-resolved, serializable
`MarkdownDoc` (blocks + a concrete `MdTheme`). `MdTheme` is the SDK's
complete styling surface — a color or size per element type:

```rust
pub struct MdTheme {
    pub text: String,        // body
    pub muted: String,       // list markers, rules
    pub heading: String,
    pub link: String,
    pub code_fg: String,
    pub code_bg: String,
    pub quote_fg: String,
    pub base_size: f32,
    pub heading_scale: [f32; 6],   // h1 = base * scale[0], …
    pub mono_family: Option<String>,
}
```

`MdTheme::light()` and `MdTheme::dark()` ship as defaults; override any
field to restyle. Because the component reads `source`/`theme` **reactively**,
a theme toggle re-resolves the doc → new `Element::External` props → the
single native node is rebuilt with the new colors. The `markdown-demo`
example wires a light/dark toggle to a reactive `theme` prop and the page
re-paints live on every backend.

### Why a struct, not framework stylesheets

The native handlers build raw platform nodes (an `NSAttributedString`, a
`SpannableString`) *below* the framework's `StyleRules`/token layer, so the
stylesheet/token system can't flow into them automatically. Resolving a
plain `MdTheme` author-side — and re-resolving it reactively — is what makes
the styling both fully controllable and theme-reactive across all backends
without per-platform code. Drive `MdTheme`'s fields from your app theme
(tokens, `color_scheme()`, a `Signal`) and a theme switch propagates.

## Over the runtime-server wire

`markdown(...)` registers a wire serde pair for `MarkdownDoc` automatically
(idempotent, thread-local guarded): the recorder serializes the resolved
doc into `CreateExternal`, the device deserializes it and dispatches to its
real per-backend handler. No app-level recorder wiring needed.

## Supported syntax (v1)

**Blocks:** headings (h1–h6), paragraphs, unordered + ordered lists
(including nesting via indentation), block quotes, fenced/indented code
blocks, thematic breaks.

**Inline:** bold, italic, bold+italic, inline code, links (styled: link
color + underline), strikethrough (GFM), soft/hard breaks.

## Non-trivial decisions & limitations (v1)

These were the judgment calls; they're logged here so the next person hits
the constraint, not the workaround.

- **Whole-document single node on native, semantic DOM on web.** See above.
  The alternative — one native node *per block* — would need every block
  registered with Taffy + a measure_fn, multiplying layout cost for no
  visual gain over a single wrapping label. One node is both faster and
  simpler.
- **Block spacing is blank lines; list indents are leading spaces; rules
  are a dash run.** On the single native label there's no per-block box to
  carry margins/padding, so vertical rhythm is encoded as `\n\n` separators
  and list nesting as leading spaces. This keeps iOS and Android pixel-
  consistent with zero paragraph-style plumbing. A future revision could
  use `NSParagraphStyle` / `LeadingMarginSpan` for true hanging indents.
- **Code blocks get a rectangular background tint** (`NSBackgroundColor` /
  `BackgroundColorSpan`), not a rounded padded card, on native — span
  backgrounds are rectangular and span the text run. Web uses a real padded,
  rounded `<pre>`.
- **Links are styled but not tappable in v1.** The destination URL is parsed
  and carried in the IR (`MdRun::link`), and links render with the link
  color + underline, but tap-to-navigate is not wired. Hooking it up means a
  per-backend tap target (an `NSTextView`/`UITextView` link attribute, an
  Android `ClickableSpan`, an `<a href>` on web) — deferred, not faked.
- **No syntax highlighting** of code-block bodies — they render monospace in
  one color. Compose with the `codeblock` SDK if you need highlighting.
- **Not yet supported:** images, tables, task-list checkboxes, footnotes.
  These parse to nothing (or, for tables, are ignored) rather than
  panicking.
- **`line_height` is absolute px** in this framework's `StyleRules`, not a
  unitless multiplier — the web handler leaves it unset so multi-size text
  (headings vs. body) keeps the browser's proportional `normal` line height.

## Verification

Implemented and exercised on all three primary backends via the
`markdown-demo` example:

- **Web** — rendered in a browser; verified semantic blocks, every inline
  style (bold/italic/both/code/strikethrough/link), nested + ordered lists,
  code block, blockquote, rule, and a **live light↔dark theme toggle**
  re-painting text and background.
- **iOS** — built for and launched on the iOS Simulator; one `UILabel`
  renders the document, wraps to the column, and re-themes on toggle.
- **Android** — built for and launched on an emulator; one `TextView` with
  the SpannableString renders the document and re-themes on toggle.

Parser logic is unit-tested (`src/parse.rs` — headings, inline styles,
links, ordered/unordered/nested lists, code blocks, quotes, rules).
