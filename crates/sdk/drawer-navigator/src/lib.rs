//! First-party Drawer navigator SDK.
//!
//! Routes through `Element::Navigator`; the SDK registers a
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
    Screen, ScreenBuilder, ScrollContext,
};
use runtime_core::{
    Bound, Color, IntoStyleSource, Element, Ref, RefFill, Signal, StyleApplication, StyleRules,
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
pub type SidebarBuilder = Rc<dyn Fn(DrawerSlotProps) -> Element>;

// =============================================================================
// Next-gen slot system: leading / top / bottom / trailing
//
// `sidebar_with` (above) is the original API — a single closure
// receiving `DrawerSlotProps`. The four named slots below
// generalize it into uniform "chrome positions" around the screen
// outlet that any navigator kind can opt into. Each slot is
// optional and per-backend honor is also optional (iOS/Android
// drawer handlers may ignore `top` and `bottom` in favor of native
// chrome).
//
// The chrome built by these slot closures **mounts ONCE** at
// navigator init and persists across screen swaps — fixing the
// per-navigation rebuild problem the original `sidebar_with` model
// also has for stateful chrome.
// =============================================================================

/// Reactive props every slot closure receives.
///
/// Carries the framework's nav-state mirrors (`active_route`,
/// `depth`, etc.), the drawer-specific `is_open` signal, semantic
/// **intent** signals that describe what the leading/trailing
/// positions currently represent (hamburger? back arrow?
/// nothing?), the default screen title, and pre-bound dispatchers
/// (`open_drawer`, `close_drawer`, `pop`, `on_select`) the
/// renderer can wire straight to pressables.
///
/// Cross-navigator stability: every navigator kind (drawer, stack,
/// tab) hands slot closures the same `SlotProps` shape. Fields
/// without semantic meaning for a particular navigator stay valid
/// (e.g., `is_open` on a stack navigator is a const-false signal).
/// This lets author code write a single chrome closure that works
/// across navigator kinds.
///
/// `Clone` is cheap — every field is either `Copy` (Signals) or
/// an `Rc`. The drawer SDK constructs one `SlotProps` per
/// navigator init and clones into each slot's closure invocation.
#[derive(Clone)]
pub struct SlotProps {
    pub active_route: Signal<&'static str>,
    pub active_path: Signal<String>,
    pub depth: Signal<usize>,
    pub can_go_back: Signal<bool>,
    /// Drawer's open/close state. Const-false on non-drawer
    /// navigators — present so slot signatures stay uniform.
    pub is_open: Signal<bool>,
    /// What the leading bar position semantically *is* on the
    /// current screen — re-evaluated on nav state changes.
    pub leading_intent: Signal<LeadingIntent>,
    /// Mirror of `leading_intent` for the trailing position.
    pub trailing_intent: Signal<TrailingIntent>,
    /// The title `TopSlot::Filled { title: BarTitle::Default }`
    /// would show. Custom renderers read this for parity. Driven
    /// by the active screen's `DrawerScreenOptions::title`
    /// (empty string if unset).
    pub screen_title: Signal<String>,
    /// Dispatch a `Select` command on the ambient navigator —
    /// "tap this nav link". Used by sidebars / leading slots.
    pub on_select: Rc<dyn Fn(&'static str)>,
    /// Open the drawer. No-op on navigators without an open state.
    pub open_drawer: Rc<dyn Fn()>,
    /// Close the drawer. No-op on navigators without an open state.
    pub close_drawer: Rc<dyn Fn()>,
    /// Pop the stack. No-op on navigators without a stack.
    pub pop: Rc<dyn Fn()>,
    /// The navigator's scroll surface, when the drawer's body is
    /// itself the scroll context (default `bottom_in_scroll`
    /// mode). All the dimension + offset signals (viewport top,
    /// height/width, scroll x/y, scroll-height/width) plus the
    /// programmatic `scroll_to` dispatcher live on this typed
    /// bundle — see [`runtime_core::primitives::navigator::ScrollContext`].
    ///
    /// `None` for navigators / modes that don't own a single
    /// scroll surface (legacy `bottom_pinned` drawer mode, where
    /// each screen carries its own `ScrollView`). Slots should
    /// guard accordingly.
    ///
    /// Author code in **screens** can read the same bundle via
    /// the framework-level
    /// [`runtime_core::primitives::navigator::ambient_scroll_context`]
    /// — no `SlotProps` plumbing needed.
    pub scroll: Option<ScrollContext>,
}

/// What the *leading* (left in LTR) bar position semantically does
/// on the current screen. Slot renderers read this to pick the
/// right button + dispatcher; the SDK populates it from the active
/// nav state.
///
/// Third-party navigator SDKs register custom intents via
/// [`LeadingIntent::Custom`] — Filled-mode renderers fall back to
/// "no default button" on unknown intents; Custom renderers
/// `match` on the string tag.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum LeadingIntent {
    /// No conventional leading button on this screen.
    None,
    /// Hamburger that opens the drawer. Dispatcher is
    /// [`SlotProps::open_drawer`].
    OpenDrawer,
    /// Back arrow that pops the stack. Dispatcher is
    /// [`SlotProps::pop`].
    PopStack,
    /// SDK-extension hook — string is the third-party SDK's
    /// chosen tag (e.g. `"close_modal"`).
    Custom(&'static str),
}

/// Same idea as [`LeadingIntent`] for the trailing (right in LTR)
/// position. Most screens use `None`; SDKs that conventionally
/// put a button on the right populate `Custom` with their tag.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum TrailingIntent {
    None,
    Custom(&'static str),
}

/// Icon-backed pressable for [`TopSlot::Filled`]'s
/// `leading` / `trailing` arrays. Same shape as [`BarButton`]
/// (which the original per-screen `header_left` / `header_right`
/// uses) but the icon is the framework's typed
/// [`runtime_core::IconData`] rather than a string name — the same
/// vocabulary `idea-ui`'s `Icon` component uses. We use a separate
/// type during the migration so existing `BarButton` callers
/// (`stack-navigator`, `DrawerScreenOptions`) keep working
/// unchanged.
#[derive(Clone)]
pub struct SlotBarButton {
    pub icon: runtime_core::IconData,
    pub on_press: Rc<dyn Fn()>,
    pub tint: Option<Color>,
}

impl SlotBarButton {
    pub fn new(icon: runtime_core::IconData, on_press: impl Fn() + 'static) -> Self {
        Self { icon, on_press: Rc::new(on_press), tint: None }
    }

    pub fn tint(mut self, color: Color) -> Self {
        self.tint = Some(color);
        self
    }
}

/// What text/view to render in the center of [`TopSlot::Filled`].
pub enum BarTitle {
    /// Use the active screen's `DrawerScreenOptions::title`
    /// (delivered via [`SlotProps::screen_title`]). Updates
    /// reactively on navigation. This is the default when
    /// `BarTitle` is left unset.
    Default,
    /// Override with an author-controlled reactive string.
    Text(Signal<String>),
    /// Author-supplied view — search input, breadcrumb, logo, etc.
    View(Box<dyn Fn(SlotProps) -> Element>),
}

impl Default for BarTitle {
    fn default() -> Self {
        Self::Default
    }
}

/// Top slot's rendering mode.
///
/// `Filled` is the path with cross-platform native-chrome parity:
/// leading buttons → UIBarButtonItems / Toolbar items, title →
/// UINavigationItem.titleView, trailing buttons → same as leading
/// but on the right. Per-backend drawer handlers can either
/// render this themselves (web) or translate to native chrome
/// (iOS/Android — pass-2).
///
/// `Custom` is the escape hatch where the author owns the bar's
/// pixel layout. Receives [`SlotProps`] so the closure can read
/// `leading_intent` / `screen_title` and wire the dispatchers
/// itself. On iOS/Android, opting into Custom *replaces* the
/// native nav bar — the handler honors the closure and disables
/// UIKit/Material chrome for that navigator.
pub enum TopSlot {
    Filled {
        leading: Vec<SlotBarButton>,
        title: BarTitle,
        trailing: Vec<SlotBarButton>,
    },
    Custom(Box<dyn Fn(SlotProps) -> Element>),
}

/// Closure type for the slot variants that don't have a Filled /
/// Custom split. `leading`, `bottom`, and `trailing` each take one
/// of these because there's no conventional platform-native widget
/// shape to mirror — every author wants different pixels in those
/// positions, so the SDK doesn't impose a structure beyond the
/// `SlotProps` it hands in.
pub type SlotBuilder = Box<dyn Fn(SlotProps) -> Element>;

// Screens / SDK-foreign code that want to react to the drawer's
// scroll surface should call
// [`runtime_core::primitives::navigator::ambient_scroll_context`]
// — the framework owns the ambient lookup. This SDK no longer
// publishes its own thread-locals (an earlier version did,
// duplicating what's now the framework primitive); the web
// handler still measures the body and constructs a
// [`ScrollContext`] at `init`, then hands it to the framework
// via the per-backend handler's setup.

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
    /// Per-screen override of the navigator's [`DrawerPresentation::mount_policy`].
    /// `None` defers to the navigator-global policy (the default —
    /// matches existing behavior). `Some(MountPolicy::LazyDisposing)`
    /// makes this one screen drop its scope (and stop background
    /// work — animation ticks, render loops, polled effects) when
    /// the user navigates away, even when the rest of the screens
    /// stay cached. `Some(MountPolicy::LazyPersistent)` keeps it
    /// mounted (React Navigation Stack default) even when the
    /// navigator-global says LazyDisposing.
    ///
    /// Pair with the per-screen focus signal (`use_focus()`) for
    /// app-defined pause/resume of embedded work that should stay
    /// mounted but go idle while not focused.
    pub mount_policy: Option<MountPolicy>,
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

    pub fn mount_policy(mut self, policy: MountPolicy) -> Self {
        self.mount_policy = Some(policy);
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
    fn mount_policy(self, policy: MountPolicy) -> Self;
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
    fn mount_policy(self, policy: MountPolicy) -> Self {
        with_drawer_options(self, |o| o.mount_policy = Some(policy))
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
// DrawerPresentation — SDK's typed payload riding on Element::Navigator
// =============================================================================

pub struct DrawerPresentation {
    pub side: DrawerSide,
    pub drawer_type: DrawerType,
    pub drawer_width: f32,
    pub swipe_to_open: bool,
    pub mount_policy: MountPolicy,
    /// Sidebar Element builder. Author sets via `.sidebar(prim)` or
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
    ///
    /// Authors can supply their own via [`DrawerBuilder::is_open`]
    /// to control drawer open/close from outside the navigator
    /// (e.g., a button in the app shell). When unset, the SDK's
    /// constructor-allocated signal is used.
    pub is_open: Signal<bool>,
    // ---- next-gen slot system ----
    /// Persistent chrome at the leading (left in LTR) position.
    /// Set via [`DrawerBuilder::leading_with`]. Eventually
    /// replaces `sidebar` — for now both coexist and the
    /// per-backend handler prefers `leading_slot` when both are
    /// set.
    pub leading_slot: RefCell<Option<SlotBuilder>>,
    /// Persistent top bar. Set via [`DrawerBuilder::top_with`].
    /// On iOS/Android, `TopSlot::Filled` translates to native nav
    /// chrome (pass-2); `TopSlot::Custom` replaces it.
    pub top_slot: RefCell<Option<TopSlot>>,
    /// Persistent bottom bar / footer. Set via
    /// [`DrawerBuilder::bottom_with`]. No native-chrome conflict
    /// on iOS/Android — there's no convention to override.
    pub bottom_slot: RefCell<Option<SlotBuilder>>,
    /// Persistent trailing (right in LTR) column. Set via
    /// [`DrawerBuilder::trailing_with`]. Uncommon but available
    /// for utility-panel layouts.
    pub trailing_slot: RefCell<Option<SlotBuilder>>,
    /// When `true` (the default), the drawer's body div is the
    /// scroll context and the bottom slot mounts inside it as a
    /// flow sibling AFTER the screen — the footer scrolls with
    /// content. Screens drop their own `ScrollView` wrappers and
    /// render directly. Set to `false` via
    /// [`DrawerBuilder::bottom_pinned`] for the historical
    /// behavior: body has `overflow: hidden`, each screen owns
    /// its own scroll context via `ScrollView`, and the bottom
    /// slot pins to the viewport bottom.
    pub bottom_in_scroll: bool,
    /// When `true` (the default), drawer screens render the backend's
    /// native header chrome (iOS `UINavigationController` nav bar,
    /// Android `Toolbar`) seeded from each screen's
    /// [`DrawerScreenOptions::title`]. Set to `false` via
    /// [`DrawerBuilder::native_header`] to suppress that chrome on
    /// every screen so the app owns its header at the page level — a
    /// screen renders its own bar (typically with a menu button via
    /// [`runtime_core::primitives::navigator::ambient_drawer`]) that
    /// looks identical across web/iOS/Android.
    ///
    /// This is the navigator-wide default; a screen can still opt back
    /// in or out with [`DrawerScreenExt::header_shown`], which always
    /// wins (see [`resolve_header_shown`]). Web has no native header to
    /// suppress, so this flag is a no-op there.
    pub native_header: bool,
}

/// Resolve a screen's effective `header_shown` from its per-screen
/// override and the navigator-wide [`DrawerPresentation::native_header`]
/// default. A per-screen `Some(_)` always wins; otherwise the navigator
/// default decides. Shared by the iOS and Android handlers so both
/// backends agree on the same precedence (and so the precedence is
/// host-testable without a device).
///
/// Returns the value the backend's `header_shown` option carries:
/// - `Some(false)` → hide native chrome (page owns the header).
/// - `None` → leave the backend's default (show the native bar).
/// - `Some(true)` → explicit per-screen opt-in to native chrome.
pub(crate) fn resolve_header_shown(per_screen: Option<bool>, native_header: bool) -> Option<bool> {
    match per_screen {
        // Explicit per-screen choice wins over the navigator default.
        Some(v) => Some(v),
        // No per-screen override: defer to the navigator default. When
        // native headers are on we leave it `None` (the backend's
        // existing "show" path); when off we force-hide.
        None => {
            if native_header {
                None
            } else {
                Some(false)
            }
        }
    }
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
            leading_slot: RefCell::new(None),
            top_slot: RefCell::new(None),
            bottom_slot: RefCell::new(None),
            trailing_slot: RefCell::new(None),
            bottom_in_scroll: true,
            native_header: true,
        }
    }
}

/// Register the client-side wire factory that rebuilds a
/// `DrawerPresentation` from the wire's drawer config.
///
/// Called from each platform's `register(&mut backend)` on the
/// runtime-server **client**. The wire client (`dev-client`) is
/// SDK-agnostic — it can't construct a `DrawerPresentation` — so it
/// looks this factory up by nav kind and hands the result to the
/// client's real `Backend::create_navigator`, which dispatches to the
/// platform `NavigatorHandler` registered on the same `register` call.
/// That handler builds the real native chrome.
///
/// Only the serializable config crosses the wire; the sidebar/screen
/// *content* arrives separately as primitive subtrees. So `leading_slot`
/// is a stub builder whose `Element` the client's `build_node` ignores
/// (it returns a holder the wire sidebar mounts into) — but it must be
/// `Some` so the helper creates the sidebar region and calls
/// `build_node`. `is_open` is a fresh client-side signal; the recorder
/// syncs it over the reverse channel.
///
/// Registered on every backend (harmless where `dev-client` is absent,
/// e.g. `--local`, since `build_drawer_presentation` is then never
/// called). Idempotent — last write wins.
pub fn register_wire_drawer_factory() {
    wire::register_drawer_factory(|cfg| {
        let mut p = DrawerPresentation::new();
        p.side = match cfg.side {
            wire::WireDrawerSide::Left => DrawerSide::Start,
            wire::WireDrawerSide::Right => DrawerSide::End,
        };
        p.drawer_type = match cfg.drawer_type {
            wire::WireDrawerType::Slide => DrawerType::Slide,
            // Front / Back both present as an overlay-style drawer on
            // the client; the SDK only models Front / Slide.
            wire::WireDrawerType::Front | wire::WireDrawerType::Back => DrawerType::Front,
        };
        p.drawer_width = cfg.drawer_width;
        p.swipe_to_open = cfg.swipe_to_open;
        p.mount_policy = match cfg.mount_policy {
            wire::WireMountPolicy::EagerPersistent => MountPolicy::EagerPersistent,
            wire::WireMountPolicy::LazyPersistent => MountPolicy::LazyPersistent,
            wire::WireMountPolicy::LazyDisposing => MountPolicy::LazyDisposing,
        };
        // Adopt-sentinel leading slot — see the doc comment. The handler
        // builds its real chrome around this leaf (e.g. iOS wraps it in a
        // `scroll_view`); when `dev-client` materializes the slot via
        // `build_detached`, the walker's External path returns the
        // client's holder node for this marker `TypeId` (the wire sidebar
        // subtree is then inserted into the holder by `DrawerAttachSidebar`).
        // The payload carries an instance only for type safety; the walker
        // intercepts on `type_id` before any payload downcast.
        *p.leading_slot.borrow_mut() = Some(Box::new(|_props: SlotProps| {
            runtime_core::Element::External {
                type_id: std::any::TypeId::of::<wire::WireSidebarAdopt>(),
                type_name: std::any::type_name::<wire::WireSidebarAdopt>(),
                payload: std::rc::Rc::new(wire::WireSidebarAdopt),
                children: Vec::new(),
                style: None,
                ref_fill: None,
                accessibility: runtime_core::accessibility::AccessibilityProps::default(),
            }
        }));
        wire::WireNavBuild {
            type_id: std::any::TypeId::of::<DrawerPresentation>(),
            type_name: std::any::type_name::<DrawerPresentation>(),
            presentation: std::rc::Rc::new(p),
        }
    });
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
        Bound::new(nav.into_element())
    }

    fn into_element(self) -> Element {
        let DrawerNavigator { config, presentation, slot_styles, style, ref_fill } = self;
        // The navigator is the app shell — it must fill its parent box on
        // every backend. macOS sizes its own container and web fills via
        // its CSS classes, but the iOS/Android handlers materialize the
        // navigator container's Taffy node from this `style` field alone.
        // With `style: None` that container carries no size, so as a
        // non-grow flex child (e.g. when the app places the navigator
        // beside a `ToastHost` in a flex column) it collapses to 0 height
        // and the whole app renders blank — even though web looks fine,
        // because web's navigator CSS self-fills. Default to a fill style
        // so the container claims its parent's box uniformly. The walker
        // applies this via `attach_style` on the container (see
        // `walker/navigator.rs`), the same path an explicit style takes.
        let style = style.or_else(|| {
            let mut fill = StyleRules::default();
            fill.flex_grow = Some(1.0f32.into());
            fill.width = Some(runtime_core::Length::pct(100.0).into());
            fill.height = Some(runtime_core::Length::pct(100.0).into());
            Some(Rc::new(StyleSheet::r#static(fill)).into_style_source())
        });
        Element::Navigator {
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

fn with_navigator_prim<F: FnOnce(&mut Element)>(b: &mut Bound<DrawerHandle>, f: F) {
    f(b.primitive_mut());
}

fn with_presentation<F: FnOnce(&DrawerPresentation)>(b: &mut Bound<DrawerHandle>, f: F) {
    if let Element::Navigator { presentation, .. } = b.primitive_mut() {
        if let Some(pres) = presentation.downcast_ref::<DrawerPresentation>() {
            f(pres);
        }
    }
}

fn with_presentation_mut<F: FnOnce(&mut DrawerPresentation)>(
    b: &mut Bound<DrawerHandle>,
    f: F,
) {
    if let Element::Navigator { presentation, .. } = b.primitive_mut() {
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
    /// Pass a pre-built sidebar Element. Used when the sidebar
    /// doesn't need reactive access to nav state.
    fn sidebar(self, prim: Element) -> Self;
    /// Pass a builder closure that receives reactive `DrawerSlotProps`
    /// (active route, is_open, on_select, on_close). Used when the
    /// sidebar's content needs to react to nav state — nav-link
    /// highlights, animated open/close, etc.
    fn sidebar_with<F>(self, f: F) -> Self
    where
        F: Fn(DrawerSlotProps) -> Element + 'static;
    fn sidebar_style(self, s: impl IntoStyleSource) -> Self;
    fn scrim_style(self, s: impl IntoStyleSource) -> Self;
    /// Bundled header styling — sets background/title/tint/body
    /// colors via per-slot reactive style sources the SDK dispatches
    /// via `apply_slot_style`.
    fn header<F>(self, f: F) -> Self
    where
        F: Fn() -> HeaderStyle + 'static;
    fn bind(self, r: Ref<DrawerHandle>) -> Self;

    // ---- next-gen slot builders ----

    /// Mount persistent chrome at the leading edge — sidebar slot.
    /// Replaces `sidebar_with(...)` going forward; both currently
    /// work and the handler prefers `leading_with` if set. The
    /// closure runs once at navigator init and returns a
    /// [`Element`] that survives every screen swap.
    fn leading_with<F>(self, f: F) -> Self
    where
        F: Fn(SlotProps) -> Element + 'static;

    /// Mount the persistent top bar. Pass [`TopSlot::Filled`] for
    /// the platform-conventional shape (leading buttons + title +
    /// trailing buttons), or [`TopSlot::Custom`] to own the bar's
    /// pixels with a closure that receives [`SlotProps`].
    fn top_with(self, slot: TopSlot) -> Self;

    /// Mount persistent chrome at the bottom — footer / toolbar.
    /// Closure runs once at init.
    fn bottom_with<F>(self, f: F) -> Self
    where
        F: Fn(SlotProps) -> Element + 'static;

    /// Mount persistent chrome at the trailing edge — utility
    /// panel / inspector. Closure runs once at init.
    fn trailing_with<F>(self, f: F) -> Self
    where
        F: Fn(SlotProps) -> Element + 'static;

    /// Supply an author-owned `Signal<bool>` for the drawer's
    /// open state. Without this, the SDK allocates one internally
    /// (visible via `DrawerHandle::is_open_signal()` after bind).
    /// Use this when the open state needs to be driven from
    /// outside the navigator — e.g., a button in the app shell, a
    /// keyboard shortcut, or unit tests setting state directly.
    fn is_open(self, sig: Signal<bool>) -> Self;

    /// Switch the drawer to "bottom slot pins to the viewport"
    /// mode (the legacy behavior). The default is
    /// `bottom_in_scroll`: the body div is the scroll context,
    /// the bottom slot mounts inside it, and the footer scrolls
    /// with content — typical for docs sites and content-heavy
    /// drawers. Use `bottom_pinned()` when the footer must stay
    /// visible regardless of scroll position (e.g., a persistent
    /// command bar / status strip).
    ///
    /// Effect on screens: in `bottom_pinned` mode the body is
    /// `overflow: hidden` and each screen must provide its own
    /// scroll context (typically a `ScrollView` wrapper). In the
    /// default `bottom_in_scroll` mode the body provides
    /// scrolling and screens render as flow content.
    fn bottom_pinned(self) -> Self;

    /// Suppress the backend's native header chrome (iOS nav bar,
    /// Android `Toolbar`) on every screen so the app owns its header at
    /// the page level. Pass `false` to opt the whole navigator out;
    /// `true` is the default. A single screen can still override via
    /// [`DrawerScreenExt::header_shown`].
    ///
    /// With native headers off, screens render their own bar — typically
    /// a menu button driven by
    /// [`runtime_core::primitives::navigator::ambient_drawer`], which the
    /// handler publishes on init — and the result looks identical across
    /// web (which never had native chrome), iOS, and Android.
    fn native_header(self, shown: bool) -> Self;
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
            if let Element::Navigator { config, .. } = p {
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

    fn sidebar(mut self, prim: Element) -> Self {
        // Wrap as a closure that yields the captured primitive on
        // first call. Subsequent calls panic — sidebars are built
        // exactly once per navigator lifetime.
        let cell: Rc<RefCell<Option<Element>>> = Rc::new(RefCell::new(Some(prim)));
        let builder: SidebarBuilder = Rc::new(move |_props| {
            cell.borrow_mut()
                .take()
                .expect("drawer-navigator: sidebar Element already consumed")
        });
        with_presentation(&mut self, |p| {
            *p.sidebar.borrow_mut() = Some(builder);
        });
        self
    }

    fn sidebar_with<F>(mut self, f: F) -> Self
    where
        F: Fn(DrawerSlotProps) -> Element + 'static,
    {
        let builder: SidebarBuilder = Rc::new(f);
        with_presentation(&mut self, |p| {
            *p.sidebar.borrow_mut() = Some(builder);
        });
        self
    }

    fn sidebar_style(mut self, s: impl IntoStyleSource) -> Self {
        with_navigator_prim(&mut self, |p| {
            if let Element::Navigator { slot_styles, .. } = p {
                slot_styles.push(("sidebar", s.into_style_source()));
            }
        });
        self
    }

    fn scrim_style(mut self, s: impl IntoStyleSource) -> Self {
        with_navigator_prim(&mut self, |p| {
            if let Element::Navigator { slot_styles, .. } = p {
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
            if let Element::Navigator { slot_styles, .. } = p {
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
            if let Element::Navigator { ref_fill, .. } = p {
                *ref_fill = Some(RefFill::Navigator(Box::new(move |handle| {
                    r.fill(DrawerHandle::from_inner(handle, is_open_signal));
                })));
            }
        });
        self
    }

    fn leading_with<F>(mut self, f: F) -> Self
    where
        F: Fn(SlotProps) -> Element + 'static,
    {
        let builder: SlotBuilder = Box::new(f);
        with_presentation(&mut self, |p| {
            *p.leading_slot.borrow_mut() = Some(builder);
        });
        self
    }

    fn top_with(mut self, slot: TopSlot) -> Self {
        with_presentation(&mut self, |p| {
            *p.top_slot.borrow_mut() = Some(slot);
        });
        self
    }

    fn bottom_with<F>(mut self, f: F) -> Self
    where
        F: Fn(SlotProps) -> Element + 'static,
    {
        let builder: SlotBuilder = Box::new(f);
        with_presentation(&mut self, |p| {
            *p.bottom_slot.borrow_mut() = Some(builder);
        });
        self
    }

    fn trailing_with<F>(mut self, f: F) -> Self
    where
        F: Fn(SlotProps) -> Element + 'static,
    {
        let builder: SlotBuilder = Box::new(f);
        with_presentation(&mut self, |p| {
            *p.trailing_slot.borrow_mut() = Some(builder);
        });
        self
    }

    fn is_open(mut self, sig: Signal<bool>) -> Self {
        // Overwrite the SDK-allocated signal with the author's.
        // Done via `_mut` because Signal is Copy and we're
        // replacing the field, not mutating through interior
        // mutability.
        with_presentation_mut(&mut self, |p| {
            p.is_open = sig;
        });
        self
    }

    fn bottom_pinned(mut self) -> Self {
        with_presentation_mut(&mut self, |p| {
            p.bottom_in_scroll = false;
        });
        self
    }

    fn native_header(mut self, shown: bool) -> Self {
        with_presentation_mut(&mut self, |p| {
            p.native_header = shown;
        });
        self
    }
}

/// Customize the viewport width (px) at which the drawer sidebar flips
/// between its modal (off-canvas, narrow) and pinned (in-flow, wide)
/// layouts. The collapse is driven entirely by a CSS `@media` query in
/// the navigator's shared stylesheet, so this affects the live web layout
/// AND the SSR first paint identically — there is no render-time decision.
///
/// Call once at app setup, before mount / SSR render. Defaults to the
/// Large breakpoint (`runtime_core::breakpoints().lg_min`, 1024 px) when
/// unset. Re-exported from the `css` crate (the single source of truth for
/// navigator layout CSS).
pub use css::{install_navigator_pin_width, navigator_pin_width};

// =============================================================================
// Backend selector
// =============================================================================

#[cfg(target_arch = "wasm32")]
mod web;
#[cfg(target_arch = "wasm32")]
pub use web::register;

// Backend-neutral "primitive chrome" handler (generic over `Backend`).
// No platform cfg, no backend dependency — registered where wanted (the
// SSR backend today) via `drawer_navigator::chrome::register`.
pub mod chrome;

// Recording handler for the runtime-server sidecar's recorder backend
// (`dev-server::WireRecordingBackend`). Emits navigator wire commands
// instead of rendering, so a `DrawerNavigator` app works under
// `idealyst dev` (runtime-server, the default) and can be
// headless-screenshotted. Host-side only — gated behind the
// `runtime-server` feature (off by default; enabled by the sidecar
// build). Registered via `drawer_navigator::recording::register(recorder)`.
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
        install_navigator_pin_width, navigator_pin_width, register, BarButton, BarTitle,
        DrawerBuilder, DrawerCmd, DrawerHandle, DrawerNavigator, DrawerPresentation,
        DrawerScreenExt, DrawerScreenOptions, DrawerSide, DrawerSlotProps, DrawerType, HeaderStyle,
        LeadingIntent, MountPolicy, SlotBarButton, SlotBuilder, SlotProps, TopSlot, TrailingIntent,
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime_core::{Length, Route, VariantSet};

    /// REGRESSION TEST.
    ///
    /// A freshly-built `DrawerNavigator` must carry a default outer
    /// `style` that fills its parent (`flex_grow: 1`, `width: 100%`,
    /// `height: 100%`). The iOS and Android handlers materialize the
    /// navigator container's Taffy node from this style alone; with
    /// `style: None` (the prior behavior) the container collapsed to
    /// 0 height as a non-grow flex child — so an app that placed the
    /// navigator beside a sibling (e.g. a `ToastHost`) in a flex column
    /// rendered a blank screen on both native backends, while web (whose
    /// navigator CSS self-fills) looked fine and hid the bug.
    ///
    /// Asserts the default style resolves to the fill rules. Fails
    /// against the old `style: None`. The on-device behavior (container
    /// actually filling) is verified by running the sim/emulator —
    /// there's no host-side Taffy path for the per-backend navigator
    /// container to assert against here.
    #[test]
    fn navigator_defaults_to_fill_style_so_native_container_doesnt_collapse() {
        let route: Route<()> = Route::new("home", "/");
        let mut nav = DrawerNavigator::new(&route);

        let style = match nav.primitive_mut() {
            Element::Navigator { style, .. } => style.as_ref().expect(
                "navigator must default to a fill style — without it the iOS/Android \
                 container has no size and collapses to 0 height",
            ),
            _ => panic!("DrawerNavigator::new must produce an Element::Navigator"),
        };
        let app = match style {
            StyleSource::Static(app) => app,
            _ => panic!("navigator default style must be a static fill style"),
        };

        let rules = app.sheet.resolve(&VariantSet::new());
        assert_eq!(
            rules.flex_grow,
            Some(1.0f32.into()),
            "navigator must flex-grow to fill the remaining main-axis space",
        );
        assert_eq!(
            rules.width,
            Some(Length::pct(100.0).into()),
            "navigator must be 100% wide",
        );
        assert_eq!(
            rules.height,
            Some(Length::pct(100.0).into()),
            "navigator must be 100% tall — this is the dimension that collapsed",
        );
    }

    fn native_header_of(nav: &mut Bound<DrawerHandle>) -> bool {
        match nav.primitive_mut() {
            Element::Navigator { presentation, .. } => presentation
                .downcast_ref::<DrawerPresentation>()
                .expect("DrawerNavigator presentation must be a DrawerPresentation")
                .native_header,
            _ => panic!("DrawerNavigator::new must produce an Element::Navigator"),
        }
    }

    /// A fresh navigator keeps native headers ON — existing apps that
    /// never call `.native_header(...)` must see no behavior change.
    #[test]
    fn native_header_defaults_on() {
        let route: Route<()> = Route::new("home", "/");
        let mut nav = DrawerNavigator::new(&route);
        assert!(
            native_header_of(&mut nav),
            "native_header must default to true so existing drawers keep their nav bar / Toolbar",
        );
    }

    /// `.native_header(false)` flips the navigator-wide default that the
    /// iOS/Android handlers feed into `resolve_header_shown`.
    #[test]
    fn native_header_false_suppresses_chrome() {
        let route: Route<()> = Route::new("home", "/");
        let mut nav = DrawerNavigator::new(&route).native_header(false);
        assert!(
            !native_header_of(&mut nav),
            ".native_header(false) must record the opt-out on the presentation",
        );
    }

    /// REGRESSION TEST for the page-level-header consolidation.
    ///
    /// `resolve_header_shown` is the single precedence both native
    /// backends share. The contract:
    ///   - a per-screen `header_shown` ALWAYS wins over the navigator
    ///     default (either direction), and
    ///   - with no per-screen override, `native_header = false` must
    ///     force-hide (`Some(false)`) while `native_header = true`
    ///     leaves the backend's existing "show" path (`None`).
    ///
    /// The force-hide arm is the one the consolidation depends on:
    /// without it, an app that calls `.native_header(false)` would still
    /// get the native bar on screens that set a `.title(...)` (every
    /// QuillEMR screen), since title-only screens render a Toolbar / nav
    /// bar by default.
    #[test]
    fn resolve_header_shown_precedence() {
        // No per-screen override → navigator default decides.
        assert_eq!(resolve_header_shown(None, true), None, "native on, no override → show (None)");
        assert_eq!(
            resolve_header_shown(None, false),
            Some(false),
            "native off, no override → force-hide so title-bearing screens drop their bar",
        );
        // Per-screen override always wins, regardless of the default.
        assert_eq!(
            resolve_header_shown(Some(true), false),
            Some(true),
            "a screen can opt back into native chrome even when the navigator suppressed it",
        );
        assert_eq!(
            resolve_header_shown(Some(false), true),
            Some(false),
            "a screen can drop its bar even when the navigator keeps native chrome",
        );
    }
}
