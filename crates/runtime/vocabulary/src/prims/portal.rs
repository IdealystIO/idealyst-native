//! Render-elsewhere payload: `portal` (and the [`ScreenNav`] context the
//! portal's visibility effect reads).
//!
//! `overlay` / `anchored_overlay` have NO payload of their own — they
//! are build-time compositions that lower to a `PortalPrim` item with the backdrop /
//! content wiring added around the caller's children (see
//! [`crate::builders`]'s `overlay()` / `anchored_overlay()`).

use std::rc::Rc;

use runtime_shared::accessibility::AccessibilityProps;
use runtime_shared::primitives::navigator::query::QueryParams;
use runtime_shared::primitives::portal::{PortalHandle, PortalTarget};
use runtime_world::ReadSignal;

use crate::style_attach::StyleProp;

/// The `portal` primitive — render children at a window-level target,
/// escaping layout and clipping. Fields are what the handler forwards to
/// `create_portal`.
pub struct PortalPrim {
    pub target: PortalTarget,
    /// Platform-level dismissal (web Escape, Android back, iOS
    /// swipe-down) — NOT backdrop taps, which are composition-level.
    pub on_dismiss: Option<Rc<dyn Fn()>>,
    pub trap_focus: bool,
    pub style: Option<StyleProp>,
    pub a11y: AccessibilityProps,
    pub ref_fill: Option<Box<dyn FnOnce(PortalHandle)>>,
}

/// Per-screen navigation context — the NEW-core port of
/// `runtime_shared::primitives::navigator::ScreenNav` (which carries an
/// old-arena `Signal` and therefore can't cross cores). The navigator
/// handler's `mount_screen` must `provide` one of these into each
/// screen's scope; the portal handler `inject`s it and installs the
/// visibility effect that hides a portal while its owning screen isn't
/// the active route (a portal escapes its screen's view tree to mount on
/// the window, so it isn't detached when the navigator swaps screens —
/// without this, a modal opened on screen A keeps floating over
/// screen B).
///
/// It is also how a mounted screen hears its own state change. `SwapNav` and
/// `StackNav` are provided after the first screen builds, and a nested
/// navigator would shadow them, so a screen reads `inject::<ScreenNav>()`:
/// see [`active_query`](Self::active_query).
///
/// `active_route` is a [`ReadSignal`] (narrowest half — the portal only
/// observes; the navigator keeps the writer).
#[derive(Clone)]
pub struct ScreenNav {
    pub active_route: ReadSignal<&'static str>,
    pub route: &'static str,
    /// The navigator's active query, whichever screen that is: a kept-alive
    /// screen that isn't showing sees the active screen's. Prefer
    /// [`active_query`](Self::active_query).
    pub query: ReadSignal<QueryParams>,
}

impl ScreenNav {
    /// This screen's current query, or `None` while another screen of the
    /// navigator is active. Reactive: read it in an effect or a reactive
    /// region to follow a query-only navigation, which does not rebuild the
    /// screen. `screen_query()` is the snapshot from when it was built.
    pub fn active_query(&self) -> Option<QueryParams> {
        (self.active_route.get() == self.route).then(|| self.query.get())
    }
}
