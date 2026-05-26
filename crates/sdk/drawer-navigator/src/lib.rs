//! First-party Drawer navigator SDK.
//!
//! Routes through `Primitive::Navigator`; the SDK registers a
//! per-backend `NavigatorHandler` that drives a slide-in side panel
//! on iOS/Android and a flex-row sidebar + outlet on web.
//!
//! # Usage
//!
//! ```ignore
//! drawer_navigator::register(&mut backend);
//!
//! let home = Route::<()>::new("home", "/");
//! let nav: Ref<DrawerHandle> = Ref::new();
//!
//! let sidebar = view! { Sidebar { /* ... */ } };
//!
//! DrawerNavigator::new(&home)
//!     .screen(home.clone(), |_| {
//!         Screen::new(view!{ /* body */ }).with(
//!             DrawerScreenOptions::new().title("Home")
//!         )
//!     })
//!     .sidebar(sidebar)
//!     .drawer_width(280.0)
//!     .side(DrawerSide::Start)
//!     .bind(nav.clone());
//! ```

use runtime_core::primitives::navigator::{
    NavCommand, NavigatorConfig, NavigatorHandle, NavigatorOps, Route, RouteEntry, RouteParams,
    Screen, ScreenBuilder,
};
use runtime_core::{
    Bound, Color, IntoStyleSource, Primitive, Ref, RefFill, Signal, StyleApplication, StyleRules,
    StyleSheet, StyleSource, VariantSet,
};
use std::any::{Any, TypeId};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

// =============================================================================
// Per-kind value types (SDK-owned — no privilege in core)
// =============================================================================

#[derive(Copy, Clone, Debug, Default)]
pub enum DrawerSide {
    #[default]
    Start,
    End,
}

#[derive(Copy, Clone, Debug, Default)]
pub enum DrawerType {
    #[default]
    Front,
    Slide,
}

#[derive(Copy, Clone, Debug, Default)]
pub enum MountPolicy {
    EagerPersistent,
    #[default]
    LazyPersistent,
    LazyDisposing,
}

/// Bundle of header colors for `DrawerBuilder::header(...)`. Each
/// field optional — `None` keeps the platform default for that slot.
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
// DrawerSlotProps + sidebar builder
// =============================================================================

/// Reactive props the `.sidebar_with(closure)` form receives.
/// Captures the framework's nav state alongside drawer-specific
/// signals so a reactive sidebar can highlight the active route,
/// observe open/close state, etc.
pub struct DrawerSlotProps {
    pub active_route: Signal<&'static str>,
    pub active_path: Signal<String>,
    pub depth: Signal<usize>,
    pub can_go_back: Signal<bool>,
    pub is_open: Signal<bool>,
    pub on_select: Rc<dyn Fn(&'static str)>,
    pub on_close: Rc<dyn Fn()>,
}

/// SDK-defined sidebar builder. The presentation stores one of these;
/// the per-backend handler invokes it via `host.build_node` to
/// materialize the sidebar UIView/Node/DOM-element.
pub type SidebarBuilder = Rc<dyn Fn(DrawerSlotProps) -> Primitive>;

// =============================================================================
// DrawerScreenOptions — per-screen typed options
// =============================================================================

/// SDK-defined per-screen options. Authors set fields via the
/// `DrawerScreenExt` extension trait on `Screen` (or by passing a
/// `DrawerScreenOptions` to `Screen::with(...)` directly).
#[derive(Default, Clone)]
pub struct DrawerScreenOptions {
    pub title: Option<String>,
    pub header_shown: Option<bool>,
    pub header_left: Option<BarButton>,
    pub header_right: Option<BarButton>,
    pub header_background: Option<Rc<dyn Fn() -> Color>>,
    pub header_tint: Option<Rc<dyn Fn() -> Color>>,
    pub title_color: Option<Rc<dyn Fn() -> Color>>,
}

impl DrawerScreenOptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn title(mut self, t: impl Into<String>) -> Self {
        self.title = Some(t.into());
        self
    }

    pub fn header_shown(mut self, shown: bool) -> Self {
        self.header_shown = Some(shown);
        self
    }

    pub fn header_left(mut self, btn: BarButton) -> Self {
        self.header_left = Some(btn);
        self
    }

    pub fn header_right(mut self, btn: BarButton) -> Self {
        self.header_right = Some(btn);
        self
    }

