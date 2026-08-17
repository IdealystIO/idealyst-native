# `codeblock`

Two code-surface primitives:

- **`code_block`** — read-only. A flat sequence of `(text, color)` runs
  rendered as a **single native node** on every backend, built for
  syntax-highlighted source display. The docs site renders ~140-line
  tokenized snippets and ships dozens per page.
- **`code_editor`** — editable. The same per-range styling on a live,
  focusable, IME-capable editor, driven by **byte ranges the caller
  supplies**. See [Editing with decorations](#editing-with-decorations).

```rust
use codeblock::{code_block, CodeBlockProps};
use runtime_core::Color;

// At app bootstrap, once per backend:
// `register` IS the boot registration seam:
// backend_web::newcore::start_in("#app", codeblock::register, app);

// Inside an effect / arm body:
let spans = vec![
    ("fn ".into(),       Color("#888".into())),
    ("hello".into(),     Color("#0a0".into())),
    ("() { … }".into(),  Color("#444".into())),
];
code_block(spans).with_style(my_codeblock_style())
```

## Per-platform behavior

Every backend renders **one** native node per `code_block(...)` call:

| Target | Mechanism |
| --- | --- |
| Web (+ SSR) | A `<pre>` with one styled `<span>` per run, built through the capability traits so SSR + hydration stay in lockstep. |
| Android | A `RustCodeBlock` (HorizontalScrollView + TextView) with a `SpannableString` carrying one `ForegroundColorSpan` per run. One TextView regardless of token count. |
| iOS | A horizontal `UIScrollView` wrapping an inset-honoring label whose `attributedText` is an `NSAttributedString` with per-run `NSForegroundColorAttributeName` ranges. One label per block. |
| macOS | A horizontal `NSScrollView` wrapping an `NSTextField` label with per-run `NSColor` ranges. Same shape as iOS. |
| web / SSR / terminal / gpu | The portable handler: one `create_element("pre")` with one colored `create_text` child per run. Backends with no tag concept get a plain view, so the panel still renders — only the one-node-per-block optimization is absent. |

`.with_style(...)` lands on the outer native node (the `<pre>` /
`HorizontalScrollView` / `UIScrollView` / `NSScrollView`).

Padding is **author-driven**: handlers bake none in. `padding_*` in the
style is realized *inside* the block's scroll region on every backend
(CSS padding on the `<pre>`, label `textInsets` on iOS, documentView
offset on macOS, `setPadding` + `clipToPadding=false` on Android), so
the padding scrolls with the content and mid-scroll text reaches the
block's own edge — the `<pre> { padding }` observable model. Put the
inset in the code block's style, not on a wrapping panel; panel padding
would shrink the native scroll viewport and clip moving text before the
panel's edge.

## Editing with decorations

`code_editor` is the editable sibling. It takes the buffer as a
`Signal<String>` (the controlled pattern every editable primitive in the
framework uses) and the styling as **byte ranges into that buffer**:

```rust
use codeblock::{code_editor, Decoration, DecorationStyle, Underline};

let src = signal(String::from("fn main() {}"));

code_editor(src, move |next| src.set(next))
    .decorate(|text| my_tokenizer(text))     // -> Vec<Decoration>
    .font("ui-monospace, monospace", 13.0)
    .line_height(20.0)
    .padding(12.0)
    .with_style(editor_panel_style())
```

**The primitive never parses anything.** It has no notion of a language,
a keyword or a token — it is handed ranges and told how they look. A
tree-sitter grammar, a regex sweep, a compiler's diagnostic list and a
hand-written matcher all emit the same thing, so any of them plugs in
without the primitive growing a notion of "syntax".

### The decoration model

```rust
Decoration { range: Range<usize>, style: DecorationStyle }
DecorationStyle { color, background, font_weight, font_style, underline }
Underline { style: Solid | Dotted | Dashed, color: Option<Color> }
```

Ranges are **byte** offsets, because that is what Rust text tooling
already speaks (`str::find`, `regex::Match::range`,
`tree_sitter::Node::byte_range`, rustc `Span`). Converting to each
backend's index space — UTF-16 on Apple and Android, DOM offsets on web
— is the primitive's job.

Decorations may **overlap freely** and apply in list order, field by
field. That is what lets two independent producers compose: a syntax
highlighter emits colours for the whole buffer, a diagnostics pass emits
red underlines over some of the same ranges, and the underline lands on
top *without* clearing the keyword's colour.

Out-of-bounds and mid-character ranges are normalized, not rejected: an
async producer is always a frame behind the buffer, and a panic there
would take the app down for a keystroke. Stale ranges clamp; ranges that
land inside a multi-byte character widen to whole characters.

### Two ways to supply them

| | When | Guarantee |
| --- | --- | --- |
| `.decorate(fn)` | Synchronous tokenizer | Called with the text the editor is about to display, so ranges can never describe a different buffer than the one on screen. |
| `.decorations(signal)` | Async producer (language server, worker-side parser) | Re-read whenever the signal changes; stale ranges clamp against the current text. |

### Shape, and why

```
outer  view          ← the author's box styling (`.with_style`)
  stack view         ← position: relative
    pre
      styled_text    ← IN FLOW. One attributed node; its measured size IS the editor's size.
    text_area        ← position: absolute, inset 0. Transparent glyphs, visible caret.
```

Font family, size, line height and padding are **metrics**, owned by the
primitive and written to both layers — not author style. That is the
whole point: styling one layer and forgetting the other is what makes
glyphs walk away from the caret one row at a time.

Neither layer scrolls internally. The decorated layer measures to the
full text and the editor stretches to that same box, so scrolling
happens on an ancestor and moves both layers as one — put the editor in
a `scroll_view`. The editor is always code-mode (no soft wrap): the two
layers would have to choose identical break points, and only `pre`
guarantees that.

One handler serves every backend. The hard part — an attributed run list
realized as one native node that wraps through the platform's own text
engine — is already `styled_text`'s job on all of them. The scene
`Registry` seam is still the right home: a backend that later grows a
genuinely single-widget decorated editor can register a concrete handler
for this payload the way `code_block` does, with no author-visible
change.

## Why a third-party primitive, not a framework one

It used to be `Element::CodeBlock` in `runtime-core`. A measurement
confirmed the perf justification was real: the equivalent composition
(`View` + per-token styled `Text`) generates 100–300× more backend ops
per re-render even with batched fast paths — a structural gap the
framework can't close, because composition rebuilds every span each
render while the single-node primitive replaces one node.

But the primitive doesn't fit runtime-core's intent: it isn't a
platform-native widget and is expressible from existing primitives if
perf weren't a concern. CLAUDE.md rule 3 says exactly this case belongs
in a third-party extension. So the fast
single-node renderer stayed, but the type moved out of core.

## Over the runtime-server wire

[`code_block`] registers a wire serde pair for [`CodeBlockProps`]
automatically (idempotent, thread-local guarded): the recorder
serializes the spans into `CreateExternal`, the device deserializes and
dispatches to its real per-backend handler. Without this, the External
payload couldn't cross the wire and the device would show the
portable handler. Registration happens from both [`code_block`]
(recorder side) and every [`register`] (device side), so no app-level
recorder wiring is needed.

[`code_block`]: src/lib.rs
[`register`]: src/lib.rs
[`CodeBlockProps`]: src/lib.rs

## Testing checklist

Manual verification per backend — an unchecked **native** box means the code
compiles for that target but isn't confirmed on real hardware yet. Tick each
item as you exercise it.

**Automated**
- [ ] `cargo build -p codeblock --target wasm32-unknown-unknown` — web target

**Rendering / behavior**

A tokenized snippet should render as **one** native node per `code_block(...)`,
read-only, with each `(text, color)` run carrying its own color, scrolling
horizontally when the content overflows.

- [ ] **Web** — a `<pre>` with one styled `<span>` per run (inspect DOM); colors
  match the spans; SSR + hydration stay in lockstep.
- [ ] **iOS** — ⚠️ not yet device-confirmed. A horizontal `UIScrollView` wrapping
  one `UILabel` (`NSAttributedString` with per-run color ranges); horizontal scroll
  works; one label regardless of token count.
- [ ] **Android** — ⚠️ not yet device-confirmed. A `RustCodeBlock`
  (HorizontalScrollView + TextView) with a `SpannableString` (one
  `ForegroundColorSpan` per run); one TextView regardless of token count.
- [ ] **macOS** — ⚠️ not yet device-confirmed. An `NSScrollView` wrapping an
  `NSTextField` label (`NSAttributedString` with per-run color).
- [ ] **terminal / gpu** — no handler registered; verify the framework's `External`
  placeholder renders cleanly (no layout artifact or crash).

### `code_editor`

The two failure modes to look for are **drift** (glyphs sliding away
from the caret, worst at the bottom of a long file) and **stale
highlight** (the decorated layer not following an edit).

- [x] **Web** — verified in `examples/fiddle`: both layers report identical
  font/size/line-height/padding/tab-size and the same bounding rect;
  `scrollHeight == clientHeight` on the textarea (neither layer scrolls
  internally); typing re-splits the runs in place; a dotted red
  `Underline` renders under text whose own colour is unchanged.
- [ ] **iOS** — ⚠️ not yet device-confirmed. Caret should track the glyphs
  down a 200-line file; check the software keyboard's IME composition
  updates the decorated layer.
- [ ] **Android** — ⚠️ not yet device-confirmed. Also exercises
  `RustUnderlineSpan` (the framework's own patterned-underline drawing);
  check a wrapped span underlines on every line it covers.
- [ ] **macOS** — ⚠️ not yet device-confirmed. Same drift check as iOS.
- [ ] **Tabs** — open a file with literal tab characters on each backend;
  the highlight must not slide one tab stop right of the glyphs.
