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
pub enum TextSource {
    Static(String),
    Reactive(Box<dyn Fn() -> String>),
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
