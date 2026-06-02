//! `idea-codeblock` — read-only colored-text panel primitive.
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
//! ## Per-backend rendering (single-node throughout)
//!
//! Every backend renders **one** native node per `code_block(...)` call:
//!
//! - **Web** — a `<pre>` containing one styled `<span>` per run
//!   (built via the `Backend` trait so SSR + hydration stay in lock
//!   step).
//! - **Android** — a `RustCodeBlock` (HorizontalScrollView + TextView)
//!   that sets a `SpannableString` with one `ForegroundColorSpan` per
//!   run. One TextView per code block, regardless of token count.
//! - **iOS** — a `UIScrollView` (horizontal) containing a `UILabel`
//!   whose `attributedText` is an `NSAttributedString` with per-run
//!   `NSForegroundColorAttributeName` ranges. One label per block.
//! - **macOS / terminal / gpu** — fall through to the framework's
//!   external-not-registered placeholder. Adding handlers there
//!   follows the same shape as iOS/Android.
//!
//! ## Usage
//!
//! ```ignore
//! use idea_codeblock::{code_block, CodeBlockProps};
//!
//! // At app bootstrap, once per backend:
//! idea_codeblock::register(&mut backend);
//!
//! // Inside an effect / arm body:
//! let spans: Vec<(String, Color)> = tokenize(source);
//! code_block(spans).with_style(my_codeblock_style())
//! ```
//!
//! On backends without a registered handler, the framework renders a
//! placeholder per its usual `Element::External` policy.

use runtime_core::{Bound, Color, ExternalHandle};

#[cfg(target_arch = "wasm32")]
use runtime_core::accessibility::AccessibilityProps;
#[cfg(target_arch = "wasm32")]
use runtime_core::{Backend, RegisterExternal, StyleRules, Tokenized};

#[cfg(target_arch = "wasm32")]
use std::rc::Rc;

#[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
mod android;
#[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
mod ios;

/// Type-erased payload for the CodeBlock external primitive. Lives
/// here because the framework dispatches handlers by [`TypeId`], and
/// the type needs to be the SAME type across the call site and the
/// backend leaf — so it lives in the umbrella crate that both depend
/// on.
///
/// [`TypeId`]: std::any::TypeId
#[derive(Clone)]
pub struct CodeBlockProps {
    /// Color-run sequence. Each tuple is one run of same-colored
    /// text. Consecutive same-color runs are NOT auto-coalesced —
    /// the tokenizer caller decides whether to coalesce. The per-
    /// backend handlers don't pay for the difference because every
    /// run lowers to a single `ForegroundColorSpan` /
    /// `NSForegroundColorAttributeName` range / `<span>` inside ONE
    /// outer native node.
    pub spans: Vec<(String, Color)>,
}

/// Convenience handle alias — saves callers writing
/// `Ref<ExternalHandle<CodeBlockProps>>` everywhere.
pub type CodeBlockHandle = ExternalHandle<CodeBlockProps>;

/// Construct a `CodeBlock` from a flat span list.
///
/// Always returns an `Element::External` keyed by [`CodeBlockProps`];
/// the per-backend handler installed via [`register`] decides how to
/// render it. Returns a `Bound<CodeBlockHandle>` so `.with_style(...)`
/// works the same way it would for any other primitive — the style
/// lands on the outer native node (the `<pre>` / `HorizontalScrollView`
/// / `UIScrollView`).
///
/// ```ignore
/// code_block(vec![
///     ("fn ".into(),    Color("#888".into())),
///     ("hello".into(),  Color("#0a0".into())),
///     ("() { … }".into(), Color("#444".into())),
/// ])
/// ```
pub fn code_block(spans: Vec<(String, Color)>) -> Bound<CodeBlockHandle> {
    // Register the wire serde here too: `code_block` runs while the app
    // builds its tree, including on the runtime-server RECORDER (which
    // runs app code headless). So the serializer is in place before the
    // recorder's `create_external` emits the wire command — no app-level
    // recorder registration needed.
    ensure_wire_serde();
    runtime_core::external::<CodeBlockProps>(CodeBlockProps { spans })
}