    pub fn header_background<F: Fn() -> Color + 'static>(mut self, f: F) -> Self {
        self.header_background = Some(Rc::new(f));
        self
    }

    pub fn header_tint<F: Fn() -> Color + 'static>(mut self, f: F) -> Self {
        self.header_tint = Some(Rc::new(f));
        self
    }

    pub fn title_color<F: Fn() -> Color + 'static>(mut self, f: F) -> Self {
        self.title_color = Some(Rc::new(f));
        self
    }
}

/// Extension trait adding drawer-specific builder methods to
/// `Screen`. Authors `use drawer_navigator::DrawerScreenExt;` to
/// gain `.title(...) / .header_left(...) / .header_right(...)`
/// directly on `Screen::new(...)` results.
pub trait DrawerScreenExt: Sized {
    fn title(self, t: impl Into<String>) -> Self;
    fn header_shown(self, shown: bool) -> Self;
    fn header_left(self, btn: BarButton) -> Self;
    fn header_right(self, btn: BarButton) -> Self;
    fn header_background<F: Fn() -> Color + 'static>(self, f: F) -> Self;
    fn header_tint<F: Fn() -> Color + 'static>(self, f: F) -> Self;
    fn title_color<F: Fn() -> Color + 'static>(self, f: F) -> Self;
}

impl DrawerScreenExt for Screen {
    fn title(self, t: impl Into<String>) -> Self {
        with_drawer_options(self, |o| o.title = Some(t.into()))
    }
    fn header_shown(self, shown: bool) -> Self {
        with_drawer_options(self, |o| o.header_shown = Some(shown))
    }
    fn header_left(self, btn: BarButton) -> Self {
        with_drawer_options(self, |o| o.header_left = Some(btn))
    }
    fn header_right(self, btn: BarButton) -> Self {
        with_drawer_options(self, |o| o.header_right = Some(btn))
    }
    fn header_background<F: Fn() -> Color + 'static>(self, f: F) -> Self {
        with_drawer_options(self, |o| o.header_background = Some(Rc::new(f)))
    }
    fn header_tint<F: Fn() -> Color + 'static>(self, f: F) -> Self {
        with_drawer_options(self, |o| o.header_tint = Some(Rc::new(f)))
    }
    fn title_color<F: Fn() -> Color + 'static>(self, f: F) -> Self {
        with_drawer_options(self, |o| o.title_color = Some(Rc::new(f)))
    }
}

fn with_drawer_options(
    mut screen: Screen,
    f: impl FnOnce(&mut DrawerScreenOptions),
) -> Screen {
    let existing = screen
        .options
        .downcast_ref::<DrawerScreenOptions>()
        .cloned()
        .unwrap_or_default();
    let mut opts = existing;
    f(&mut opts);
    screen.options = Box::new(opts);
    screen
}

// =============================================================================
// DrawerPresentation — SDK's typed payload riding on Primitive::Navigator
// =============================================================================

pub struct DrawerPresentation {
    pub side: DrawerSide,
    pub drawer_type: DrawerType,
    pub drawer_width: f32,
    pub swipe_to_open: bool,
    pub mount_policy: MountPolicy,
    /// Sidebar Primitive builder. Author sets via `.sidebar(prim)` or
    /// `.sidebar_with(closure)`. SDK handler invokes during `init` (via
    /// `host.build_node` deferred to microtask) to materialize the
    /// sidebar native view.
    ///
    /// `RefCell<Option<…>>` because the SDK handler needs to take
    /// ownership when materializing. Once taken, the slot is empty
    /// (subsequent reads see `None`).
    pub sidebar: RefCell<Option<SidebarBuilder>>,
    /// Shared open-state signal — read by both the SDK handler's
    /// dispatcher (writes Open/Close/Toggle) and the sidebar builder
    /// via `DrawerSlotProps.is_open`.
    pub is_open: Signal<bool>,
}

impl DrawerPresentation {
    fn new() -> Self {
        Self {
            side: DrawerSide::default(),
            drawer_type: DrawerType::default(),
            drawer_width: 280.0,
            swipe_to_open: true,
            mount_policy: MountPolicy::default(),
            sidebar: RefCell::new(None),
            is_open: Signal::new(false),
        }
    }
}

// =============================================================================
// Drawer NavCommand verbs — SDK-specific, packed in NavCommand::Custom
// =============================================================================

