//! First-party **Tab** navigator SDK — a flat set of co-equal screens
//! the user switches between, with at most one visible at a time.
//!
//! A tab navigator has no push/pop depth: selecting a tab swaps the
//! active screen. This crate is one of the three first-party navigator
//! SDKs (alongside [`stack-navigator`] and [`drawer-navigator`]); like
//! every SDK under `crates/sdk/`, it is not part of `runtime-core` — an
//! app opts in by calling [`register`] once at startup.
//!
//! # Architecture — the `Element::Navigator` path
//!
//! The navigator system has two parallel paths: the legacy
//! `Element::Navigator` / `Element::TabNavigator` /
//! `Element::DrawerNavigator` variants, and the newer
//! `Element::NavigatorExt`. **This SDK rides the `Element::Navigator`
//! path** — [`TabNavigator::new`] produces an `Element::Navigator`
//! carrying a [`TabPresentation`] payload, and [`register`] installs a
//! per-backend `NavigatorHandler` keyed by that presentation type.
//! Selecting a tab dispatches `NavCommand::Select`; the handler swaps the
//! active screen. The builder also installs a *link activator* so `Link`
//! primitives inside tab screens select (not push) by default.
//!
//! # Per-backend chrome
//!
//! The author tree is uniform; the rendered tab chrome differs per
//! backend:
//!
//! | Backend | Mechanism |
//! | --- | --- |
//! | web (wasm32) | Screen-swap; the tab bar itself is author chrome rendered via `.layout(...)` and wired to `handle.select(...)`. `Select` maps to a `Replace` (no URL-stack growth). See [`web-navigator-helpers`]. |
//! | iOS | Plain `UIView` body that swaps its single child on `Select`. The tab bar is author `.layout(...)`. See [`ios-navigator-helpers`]. |
//! | Android | `FrameLayout` body with a single active-screen child; tab bar is author chrome. See [`android-navigator-helpers`]. |
//! | macOS | Tab bar (top/bottom per [`TabPlacement`]) + outlet that swaps on `Select` (no animated transition — see `project_macos_navigator_design`). |
//! | terminal | No-op `register` — tabs are not rendered on the terminal backend. |
//! | SSR / any primitive backend | [`chrome`] builds the outlet from primitives for first paint. |
//!
//! Note: on web/iOS/Android the **tab bar is not a navigator concern** —
//! the navigator owns the screen-swap; the visual bar is just a styled
//! `view` the app (or `idea-ui`) renders and wires to `select`. The
//! [`TabSpec`] metadata (label, icon, badge) is carried for the app's bar
//! to consume.
//!
//! # Usage
//!
//! ```ignore
//! tab_navigator::register(&mut backend);
//!
//! let home = Route::<()>::new("home", "/");
//! let nav: Ref<TabsHandle> = Ref::new();
//!
//! TabNavigator::new(&home)
//!     .tab(home.clone(), TabSpec::new("Home").icon("house"), |_| Screen::new(...))
//!     .placement(TabPlacement::Bottom)
//!     .bind(nav.clone());
//!
//! // From the tab bar's buttons:
//! // nav.get().select(&home, ());
//! ```
//!
//! [`stack-navigator`]: https://docs.rs/stack-navigator
//! [`drawer-navigator`]: https://docs.rs/drawer-navigator
//! [`web-navigator-helpers`]: https://docs.rs/web-navigator-helpers
//! [`ios-navigator-helpers`]: https://docs.rs/ios-navigator-helpers
//! [`android-navigator-helpers`]: https://docs.rs/android-navigator-helpers

use runtime_core::primitives::navigator::{
    NavCommand, NavigatorConfig, NavigatorControl, NavigatorHandle, NavigatorOps, Route,
    RouteEntry, RouteParams, Screen, ScreenBuilder,
};
use runtime_core::{Bound, IntoStyleSource, Element, IdealystSchema, Ref, RefFill, StyleSource};
use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::rc::Rc;

// =============================================================================
// Per-kind value types
// =============================================================================

/// Display metadata for a single tab — the label, optional icon, and an
/// optional reactive badge. The navigator carries these so the app's tab
/// bar can render them; the navigator itself never draws the bar.
/// Construct with [`TabSpec::new`] and chain [`TabSpec::icon`] /
/// [`TabSpec::badge`].
pub struct TabSpec {
    /// Visible tab label.
    pub label: String,
    /// Optional icon name (resolved against the framework icon registry).
    pub icon: Option<String>,
    /// Optional reactive badge text (e.g. an unread count); re-evaluated
    /// when its dependencies change. `None` ⇒ no badge.
    pub badge: Option<Rc<dyn Fn() -> String>>,
}