/// Register the wire (serialize, deserialize) pair for `CodeBlockProps`
/// so a `code_block(...)` `Element::External` renders over the
/// runtime-server wire: the recorder serializes the spans into
/// `CreateExternal`, and the device deserializes them back and dispatches
/// to its real per-backend handler. Without this, External payloads can't
/// cross the wire and the device shows the not-available placeholder.
///
/// Idempotent + cheap (guarded by a thread-local flag). Called from
/// [`code_block`] (covers the recorder side) and from every [`register`]
/// (covers the device client side).
fn ensure_wire_serde() {
    thread_local! {
        static DONE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }
    if DONE.with(|d| d.replace(true)) {
        return;
    }
    runtime_core::register_external_serde(
        std::any::type_name::<CodeBlockProps>(),
        |any| {
            let props = any.downcast_ref::<CodeBlockProps>()?;
            // `Color` is a `Color(String)` newtype; ship the runs as
            // (text, color-string) pairs.
            let plain: Vec<(&str, &str)> = props
                .spans
                .iter()
                .map(|(t, c)| (t.as_str(), c.0.as_str()))
                .collect();
            serde_json::to_vec(&plain).ok()
        },
        |bytes| {
            let plain: Vec<(String, String)> = serde_json::from_slice(bytes).ok()?;
            let spans = plain.into_iter().map(|(t, c)| (t, Color(c))).collect();
            Some(std::rc::Rc::new(CodeBlockProps { spans }) as std::rc::Rc<dyn std::any::Any>)
        },
    );
}

// =============================================================================
// Web / SSR — backend-neutral handler that uses the Backend trait so
// SSR + hydration share the same DOM shape.
// =============================================================================

#[cfg(target_arch = "wasm32")]
fn build_code_block<B: Backend>(props: &Rc<CodeBlockProps>, backend: &mut B) -> B::Node {
    let a11y = AccessibilityProps::default();
    let mut pre = backend.create_element("pre");
    for (text, color) in &props.spans {
        let span = backend.create_text(text, &a11y);
        let mut rules = StyleRules::default();
        rules.color = Some(Tokenized::Literal(color.clone()));
        backend.apply_style(&span, &Rc::new(rules));
        backend.insert(&mut pre, span);
    }
    pre
}

// =============================================================================
// Per-target `register` — one per backend type. The variant of `register`
// that the compiler picks is determined by the target triple, so app
// bootstrap can write `idea_codeblock::register(&mut backend)` once and
// not care which target it's compiling for.
// =============================================================================

/// Web (+ SSR via the same wasm32-target shell) — registers the
/// generic `build_code_block` handler against the backend's external
/// registry.
#[cfg(target_arch = "wasm32")]
pub fn register<B: RegisterExternal>(backend: &mut B) {
    ensure_wire_serde();
    backend.register_external::<CodeBlockProps, _>(build_code_block::<B>);
}

/// Android — registers the [`android::build`] handler. Produces a
/// single `RustCodeBlock` (HorizontalScrollView + TextView with
/// SpannableString). See `android.rs` for the JNI plumbing.
#[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
pub fn register(backend: &mut backend_android::AndroidBackend) {
    ensure_wire_serde();
    backend.register_external::<CodeBlockProps, _>(android::build);
}

/// iOS — registers the [`ios::build`] handler. Produces a single
/// UIScrollView (horizontal) wrapping a UILabel with
/// NSAttributedString. See `ios.rs` for the obj-c plumbing.
#[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
pub fn register(backend: &mut backend_ios::IosBackend) {
    ensure_wire_serde();
    backend.register_external::<CodeBlockProps, _>(ios::build);
}

/// Fallback for other targets (macOS / terminal / gpu). No-op generic
/// over any `Backend`. Authors get the framework's standard
/// external-not-registered placeholder until a per-backend handler
/// lands.
#[cfg(all(
    not(target_arch = "wasm32"),
    not(target_os = "android"),
    not(target_os = "ios"),
))]
pub fn register<B: runtime_core::Backend>(_backend: &mut B) {
    // No per-backend native handler here, but still register the wire
    // serde so the recorder (which falls into this generic variant)
    // serializes the payload.
    ensure_wire_serde();
}
