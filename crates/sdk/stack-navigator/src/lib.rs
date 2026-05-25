//! First-party Stack navigator SDK.
//!
//! Author-facing API mirrors the legacy `runtime_core::Navigator`
//! builder so existing app code can switch by replacing one import.
//! The implementation routes through `Primitive::Navigator`, which
//! the framework dispatches via the per-backend
//! [`NavigatorRegistry`](runtime_core::primitives::navigator::NavigatorRegistry)
//! to a `StackHandler` registered by this crate's `register(&mut backend)`
//! function.
//!
//! # Usage
//!
//! ```ignore
//! // App bootstrap:
//! let mut backend = WebBackend::new("#app");
//! stack_navigator::register(&mut backend);
//!
//! // Inside the app:
//! let home = Route::<()>::new("home", "/");
//! let nav: Ref<StackHandle> = Ref::new();
//! stack_navigator::Navigator::new(&home)
//!     .screen(home.clone(), |_| Screen::new(...))
//!     .bind(nav.clone())
//! ```

use runtime_core::primitives::navigator::{
    DefaultLinkKind, NavigatorConfig, NavigatorHandle, Route, RouteEntry, RouteParams,
    ScreenBuilder, ScreenOptions,
};
use runtime_core::{Bound, IntoStyleSource, Primitive, Ref, RefFill};
use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::rc::Rc;

// =============================================================================
// StackPresentation — payload type that drives registry dispatch
// =============================================================================

/// Stack-kind presentation payload. The framework hands this to the
/// `StackHandler` registered for `TypeId::of::<StackPresentation>()`.
///
/// Carries only what the backend handler needs to render the native
/// stack — the screen registry, default options, and routing config
/// live in the shared `NavigatorConfig` carried by the same
/// `Primitive::Navigator`.
#[derive(Default)]
pub struct StackPresentation {
    /// Per-slot styles the handler dispatches via `apply_slot_style`.
    /// SDK slot names: `"header"`, `"title"`, `"button"`, `"body"`.
    /// Empty unless the author wired styles on the builder.
    pub slot_keys: Vec<&'static str>,
}

// =============================================================================
// StackHandle — typed handle for `.bind(...)`
// =============================================================================

/// Imperative handle to a mounted stack navigator. Wraps the underlying
/// `NavigatorHandle` and exposes only push/pop/replace/reset semantics.
///
/// Stack-specific method set matches the legacy `NavigatorHandle` API
/// so existing call sites carry over by import-renaming.
#[derive(Clone)]
pub struct StackHandle {
    inner: NavigatorHandle,
}

impl StackHandle {
    pub fn from_inner(inner: NavigatorHandle) -> Self {
        Self { inner }
    }

    /// Push a new screen onto the stack.
    pub fn push<P: RouteParams>(&self, route: &Route<P>, params: P) {
        self.inner.push(route, params);
    }

    /// Pop the top screen (no-op when only the root is mounted).
    pub fn pop(&self) {
        self.inner.pop();
    }

    /// Replace the top screen without changing stack depth.
    pub fn replace<P: RouteParams>(&self, route: &Route<P>, params: P) {
        self.inner.replace(route, params);
    }

    /// Clear the stack and mount `route` as the new root.
    pub fn reset<P: RouteParams>(&self, route: &Route<P>, params: P) {
        self.inner.reset(route, params);
    }

    /// Current stack depth (1 = only root).
    pub fn depth(&self) -> usize {
        self.inner.depth()
    }

    pub fn inner(&self) -> &NavigatorHandle {
        &self.inner
    }
}

// =============================================================================
// Builder — produces Primitive::Navigator
// =============================================================================

/// Stack-navigator builder. Mirrors the legacy `runtime_core::Navigator`
/// surface; under the hood it produces `Primitive::Navigator` with
/// a `StackPresentation` payload + `NavigatorConfig` shared config.
pub struct Navigator {
    config: NavigatorConfig,
    slot_styles: Vec<(&'static str, runtime_core::StyleSource)>,
    style: Option<runtime_core::StyleSource>,
    ref_fill: Option<RefFill>,
}

impl Navigator {
    /// Begin building a stack navigator with `initial` as the root.
    /// The initial route must be `Route<()>` (no params) for the
    /// framework's `initial_path` resolution.
    pub fn new(initial: &Route<()>) -> Bound<StackHandle> {
        let nav = Self {
            config: NavigatorConfig {
                initial: initial.name(),
                initial_path: initial.path(),
                screens: HashMap::new(),
                layout: None,
                default_options: None,
                default_link_kind: DefaultLinkKind::Push,
                defer_initial_mount: false,
            },
            slot_styles: Vec::new(),
            style: None,
            ref_fill: None,
        };
        Bound::new(nav.into_primitive())
    }