impl TabSpec {
    /// Start a tab spec from its label.
    pub fn new(label: impl Into<String>) -> Self {
        Self { label: label.into(), icon: None, badge: None }
    }

    /// Set the tab's icon name.
    pub fn icon(mut self, name: impl Into<String>) -> Self {
        self.icon = Some(name.into());
        self
    }

    /// Set a reactive badge — the closure is re-run to produce the badge
    /// text whenever its reactive dependencies change.
    pub fn badge<F: Fn() -> String + 'static>(mut self, f: F) -> Self {
        self.badge = Some(Rc::new(f));
        self
    }
}

/// Where the tab bar sits relative to the screen content. Advisory for
/// the app's bar / the macOS handler; `Auto` lets the backend pick the
/// platform-conventional placement (bottom on mobile, side rail on
/// desktop/web).
#[derive(Copy, Clone, Debug, Default, IdealystSchema)]
pub enum TabPlacement {
    /// Platform-conventional placement (the default).
    #[default]
    Auto,
    /// Bar above the content.
    Top,
    /// Bar below the content (mobile convention).
    Bottom,
    /// Vertical rail beside the content (desktop / wide convention).
    Sidebar,
}

/// When a tab's screen subtree is materialized and whether it survives a
/// switch away. The default ([`MountPolicy::LazyPersistent`]) matches
/// React Navigation's tab default — mount on first visit, keep mounted.
#[derive(Copy, Clone, Debug, Default, IdealystSchema)]
pub enum MountPolicy {
    /// Mount every tab at navigator creation; keep all mounted.
    EagerPersistent,
    /// Mount a tab on first activation; keep it mounted across switches
    /// (the default).
    #[default]
    LazyPersistent,
    /// Mount a tab on first activation; drop its scope (and background
    /// work) when switched away — re-mounts fresh on return.
    LazyDisposing,
}

// =============================================================================
// TabScreenOptions — per-screen typed options
// =============================================================================

/// Per-tab-screen options. Currently empty — tabs aren't header-heavy
/// like stack navigators. Kept as a named type (and stored in
/// [`Screen::options`](runtime_core::primitives::navigator::Screen)) so
/// per-tab metadata (accessibility label, etc.) can be added without an
/// API break.
#[derive(Default, Clone, IdealystSchema)]
pub struct TabScreenOptions {}

impl TabScreenOptions {
    /// Empty options (`Default`).
    pub fn new() -> Self {
        Self::default()
    }
}

// =============================================================================
// TabPresentation — SDK's typed payload
// =============================================================================

/// The SDK's typed payload that rides on the `Element::Navigator`
/// produced by [`TabNavigator::new`]. Its `TypeId` is the registry key
/// the per-backend handler is registered under (see [`register`]).
pub struct TabPresentation {
    /// Registered tabs, in declaration order: `(route_name, spec)`. The
    /// app's tab bar iterates this to render itself.
    pub tab_order: Vec<(&'static str, TabSpec)>,
    /// Where the tab bar sits — see [`TabPlacement`].
    pub placement: TabPlacement,
    /// Tab-screen mount lifecycle — see [`MountPolicy`].
    pub mount_policy: MountPolicy,
}

impl Default for TabPresentation {
    fn default() -> Self {
        Self {
            tab_order: Vec::new(),
            placement: TabPlacement::default(),
            mount_policy: MountPolicy::default(),
        }
    }
}

// =============================================================================
// TabsHandle — typed handle for `.bind(...)`
// =============================================================================

/// Typed runtime handle to a live tab navigator, filled into the [`Ref`]
/// passed to [`TabsBuilder::bind`]. Use it from the app's tab bar to
/// switch tabs. Cheap to clone — wraps a shared
/// [`NavigatorHandle`](runtime_core::primitives::navigator::NavigatorHandle).
#[derive(Clone)]
pub struct TabsHandle {
    inner: NavigatorHandle,
}

impl TabsHandle {
    /// Wrap a raw [`NavigatorHandle`](runtime_core::primitives::navigator::NavigatorHandle)
    /// in the typed tabs handle. Called by the backend `register` glue;
    /// authors get a `TabsHandle` from [`TabsBuilder::bind`] instead.
    pub fn from_inner(inner: NavigatorHandle) -> Self {
        Self { inner }
    }

