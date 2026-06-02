//! First-party **Stack** navigator SDK — push/pop screens with a native
//! header bar and platform-native back gesture.
//!
//! A stack navigator owns an ordered stack of screens; pushing a route
//! slides a new screen in on top, popping (or the iOS swipe-back / the
//! browser back button) returns to the one beneath. This crate is one of
//! the three first-party navigator SDKs (alongside [`tab-navigator`] and
//! [`drawer-navigator`]); like every SDK under `crates/sdk/`, it is not
//! part of `runtime-core` — an app opts in by calling [`register`] once
//! at startup.
//!
//! # Architecture — the `Element::Navigator` path
//!
//! The navigator system has two parallel paths in the framework: the
//! legacy `Element::Navigator` / `Element::TabNavigator` /
//! `Element::DrawerNavigator` variants, and the newer
//! `Element::NavigatorExt`. **This SDK rides the `Element::Navigator`
//! path** — [`Navigator::new`] produces an `Element::Navigator` carrying
//! a [`StackPresentation`] payload, and [`register`] installs a
//! per-backend `NavigatorHandler` keyed by that presentation type. The
//! framework walker mounts the path-matched screen and routes
//! push/pop/replace/reset commands to the handler, which drives the
//! platform-native chrome.
//!
//! # Per-backend chrome
//!
//! The author tree is uniform; each backend renders the equivalent
//! native push/pop stack:
//!
//! | Backend | Mechanism |
//! | --- | --- |
//! | web (wasm32) | SPA router — `history.pushState` per push, the browser back button drives pop; one screen mounted at a time. See [`web-navigator-helpers`]. |
//! | iOS | `UINavigationController`; a delegate reconciles interactive swipe-back. See [`ios-navigator-helpers`]. |
//! | Android | `FragmentManager` back-stack inside a `RustNavigator` host. See [`android-navigator-helpers`]. |
//! | macOS | Single-window outlet that swaps its child on each command (no animated push/pop — see `project_macos_navigator_design`). |
//! | terminal | Minimalist single-screen outlet, no chrome / no animation. |
//! | SSR / any primitive backend | [`chrome`] builds the header from `view` + `text` primitives for first paint. |
//!
//! Per `native_first_layout_for_web`, header chrome (title, bar buttons,
//! colors) is configured through **screen options** ([`StackScreenOptions`]
//! via [`StackScreenExt`]) and navigator-level builder methods — never the
//! `style` system.
//!
//! # Usage
//!
//! ```ignore
//! stack_navigator::register(&mut backend);
//!
//! let home = Route::<()>::new("home", "/");
//! let nav: Ref<StackHandle> = Ref::new();
//!
//! Navigator::new(&home)
//!     .screen(home.clone(), |_| {
//!         Screen::new(view!{ body })
//!             .title("Home")
//!             .header_right(BarButton::new("ellipsis", || open_menu()))
//!     })
//!     .bind(nav.clone());
//!
//! // Later, from an event handler, drive the stack via the bound handle:
//! // nav.get().push(&details, DetailsParams { id: 7 });
//! // nav.get().pop();
//! ```
//!
//! [`tab-navigator`]: https://docs.rs/tab-navigator
//! [`drawer-navigator`]: https://docs.rs/drawer-navigator
//! [`web-navigator-helpers`]: https://docs.rs/web-navigator-helpers
//! [`ios-navigator-helpers`]: https://docs.rs/ios-navigator-helpers
//! [`android-navigator-helpers`]: https://docs.rs/android-navigator-helpers

#![deny(missing_docs)]

use runtime_core::primitives::navigator::{
    NavCommand, NavigatorConfig, NavigatorHandle, NavigatorOps, Route, RouteEntry, RouteParams,
    Screen, ScreenBuilder,
};
use runtime_core::{
    Bound, Color, IntoStyleSource, Element, IdealystSchema, Ref, RefFill, StyleApplication,
    StyleRules, StyleSheet, StyleSource, VariantSet,
};
use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::rc::Rc;

// =============================================================================
// Per-kind value types (SDK-owned)
// =============================================================================