    fn into_primitive(self) -> Primitive {
        let Navigator { config, slot_styles, style, ref_fill } = self;
        Primitive::Navigator {
            type_id: TypeId::of::<StackPresentation>(),
            type_name: std::any::type_name::<StackPresentation>(),
            presentation: Rc::new(StackPresentation::default()) as Rc<dyn Any>,
            config: Box::new(config),
            style,
            slot_styles,
            ref_fill,
            accessibility: Default::default(),
        }
    }
}

// =============================================================================
// Builder method extension trait — the orphan rule blocks inherent
// `impl Bound<StackHandle>` here (`Bound` lives in runtime-core), so
// the builder API is shipped as a trait. Bring `StackBuilder` into
// scope (or `use stack_navigator::prelude::*`) at the call site to use
// `.screen(...)`, `.bind(...)`, etc.
// =============================================================================

/// Builder method surface for `Bound<StackHandle>`. The orphan rule
/// keeps these out of an inherent impl; bringing this trait into scope
/// makes them available. The prelude re-exports it for convenience.
pub trait StackBuilder: Sized {
    fn screen<P, R, F>(self, route: Route<P>, render: F) -> Self
    where
        P: RouteParams + 'static,
        R: Into<runtime_core::primitives::navigator::Screen> + 'static,
        F: Fn(P) -> R + 'static;

    fn default_screen_options(self, opts: ScreenOptions) -> Self;

    fn layout<F>(self, f: F) -> Self
    where
        F: Fn(runtime_core::primitives::navigator::LayoutProps) -> Primitive + 'static;

    fn header_style(self, s: impl IntoStyleSource) -> Self;
    fn title_style(self, s: impl IntoStyleSource) -> Self;
    fn button_style(self, s: impl IntoStyleSource) -> Self;
    fn bind(self, r: Ref<StackHandle>) -> Self;
}

/// Helper: extract the Navigator primitive's mutable parts.
fn with_navigator_ext<F: FnOnce(&mut Primitive)>(b: &mut Bound<StackHandle>, f: F) {
    f(b.primitive_mut());
}

impl StackBuilder for Bound<StackHandle> {
    fn screen<P, R, F>(mut self, route: Route<P>, render: F) -> Self
    where
        P: RouteParams + 'static,
        R: Into<runtime_core::primitives::navigator::Screen> + 'static,
        F: Fn(P) -> R + 'static,
    {
        with_navigator_ext(&mut self, |p| {
            if let Primitive::Navigator { config, .. } = p {
                let builder: ScreenBuilder = Rc::new(move |any_params: Box<dyn Any>| {
                    let typed: Box<P> = any_params
                        .downcast::<P>()
                        .expect("stack-navigator: route params type mismatch on mount");
                    render(*typed).into()
                });
                let from_segments = Rc::new(|segs: &HashMap<String, String>| -> Option<Box<dyn Any>> {
                    P::from_segments(segs).map(|p| Box::new(p) as Box<dyn Any>)
                });
                config.screens.insert(route.name(), RouteEntry {
                    path: route.path(),
                    build: builder,
                    from_segments,
                });
            }
        });
        self
    }

    fn default_screen_options(mut self, opts: ScreenOptions) -> Self {
        with_navigator_ext(&mut self, |p| {
            if let Primitive::Navigator { config, .. } = p {
                config.default_options = Some(opts);
            }
        });
        self
    }

    fn layout<F>(mut self, f: F) -> Self
    where
        F: Fn(runtime_core::primitives::navigator::LayoutProps) -> Primitive + 'static,
    {
        with_navigator_ext(&mut self, |p| {
            if let Primitive::Navigator { config, .. } = p {
                config.layout = Some(Rc::new(f));
            }
        });
        self
    }

    fn header_style(mut self, s: impl runtime_core::IntoStyleSource) -> Self {
        with_navigator_ext(&mut self, |p| {
            if let Primitive::Navigator { slot_styles, .. } = p {
                slot_styles.push(("header", s.into_style_source()));
            }
        });
        self
    }

    fn title_style(mut self, s: impl runtime_core::IntoStyleSource) -> Self {
        with_navigator_ext(&mut self, |p| {
            if let Primitive::Navigator { slot_styles, .. } = p {
                slot_styles.push(("title", s.into_style_source()));
            }
        });
        self
    }

    fn button_style(mut self, s: impl runtime_core::IntoStyleSource) -> Self {
        with_navigator_ext(&mut self, |p| {
            if let Primitive::Navigator { slot_styles, .. } = p {
                slot_styles.push(("button", s.into_style_source()));
            }
        });
        self
    }

    fn bind(mut self, r: Ref<StackHandle>) -> Self {
        with_navigator_ext(&mut self, |p| {
            if let Primitive::Navigator { ref_fill, .. } = p {
                *ref_fill = Some(RefFill::Navigator(Box::new(move |handle| {
                    r.fill(StackHandle::from_inner(handle));
                })));
            }
        });
        self
    }
}

// =============================================================================
// Backend selector — per-platform handler registration
// =============================================================================

#[cfg(target_arch = "wasm32")]
mod web;
#[cfg(target_arch = "wasm32")]
pub use web::register;

#[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
mod android;
#[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
pub use android::register;

#[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
mod ios;
#[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
pub use ios::register;

#[cfg(not(any(
    target_arch = "wasm32",
    target_os = "android",
    target_os = "ios",
)))]
mod fallback {
    use runtime_core::Backend;
    /// No-op register for unsupported targets. The framework's
    /// `create_navigator` default impl panics — user code
    /// hitting Stack on an unsupported target should be explicit about
    /// providing its own handler.
    pub fn register<B: Backend>(_backend: &mut B) {}
}

#[cfg(not(any(
    target_arch = "wasm32",
    target_os = "android",
    target_os = "ios",
)))]
pub use fallback::register;

// =============================================================================
// Prelude
// =============================================================================

pub mod prelude {
    pub use super::{register, Navigator, StackBuilder, StackHandle, StackPresentation};
}
