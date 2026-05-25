//! First-party Drawer navigator SDK.
//!
//! Routes through `Primitive::Navigator`; the registered
//! `DrawerHandler` drives a platform-native slide-in side panel +
//! switchable body region.
//!
//! # Usage
//!
//! ```ignore
//! drawer_navigator::register(&mut backend);
//!
//! let home = Route::<()>::new("home", "/");
//! let nav: Ref<DrawerHandle> = Ref::new();
//! drawer_navigator::DrawerNavigator::new(&home)
//!     .screen(home.clone(), |_| Screen::new(...))
//!     .drawer_width(280.0)
//!     .side(DrawerSide::Start)
//!     .content(|props| /* sidebar content */)
//!     .bind(nav.clone())
//! ```

use runtime_core::primitives::navigator::{
    DefaultLinkKind, NavigatorConfig, NavigatorHandle, Route, RouteEntry, RouteParams,
    ScreenBuilder, ScreenOptions,
};
use runtime_core::{
    Bound, Color, HeaderStyle, IntoStyleSource, Primitive, Ref, RefFill, Signal, StyleApplication,
    StyleRules, StyleSheet, StyleSource, VariantSet,
};
use std::any::{Any, TypeId};
use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

// =============================================================================
// Drawer-specific value types
// =============================================================================

#[derive(Copy, Clone, Debug)]
pub enum DrawerSide {
    Start,
    End,
}
impl Default for DrawerSide {
    fn default() -> Self {
        DrawerSide::Start
    }
}

#[derive(Copy, Clone, Debug)]
pub enum DrawerType {
    Front,
    Slide,
}
impl Default for DrawerType {
    fn default() -> Self {
        DrawerType::Front
    }
}

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

// Re-export `DrawerContentProps` / `ContentBuilder` from runtime-core
// so authors can use a single canonical type regardless of whether
// they're driving the legacy `Primitive::DrawerNavigator` or the new
// SDK path. Same fields, same semantics.
pub use runtime_core::{ContentBuilder, DrawerContentProps};

/// Drawer-kind presentation payload.
pub struct DrawerPresentation {
    pub side: DrawerSide,
    pub drawer_type: DrawerType,
    pub drawer_width: f32,
    pub swipe_to_open: bool,
    pub mount_policy: MountPolicy,
    pub content: Option<ContentBuilder>,
}

impl Default for DrawerPresentation {
    fn default() -> Self {
        Self {
            side: DrawerSide::default(),
            drawer_type: DrawerType::default(),
            drawer_width: 280.0,
            swipe_to_open: true,
            mount_policy: MountPolicy::default(),
            content: None,
        }
    }
}

// =============================================================================
// DrawerHandle — typed handle for `.bind(...)`
// =============================================================================

#[derive(Clone)]
pub struct DrawerHandle {
    inner: NavigatorHandle,
    /// Mirror of the drawer's open state, set by the registered handler
    /// when its `on_command` processes Open/Close/Toggle.
    is_open: Rc<Cell<bool>>,
}

impl DrawerHandle {
    pub fn from_inner(inner: NavigatorHandle, is_open: Rc<Cell<bool>>) -> Self {
        Self { inner, is_open }
    }

    pub fn select<P: RouteParams>(&self, route: &Route<P>, params: P) {
        self.inner.replace(route, params);
    }

    pub fn open(&self) {
        if let Some(c) = self.inner.control() {
            c.dispatch(runtime_core::primitives::navigator::NavCommand::OpenDrawer);
        }
    }

    pub fn close(&self) {
        if let Some(c) = self.inner.control() {
            c.dispatch(runtime_core::primitives::navigator::NavCommand::CloseDrawer);
        }
    }

    pub fn toggle(&self) {
        if let Some(c) = self.inner.control() {
            c.dispatch(runtime_core::primitives::navigator::NavCommand::ToggleDrawer);
        }
    }

    pub fn is_open(&self) -> bool {
        self.is_open.get()
    }

    pub fn inner(&self) -> &NavigatorHandle {
        &self.inner
    }
}

// =============================================================================
// Builder
// =============================================================================

pub struct DrawerNavigator {
    config: NavigatorConfig,
    presentation: DrawerPresentation,
    slot_styles: Vec<(&'static str, StyleSource)>,
    style: Option<StyleSource>,
    ref_fill: Option<RefFill>,
}

impl DrawerNavigator {
    pub fn new(initial: &Route<()>) -> Bound<DrawerHandle> {
        let nav = Self {
            config: NavigatorConfig {
                initial: initial.name(),
                initial_path: initial.path(),
                screens: HashMap::new(),
                layout: None,
                default_options: None,
                default_link_kind: DefaultLinkKind::Select,
                defer_initial_mount: false,
            },
            presentation: DrawerPresentation::default(),
            slot_styles: Vec::new(),
            style: None,
            ref_fill: None,
        };
        Bound::new(nav.into_primitive())
    }

