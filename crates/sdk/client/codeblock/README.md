# `codeblock`

A read-only colored-text panel primitive. A flat
sequence of `(text, color)` runs rendered as a **single native node** on
every backend — built for syntax-highlighted source display. The docs
site renders ~140-line tokenized snippets and ships dozens per page.

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
