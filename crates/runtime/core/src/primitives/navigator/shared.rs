//! Shared navigator substrate — the kind-agnostic core every navigator
//! SDK builds on.
//!
//! The framework owns the substrate (routing, screen scopes, ambient
//! capture, command queue, reactive nav state, per-screen state stack).
//! SDK crates own everything kind-specific (chrome, animations, gestures,
//! typed handles, typed screen options). No kind names appear in this
//! module — `Stack` / `Tab` / `Drawer` are SDK concepts, not framework
//! concepts.
//!
//! What lives here:
//!
//! - `Route<P>` + `RouteParams` — typed route declaration + URL ⇄ params.
//! - `ScreenBuilder` / `RouteEntry` / `ParamsFromSegments` — type-erased
//!   per-route registry the framework walks.
//! - `Screen` + `MountResult` — what a screen builder returns, what
//!   `mount_screen` hands back. SDK-defined options ride as
//!   `Box<dyn Any>`.
//! - `NavCommand` — the command channel. Built-in verbs cover the
//!   common shapes (Push / Pop / Replace / Reset / Select); SDKs add
//!   their own via `NavCommand::Custom(Rc<dyn Any>)`.
//! - `NavigatorControl` — the dispatcher + reactive nav-state bridge.
//! - `NavigatorHandle` — the framework-side handle. Just dispatch +
//!   control accessor; SDK typed handles wrap it.
//! - `NavigatorOps` — the trait the handle's `&dyn NavigatorOps`
//!   points to (currently empty; reserved for backend extension hooks).
//! - `NavState` — reactive `active_route` / `active_path` / `depth` /
//!   `can_go_back` signals layout/chrome subscribes to.
//! - `AmbientNavGuard` / `ambient_navigator()` — thread-local stack
//!   `Link` reads at build time.
//! - `ScreenStateGuard` / `current_screen_state` — per-screen opaque
//!   state stack the screen render closure reads via downcast.
//! - `NavigatorConfig` — the framework-owned routing config (initial
//!   route, screen registry, defer flag). Kind-specific config lives
//!   on the SDK's presentation payload.
//! - `match_pattern` — pure-Rust URL-against-pattern matcher.

use crate::Element;
use std::any::Any;
use std::collections::HashMap;
use std::rc::Rc;

// ---------------------------------------------------------------------------
// Ambient navigator stack — Link primitives find their navigator here
// ---------------------------------------------------------------------------

// The Element-free navigator substrate (registry, path matching,
// Route/RouteParams, NavigatorControl/NavState/NavCommand, screen-state
// stacks, fill-rule sheets, ScreenNav, header slot data) moved to
// `runtime-shared`. This file keeps ONLY the items that reference the
// old-core `Element` wire tree: `Screen` (holds the built body
// Element), the type-erased route registry that returns it
// (`ScreenBuilder`/`RouteEntry`/`NavigatorConfig`), `SwapContext`
// (carries the outlet Element), and `navigator_outlet()`.
pub use runtime_shared::primitives::navigator::shared::*;

pub type ScreenBuilder = Rc<dyn Fn(Box<dyn Any>) -> Screen>;

pub type ParamsFromSegments = Rc<dyn Fn(&HashMap<String, String>) -> Option<Box<dyn Any>>>;

pub struct RouteEntry {
    pub path: &'static str,
    pub build: ScreenBuilder,
    pub from_segments: ParamsFromSegments,
}

/// A renderable screen: the body Element plus SDK-defined options.
///
/// Options are opaque to the framework (`Box<dyn Any>`). Each SDK
/// defines its own typed options struct (e.g. `StackScreenOptions`
/// with title + bar buttons; `TabScreenOptions` with icon + label).
/// Authors call SDK-provided builder methods (`.title(…)`, `.left(…)`)
/// which stash a typed value into `Screen.options`. The SDK handler
/// downcasts at apply time.
///
/// `impl From<Element> for Screen` keeps the no-options form
/// ergonomic: `.screen(R, |_| my_body_view().into())`.
pub struct Screen {
    pub primitive: Element,
    pub options: Box<dyn Any>,
}

impl Screen {
    pub fn new(primitive: impl Into<Element>) -> Self {
        Self {
            primitive: primitive.into(),
            options: Box::new(()),
        }
    }

    /// Set this screen's SDK-defined options. Replaces any existing
    /// options. Each SDK defines its own typed options struct and
    /// exposes builder methods (via an extension trait on `Screen`)
    /// that wrap this.
    pub fn with<T: Any + 'static>(mut self, options: T) -> Self {
        self.options = Box::new(options);
        self
    }

    /// Downcast the options to a borrow of `T`. `None` when this
    /// screen has no options or the stored type doesn't match.
    pub fn options_as<T: Any + 'static>(&self) -> Option<&T> {
        self.options.downcast_ref::<T>()
    }
}

