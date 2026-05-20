//! `TextSource` + `StyleSource` and their `Into*Source` constructor
//! traits.
//!
//! These are foundational types — `Primitive` variants store them
//! directly, and the public-facing primitive constructors (`text`,
//! `button`, `with_style`, …) accept anything implementing the
//! associated `Into*Source` trait so author code can pass static
//! values OR closures uniformly.

use crate::style::{StyleApplication, StyleSheet};
use std::rc::Rc;

// =============================================================================
// TextSource + IntoTextSource
// =============================================================================

/// Source for a text node. `Static` ships the string verbatim;
/// `Bound` carries a `Derived<String>` so backends pick how to
/// realize it — runtime backends call `compute` inside an Effect
/// and re-paint the text; generator backends serialize the
/// `method` + `inputs` and the device-side runtime dispatches the
/// transpiled named function on every signal change.
pub enum TextSource {
    Static(String),
    Bound(crate::derive::Derived<String>),
    /// Pre-decomposed reactive text binding suitable for backends
    /// that can run the per-fire fan-out entirely on their own side
    /// (web backend: JS-side reactive layer; future native backends:
    /// possibly platform-specific binding primitives). Carries the
    /// signal ids + template static parts + initial values directly,
    /// so the walker can hand the structured data straight to the
    /// backend without running a Rust closure per fire.
    ///
    /// `compute_fallback` is the same closure the `Bound` variant
    /// would carry — it gets invoked through the legacy Effect path
    /// on backends that DON'T support JS bindings (default-impl
    /// `Backend::supports_js_text_bindings` returns `false`). The
    /// variant author writes the binding once via [`crate::text_fmt!`]
    /// or by constructing `JsBindingSpec` directly, and the
    /// framework picks the fast or fallback path based on the
    /// active backend's capabilities.
    JsBinding(JsBindingSpec),
}

/// Pre-decomposed reactive text binding shipped via
/// [`TextSource::JsBinding`].
///
/// For `"leaf {}: g={}"` with `id=42` (captured) and `global`
/// (a `Signal<u32>`), the bench's structured form is:
///
/// ```ignore
/// JsBindingSpec {
///     signal_ids: vec![global.id()],
///     template_parts: vec![
///         "leaf 42: g=".into(),  // captured `id` baked in
///         "".into(),
///     ],
///     initial_values: vec!["0".into()],
///     compute_fallback: Rc::new(move || format!("leaf {}: g={}", id, global.get())),
/// }
/// ```
///
/// `template_parts` has exactly `signal_ids.len() + 1` entries —
/// the static text surrounding each signal interpolation slot,
/// with any captured (non-signal) values pre-formatted into the
/// adjacent parts.
pub struct JsBindingSpec {
    pub signal_ids: Vec<u64>,
    pub template_parts: Vec<String>,
    pub initial_values: Vec<String>,
    /// Same shape as `Derived<String>::compute` — used by the
    /// fallback Effect path on backends that don't support JS
    /// bindings. Built from the same expression that
    /// `compute_fallback` would otherwise wrap.
    pub compute_fallback: std::rc::Rc<dyn Fn() -> String>,
}

/// Allows `text(...)` to accept strings, owned strings, or closures.
/// The `#[component]` macro rewrites reactive call sites into closures;
/// this trait makes the rewrite type-check transparently.
pub trait IntoTextSource {
    fn into_text_source(self) -> TextSource;
}

impl IntoTextSource for &str {
    fn into_text_source(self) -> TextSource {
        TextSource::Static(self.to_string())
    }
}

impl IntoTextSource for String {
    fn into_text_source(self) -> TextSource {
        TextSource::Static(self)
    }
}

impl<F> IntoTextSource for F
where
    F: Fn() -> String + 'static,
{
    fn into_text_source(self) -> TextSource {
        // Opaque closure path: coerce into a `Derived<String>`
        // with no structured metadata. Runtime backends use
        // `compute`; generator backends report a build-time error
        // if they receive one (no method name to dispatch).
        TextSource::Bound(crate::derive::IntoDerived::<String>::into_derived(self))
    }
}

/// Identity passthrough so macro-generated `TextSource` values (e.g.
/// from `bind!`) can be used at the same call sites that accept
/// `&str` / `String` / closures. Without this, `Text { bind!(..) }`
/// would fail to type-check because the `IntoTextSource` blanket
/// `Fn()` impl above doesn't cover a fully-constructed `TextSource`.
impl IntoTextSource for TextSource {
    fn into_text_source(self) -> TextSource {
        self
    }
}

// =============================================================================
// StyleSource + IntoStyleSource
// =============================================================================

/// A style source. `Static` is a fixed `StyleApplication` known at
/// build time — no signal subscriptions, no per-node `Effect`. The
/// node is registered with the global theme cohort so a `set_theme`
/// call re-applies it in bulk. `Reactive` is a closure that re-runs
/// inside an `Effect`; signals it reads become deps and changes
/// re-fire the apply path. Most styles are `Static`; the `Reactive`
/// path exists for cases that need per-node reactive overrides.
///
/// The split matters at scale: 10 000 styled rows used to allocate
/// 10 000 `Effect`s + 10 000 `Box<dyn Fn>` closures + 10 000 entries
/// in the active-theme signal's subscriber set. With `Static`, those
/// per-node allocations vanish — the cohort holds a single entry per
/// node and a single `Effect` subscribes to the theme on behalf of
/// the whole set.
pub enum StyleSource {
    Static(StyleApplication),
    Reactive(Box<dyn Fn() -> StyleApplication>),
}

/// Allows `with_style(...)` to accept any of:
///   - a bare `Rc<StyleSheet>` — applies the stylesheet with no
///     variant selection, no overrides. Best for static one-shot
///     styles like `banner_style()`.
///   - a fixed `StyleApplication` — for the case where you already
///     have a built-up application with variants/overrides.
///   - a closure returning a `StyleApplication` — enables reactive
///     styling: signals read inside the closure become dependencies
///     and changes re-fire the apply-style effect.
///
/// The `Rc<StyleSheet>` impl exists so authors don't have to write
/// `StyleApplication::new(sheet)` for the trivial case — most styles
/// are like that, and the wrapping was pure ceremony.
pub trait IntoStyleSource {
    fn into_style_source(self) -> StyleSource;
}

impl IntoStyleSource for Rc<StyleSheet> {
    fn into_style_source(self) -> StyleSource {
        StyleSource::Static(StyleApplication::new(self))
    }
}

impl IntoStyleSource for StyleApplication {
    fn into_style_source(self) -> StyleSource {
        StyleSource::Static(self)
    }
}

impl<F> IntoStyleSource for F
where
    F: Fn() -> StyleApplication + 'static,
{
    fn into_style_source(self) -> StyleSource {
        StyleSource::Reactive(Box::new(self))
    }
}