/// Bundle of header colors for [`StackBuilder::header`]. Each field is
/// optional — `None` keeps the platform default for that slot, so a
/// builder can theme just the background and leave title/tint native.
#[derive(Default, Clone)]
pub struct HeaderStyle {
    /// Nav-bar background color. `None` ⇒ platform default.
    pub background: Option<Color>,
    /// Title-text color. `None` ⇒ platform default.
    pub title: Option<Color>,
    /// Tint color for bar buttons (back chevron, header buttons).
    /// `None` ⇒ platform default.
    pub tint: Option<Color>,
    /// Background of the screen body beneath the bar. `None` ⇒
    /// platform default.
    pub body_background: Option<Color>,
}

/// Icon-based header bar button — the value type for
/// [`StackScreenExt::header_left`] / [`StackScreenExt::header_right`].
/// Construct with [`BarButton::new`].
#[derive(Clone)]
pub struct BarButton {
    /// Icon name (resolved against the framework icon registry).
    pub icon: String,
    /// Tap handler, invoked on press. Stored as an `Rc` so the button
    /// is cheap to clone into per-screen options.
    pub on_press: Rc<dyn Fn()>,
    /// Optional per-button tint override. `None` inherits the bar tint.
    pub tint: Option<Color>,
}

impl BarButton {
    /// Build a header button from an icon name and a press handler.
    pub fn new(icon: impl Into<String>, on_press: impl Fn() + 'static) -> Self {
        Self {
            icon: icon.into(),
            on_press: Rc::new(on_press),
            tint: None,
        }
    }

    /// Override the bar tint for just this button.
    pub fn tint(mut self, color: Color) -> Self {
        self.tint = Some(color);
        self
    }
}

// =============================================================================
// StackScreenOptions — per-screen typed options
// =============================================================================

/// Per-screen options for a stack screen — title, header chrome, and
/// scope lifecycle. Authors usually set these through the
/// [`StackScreenExt`] builder methods on `Screen::new(...)` rather than
/// constructing the struct directly; the SDK stores it in
/// [`Screen::options`](runtime_core::primitives::navigator::Screen) and
/// the per-backend handler reads it on mount.
#[derive(Default, Clone, IdealystSchema)]
pub struct StackScreenOptions {
    /// Title shown in the native nav bar / header. `None` ⇒ no title.
    pub title: Option<String>,
    /// Force the header bar visible (`Some(true)`) or hidden
    /// (`Some(false)`). `None` ⇒ backend default (the bar shows when a
    /// title is set).
    pub header_shown: Option<bool>,
    /// Leading (left in LTR) header button — typically back / close.
    pub header_left: Option<BarButton>,
    /// Trailing (right in LTR) header button — typically an action.
    pub header_right: Option<BarButton>,
    /// Reactive nav-bar background color; re-resolved on theme swap.
    pub header_background: Option<Rc<dyn Fn() -> Color>>,
    /// Reactive bar-button tint color; re-resolved on theme swap.
    pub header_tint: Option<Rc<dyn Fn() -> Color>>,
    /// Reactive title-text color; re-resolved on theme swap.
    pub title_color: Option<Rc<dyn Fn() -> Color>>,
    /// React Navigation-style `unmountOnBlur`. When `Some(true)`,
    /// this screen's reactive scope is dropped (its effects /
    /// AnimatedValue subscribers / scheduled work) when a new
    /// screen is pushed on top of it; pop-back rebuilds from
    /// initial state. When `Some(false)` (or `None`, which is the
    /// default), the screen stays mounted below the new one and
    /// pop-back resumes its existing state — matching native
    /// UINavigationController / Android Fragment-back-stack
    /// behavior.
    ///
    /// Pair with [`runtime_core::primitives::navigator::use_focus`]
    /// for finer-grained "still mounted but pause work" semantics
    /// (e.g. pause a wgpu host without dropping the whole scope).
    pub unmount_on_blur: Option<bool>,
}

impl StackScreenOptions {
    /// Empty options (`Default`). Equivalent to `StackScreenOptions::default()`.
    pub fn new() -> Self {
        Self::default()
    }
}

