//! Render-elsewhere payload: `portal` (and the new-core [`ScreenNav`]
//! context the portal's visibility effect reads).
//!
//! `overlay` / `anchored_overlay` have NO payload of their own — exactly
//! like the old core (`primitives/overlay.rs`), they are build-time
//! compositions that lower to a `PortalPrim` item with the backdrop /
//! content wiring added around the caller's children (see
//! [`crate::builders`]'s `overlay()` / `anchored_overlay()`).

use std::rc::Rc;

use runtime_core::accessibility::AccessibilityProps;
use runtime_core::primitives::portal::{PortalHandle, PortalTarget};
use runtime_world::ReadSignal;

use crate::style_attach::StyleProp;

/// The `portal` primitive — render children at a window-level target,
/// escaping layout and clipping. Fields mirror what `walker/portal.rs`
/// receives/forwards to `create_portal`.
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
/// `runtime_core::primitives::navigator::ScreenNav` (which carries an
/// old-arena `Signal` and therefore can't cross cores). The navigator
/// handler's `mount_screen` must `provide` one of these into each
/// screen's scope; the portal handler `inject`s it and installs the
/// visibility effect that hides a portal while its owning screen isn't
/// the active route (a portal escapes its screen's view tree to mount on
/// the window, so it isn't detached when the navigator swaps screens —
/// without this, a modal opened on screen A keeps floating over
/// screen B).
///
/// `active_route` is a [`ReadSignal`] (narrowest half — the portal only
/// observes; the navigator keeps the writer).
#[derive(Clone)]
pub struct ScreenNav {
    pub active_route: ReadSignal<&'static str>,
    pub route: &'static str,
}