    /// Switch to the tab for `route`, building its URL from `params`.
    /// Selecting the already-active tab is a no-op.
    pub fn select<P: RouteParams + Clone>(&self, route: &Route<P>, params: P) {
        let url = params.to_path(route.path());
        self.inner.dispatch(NavCommand::Select {
            name: route.name(),
            url,
            params: Box::new(params),
            state: None,
        });
    }

    /// Borrow the underlying kind-agnostic
    /// [`NavigatorHandle`](runtime_core::primitives::navigator::NavigatorHandle).
    pub fn inner(&self) -> &NavigatorHandle {
        &self.inner
    }
}

struct TabsOps;
impl NavigatorOps for TabsOps {}
pub(crate) static TABS_OPS: TabsOps = TabsOps;

// =============================================================================
// Builder
// =============================================================================

/// The tab-navigator builder. [`TabNavigator::new`] starts one; the
/// fluent methods on the [`TabsBuilder`] trait add tabs, set placement,
/// and bind the `Ref`. The result is a
/// [`Bound<TabsHandle>`](runtime_core::Bound) you drop into a `ui!` tree.
pub struct TabNavigator {
    config: NavigatorConfig,
    presentation: TabPresentation,
    slot_styles: Vec<(&'static str, StyleSource)>,
    style: Option<StyleSource>,
    ref_fill: Option<RefFill>,
}

impl TabNavigator {
    /// Start a tab navigator whose initial (selected) tab is `initial`.
    /// Add tabs via [`TabsBuilder::tab`], then place the returned
    /// [`Bound`](runtime_core::Bound) in your tree.
    pub fn new(initial: &Route<()>) -> Bound<TabsHandle> {
        let nav = Self {
            config: NavigatorConfig::new(initial.name(), initial.path()),
            presentation: TabPresentation::default(),
            slot_styles: Vec::new(),
            style: None,
            ref_fill: None,
        };
        Bound::new(nav.into_element())
    }

