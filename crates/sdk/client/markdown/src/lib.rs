//! `markdown` — a CommonMark/GFM document primitive rendered as a
//! **single native styled-text node** per backend.
//!
//! ## Why a single node (performance)
//!
//! A markdown document is a deep tree: blocks (headings, paragraphs,
//! lists, quotes, code) containing inline runs (bold, italic, code,
//! links). Lowering that tree to framework primitives would emit one
//! styled `text`/`view` node *per inline run* — the exact per-token
//! explosion the `codeblock` SDK was carved out of `runtime-core` to
//! avoid (a measured 100–300× more backend ops per render). Native
//! rich-text engines express the whole tree as inline attribute ranges
//! on ONE widget, so a 50-block document is ONE native node, not
//! thousands.
//!
//! - **iOS** — one `UILabel` whose `attributedText` is an
//!   `NSAttributedString` with per-range font/size/color/background/
//!   underline/strikethrough attributes. Wraps to the column width via a
//!   width-aware Taffy measure (`install_external_wrapping_measure`).
//! - **Android** — one `android.widget.TextView` fed a
//!   `SpannableStringBuilder` carrying `RelativeSizeSpan` / `StyleSpan`
//!   / `TypefaceSpan` / `ForegroundColorSpan` / `BackgroundColorSpan` /
//!   `UnderlineSpan` / `StrikethroughSpan` ranges. A plain TextView gets
//!   width-aware wrapping measurement automatically.
//! - **Web** — semantic DOM (`<h1>`, `<p>`, `<pre>`, `<ul>`,
//!   `<blockquote>`, `<hr>`) built through the `Backend` trait, with
//!   per-run inline styling. DOM layout is cheap and the semantic tree
//!   is accessible, so web keeps real elements rather than one node.
//! - **Other targets** (macOS / terminal / gpu) — the framework's
//!   external-not-registered placeholder until a handler lands.
//!
//! ## One authored surface, two cores
//!
//! The default build is the old-core implementation (`Element::External`
//! payload + per-backend `ExternalRegistry`), byte-moved into
//! [`oldcore`]; the `new-core` feature swaps in [`newcore`], which
//! re-expresses the SAME public call shape (`ui! { Markdown(source =
//! …) }` / `markdown(src, theme).with_style(…)` + a bootstrap
//! `register`) over the scene registry — the new core's unified
//! primitive==external contract. Mutually exclusive (same names),
//! mirroring the codeblock/svg/table SDK precedents.
//!
//! Because the old WEB handler was generic over the `Backend` trait,
//! the new-core handler is a caps-GENERIC scene handler (`StyleServices
//! + TextOps`) shared by EVERY caps-complete host — web and SSR both
//! mount the identical semantic DOM (an upgrade for SSR: the old-core
//! host-side `register` never installed the wasm32-gated web handler,
//! so old SSR fell back to the External-placeholder `<div>`). The
//! native single-node handlers (iOS/Android) stay old-core-only until
//! the new-core native boots grow an external story — the codeblock
//! precedent.
//!
//! ## Styling + theming
//!
//! Parsing and theme *resolution* happen author-side, inside the
//! [`Markdown`] component's reactive scope, producing a fully-resolved,
//! serializable [`MarkdownDoc`] (blocks + a concrete [`MdTheme`]). The
//! [`MdTheme`] is the SDK's complete styling surface — a color/size per
//! element type. Because the component reads its `source`/`theme` props
//! reactively, a theme toggle re-resolves the doc → new props for the
//! one node, which is rebuilt with the new colors. See the
//! `markdown-demo` example for a light/dark toggle.
//!
//! ## Usage
//!
//! ```ignore
//! use markdown::{Markdown, MdTheme};
//!
//! // At app bootstrap, once per backend (old core; on the new core
//! // pass `markdown::register` as the boot registration seam, e.g.
//! // `backend_web::newcore::start_in("#app", markdown::register, app)`):
//! markdown::register(&mut backend);
//!
//! // In a component tree:
//! ui! { Markdown(source = "# Hello\n\nWorld **bold**".to_string()) }
//!
//! // Or the low-level builder (matches `code_block`):
//! markdown::markdown("# Hi", MdTheme::dark()).with_style(my_panel_style())
//! ```
#![deny(missing_docs)]

mod ir;
mod parse;

// Native single-node handlers share the block-tree → linear-segment
// lowering. Old-core-only: the new-core native boots have no external
// story yet (codeblock precedent).
#[cfg(all(
    not(target_arch = "wasm32"),
    any(target_os = "ios", target_os = "android"),
    not(feature = "new-core")
))]
mod segments;

#[cfg(all(
    target_os = "android",
    not(target_arch = "wasm32"),
    not(feature = "new-core")
))]
mod android;
#[cfg(all(target_os = "ios", not(target_arch = "wasm32"), not(feature = "new-core")))]
mod ios;
#[cfg(all(target_arch = "wasm32", not(feature = "new-core")))]
mod web;

// The resolved IR + parser are pure data (serde, no core types) and are
// SHARED between the two cores untouched.
pub use ir::{MarkdownDoc, MdBlock, MdListItem, MdRun, MdTheme};

// One authored surface, two cores: the default build is the old-core
// implementation, byte-moved into `oldcore`; the `new-core` feature
// swaps in `newcore`, which re-expresses the SAME public names over the
// scene registry. Mutually exclusive (same names).
#[cfg(not(feature = "new-core"))]
mod oldcore;
#[cfg(not(feature = "new-core"))]
pub use oldcore::*;

#[cfg(feature = "new-core")]
mod newcore;
#[cfg(feature = "new-core")]
pub use newcore::*;