/// Extension trait that adds stack-specific builder methods to
/// [`Screen`](runtime_core::primitives::navigator::Screen).
/// `use stack_navigator::StackScreenExt;` to get `.title(...) /
/// .header_left(...) / …` chained on `Screen::new(...)`. Each method
/// merges into the screen's [`StackScreenOptions`].
pub trait StackScreenExt: Sized {
    /// Set the screen's nav-bar title.
    fn title(self, t: impl Into<String>) -> Self;
    /// Force the header bar visible / hidden, overriding the default.
    fn header_shown(self, shown: bool) -> Self;
    /// Set the leading (left in LTR) header button.
    fn header_left(self, btn: BarButton) -> Self;
    /// Set the trailing (right in LTR) header button.
    fn header_right(self, btn: BarButton) -> Self;
    /// Set a reactive nav-bar background color (re-resolved on theme swap).
    fn header_background<F: Fn() -> Color + 'static>(self, f: F) -> Self;
    /// Set a reactive bar-button tint color (re-resolved on theme swap).
    fn header_tint<F: Fn() -> Color + 'static>(self, f: F) -> Self;
    /// Set a reactive title-text color (re-resolved on theme swap).
    fn title_color<F: Fn() -> Color + 'static>(self, f: F) -> Self;
    /// React Navigation-style `unmountOnBlur` — drop this screen's scope
    /// when a screen is pushed above it (vs. keeping it mounted, the
    /// default). See [`StackScreenOptions::unmount_on_blur`].
    fn unmount_on_blur(self, unmount: bool) -> Self;
}

impl StackScreenExt for Screen {
    fn title(self, t: impl Into<String>) -> Self {
        with_stack_options(self, |o| o.title = Some(t.into()))
    }
    fn header_shown(self, shown: bool) -> Self {
        with_stack_options(self, |o| o.header_shown = Some(shown))
    }
    fn header_left(self, btn: BarButton) -> Self {
        with_stack_options(self, |o| o.header_left = Some(btn))
    }
    fn header_right(self, btn: BarButton) -> Self {
        with_stack_options(self, |o| o.header_right = Some(btn))
    }
    fn header_background<F: Fn() -> Color + 'static>(self, f: F) -> Self {
        with_stack_options(self, |o| o.header_background = Some(Rc::new(f)))
    }
    fn header_tint<F: Fn() -> Color + 'static>(self, f: F) -> Self {
        with_stack_options(self, |o| o.header_tint = Some(Rc::new(f)))
    }
    fn title_color<F: Fn() -> Color + 'static>(self, f: F) -> Self {
        with_stack_options(self, |o| o.title_color = Some(Rc::new(f)))
    }
    fn unmount_on_blur(self, unmount: bool) -> Self {
        with_stack_options(self, |o| o.unmount_on_blur = Some(unmount))
    }
}

fn with_stack_options(mut screen: Screen, f: impl FnOnce(&mut StackScreenOptions)) -> Screen {
    let mut opts = screen
        .options
        .downcast_ref::<StackScreenOptions>()
        .cloned()
        .unwrap_or_default();
    f(&mut opts);
    screen.options = Box::new(opts);
    screen
}

// =============================================================================
// StackPresentation — SDK's typed payload
// =============================================================================

/// The SDK's typed payload that rides on the `Element::Navigator`
/// produced by [`Navigator::new`]. Its `TypeId` is the registry key the
/// per-backend handler is registered under (see [`register`]).
#[derive(Default)]
pub struct StackPresentation {
    /// SDK slot names emitted via `.header_style(...)` etc. (the
    /// per-backend handler consults `host.slot_styles` indirectly via
    /// the framework's slot-style pipeline, so this just bookkeeps
    /// which slots the SDK has wired).
    pub slot_keys: Vec<&'static str>,
}

// =============================================================================
// StackHandle — typed handle for `.bind(...)`
// =============================================================================

/// Typed runtime handle to a live stack navigator, filled into the
/// [`Ref`] passed to [`StackBuilder::bind`]. Use it from event handlers
/// to drive the stack imperatively (`push` / `pop` / `replace` /
/// `reset`). Cheap to clone — it wraps a shared
/// [`NavigatorHandle`](runtime_core::primitives::navigator::NavigatorHandle).
#[derive(Clone)]
pub struct StackHandle {
    inner: NavigatorHandle,
}