    fn into_element(self) -> Element {
        let TabNavigator { config, presentation, slot_styles, style, ref_fill } = self;
        Element::Navigator {
            type_id: TypeId::of::<TabPresentation>(),
            type_name: std::any::type_name::<TabPresentation>(),
            presentation: Rc::new(presentation) as Rc<dyn Any>,
            config: Box::new(config),
            style,
            slot_styles,
            ref_fill,
            accessibility: Default::default(),
        }
    }
}

fn with_navigator_prim<F: FnOnce(&mut Element)>(b: &mut Bound<TabsHandle>, f: F) {
    f(b.primitive_mut());
}

fn with_presentation_mut<F: FnOnce(&mut TabPresentation)>(b: &mut Bound<TabsHandle>, f: F) {
    if let Element::Navigator { presentation, .. } = b.primitive_mut() {
        let pres = Rc::get_mut(presentation)
            .expect("tab-navigator: presentation Rc already shared (builder misuse)");
        if let Some(typed) = (pres as &mut dyn Any).downcast_mut::<TabPresentation>() {
            f(typed);
        }
    }
}

/// Install a link activator on the navigator's control plane that
/// rewrites `Link` activation to `Select`. The TabNavigator builder
/// must wire this so `Link` primitives inside its screens select
/// tabs by default (instead of pushing).
pub(crate) fn install_select_link_activator(control: &Rc<NavigatorControl>) {
    let activator: Rc<dyn Fn(&'static str, String, Box<dyn Any>) -> NavCommand> =
        Rc::new(|name, url, params| NavCommand::Select { name, url, params, state: None });
    control.install_link_activator(activator);
}

/// Fluent builder methods for the tab navigator, implemented on
/// [`Bound<TabsHandle>`](runtime_core::Bound). A trait (not inherent
/// methods) because `Bound` lives in `runtime-core` — the app `use`s the
/// trait to gain the methods (orphan-rule workaround).
pub trait TabsBuilder: Sized {
    /// Register a tab: its route, display [`TabSpec`], and the closure
    /// that builds the screen from typed params.
    fn tab<P, R, F>(self, route: Route<P>, spec: TabSpec, render: F) -> Self
    where
        P: RouteParams + 'static,
        R: Into<Screen> + 'static,
        F: Fn(P) -> R + 'static;
    /// Set where the tab bar sits — see [`TabPlacement`].
    fn placement(self, placement: TabPlacement) -> Self;
    /// Set the tab-screen mount lifecycle — see [`MountPolicy`].
    fn mount_policy(self, policy: MountPolicy) -> Self;
    /// Style the `"tab_bar"` slot (the app's bar container, where honored).
    fn tab_bar_style(self, s: impl IntoStyleSource) -> Self;
    /// Style the `"tab_icon"` slot.
    fn tab_icon_style(self, s: impl IntoStyleSource) -> Self;
    /// Style the `"tab_label"` slot.
    fn tab_label_style(self, s: impl IntoStyleSource) -> Self;
    /// Bind a [`Ref<TabsHandle>`](runtime_core::Ref) so the app can switch
    /// tabs imperatively once the navigator mounts.
    fn bind(self, r: Ref<TabsHandle>) -> Self;
}

impl TabsBuilder for Bound<TabsHandle> {
    fn tab<P, R, F>(mut self, route: Route<P>, spec: TabSpec, render: F) -> Self
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
                        .expect("tab-navigator: route params type mismatch");
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
        with_presentation_mut(&mut self, |pres| {
            pres.tab_order.push((route_name, spec));
        });
        self
    }

    fn placement(mut self, placement: TabPlacement) -> Self {
        with_presentation_mut(&mut self, |p| p.placement = placement);
        self
    }
    fn mount_policy(mut self, policy: MountPolicy) -> Self {
        with_presentation_mut(&mut self, |p| p.mount_policy = policy);
        self
    }
    fn tab_bar_style(mut self, s: impl IntoStyleSource) -> Self {
        with_navigator_prim(&mut self, |p| {
            if let Element::Navigator { slot_styles, .. } = p {
                slot_styles.push(("tab_bar", s.into_style_source()));
            }
        });
        self
    }
    fn tab_icon_style(mut self, s: impl IntoStyleSource) -> Self {
        with_navigator_prim(&mut self, |p| {
            if let Element::Navigator { slot_styles, .. } = p {
                slot_styles.push(("tab_icon", s.into_style_source()));
            }
        });
        self
    }
    fn tab_label_style(mut self, s: impl IntoStyleSource) -> Self {
        with_navigator_prim(&mut self, |p| {
            if let Element::Navigator { slot_styles, .. } = p {
                slot_styles.push(("tab_label", s.into_style_source()));
            }
        });
        self
    }
    fn bind(mut self, r: Ref<TabsHandle>) -> Self {
        with_navigator_prim(&mut self, |p| {
            if let Element::Navigator { ref_fill, .. } = p {
                *ref_fill = Some(RefFill::Navigator(Box::new(move |handle| {
                    r.fill(TabsHandle::from_inner(handle));
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

// Backend-neutral "primitive chrome" handler (generic over `Backend`);
// no platform cfg, no backend dependency. Registered where wanted (the
// SSR backend today) via `tab_navigator::chrome::register`.
pub mod chrome;

// Recording handler for the runtime-server sidecar's recorder backend.
// Emits CreateTabNavigator + NavigatorAttachInitial + NavigatorSelect
// instead of rendering, so a tab-navigator app works under
// `idealyst dev` (runtime-server). Host-side only — gated behind the
// `runtime-server` feature.
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

// macOS: tabbar (top or bottom per `TabPlacement`) + outlet that
// swaps its child on Select. Per `project_macos_navigator_design`,
// no animated tab transition.
#[cfg(all(target_os = "macos", not(target_arch = "wasm32")))]
mod macos;
#[cfg(all(target_os = "macos", not(target_arch = "wasm32")))]
pub use macos::register;

#[cfg(not(any(
    target_arch = "wasm32",
    target_os = "android",
    target_os = "ios",
    target_os = "macos"
)))]
mod fallback {
    use runtime_core::Backend;
    pub fn register<B: Backend>(_backend: &mut B) {}
}
#[cfg(not(any(
    target_arch = "wasm32",
    target_os = "android",
    target_os = "ios",
    target_os = "macos"
)))]
pub use fallback::register;

// =============================================================================
// Prelude
// =============================================================================

pub mod prelude {
    pub use super::{
        register, MountPolicy, TabNavigator, TabPlacement, TabPresentation, TabScreenOptions,
        TabSpec, TabsBuilder, TabsHandle,
    };
}