/// Drawer-specific verbs that ride on `NavCommand::Custom`. The SDK
/// handler's dispatcher downcasts the `Custom` payload to this.
#[derive(Copy, Clone, Debug)]
pub enum DrawerCmd {
    Open,
    Close,
    Toggle,
}

// =============================================================================
// DrawerHandle — typed handle for `.bind(...)`
// =============================================================================

#[derive(Clone)]
pub struct DrawerHandle {
    inner: NavigatorHandle,
    /// Mirror of the open-state signal that lives on the presentation;
    /// stashed here so `handle.is_open()` doesn't need to round-trip
    /// through ambient lookup. Same `Signal<bool>` instance.
    is_open: Signal<bool>,
}

impl DrawerHandle {
    pub fn from_inner(inner: NavigatorHandle, is_open: Signal<bool>) -> Self {
        Self { inner, is_open }
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

    pub fn open(&self) {
        self.inner.dispatch(NavCommand::Custom(Rc::new(DrawerCmd::Open)));
    }

    pub fn close(&self) {
        self.inner.dispatch(NavCommand::Custom(Rc::new(DrawerCmd::Close)));
    }

    pub fn toggle(&self) {
        self.inner.dispatch(NavCommand::Custom(Rc::new(DrawerCmd::Toggle)));
    }

    pub fn is_open(&self) -> bool {
        self.is_open.get()
    }

    pub fn is_open_signal(&self) -> Signal<bool> {
        self.is_open
    }

    pub fn inner(&self) -> &NavigatorHandle {
        &self.inner
    }
}

/// No-op `NavigatorOps` for the handle. Drawer doesn't carry per-op
/// hooks; the dispatcher closure does everything.
struct DrawerOps;
impl NavigatorOps for DrawerOps {}
pub(crate) static DRAWER_OPS: DrawerOps = DrawerOps;

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
            config: NavigatorConfig::new(initial.name(), initial.path()),
            presentation: DrawerPresentation::new(),
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

fn with_navigator_prim<F: FnOnce(&mut Primitive)>(b: &mut Bound<DrawerHandle>, f: F) {
    f(b.primitive_mut());
}

fn with_presentation<F: FnOnce(&DrawerPresentation)>(b: &mut Bound<DrawerHandle>, f: F) {
    if let Primitive::Navigator { presentation, .. } = b.primitive_mut() {
        if let Some(pres) = presentation.downcast_ref::<DrawerPresentation>() {
            f(pres);
        }
    }
}

fn with_presentation_mut<F: FnOnce(&mut DrawerPresentation)>(
    b: &mut Bound<DrawerHandle>,
    f: F,
) {
    if let Primitive::Navigator { presentation, .. } = b.primitive_mut() {
        let pres = Rc::get_mut(presentation)
            .expect("drawer-navigator: presentation Rc already shared (builder misuse)");
        if let Some(typed) = (pres as &mut dyn Any).downcast_mut::<DrawerPresentation>() {
            f(typed);
        }
    }
}

/// Builder method surface for `Bound<DrawerHandle>`. Orphan-rule
/// workaround — `Bound` lives in runtime-core, so the methods ride
/// on a trait the user `use`s.
pub trait DrawerBuilder: Sized {
    fn screen<P, R, F>(self, route: Route<P>, render: F) -> Self
    where
        P: RouteParams + 'static,
        R: Into<Screen> + 'static,
        F: Fn(P) -> R + 'static;
    fn drawer_width(self, width: f32) -> Self;
    fn side(self, side: DrawerSide) -> Self;
    fn drawer_type(self, dt: DrawerType) -> Self;
    fn swipe_to_open(self, enabled: bool) -> Self;
    fn mount_policy(self, policy: MountPolicy) -> Self;
    /// Pass a pre-built sidebar Primitive. Used when the sidebar
    /// doesn't need reactive access to nav state.
    fn sidebar(self, prim: Primitive) -> Self;
    /// Pass a builder closure that receives reactive `DrawerSlotProps`
    /// (active route, is_open, on_select, on_close). Used when the
    /// sidebar's content needs to react to nav state — nav-link
    /// highlights, animated open/close, etc.
    fn sidebar_with<F>(self, f: F) -> Self
    where
        F: Fn(DrawerSlotProps) -> Primitive + 'static;
    fn sidebar_style(self, s: impl IntoStyleSource) -> Self;
    fn scrim_style(self, s: impl IntoStyleSource) -> Self;
    /// Bundled header styling — sets background/title/tint/body
    /// colors via per-slot reactive style sources the SDK dispatches
    /// via `apply_slot_style`.
    fn header<F>(self, f: F) -> Self
    where
        F: Fn() -> HeaderStyle + 'static;
    fn bind(self, r: Ref<DrawerHandle>) -> Self;
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
                "DrawerBuilder::header — HeaderStyle.{} must stay Some after \
                 the initial probe (toggling to None at runtime isn't supported).",
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
                        .expect("drawer-navigator: route params type mismatch");
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

