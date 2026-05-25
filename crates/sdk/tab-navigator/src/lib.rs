//! First-party Tab navigator SDK.
//!
//! Routes through `Primitive::Navigator`; the SDK registers a
//! per-backend `NavigatorHandler` that drives a native tab bar
//! (UITabBarController, BottomNavigationView, or DOM `role=tablist`).
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
//! ```

use runtime_core::primitives::navigator::{
    NavCommand, NavigatorConfig, NavigatorControl, NavigatorHandle, NavigatorOps, Route,
    RouteEntry, RouteParams, Screen, ScreenBuilder,
};
use runtime_core::{Bound, IntoStyleSource, Primitive, Ref, RefFill, StyleSource};
use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::rc::Rc;

// =============================================================================
// Per-kind value types
// =============================================================================

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

#[derive(Copy, Clone, Debug, Default)]
pub enum TabPlacement {
    #[default]
    Auto,
    Top,
    Bottom,
    Sidebar,
}

#[derive(Copy, Clone, Debug, Default)]
pub enum MountPolicy {
    EagerPersistent,
    #[default]
    LazyPersistent,
    LazyDisposing,
}

// =============================================================================
// TabScreenOptions — per-screen typed options
// =============================================================================

/// Per-tab-screen options. Tabs aren't header-heavy like stack
/// navigators, but they may want an accessibility label or per-tab
/// metadata in the future.
#[derive(Default, Clone)]
pub struct TabScreenOptions {}

impl TabScreenOptions {
    pub fn new() -> Self {
        Self::default()
    }
}

// =============================================================================
// TabPresentation — SDK's typed payload
// =============================================================================

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

    pub fn select<P: RouteParams + Clone>(&self, route: &Route<P>, params: P) {
        let url = params.to_path(route.path());
        self.inner.dispatch(NavCommand::Select {
            name: route.name(),
            url,
            params: Box::new(params),
            state: None,
        });
    }

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

pub struct TabNavigator {
    config: NavigatorConfig,
    presentation: TabPresentation,
    slot_styles: Vec<(&'static str, StyleSource)>,
    style: Option<StyleSource>,
    ref_fill: Option<RefFill>,
}

impl TabNavigator {
    pub fn new(initial: &Route<()>) -> Bound<TabsHandle> {
        let nav = Self {
            config: NavigatorConfig::new(initial.name(), initial.path()),
            presentation: TabPresentation::default(),
            slot_styles: Vec::new(),
            style: None,
            ref_fill: None,
        };
        Bound::new(nav.into_primitive())
    }

    fn into_primitive(self) -> Primitive {
        let TabNavigator { config, presentation, slot_styles, style, ref_fill } = self;
        Primitive::Navigator {
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

fn with_navigator_prim<F: FnOnce(&mut Primitive)>(b: &mut Bound<TabsHandle>, f: F) {
    f(b.primitive_mut());
}

fn with_presentation_mut<F: FnOnce(&mut TabPresentation)>(b: &mut Bound<TabsHandle>, f: F) {
    if let Primitive::Navigator { presentation, .. } = b.primitive_mut() {
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

pub trait TabsBuilder: Sized {
    fn tab<P, R, F>(self, route: Route<P>, spec: TabSpec, render: F) -> Self
    where
        P: RouteParams + 'static,
        R: Into<Screen> + 'static,
        F: Fn(P) -> R + 'static;
    fn placement(self, placement: TabPlacement) -> Self;
    fn mount_policy(self, policy: MountPolicy) -> Self;
    fn tab_bar_style(self, s: impl IntoStyleSource) -> Self;
    fn tab_icon_style(self, s: impl IntoStyleSource) -> Self;
    fn tab_label_style(self, s: impl IntoStyleSource) -> Self;
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
            if let Primitive::Navigator { config, .. } = p {
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
            if let Primitive::Navigator { slot_styles, .. } = p {
                slot_styles.push(("tab_bar", s.into_style_source()));
            }
        });
        self
    }
    fn tab_icon_style(mut self, s: impl IntoStyleSource) -> Self {
        with_navigator_prim(&mut self, |p| {
            if let Primitive::Navigator { slot_styles, .. } = p {
                slot_styles.push(("tab_icon", s.into_style_source()));
            }
        });
        self
    }
    fn tab_label_style(mut self, s: impl IntoStyleSource) -> Self {
        with_navigator_prim(&mut self, |p| {
            if let Primitive::Navigator { slot_styles, .. } = p {
                slot_styles.push(("tab_label", s.into_style_source()));
            }
        });
        self
    }
    fn bind(mut self, r: Ref<TabsHandle>) -> Self {
        with_navigator_prim(&mut self, |p| {
            if let Primitive::Navigator { ref_fill, .. } = p {
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

#[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
mod android;
#[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
pub use android::register;

#[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
mod ios;
#[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
pub use ios::register;

#[cfg(not(any(target_arch = "wasm32", target_os = "android", target_os = "ios")))]
mod fallback {
    use runtime_core::Backend;
    pub fn register<B: Backend>(_backend: &mut B) {}
}
#[cfg(not(any(target_arch = "wasm32", target_os = "android", target_os = "ios")))]
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
