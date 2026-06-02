# `codeblock`

A read-only colored-text panel primitive on `Element::External`. A flat
sequence of `(text, color)` runs rendered as a **single native node** on
every backend — built for syntax-highlighted source display. The docs
site renders ~140-line tokenized snippets and ships dozens per page.

```rust
use codeblock::{code_block, CodeBlockProps};
use runtime_core::Color;

// At app bootstrap, once per backend:
codeblock::register(&mut backend);

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
| Web (+ SSR) | A `<pre>` with one styled `<span>` per run, built through the `Backend` trait so SSR + hydration stay in lockstep. |
| Android | A `RustCodeBlock` (HorizontalScrollView + TextView) with a `SpannableString` carrying one `ForegroundColorSpan` per run. One TextView regardless of token count. |
| iOS | A horizontal `UIScrollView` wrapping a `UILabel` whose `attributedText` is an `NSAttributedString` with per-run `NSForegroundColorAttributeName` ranges. One label per block. |
| macOS / terminal / gpu | Fall through to the framework's external-not-registered placeholder. Adding handlers follows the iOS/Android shape. |

`.with_style(...)` lands on the outer native node (the `<pre>` /
`HorizontalScrollView` / `UIScrollView`).

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
in a third-party extension via `Element::External`. So the fast
single-node renderer stayed, but the type moved out of core.

## Over the runtime-server wire

[`code_block`] registers a wire serde pair for [`CodeBlockProps`]
automatically (idempotent, thread-local guarded): the recorder
serializes the spans into `CreateExternal`, the device deserializes and
dispatches to its real per-backend handler. Without this, the External
payload couldn't cross the wire and the device would show the
not-available placeholder. Registration happens from both [`code_block`]
(recorder side) and every [`register`] (device side), so no app-level
recorder wiring is needed.

[`code_block`]: src/lib.rs
[`register`]: src/lib.rs
[`CodeBlockProps`]: src/lib.rs