impl StackHandle {
    /// Wrap a raw [`NavigatorHandle`](runtime_core::primitives::navigator::NavigatorHandle)
    /// in the typed stack handle. Called by the backend `register` glue;
    /// authors get a `StackHandle` from [`StackBuilder::bind`] instead.
    pub fn from_inner(inner: NavigatorHandle) -> Self {
        Self { inner }
    }

    /// Push a screen onto the stack, building its URL from `params` and
    /// the route's path template.
    pub fn push<P: RouteParams + Clone>(&self, route: &Route<P>, params: P) {
        let url = params.to_path(route.path());
        self.inner.dispatch(NavCommand::Push {
            name: route.name(),
            url,
            params: Box::new(params),
            state: None,
        });
    }

    /// Pop the top screen, returning to the one beneath. No-op at the
    /// root.
    pub fn pop(&self) {
        self.inner.dispatch(NavCommand::Pop);
    }

    /// Replace the top screen in place — same depth, new content (no
    /// push/pop animation accumulating a back entry).
    pub fn replace<P: RouteParams + Clone>(&self, route: &Route<P>, params: P) {
        let url = params.to_path(route.path());
        self.inner.dispatch(NavCommand::Replace {
            name: route.name(),
            url,
            params: Box::new(params),
            state: None,
        });
    }

    /// Reset the entire stack to a single screen (clears the back
    /// stack). Useful after login / logout flows.
    pub fn reset<P: RouteParams + Clone>(&self, route: &Route<P>, params: P) {
        let url = params.to_path(route.path());
        self.inner.dispatch(NavCommand::Reset {
            name: route.name(),
            url,
            params: Box::new(params),
            state: None,
        });
    }

    /// Current stack depth (number of screens, including the visible top).
    pub fn depth(&self) -> usize {
        self.inner.depth()
    }

    /// Borrow the underlying kind-agnostic
    /// [`NavigatorHandle`](runtime_core::primitives::navigator::NavigatorHandle)
    /// for lower-level access.
    pub fn inner(&self) -> &NavigatorHandle {
        &self.inner
    }
}

struct StackOps;
impl NavigatorOps for StackOps {}
pub(crate) static STACK_OPS: StackOps = StackOps;

// =============================================================================
// Builder
// =============================================================================

/// The stack-navigator builder. [`Navigator::new`] starts one; the
/// fluent methods on the [`StackBuilder`] trait add screens, header
/// styling, and the `Ref` to bind. The result is a
/// [`Bound<StackHandle>`](runtime_core::Bound) you drop into a `ui!`
/// tree (it `Deref`s to an `Element::Navigator`).
pub struct Navigator {
    config: NavigatorConfig,
    presentation: StackPresentation,
    slot_styles: Vec<(&'static str, StyleSource)>,
    style: Option<StyleSource>,
    ref_fill: Option<RefFill>,
}

impl Navigator {
    /// Start a stack navigator whose initial (root) screen is `initial`.
    /// Add screens and configure chrome via the [`StackBuilder`] methods,
    /// then place the returned [`Bound`](runtime_core::Bound) in your tree.
    pub fn new(initial: &Route<()>) -> Bound<StackHandle> {
        let nav = Self {
            config: NavigatorConfig::new(initial.name(), initial.path()),
            presentation: StackPresentation::default(),
            slot_styles: Vec::new(),
            style: None,
            ref_fill: None,
        };
        Bound::new(nav.into_element())
    }

    fn into_element(self) -> Element {
        let Navigator { config, presentation, slot_styles, style, ref_fill } = self;
        Element::Navigator {
            type_id: TypeId::of::<StackPresentation>(),
            type_name: std::any::type_name::<StackPresentation>(),
            presentation: Rc::new(presentation) as Rc<dyn Any>,
            config: Box::new(config),
            style,
            slot_styles,
            ref_fill,
            accessibility: Default::default(),
        }
    }
}

fn with_navigator_prim<F: FnOnce(&mut Element)>(b: &mut Bound<StackHandle>, f: F) {
    f(b.primitive_mut());
}

/// Fluent builder methods for the stack navigator, implemented on
/// [`Bound<StackHandle>`](runtime_core::Bound). It's a trait (rather
/// than inherent methods) because `Bound` lives in `runtime-core` — the
/// orphan rule means the SDK adds its methods via a trait the app
/// `use`s.
pub trait StackBuilder: Sized {
    /// Register a route + the closure that builds its screen. The closure
    /// receives the typed route params (`P`) and returns anything that
    /// `Into<Screen>` — typically `Screen::new(...)` chained with
    /// [`StackScreenExt`] options.
    fn screen<P, R, F>(self, route: Route<P>, render: F) -> Self
    where
        P: RouteParams + 'static,
        R: Into<Screen> + 'static,
        F: Fn(P) -> R + 'static;

