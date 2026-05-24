//! First-party Tab navigator SDK.
//!
//! Routes through `Primitive::NavigatorExt`; the registered
//! `TabHandler` drives a platform-native tab bar (UITabBarController,
//! BottomNavigationView, or a web `role=tablist`).
//!
//! # Usage
//!
//! ```ignore
//! tab_navigator::register(&mut backend);
//!
//! let home = Route::<()>::new("home", "/");
//! let settings = Route::<()>::new("settings", "/settings");
//! let nav: Ref<TabsHandle> = Ref::new();
//! tab_navigator::TabNavigator::new(&home)
//!     .tab(home.clone(), TabSpec::new("Home").icon("house"), |_| Screen::new(...))
//!     .tab(settings.clone(), TabSpec::new("Settings").icon("gear"), |_| Screen::new(...))
//!     .placement(TabPlacement::Bottom)
//!     .bind(nav.clone())
//! ```

use runtime_core::primitives::navigator::{
    DefaultLinkKind, NavigatorExtConfig, NavigatorHandle, Route, RouteEntry, RouteParams,
    ScreenBuilder, ScreenOptions,
};
use runtime_core::{Bound, IntoStyleSource, Primitive, Ref, RefFill, StyleSource};
use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::rc::Rc;

// =============================================================================
// Tab-specific value types
// =============================================================================

/// Per-tab presentation spec — label, icon, optional reactive badge.
pub struct TabSpec {
    pub label: String,
    pub icon: Option<String>,
    pub badge: Option<Rc<dyn Fn() -> String>>,
}

impl TabSpec {
    pub fn new(label: impl Into<String>) -> Self {
        Self { label: label.into(), icon: None, badge: None }
    }

    pub fn icon(mut self, name: impl Into<String>) -> Self {
        self.icon = Some(name.into());
        self
    }

    pub fn badge<F: Fn() -> String + 'static>(mut self, f: F) -> Self {
        self.badge = Some(Rc::new(f));
        self
    }
}

/// Tab bar placement.
#[derive(Copy, Clone, Debug)]
pub enum TabPlacement {
    Auto,
    Top,
    Bottom,
    Sidebar,
}

impl Default for TabPlacement {
    fn default() -> Self {
        TabPlacement::Auto
    }
}

/// Per-screen mount policy across tab switches.
#[derive(Copy, Clone, Debug)]
pub enum MountPolicy {
    EagerPersistent,
    LazyPersistent,
    LazyDisposing,
}

impl Default for MountPolicy {
    fn default() -> Self {
        MountPolicy::LazyPersistent
    }
}

