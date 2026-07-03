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
//! ## Styling + theming
//!
//! Parsing and theme *resolution* happen author-side, inside the
//! [`Markdown`] component's reactive scope, producing a fully-resolved,
//! serializable [`MarkdownDoc`] (blocks + a concrete [`MdTheme`]). The
//! [`MdTheme`] is the SDK's complete styling surface — a color/size per
//! element type. Because the component reads its `source`/`theme` props
//! reactively, a theme toggle re-resolves the doc → new `Element::
//! External` props → the one native node is rebuilt with the new colors.
//! See the `markdown-demo` example for a light/dark toggle.
//!
//! ## Usage
//!
//! ```ignore
//! use markdown::{Markdown, MdTheme};
//!
//! // At app bootstrap, once per backend:
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
// lowering.
#[cfg(all(
    not(target_arch = "wasm32"),
    any(target_os = "ios", target_os = "android")
))]
mod segments;

#[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
mod android;
#[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
mod ios;
#[cfg(target_arch = "wasm32")]
mod web;

use std::any::Any;
use std::rc::Rc;

use runtime_core::{component, Bound, Element, ExternalHandle, IdealystSchema, Reactive};

pub use ir::{MarkdownDoc, MdBlock, MdListItem, MdRun, MdTheme};

/// Convenience handle alias — the typed `Ref` target for a markdown
/// external node. Saves callers writing `ExternalHandle<MarkdownDoc>`.
pub type MarkdownHandle = ExternalHandle<MarkdownDoc>;

/// Props for the [`Markdown`] component.
///
/// Both props are [`Reactive`], so passing a live `Signal`/`rx!` (for
/// either the source text or the theme) makes the rendered document
/// update — the single native node is rebuilt on change.
#[derive(IdealystSchema)]
pub struct MarkdownProps {
    /// The CommonMark/GFM source to render. Static or reactive.
    pub source: Reactive<String>,
    /// Per-element-type resolved styling. Default is [`MdTheme::light`];
    /// pass a reactive theme (e.g. `rx!(if dark.get() { MdTheme::dark() }
    /// else { MdTheme::light() })`) to follow an app theme toggle.
    pub theme: Reactive<MdTheme>,
}

impl Default for MarkdownProps {
    fn default() -> Self {
        Self {
            source: Reactive::Static(String::new()),
            theme: Reactive::Static(MdTheme::light()),
        }
    }
}

/// Render a markdown document.
///
/// Parses `source` and paints it with `theme`, rebuilding the single
/// native node whenever either prop changes. On a backend without a
/// registered handler the framework shows its external placeholder.
#[component]
pub fn Markdown(props: &MarkdownProps) -> Element {
    let source = props.source.clone();
    let theme = props.theme.clone();
    // Reactive region: rebuild the one external node whenever the source
    // text or the resolved theme changes. `ui!` has no ergonomic form
    // for a reactive region keyed on a `(String, MdTheme)` tuple, so we
    // call `switch` directly — the documented direct-call form (see
    // `runtime_core::switch`). `switch` re-runs the branch only when the
    // tuple's `PartialEq` value actually differs, so static props build
    // exactly once.
    runtime_core::switch(
        move || (source.get(), theme.get()),
        move |key| {
            let (src, th) = key;
            markdown(src.clone(), th.clone()).into()
        },
    )
}

/// Low-level builder: construct a markdown `Element::External` from a
/// source string + resolved theme. Mirrors `codeblock::code_block` —
/// returns a `Bound<MarkdownHandle>` so `.with_style(...)` lands on the
/// outer native node (the container `<div>` / `UILabel` / `TextView`).
///
/// Prefer the [`Markdown`] component for reactive source/theme; this is
/// the escape hatch for one-shot rendering or custom plumbing.
pub fn markdown(source: impl Into<String>, theme: MdTheme) -> Bound<MarkdownHandle> {
    // Register the wire serde here too: `markdown` runs while the app
    // builds its tree, including on the runtime-server RECORDER (headless
    // app code). So the serializer is in place before the recorder's
    // `create_external` emits the wire command — no app-level recorder
    // wiring needed (codeblock pattern).
    ensure_wire_serde();
    let doc = parse::parse(&source.into(), theme);
    runtime_core::external::<MarkdownDoc>(doc)
}

/// Register the wire (serialize, deserialize) pair for [`MarkdownDoc`] so
/// a `markdown(...)` external renders over the runtime-server wire: the
/// recorder serializes the resolved doc into `CreateExternal`, the device
/// deserializes it and dispatches to its real per-backend handler.
///
/// Idempotent + cheap (thread-local guard). Called from [`markdown`]
/// (recorder side) and every [`register`] (device side).
fn ensure_wire_serde() {
    thread_local! {
        static DONE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }
    if DONE.with(|d| d.replace(true)) {
        return;
    }
    runtime_core::register_external_serde(
        std::any::type_name::<MarkdownDoc>(),
        |any| {
            let doc = any.downcast_ref::<MarkdownDoc>()?;
            serde_json::to_vec(doc).ok()
        },
        |bytes| {
            let doc: MarkdownDoc = serde_json::from_slice(bytes).ok()?;
            Some(Rc::new(doc) as Rc<dyn Any>)
        },
    );
}

// =============================================================================
// Per-target `register` — the compiler picks the variant by target triple,
// so app bootstrap writes `markdown::register(&mut backend)` once.
// =============================================================================

/// Web (+ SSR) — registers the semantic-DOM handler.
#[cfg(target_arch = "wasm32")]
pub fn register<B: runtime_core::RegisterExternal>(backend: &mut B) {
    ensure_wire_serde();
    backend.register_external::<MarkdownDoc, _>(web::build::<B>);
}

// Self-register at backend construction. See [[project_inventory_self_registration]].
#[cfg(target_arch = "wasm32")]
inventory::submit! {
    backend_web::WebExternalRegistrar(register::<backend_web::WebBackend>)
}

/// Android — registers the `android` handler (one `TextView` +
/// `SpannableStringBuilder`).
#[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
pub fn register(backend: &mut backend_android::AndroidBackend) {
    ensure_wire_serde();
    backend.register_external::<MarkdownDoc, _>(android::build);
}

// Self-register at backend construction. See [[project_inventory_self_registration]].
#[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
inventory::submit! {
    backend_android::AndroidExternalRegistrar(register)
}

/// iOS — registers the `ios` handler (one `UILabel` +
/// `NSAttributedString`).
#[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
pub fn register(backend: &mut backend_ios::IosBackend) {
    ensure_wire_serde();
    backend.register_external::<MarkdownDoc, _>(ios::build);
}

// Self-register at backend construction. See [[project_inventory_self_registration]].
#[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
inventory::submit! {
    backend_ios::IosExternalRegistrar(register)
}

/// Fallback for other targets (macOS / terminal / gpu). No native
/// handler yet — still registers the wire serde so the recorder (which
/// compiles into this generic variant) serializes the payload.
#[cfg(all(
    not(target_arch = "wasm32"),
    not(target_os = "android"),
    not(target_os = "ios"),
))]
pub fn register<B: runtime_core::Backend>(_backend: &mut B) {
    ensure_wire_serde();
}