    fn drawer_width(mut self, width: f32) -> Self {
        with_presentation_mut(&mut self, |p| p.drawer_width = width);
        self
    }
    fn side(mut self, side: DrawerSide) -> Self {
        with_presentation_mut(&mut self, |p| p.side = side);
        self
    }
    fn drawer_type(mut self, dt: DrawerType) -> Self {
        with_presentation_mut(&mut self, |p| p.drawer_type = dt);
        self
    }
    fn swipe_to_open(mut self, enabled: bool) -> Self {
        with_presentation_mut(&mut self, |p| p.swipe_to_open = enabled);
        self
    }
    fn mount_policy(mut self, policy: MountPolicy) -> Self {
        with_presentation_mut(&mut self, |p| p.mount_policy = policy);
        self
    }

    fn sidebar(mut self, prim: Primitive) -> Self {
        // Wrap as a closure that yields the captured primitive on
        // first call. Subsequent calls panic — sidebars are built
        // exactly once per navigator lifetime.
        let cell: Rc<RefCell<Option<Primitive>>> = Rc::new(RefCell::new(Some(prim)));
        let builder: SidebarBuilder = Rc::new(move |_props| {
            cell.borrow_mut()
                .take()
                .expect("drawer-navigator: sidebar Primitive already consumed")
        });
        with_presentation(&mut self, |p| {
            *p.sidebar.borrow_mut() = Some(builder);
        });
        self
    }

    fn sidebar_with<F>(mut self, f: F) -> Self
    where
        F: Fn(DrawerSlotProps) -> Primitive + 'static,
    {
        let builder: SidebarBuilder = Rc::new(f);
        with_presentation(&mut self, |p| {
            *p.sidebar.borrow_mut() = Some(builder);
        });
        self
    }

    fn sidebar_style(mut self, s: impl IntoStyleSource) -> Self {
        with_navigator_prim(&mut self, |p| {
            if let Primitive::Navigator { slot_styles, .. } = p {
                slot_styles.push(("sidebar", s.into_style_source()));
            }
        });
        self
    }

    fn scrim_style(mut self, s: impl IntoStyleSource) -> Self {
        with_navigator_prim(&mut self, |p| {
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

    fn bind(mut self, r: Ref<DrawerHandle>) -> Self {
        // Capture the is_open signal from the presentation so the
        // DrawerHandle exposes it via `is_open()` after fill.
        let mut is_open_signal = Signal::new(false);
        with_presentation(&mut self, |p| {
            is_open_signal = p.is_open;
        });
        with_navigator_prim(&mut self, |p| {
            if let Primitive::Navigator { ref_fill, .. } = p {
                *ref_fill = Some(RefFill::Navigator(Box::new(move |handle| {
                    r.fill(DrawerHandle::from_inner(handle, is_open_signal));
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

// macOS: single-window, persistent sidebar (per
// `project_macos_navigator_design`). No scrim, no slide-in
// animation — sidebar is always visible and the outlet swaps
// its child on `Select`.
#[cfg(all(target_os = "macos", not(target_arch = "wasm32")))]
mod macos;
#[cfg(all(target_os = "macos", not(target_arch = "wasm32")))]
pub use macos::register;

// Non-mobile, non-wasm, non-macOS hosts target the terminal backend.
// Drawer renders as a persistent sidebar column beside the screen
// outlet — no animation, no scrim, always visible.
// See [[feedback_terminal_minimalism]] and `terminal::TerminalDrawerHandler`.
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

pub mod prelude {
    pub use super::{
        register, BarButton, DrawerBuilder, DrawerCmd, DrawerHandle, DrawerNavigator,
        DrawerPresentation, DrawerScreenExt, DrawerScreenOptions, DrawerSide, DrawerSlotProps,
        DrawerType, HeaderStyle, MountPolicy,
    };
}
