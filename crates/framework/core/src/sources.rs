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

/// Source for a text node. Static is rendered once; Reactive is wrapped in
/// an `Effect` during rendering so the node updates whenever its dependencies
/// change. Author code never names this enum directly.
///
/// `Bound` is a structured reactive form: it carries the same
/// closure as `Reactive` (so Effect-driven backends behave
/// identically) plus *symbolic* metadata — which signals are read
/// and the name of the transformer function — that backends with
/// declarative wire formats can use to ship a binding to a remote
/// renderer instead of round-tripping every change through the
/// host. Each backend reads only what it needs: closure-driven
/// backends use `closure`; backends that ship bindings read
/// `signal_ids` + `method`. The two views of intent live in one
/// value so authors write the binding once.
pub enum TextSource {
    Static(String),
    Reactive(Box<dyn Fn() -> String>),
    Bound {
        /// Reactive closure for Effect-driven backends. Same shape
        /// as `Reactive` — registered with `Effect::new(..)` by the
        /// walker so the text re-flows on signal change.
        closure: Box<dyn Fn() -> String>,
        /// Signal arena IDs the binding reads. Captured at macro
        /// expansion via `Signal::id()`. Opaque to the framework;
        /// only backends that consume bindings interpret them.
        signal_ids: Vec<u64>,
        /// Symbolic name of the transformer. Resolution is the
        /// consuming backend's concern — typically the name of a
        /// platform-specific helper the build pipeline emits
        /// alongside the binding.
        method: &'static str,
        /// Snapshot of each signal's value at binding construction
        /// time, parallel to `signal_ids`. Used by backends that
        /// ship signals across a wire boundary (and therefore need
        /// the initial value declaratively, since they can't reach
        /// back into the framework's arena at runtime). Closure-
        /// driven backends ignore this field — the framework's
        /// `Effect` reads live signal values directly.
        ///
        /// The macro that produced this binding (`bind!`) captured
        /// each value via `serde_json::to_value(&signal.get())`, so
        /// the type appears as `serde_json::Value` regardless of
        /// the originating `Signal<T>` type parameter.
        initial_values: Vec<crate::__serde_json::Value>,
    },
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
        TextSource::Reactive(Box::new(self))
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
// ButtonAction + IntoButtonAction
// =============================================================================

/// What a button does when activated. Always carries a `closure`
/// (the Effect-driven path every backend can run), and optionally
/// an [`ActionBinding`] that backends with declarative wire formats
/// can ship to a remote renderer instead of round-tripping every
/// press through the host.
///
/// Closure-driven backends (iOS, Android, Web) use only `closure`.
/// Backends like Roku read `binding` to emit a wire command that
/// wires the press event on-device.
pub struct ButtonAction {
    pub closure: Rc<dyn Fn()>,
    pub binding: Option<ActionBinding>,
}

/// Metadata for a press handler that should ship across a wire
/// boundary. Mirrors the shape of [`TextSource::Bound`] —
/// `input_signal_ids` plus `method` is enough for a declarative
/// backend to fire the right transformer on the remote side.
/// `output_signal_id`, when present, instructs the remote runtime
/// to write the method's return value back into that signal,
/// which propagates through any text bindings subscribed to it.
pub struct ActionBinding {
    pub input_signal_ids: Vec<u64>,
    pub method: &'static str,
    pub output_signal_id: Option<u64>,
    pub initial_values: Vec<crate::__serde_json::Value>,
}

/// Allows `button(..., on_click: ...)` to accept either a bare
/// `Fn()` closure (legacy / non-bound) or a fully-built
/// `ButtonAction` (produced by `bind_press!`).
pub trait IntoButtonAction {
    fn into_button_action(self) -> ButtonAction;
}

impl<F> IntoButtonAction for F
where
    F: Fn() + 'static,
{
    fn into_button_action(self) -> ButtonAction {
        ButtonAction {
            closure: Rc::new(self),
            binding: None,
        }
    }
}

impl IntoButtonAction for ButtonAction {
    fn into_button_action(self) -> ButtonAction {
        self
    }
}

// =============================================================================
// WhenBinding — declarative metadata for `Primitive::When`
// =============================================================================

/// Optional metadata attached to a `Primitive::When` by the
/// `bind_when!` macro, surfacing the dependency information that
/// backends with declarative wire formats need to ship a reactive
/// conditional to a remote renderer.
///
/// Mirrors the shape of [`ActionBinding`] / `TextSource::Bound`'s
/// metadata: signal IDs, the name of the boolean transformer
/// (`#[method]`) the renderer dispatches to evaluate the condition,
/// and a snapshot of each input's initial value so the renderer can
/// declare the signals it doesn't yet know about.
///
/// Effect-driven backends ignore this — they read `cond` and
/// reactivate via the closure. Backends that opt in via
/// `Backend::handles_when_natively` read this and call
/// `Backend::note_when_binding` instead of running the closure
/// inside an `Effect`.
pub struct WhenBinding {
    pub signal_ids: Vec<u64>,
    pub cond_method: &'static str,
    pub initial_values: Vec<crate::__serde_json::Value>,
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
