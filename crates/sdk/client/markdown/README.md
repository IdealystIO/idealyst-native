# `markdown`

Render a CommonMark/GFM document from a **resolved document payload** —
parsed and themed author-side, then mounted by ONE scene handler. Same
third-party-primitive mechanism as [`codeblock`](../codeblock), and for
the same reason: performance.

```rust
use markdown::{Markdown, MdTheme};

// At app bootstrap — the boot entry's `register` argument IS the
// registration seam:
backend_web::newcore::start_in("#app", markdown::register, app);

// In a component tree:
ui! { Markdown(source = "# Hello\n\nWorld **bold**".to_string()) }

// Or the low-level builder (mirrors `code_block`):
markdown::markdown("# Hi", MdTheme::dark()).with_style(my_panel_style())
```

An UNREGISTERED payload panics at realize (the scene contract), so a
missed `register` fails loud rather than rendering a silent placeholder.

## One handler, every host

`markdown::register` is **caps-generic** (`StyleServices + TextOps`):
one registration covers every caps-complete host, and web, SSR and
native all mount the identical semantic element tree. The IR (`ir.rs`)
and parser (`parse.rs`) are pure data — no core types.

There is **no dedicated native single-node renderer**. An iOS `UILabel`
+ `NSAttributedString` / Android `TextView` + `SpannableStringBuilder`
handler would be a separate `Registry<IosBackend>` /
`Registry<AndroidBackend>` registration that lowers the same
`MarkdownDoc` into attribute spans; it is not implemented. Native hosts
realize the semantic element tree through their own caps impls instead.

## Why a resolved document — the performance contract

A markdown document is a deep tree: blocks (headings, paragraphs, lists,
quotes, code) containing inline runs (bold, italic, code, links). The
naive lowering emits **one framework `Element` per inline run** — the
exact per-token explosion `codeblock` was carved out to avoid (a
measured 100–300× more backend ops per render). A paragraph with twenty
emphasis spans would be twenty-plus framework nodes, each with its own
reactive scope and layout entry.

Instead the whole document is resolved to a serializable `MarkdownDoc`
author-side and handed to the registry as ONE payload. The handler
builds raw host nodes directly — real elements, but **not** framework
reactive `Element`s, so there is no per-node reactive overhead.

| Element | Host node |
| --- | --- |
| Heading | `<h1>`…`<h6>`, UA margins zeroed (the column `gap` owns spacing) |
| Paragraph | `<p>` |
| Code block | `<pre>` with the theme's code background/foreground, mono family, padding, 6px radii |
| Quote | `<blockquote>` with a 3px left rule + italic |
| List | a column `<div>` of row `<div>`s: marker text + content, indented per nesting depth |
| Rule | `<hr>` with a 1px top border |
| Inline run | a styled text node (color, bold, italic, strikethrough, underline for links, mono + tinted background for code) |

## Styling + theming

Parsing and theme resolution happen **author-side**, inside the
`Markdown` tag's reactive region, producing a fully-resolved,
serializable `MarkdownDoc` (blocks + a concrete `MdTheme`). `MdTheme` is
the SDK's complete styling surface — a color or size per element type:

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
field to restyle. Both props are `Reactive`, so a theme toggle
re-resolves the doc and the node is rebuilt with the new colors — the
`switch` region dedupes on an equal `(source, theme)` tuple, so static
props build exactly once. The `markdown-demo` example wires a light/dark
toggle to a reactive `theme` prop and the page re-paints live.

### Why a struct, not framework stylesheets

The handler builds raw host nodes *below* the framework's
`StyleRules`/token layer, so the stylesheet/token system can't flow into
them automatically. Resolving a plain `MdTheme` author-side — and
re-resolving it reactively — is what makes the styling both fully
controllable and theme-reactive without per-platform code. Drive
`MdTheme`'s fields from your app theme (tokens, `color_scheme()`, a
`Signal`) and a theme switch propagates.

## Supported syntax (v1)

**Blocks:** headings (h1–h6), paragraphs, unordered + ordered lists
(including nesting via indentation), block quotes, fenced/indented code
blocks, thematic breaks.

**Inline:** bold, italic, bold+italic, inline code, links (styled: link
color + underline), strikethrough (GFM), soft/hard breaks.

## Non-trivial decisions & limitations (v1)

These were the judgment calls; they're logged here so the next person hits
the constraint, not the workaround.

- **One payload, raw host nodes.** The alternative — lowering blocks and
  runs to framework `Element`s — reintroduces the per-token explosion
  this SDK exists to avoid.
- **Links are styled but not tappable in v1.** The destination URL is parsed
  and carried in the IR (`MdRun::link`), and links render with the link
  color + underline, but tap-to-navigate is not wired. Hooking it up means a
  per-host tap target (an `<a href>` on web, an `NSTextView`/`UITextView`
  link attribute, an Android `ClickableSpan`) — deferred, not faked.
- **No syntax highlighting** of code-block bodies — they render monospace in
  one color. Compose with the `codeblock` SDK if you need highlighting.
- **Not yet supported:** images, tables, task-list checkboxes, footnotes.
  These parse to nothing (or, for tables, are ignored) rather than
  panicking.
- **`line_height` is absolute px** in this framework's `StyleRules`, not a
  unitless multiplier — the handler leaves it unset so multi-size text
  (headings vs. body) keeps the host's proportional `normal` line height.

## Testing checklist

**Automated**
- [ ] `cargo test -p markdown` — parser unit tests (`src/parse.rs`) plus
  the `tests/markdown.rs` op-log fence over the mounted DOM shape
- [ ] `cargo check -p markdown --target wasm32-unknown-unknown` — web target

**Rendering / behavior**

Use the `markdown-demo` example. A CommonMark/GFM doc should render headings
(h1–h6), paragraphs, ordered/unordered/nested lists, block quotes, code blocks,
thematic rules, and every inline style (bold/italic/both/code/strikethrough/link),
plus a live light↔dark theme toggle re-painting text + background.

- [ ] **Web** — semantic DOM (`<h1>`/`<p>`/`<pre>`/`<blockquote>`/`<hr>`/list rows);
  inspect the DOM to confirm real elements with per-run inline styling; theme toggle
  re-paints.
- [ ] **SSR** — the same semantic DOM in the rendered HTML.
- [ ] **iOS / Android / macOS / gpu** — the same element tree realized through
  each host's caps impls; confirm block spacing, list indents and inline
  styling read correctly and the theme toggle re-paints.