    /// Style the navigator's `"header"` slot (nav-bar background).
    fn header_style(self, s: impl IntoStyleSource) -> Self;
    /// Style the navigator's `"title"` slot (title text color/font).
    fn title_style(self, s: impl IntoStyleSource) -> Self;
    /// Style the navigator's `"button"` slot (bar-button tint).
    fn button_style(self, s: impl IntoStyleSource) -> Self;

    /// Bundled header styling — set background / title / tint / body
    /// colors from one [`HeaderStyle`]-returning closure (re-resolved on
    /// theme swap). Same shape as `drawer-navigator`'s `header`.
    fn header<F>(self, f: F) -> Self
    where
        F: Fn() -> HeaderStyle + 'static;

    /// Bind a [`Ref<StackHandle>`](runtime_core::Ref) so the app can drive
    /// the stack imperatively once the navigator mounts.
    fn bind(self, r: Ref<StackHandle>) -> Self;
}

#[derive(Copy, Clone)]
enum HeaderProp {
    Background,
    Color,
}

fn header_slot_source(
    f: Rc<dyn Fn() -> HeaderStyle>,
    getter: fn(&HeaderStyle) -> &Option<Color>,
    prop: HeaderProp,
    field_name: &'static str,
) -> StyleSource {
    StyleSource::Reactive(Box::new(move || {
        let style = f();
        let color = getter(&style).clone().unwrap_or_else(|| {
            panic!(
                "StackBuilder::header — HeaderStyle.{} must stay Some after \
                 the initial probe.",
                field_name
            )
        });
        let sheet = Rc::new(StyleSheet::new(|_vs: &VariantSet| StyleRules::default()));
        let app = StyleApplication::new(sheet);
        match prop {
            HeaderProp::Background => app.override_background(color),
            HeaderProp::Color => app.override_color(color),
        }
    }))
}

impl StackBuilder for Bound<StackHandle> {
    fn screen<P, R, F>(mut self, route: Route<P>, render: F) -> Self
    where
        P: RouteParams + 'static,
        R: Into<Screen> + 'static,
        F: Fn(P) -> R + 'static,
    {
        let route_name = route.name();
        let route_path = route.path();
        with_navigator_prim(&mut self, |p| {
            if let Element::Navigator { config, .. } = p {
                let builder: ScreenBuilder = Rc::new(move |any_params: Box<dyn Any>| {
                    let typed: Box<P> = any_params
                        .downcast::<P>()
                        .expect("stack-navigator: route params type mismatch");
                    render(*typed).into()
                });
                let from_segments = Rc::new(
                    |segs: &HashMap<String, String>| -> Option<Box<dyn Any>> {
                        P::from_segments(segs).map(|p| Box::new(p) as Box<dyn Any>)
                    },
                );
                config.screens.insert(
                    route_name,
                    RouteEntry { path: route_path, build: builder, from_segments },
                );
            }
        });
        self
    }

    fn header_style(mut self, s: impl IntoStyleSource) -> Self {
        with_navigator_prim(&mut self, |p| {
            if let Element::Navigator { slot_styles, .. } = p {
                slot_styles.push(("header", s.into_style_source()));
            }
        });
        self
    }

    fn title_style(mut self, s: impl IntoStyleSource) -> Self {
        with_navigator_prim(&mut self, |p| {
            if let Element::Navigator { slot_styles, .. } = p {
                slot_styles.push(("title", s.into_style_source()));
            }
        });
        self
    }

    fn button_style(mut self, s: impl IntoStyleSource) -> Self {
        with_navigator_prim(&mut self, |p| {
            if let Element::Navigator { slot_styles, .. } = p {
                slot_styles.push(("button", s.into_style_source()));
            }
        });
        self
    }

