//! First-party Stack navigator SDK.
//!
//! Routes through `Primitive::Navigator`; the SDK registers a
//! per-backend `NavigatorHandler` that drives a native push/pop stack
//! (UINavigationController on iOS, FragmentManager on Android, DOM
//! subtree swap on web).
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
//! ```

use runtime_core::primitives::navigator::{
    NavCommand, NavigatorConfig, NavigatorHandle, NavigatorOps, Route, RouteEntry, RouteParams,
    Screen, ScreenBuilder,
};
use runtime_core::{
    Bound, Color, IntoStyleSource, Primitive, Ref, RefFill, StyleApplication, StyleRules,
    StyleSheet, StyleSource, VariantSet,
};
use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::rc::Rc;

// =============================================================================
// Per-kind value types (SDK-owned)
// =============================================================================

/// Header colors for `StackBuilder::header(...)`. Each field optional —
/// `None` keeps the platform default for that slot.
#[derive(Default, Clone)]
pub struct HeaderStyle {
    pub background: Option<Color>,
    pub title: Option<Color>,
    pub tint: Option<Color>,
    pub body_background: Option<Color>,
}

/// Icon-based header bar button.
#[derive(Clone)]
pub struct BarButton {
    pub icon: String,
    pub on_press: Rc<dyn Fn()>,
    pub tint: Option<Color>,
}

impl BarButton {
    pub fn new(icon: impl Into<String>, on_press: impl Fn() + 'static) -> Self {
        Self {
            icon: icon.into(),
            on_press: Rc::new(on_press),
            tint: None,
        }
    }

    pub fn tint(mut self, color: Color) -> Self {
        self.tint = Some(color);
        self
    }
}

// =============================================================================
// StackScreenOptions — per-screen typed options
// =============================================================================

#[derive(Default, Clone)]
pub struct StackScreenOptions {
    pub title: Option<String>,
    pub header_shown: Option<bool>,
    pub header_left: Option<BarButton>,
    pub header_right: Option<BarButton>,
    pub header_background: Option<Rc<dyn Fn() -> Color>>,
    pub header_tint: Option<Rc<dyn Fn() -> Color>>,
    pub title_color: Option<Rc<dyn Fn() -> Color>>,
}

impl StackScreenOptions {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Extension trait that adds stack-specific builder methods to
/// `Screen`. `use stack_navigator::StackScreenExt;` to get
/// `.title(...) / .header_left(...) / etc.` on `Screen::new(...)`.
pub trait StackScreenExt: Sized {
    fn title(self, t: impl Into<String>) -> Self;
    fn header_shown(self, shown: bool) -> Self;
    fn header_left(self, btn: BarButton) -> Self;
    fn header_right(self, btn: BarButton) -> Self;
    fn header_background<F: Fn() -> Color + 'static>(self, f: F) -> Self;
    fn header_tint<F: Fn() -> Color + 'static>(self, f: F) -> Self;
    fn title_color<F: Fn() -> Color + 'static>(self, f: F) -> Self;
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

#[derive(Clone)]
pub struct StackHandle {
    inner: NavigatorHandle,
}

impl StackHandle {
    pub fn from_inner(inner: NavigatorHandle) -> Self {
        Self { inner }
    }

    pub fn push<P: RouteParams + Clone>(&self, route: &Route<P>, params: P) {
        let url = params.to_path(route.path());
        self.inner.dispatch(NavCommand::Push {
            name: route.name(),
            url,
            params: Box::new(params),
            state: None,
        });
    }

    pub fn pop(&self) {
        self.inner.dispatch(NavCommand::Pop);
    }

    pub fn replace<P: RouteParams + Clone>(&self, route: &Route<P>, params: P) {
        let url = params.to_path(route.path());
        self.inner.dispatch(NavCommand::Replace {
            name: route.name(),
            url,
            params: Box::new(params),
            state: None,
        });
    }

    pub fn reset<P: RouteParams + Clone>(&self, route: &Route<P>, params: P) {
        let url = params.to_path(route.path());
        self.inner.dispatch(NavCommand::Reset {
            name: route.name(),
            url,
            params: Box::new(params),
            state: None,
        });
    }

    pub fn depth(&self) -> usize {
        self.inner.depth()
    }

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

pub struct Navigator {
    config: NavigatorConfig,
    presentation: StackPresentation,
    slot_styles: Vec<(&'static str, StyleSource)>,
    style: Option<StyleSource>,
    ref_fill: Option<RefFill>,
}

impl Navigator {
    pub fn new(initial: &Route<()>) -> Bound<StackHandle> {
        let nav = Self {
            config: NavigatorConfig::new(initial.name(), initial.path()),
            presentation: StackPresentation::default(),
            slot_styles: Vec::new(),
            style: None,
            ref_fill: None,
        };
        Bound::new(nav.into_primitive())
    }

    fn into_primitive(self) -> Primitive {
        let Navigator { config, presentation, slot_styles, style, ref_fill } = self;
        Primitive::Navigator {
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

fn with_navigator_prim<F: FnOnce(&mut Primitive)>(b: &mut Bound<StackHandle>, f: F) {
    f(b.primitive_mut());
}

pub trait StackBuilder: Sized {
    fn screen<P, R, F>(self, route: Route<P>, render: F) -> Self
    where
        P: RouteParams + 'static,
        R: Into<Screen> + 'static,
        F: Fn(P) -> R + 'static;

    fn header_style(self, s: impl IntoStyleSource) -> Self;
    fn title_style(self, s: impl IntoStyleSource) -> Self;
    fn button_style(self, s: impl IntoStyleSource) -> Self;

    /// Bundled header styling — same shape as drawer-navigator's.
    fn header<F>(self, f: F) -> Self
    where
        F: Fn() -> HeaderStyle + 'static;

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
            if let Primitive::Navigator { config, .. } = p {
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
            if let Primitive::Navigator { slot_styles, .. } = p {
                slot_styles.push(("header", s.into_style_source()));
            }
        });
        self
    }

    fn title_style(mut self, s: impl IntoStyleSource) -> Self {
        with_navigator_prim(&mut self, |p| {
            if let Primitive::Navigator { slot_styles, .. } = p {
                slot_styles.push(("title", s.into_style_source()));
            }
        });
        self
    }

    fn button_style(mut self, s: impl IntoStyleSource) -> Self {
        with_navigator_prim(&mut self, |p| {
            if let Primitive::Navigator { slot_styles, .. } = p {
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
            if let Primitive::Navigator { slot_styles, .. } = p {
                slot_styles.extend(pushes);
            }
        });
        self
    }

    fn bind(mut self, r: Ref<StackHandle>) -> Self {
        with_navigator_prim(&mut self, |p| {
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
        register, BarButton, HeaderStyle, Navigator, StackBuilder, StackHandle, StackPresentation,
        StackScreenExt, StackScreenOptions,
    };
}