    fn into_primitive(self) -> Primitive {
        let DrawerNavigator { config, presentation, slot_styles, style, ref_fill } = self;
        Primitive::Navigator {
            type_id: TypeId::of::<DrawerPresentation>(),
            type_name: std::any::type_name::<DrawerPresentation>(),
            presentation: Rc::new(presentation) as Rc<dyn Any>,
            config: Box::new(config),
            style,
            slot_styles,
            ref_fill,
            accessibility: Default::default(),
        }
    }
}

fn with_navigator_ext<F: FnOnce(&mut Primitive)>(b: &mut Bound<DrawerHandle>, f: F) {
    f(b.primitive_mut());
}

fn with_presentation<F: FnOnce(&mut DrawerPresentation)>(b: &mut Bound<DrawerHandle>, f: F) {
    if let Primitive::Navigator { presentation, .. } = b.primitive_mut() {
        let pres = Rc::get_mut(presentation)
            .expect("drawer-navigator: presentation Rc already shared (builder misuse)");
        let pres: &mut dyn Any = pres;
        if let Some(typed) = pres.downcast_mut::<DrawerPresentation>() {
            f(typed);
        }
    }
}

/// Builder method surface for `Bound<DrawerHandle>`. Orphan-rule
/// workaround — see [`crate::DrawerNavigator`] doc.
pub trait DrawerBuilder: Sized {
    fn screen<P, R, F>(self, route: Route<P>, render: F) -> Self
    where
        P: RouteParams + 'static,
        R: Into<runtime_core::primitives::navigator::Screen> + 'static,
        F: Fn(P) -> R + 'static;
    fn drawer_width(self, width: f32) -> Self;
    fn side(self, side: DrawerSide) -> Self;
    fn drawer_type(self, dt: DrawerType) -> Self;
    fn swipe_to_open(self, enabled: bool) -> Self;
    fn mount_policy(self, policy: MountPolicy) -> Self;
    fn content<F>(self, f: F) -> Self
    where
        F: Fn(DrawerContentProps) -> Primitive + 'static;
    fn default_screen_options(self, opts: ScreenOptions) -> Self;
    fn layout<F>(self, f: F) -> Self
    where
        F: Fn(runtime_core::primitives::navigator::LayoutProps) -> Primitive + 'static;
    fn sidebar_style(self, s: impl IntoStyleSource) -> Self;
    fn scrim_style(self, s: impl IntoStyleSource) -> Self;
    /// Bundled header styling — decomposes a `HeaderStyle`-returning
    /// closure into individual slot-style entries the handler dispatches
    /// via `apply_navigator_slot_style`. Each `Some(...)`
    /// field on the initial probe becomes its own per-slot reactive
    /// effect; fields that flip from `Some` to `None` at runtime
    /// panic, matching the legacy `Navigator::header` contract.
    ///
    /// Theme reactivity comes from the closure itself reading from
    /// theme signals — the resulting `StyleSource::Reactive` re-fires
    /// when any signal touched inside the closure changes.
    fn header<F>(self, f: F) -> Self
    where
        F: Fn() -> HeaderStyle + 'static;
    fn bind(self, r: Ref<DrawerHandle>) -> Self;
}

/// What CSS-level property a `HeaderStyle` Color drives through the
/// slot-style pipeline.
#[derive(Copy, Clone)]
enum HeaderProp {
    /// `rules.background` — backgrounds of the header bar and body.
    Background,
    /// `rules.color` — text/tint color (title text + button icons).
    Color,
}

/// Build a `StyleSource::Reactive` that reads a specific `Color` field
/// out of `f()`'s `HeaderStyle` and produces a single-property
/// `StyleApplication`. Used by `.header(...)` to decompose a
/// `Fn() -> HeaderStyle` closure into per-slot reactive style sources
/// the SDK can dispatch via the standard slot-style pipeline.
///
/// `field_name` is purely for the panic message — it names the
/// `HeaderStyle` field that's expected to stay `Some` for the
/// navigator's lifetime.
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
                "DrawerBuilder::header — HeaderStyle.{} must stay Some \
                 after the initial probe (toggling to None at runtime \
                 isn't supported).",
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

impl DrawerBuilder for Bound<DrawerHandle> {
    fn screen<P, R, F>(mut self, route: Route<P>, render: F) -> Self
    where
        P: RouteParams + 'static,
        R: Into<runtime_core::primitives::navigator::Screen> + 'static,
        F: Fn(P) -> R + 'static,
    {
        let route_name = route.name();
        let route_path = route.path();
        with_navigator_ext(&mut self, |p| {
            if let Primitive::Navigator { config, .. } = p {
                let builder: ScreenBuilder = Rc::new(move |any_params: Box<dyn Any>| {
                    let typed: Box<P> = any_params
                        .downcast::<P>()
                        .expect("drawer-navigator: route params type mismatch on mount");
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
        self
    }

    fn drawer_width(mut self, width: f32) -> Self {
        with_presentation(&mut self, |pres| pres.drawer_width = width);
        self
    }

    fn side(mut self, side: DrawerSide) -> Self {
        with_presentation(&mut self, |pres| pres.side = side);
        self
    }

    fn drawer_type(mut self, dt: DrawerType) -> Self {
        with_presentation(&mut self, |pres| pres.drawer_type = dt);
        self
    }

    fn swipe_to_open(mut self, enabled: bool) -> Self {
        with_presentation(&mut self, |pres| pres.swipe_to_open = enabled);
        self
    }

    fn mount_policy(mut self, policy: MountPolicy) -> Self {
        with_presentation(&mut self, |pres| pres.mount_policy = policy);
        self
    }

    fn content<F>(mut self, f: F) -> Self
    where
        F: Fn(DrawerContentProps) -> Primitive + 'static,
    {
        with_presentation(&mut self, |pres| pres.content = Some(Rc::new(f)));
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

    fn sidebar_style(mut self, s: impl IntoStyleSource) -> Self {
        with_navigator_ext(&mut self, |p| {
            if let Primitive::Navigator { slot_styles, .. } = p {
                slot_styles.push(("sidebar", s.into_style_source()));
            }
        });
        self
    }

    fn scrim_style(mut self, s: impl IntoStyleSource) -> Self {
        with_navigator_ext(&mut self, |p| {
            if let Primitive::Navigator { slot_styles, .. } = p {
                slot_styles.push(("scrim", s.into_style_source()));
            }
        });
        self
    }

    fn header<F>(mut self, f: F) -> Self
    where
        F: Fn() -> HeaderStyle + 'static,
    {
        // Probe once to figure out which slots to wire — fields that
        // are None on the initial probe stay platform-default and
        // aren't re-evaluated. Matches the legacy contract.
        let f: Rc<dyn Fn() -> HeaderStyle> = Rc::new(f);
        let probe = f();
        if probe.background.is_some() {
            let src = header_slot_source(
                f.clone(),
                |hs| &hs.background,
                HeaderProp::Background,
                "background",
            );
            with_navigator_ext(&mut self, |p| {
                if let Primitive::Navigator { slot_styles, .. } = p {
                    slot_styles.push(("header", src));
                }
            });
        }
        if probe.title.is_some() {
            let src = header_slot_source(
                f.clone(),
                |hs| &hs.title,
                HeaderProp::Color,
                "title",
            );
            with_navigator_ext(&mut self, |p| {
                if let Primitive::Navigator { slot_styles, .. } = p {
                    slot_styles.push(("title", src));
                }
            });
        }
        if probe.tint.is_some() {
            let src = header_slot_source(
                f.clone(),
                |hs| &hs.tint,
                HeaderProp::Color,
                "tint",
            );
            with_navigator_ext(&mut self, |p| {
                if let Primitive::Navigator { slot_styles, .. } = p {
                    slot_styles.push(("button", src));
                }
            });
        }
        if probe.body_background.is_some() {
            let src = header_slot_source(
                f.clone(),
                |hs| &hs.body_background,
                HeaderProp::Background,
                "body_background",
            );
            with_navigator_ext(&mut self, |p| {
                if let Primitive::Navigator { slot_styles, .. } = p {
                    slot_styles.push(("body", src));
                }
            });
        }
        self
    }

    fn bind(mut self, r: Ref<DrawerHandle>) -> Self {
        let is_open = Rc::new(Cell::new(false));
        let is_open_for_fill = is_open.clone();
        with_navigator_ext(&mut self, |p| {
            if let Primitive::Navigator { ref_fill, .. } = p {
                *ref_fill = Some(RefFill::Navigator(Box::new(move |handle| {
                    r.fill(DrawerHandle::from_inner(handle, is_open_for_fill));
                })));
            }
        });
        // Note: the registered handler should update `is_open` via the
        // `Signal<bool>` carried in the host's nav-state — this Cell is
        // a non-reactive mirror exposed through `DrawerHandle::is_open()`.
        let _keep_is_open_alive = is_open;
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
        register, ContentBuilder, DrawerBuilder, DrawerContentProps, DrawerHandle, DrawerNavigator,
        DrawerPresentation, DrawerSide, DrawerType, MountPolicy,
    };
}