impl From<Element> for Screen {
    fn from(p: Element) -> Self {
        Self::new(p)
    }
}

/// Publish a navigator's screen paths to the collector, if one is
/// enabled. Called by `dispatch_navigator` at mount time; a no-op when
/// the collector is off (live backends).
pub fn record_routes(config: &NavigatorConfig) {
    record_route_paths(config.screens.values().map(|entry| entry.path));
}

/// The value a navigator hands to an author's `.layout(|nav| …)`
/// closure. The closure owns the whole chrome tree and splats
/// [`outlet`](Self::outlet) (`{nav.outlet}`) wherever the active screen
/// should render — the analog of react-router's `useOutletContext()` +
/// `<Outlet/>`. "Tab bar" = wrap the outlet in a bar; "drawer" = wrap it
/// in an idea-ui `Drawer`. The navigator owns only what goes INSIDE the
/// outlet; everything around it is ordinary author layout.
///
/// Shared by the `swap` and `stack` SDKs. `on_select` mirrors
/// `DrawerSlotProps::on_select` — dispatch a `Select` by route name (the
/// common no-param case; typed-param navigation goes through the handle).
///
/// Not `Clone` — the `outlet` [`Element`](crate::element::Element) is a
/// **one-shot value the layout closure splats exactly once, in one stable
/// spot**. It cannot be duplicated into the branches of a reactive
/// `if`/`when`: the walker captures ONE outlet node at layout-build time and
/// the handler swaps screens into exactly that node, so an outlet inside a
/// rebuilt branch would strand the mounted screen. Responsive layouts keep
/// the outlet pinned and reactively toggle the CHROME around it — restyle
/// always-mounted sidebars/bars instead of moving the outlet between
/// branches. `idea_ui_nav::AppShell` packages that pattern (pinned sidebar ⇄
/// off-canvas drawer, one sidebar build, outlet never moves); reach for it
/// before re-deriving the shape by hand.
pub struct SwapContext {
    /// Splat this into the layout tree (`{nav.outlet}`) where the active
    /// screen mounts. One per layout — the walker captures its node so
    /// the handler can swap screens into it.
    pub outlet: crate::element::Element,
    /// Currently active route key — read it to highlight the live tab /
    /// nav item.
    pub active_route: crate::Signal<&'static str>,
    /// Full resolved path of the active screen.
    pub active_path: crate::Signal<String>,
    /// Switch to a sibling screen by route name (`Select`).
    pub on_select: Rc<dyn Fn(&'static str)>,
}

/// [`outlet_fill_rules`] packaged as the `StyleSource` the walker attaches
/// to a style-less `NavigatorOutlet`.
#[cfg_attr(not(feature = "prim-navigator"), allow(dead_code))]
pub(crate) fn default_outlet_style() -> crate::sources::StyleSource {
    static KEY: u8 = 0;
    let sheet = crate::style::cached_stylesheet(&KEY as *const u8 as usize, || {
        Rc::new(crate::style::StyleSheet::r#static(crate::style::StyleRules::default()))
    });
    crate::sources::StyleSource::Static(
        crate::style::StyleApplication::new(sheet)
            .with_computed("__navigator_outlet_fill", outlet_fill_rules),
    )
}

/// Mint an [`crate::element::Element::NavigatorOutlet`] — the placeholder
/// an author layout splats to mark where the active screen renders.
/// Normally reached via [`SwapContext::outlet`]; exposed for hand-built
/// layouts.
pub fn navigator_outlet() -> crate::element::Element {
    crate::element::Element::NavigatorOutlet {
        style: None,
        ref_fill: None,
        accessibility: crate::accessibility::AccessibilityProps::default(),
    }
}

/// The framework-owned routing config carried by every
/// `Element::Navigator`. SDK builders fill this from their
/// `.screen(...)` declarations. Kind-specific config (drawer width,
/// tab placement, sidebar Element, etc.) lives on the SDK's
/// presentation payload, not here.
pub struct NavigatorConfig {
    pub initial: &'static str,
    pub initial_path: &'static str,
    pub screens: HashMap<&'static str, RouteEntry>,
    /// When `true`, the framework does NOT auto-mount the initial
    /// screen — the SDK handler is expected to self-mount (typically
    /// after reading the current URL on web). Defaults to `false`.
    pub defer_initial_mount: bool,
}

impl NavigatorConfig {
    pub fn new(initial: &'static str, initial_path: &'static str) -> Self {
        Self {
            initial,
            initial_path,
            screens: HashMap::new(),
            defer_initial_mount: false,
        }
    }
}
