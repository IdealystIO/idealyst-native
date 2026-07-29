//! `codeblock` — read-only colored-text panel primitive.
//!
//! A flat sequence of `(text, color)` runs rendered as a **single
//! native node** on every backend. Built for syntax-highlighted source
//! display — the docs site renders ~140 line tokenized snippets and
//! ships dozens of them per page.
//!
//! ## Why this is a third-party primitive, not a framework one
//!
//! It used to be `Element::CodeBlock` in `runtime-core`. A measurement
//! showed the perf justification was real: the equivalent composition
//! (`View` + per-token styled `Text`) generates 100–300× more backend
//! ops per re-render even with batched fast paths. The structural gap
//! (composition rebuilds every span on each render; the single-node
//! primitive replaces one node) can't be closed by framework
//! optimization alone.
//!
//! But the primitive doesn't fit runtime-core's intent — it isn't a
//! platform-native widget and is expressible from existing primitives
//! if perf weren't a concern. CLAUDE.md rule 3 says exactly this case
//! belongs in a third-party extension via `Element::External`. So we
//! kept the fast single-node renderer but moved the type out of core.
//!
//! ## One authored surface, two cores
//!
//! The default build is the old-core implementation (`Element::External`
//! payload + per-backend `ExternalRegistry`), byte-moved into
//! [`oldcore`]; the `new-core` feature swaps in [`newcore`], which
//! re-expresses the SAME public call shape
//! (`code_block(spans).with_style(…)` + a bootstrap `register`) over
//! the scene registry — the new core's unified primitive==external
//! contract — so a consuming app compiles the same source against
//! either core. Mutually exclusive (same names), mirroring the
//! build-graph-wide macro-lowering switch. The new-core web/SSR handler
//! reproduces the old handler's `<pre>`/span DOM byte-for-byte (see
//! `newcore.rs`).
//!
//! ## Per-backend rendering (single-node throughout)
//!
//! Every backend renders **one** native node per `code_block(...)` call:
//!
//! - **Web** — a `<pre>` containing one styled `<span>` per run
//!   (built via the `Backend` trait so SSR + hydration stay in lock
//!   step; the new-core leg drives the same calls through the caps
//!   traits).
//! - **Android** — a `RustCodeBlock` (HorizontalScrollView + TextView)
//!   that sets a `SpannableString` with one `ForegroundColorSpan` per
//!   run. One TextView per code block, regardless of token count.
//! - **iOS** — a `UIScrollView` (horizontal) containing an inset-honoring
//!   label whose `attributedText` is an `NSAttributedString` with per-run
//!   `NSForegroundColorAttributeName` ranges. One label per block.
//! - **macOS** — an `NSScrollView` (horizontal) wrapping an `NSTextField`
//!   label with per-run `NSColor` ranges. Same shape as iOS.
//! - **terminal / gpu** — fall through to the framework's
//!   external-not-registered placeholder. Adding handlers there
//!   follows the same shape as iOS/Android.
//!
//! The native handlers are old-core-only today (new-core native boots
//! don't exist yet); the new-core leg covers web + SSR, the two
//! surfaces new-core apps actually boot.
//!
//! ## Padding is author-driven
//!
//! Handlers apply NO padding of their own. The author's `padding_*` (via
//! `.with_style` on the `code_block`) lands on the outer node; each
//! backend realizes it INSIDE the scroll region (CSS padding on the web
//! `<pre>`, label `textInsets` on iOS, documentView offset on macOS,
//! `setPadding` + `clipToPadding=false` on Android), so the padding
//! scrolls WITH the content — text sits inset at rest and reaches the
//! block's own edge mid-scroll, the `<pre> { padding }` observable model.
//! Handlers previously hard-coded a 20pt/dp inset; combined with author
//! padding on a wrapping panel that doubled the visible padding and
//! clipped scrolled content before the block's edge (and made overlay
//! use-cases like the fiddle editor impossible to align).
//!
//! ## Usage
//!
//! ```ignore
//! use codeblock::code_block;
//!
//! // At app bootstrap, once per backend:
//! codeblock::register(&mut backend);          // old core
//! // …or on the new core, as the boot registration seam:
//! backend_web::newcore::start_in("#app", codeblock::register, app);
//!
//! // Inside an effect / arm body:
//! let spans: Vec<(String, Color)> = tokenize(source);
//! code_block(spans).with_style(my_codeblock_style())
//! ```
//!
//! On backends without a registered handler, the framework renders a
//! placeholder per its usual `Element::External` policy.
#![deny(missing_docs)]

#[cfg(all(
    target_os = "android",
    not(target_arch = "wasm32"),
    not(feature = "new-core")
))]
mod android;
#[cfg(all(target_os = "ios", not(target_arch = "wasm32"), not(feature = "new-core")))]
mod ios;
#[cfg(all(
    target_os = "macos",
    not(target_arch = "wasm32"),
    not(feature = "new-core")
))]
mod macos;

// One authored surface, two cores: the default build is the old-core
// implementation, byte-moved into `oldcore`; the `new-core` feature
// swaps in `newcore`, which re-expresses the SAME public call shape
// over the scene registry. Mutually exclusive (same names).
#[cfg(not(feature = "new-core"))]
mod oldcore;
#[cfg(not(feature = "new-core"))]
pub use oldcore::*;

#[cfg(feature = "new-core")]
mod newcore;
#[cfg(feature = "new-core")]
pub use newcore::*;