/// Tab-kind presentation payload. Carries everything tab-handlers need
/// that *isn't* in the shared `NavigatorExtConfig`.
pub struct TabPresentation {
    pub tab_order: Vec<(&'static str, TabSpec)>,
    pub placement: TabPlacement,
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

#[derive(Clone)]
pub struct TabsHandle {
    inner: NavigatorHandle,
}

impl TabsHandle {
    pub fn from_inner(inner: NavigatorHandle) -> Self {
        Self { inner }
    }

    /// Switch the active tab. Mirrors the legacy `TabsHandle::select`.
    pub fn select<P: RouteParams>(&self, route: &Route<P>, params: P) {
        // Stack/select shape dispatch is handled by the underlying
        // NavigatorHandle. Tab handlers route Select through their
        // `on_command`; the framework rewrites Push to Select for
        // tabs (via DefaultLinkKind::Select).
        self.inner.replace(route, params);
    }

    pub fn inner(&self) -> &NavigatorHandle {
        &self.inner
    }
}

// =============================================================================
// Builder
// =============================================================================

pub struct TabNavigator {
    config: NavigatorExtConfig,
    presentation: TabPresentation,
    slot_styles: Vec<(&'static str, StyleSource)>,
    style: Option<StyleSource>,
    ref_fill: Option<RefFill>,
}

impl TabNavigator {
    pub fn new(initial: &Route<()>) -> Bound<TabsHandle> {
        let nav = Self {
            config: NavigatorExtConfig {
                initial: initial.name(),
                initial_path: initial.path(),
                screens: HashMap::new(),
                layout: None,
                default_options: None,
                default_link_kind: DefaultLinkKind::Select,
                defer_initial_mount: false,
            },
            presentation: TabPresentation::default(),
            slot_styles: Vec::new(),
            style: None,
            ref_fill: None,
        };
        Bound::new(nav.into_primitive())
    }

    fn into_primitive(self) -> Primitive {
        let TabNavigator { config, presentation, slot_styles, style, ref_fill } = self;
        Primitive::NavigatorExt {
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

fn with_navigator_ext<F: FnOnce(&mut Primitive)>(b: &mut Bound<TabsHandle>, f: F) {
    f(b.primitive_mut());
}

fn with_presentation<F: FnOnce(&mut TabPresentation)>(b: &mut Bound<TabsHandle>, f: F) {
    if let Primitive::NavigatorExt { presentation, .. } = b.primitive_mut() {
        let pres = Rc::get_mut(presentation)
            .expect("tab-navigator: presentation Rc already shared (builder misuse)");
        let pres: &mut dyn Any = pres;
        if let Some(typed) = pres.downcast_mut::<TabPresentation>() {
            f(typed);
        }
    }
}

/// Builder method surface for `Bound<TabsHandle>`. Orphan-rule
/// workaround: see [`crate::TabNavigator`] doc.
pub trait TabsBuilder: Sized {
    fn tab<P, R, F>(self, route: Route<P>, spec: TabSpec, render: F) -> Self
    where
        P: RouteParams + 'static,
        R: Into<runtime_core::primitives::navigator::Screen> + 'static,
        F: Fn(P) -> R + 'static;
    fn placement(self, placement: TabPlacement) -> Self;
    fn mount_policy(self, policy: MountPolicy) -> Self;
    fn default_screen_options(self, opts: ScreenOptions) -> Self;
    fn layout<F>(self, f: F) -> Self
    where
        F: Fn(runtime_core::primitives::navigator::LayoutProps) -> Primitive + 'static;
    fn tab_bar_style(self, s: impl IntoStyleSource) -> Self;
    fn tab_icon_style(self, s: impl IntoStyleSource) -> Self;
    fn tab_label_style(self, s: impl IntoStyleSource) -> Self;
    fn bind(self, r: Ref<TabsHandle>) -> Self;
}

impl TabsBuilder for Bound<TabsHandle> {
    fn tab<P, R, F>(mut self, route: Route<P>, spec: TabSpec, render: F) -> Self
    where
        P: RouteParams + 'static,
        R: Into<runtime_core::primitives::navigator::Screen> + 'static,
        F: Fn(P) -> R + 'static,
    {
        let route_name = route.name();
        let route_path = route.path();
        with_navigator_ext(&mut self, |p| {
            if let Primitive::NavigatorExt { config, .. } = p {
                let builder: ScreenBuilder = Rc::new(move |any_params: Box<dyn Any>| {
                    let typed: Box<P> = any_params
                        .downcast::<P>()
                        .expect("tab-navigator: route params type mismatch on mount");
                    render(*typed).into()
                });
                let from_segments = Rc::new(|segs: &HashMap<String, String>| -> Option<Box<dyn Any>> {
                    P::from_segments(segs).map(|p| Box::new(p) as Box<dyn Any>)
                });
                config.screens.insert(route_name, RouteEntry {
                    path: route_path,
                    build: builder,
                    from_segments,
                });
            }
        });
        with_presentation(&mut self, |pres| {
            pres.tab_order.push((route_name, spec));
        });
        self
    }

    fn placement(mut self, placement: TabPlacement) -> Self {
        with_presentation(&mut self, |pres| pres.placement = placement);
        self
    }

    fn mount_policy(mut self, policy: MountPolicy) -> Self {
        with_presentation(&mut self, |pres| pres.mount_policy = policy);
        self
    }

    fn default_screen_options(mut self, opts: ScreenOptions) -> Self {
        with_navigator_ext(&mut self, |p| {
            if let Primitive::NavigatorExt { config, .. } = p {
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
            if let Primitive::NavigatorExt { config, .. } = p {
                config.layout = Some(Rc::new(f));
            }
        });
        self
    }

    fn tab_bar_style(mut self, s: impl IntoStyleSource) -> Self {
        with_navigator_ext(&mut self, |p| {
            if let Primitive::NavigatorExt { slot_styles, .. } = p {
                slot_styles.push(("tab_bar", s.into_style_source()));
            }
        });
        self
    }

    fn tab_icon_style(mut self, s: impl IntoStyleSource) -> Self {
        with_navigator_ext(&mut self, |p| {
            if let Primitive::NavigatorExt { slot_styles, .. } = p {
                slot_styles.push(("tab_icon", s.into_style_source()));
            }
        });
        self
    }

    fn tab_label_style(mut self, s: impl IntoStyleSource) -> Self {
        with_navigator_ext(&mut self, |p| {
            if let Primitive::NavigatorExt { slot_styles, .. } = p {
                slot_styles.push(("tab_label", s.into_style_source()));
            }
        });
        self
    }

    fn bind(mut self, r: Ref<TabsHandle>) -> Self {
        with_navigator_ext(&mut self, |p| {
            if let Primitive::NavigatorExt { ref_fill, .. } = p {
                *ref_fill = Some(RefFill::NavigatorExt(Box::new(move |handle| {
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
    pub use super::{
        register, MountPolicy, TabNavigator, TabPlacement, TabPresentation, TabSpec, TabsBuilder,
        TabsHandle,
    };
}