    fn header<F>(mut self, f: F) -> Self
    where
        F: Fn() -> HeaderStyle + 'static,
    {
        let f: Rc<dyn Fn() -> HeaderStyle> = Rc::new(f);
        let probe = f();
        let mut pushes: Vec<(&'static str, StyleSource)> = Vec::new();
        if probe.background.is_some() {
            pushes.push((
                "header",
                header_slot_source(f.clone(), |hs| &hs.background, HeaderProp::Background, "background"),
            ));
        }
        if probe.title.is_some() {
            pushes.push((
                "title",
                header_slot_source(f.clone(), |hs| &hs.title, HeaderProp::Color, "title"),
            ));
        }
        if probe.tint.is_some() {
            pushes.push((
                "button",
                header_slot_source(f.clone(), |hs| &hs.tint, HeaderProp::Color, "tint"),
            ));
        }
        if probe.body_background.is_some() {
            pushes.push((
                "body",
                header_slot_source(
                    f.clone(),
                    |hs| &hs.body_background,
                    HeaderProp::Background,
                    "body_background",
                ),
            ));
        }
        with_navigator_prim(&mut self, |p| {
            if let Element::Navigator { slot_styles, .. } = p {
                slot_styles.extend(pushes);
            }
        });
        self
    }

    fn bind(mut self, r: Ref<StackHandle>) -> Self {
        with_navigator_prim(&mut self, |p| {
            if let Element::Navigator { ref_fill, .. } = p {
                *ref_fill = Some(RefFill::Navigator(Box::new(move |handle| {
                    r.fill(StackHandle::from_inner(handle));
                })));
            }
        });
        self
    }
}

// =============================================================================
// Backend selector
// =============================================================================

#[cfg(target_arch = "wasm32")]
mod web;
#[cfg(target_arch = "wasm32")]
pub use web::register;

// Backend-neutral "primitive chrome" handler (generic over `Backend`).
// No platform cfg and no backend dependency — it builds chrome from
// primitives, so it compiles everywhere and is registered only where
// wanted (the SSR backend today) via `stack_navigator::chrome::register`.
pub mod chrome;

// Recording handler for the runtime-server sidecar's recorder backend.
// Emits navigator wire commands (CreateNavigator / NavigatorAttachInitial
// / NavigatorPush / NavigatorPop / NavigatorReplace / NavigatorReset)
// instead of rendering, so a stack-navigator app works under
// `idealyst dev` (runtime-server) and can be headless-screenshotted.
// Host-side only — gated behind the `runtime-server` feature.
#[cfg(feature = "runtime-server")]
pub mod recording;

#[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
mod android;
#[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
pub use android::register;

#[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
mod ios;
#[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
pub use ios::register;

// macOS: single-window outlet that swaps its child on Push/Pop/
// Replace/Reset. No animated push/pop chrome — per
// `project_macos_navigator_design`.
#[cfg(all(target_os = "macos", not(target_arch = "wasm32")))]
mod macos;
#[cfg(all(target_os = "macos", not(target_arch = "wasm32")))]
pub use macos::register;

// Non-mobile, non-wasm, non-macOS hosts target the terminal backend.
// The handler is minimalist (no chrome, no animations); see
// [[feedback_terminal_minimalism]] and `terminal::TerminalStackHandler`.
#[cfg(not(any(
    target_arch = "wasm32",
    target_os = "android",
    target_os = "ios",
    target_os = "macos"
)))]
mod terminal;
#[cfg(not(any(
    target_arch = "wasm32",
    target_os = "android",
    target_os = "ios",
    target_os = "macos"
)))]
pub use terminal::register;

// =============================================================================
// Prelude
// =============================================================================

/// Convenience re-exports of the crate's public surface — glob-import
/// (`use stack_navigator::prelude::*;`) to bring the navigator builder,
/// handle, screen options, and value types into scope.
pub mod prelude {
    pub use super::{
        register, BarButton, HeaderStyle, Navigator, StackBuilder, StackHandle, StackPresentation,
        StackScreenExt, StackScreenOptions,
    };
}
