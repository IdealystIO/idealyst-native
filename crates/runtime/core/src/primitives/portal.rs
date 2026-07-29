//! Portal primitive — render a subtree at a different location in the
//! host tree, escaping the parent's layout and clipping context. The
//! lowest-level "floating UI" capability backends provide.
//!
//! Portals are the only render-elsewhere primitive in the framework.
//! Modals, popovers, dropdowns, tooltips, sheets, alerts — all
//! decompose into `portal()` + (optional) backdrop child + (optional)
//! dismissal handler. The framework ships those as compositions in
//! [`primitives::overlay`]; authors building novel floating UX
//! reach for `portal()` directly.
//!
//! # Cross-platform mapping
//!
//! Each backend implements `Backend::create_portal` against its
//! native window-level mount API:
//!
//! - **Web**: a `<div>` appended to `document.body` (escapes
//!   `overflow:hidden` and stacking contexts). The div's
//!   `position`/`inset`/anchor offset is derived from the target.
//! - **iOS**: window-level `addSubview:` against the key window,
//!   with the frame computed from the target.
//! - **Android**: window-level `WindowManager.addView` or a
//!   `Dialog`-hosted view.
//! - **wgpu / native skins**: top-of-stack rectangle inserted into
//!   the renderer's scene graph at root z.
//! - **Roku**: a `Group` parented to the root scene above all other
//!   content.
//!
//! # Target & positioning
//!
//! [`PortalTarget`] carries the positioning intent rather than a
//! separate "placement" argument. The variants are:
//!
//! - [`PortalTarget::Viewport`] — viewport-relative, positioned by
//!   the embedded [`ViewportPlacement`]. The backend translates
//!   `Center` / `Top` / `Bottom` / `Left` / `Right` / `FullScreen`
//!   into native frames or CSS positioning. Use for modals,
//!   drawers, sheets, alerts.
//! - [`PortalTarget::Anchor`] — element-tracking, positioned by
//!   [`ElementSide`] + [`ElementAlign`] + offset. The backend
//!   subscribes to scroll / layout / orientation events and
//!   re-queries `target.rect()` on each, repositioning the portal
//!   accordingly. Use for popovers, tooltips, dropdowns.
//! - [`PortalTarget::Named`] — mount into a named container
//!   previously registered with the backend. Reserved for future
//!   "slot" routing.
//!
//! # Stacking
//!
//! Portals stack freely. Mounting a second portal while the first
//! is alive layers it on top. Backends order by mount order
//! (z-index on web, addSubview order on iOS, attachment order on
//! Android). Platform dismiss events (Android back, web Escape,
//! iOS swipe-down) are routed to the topmost portal whose
//! `on_dismiss` is set.
//!
//! # Dismissal
//!
//! `on_dismiss` fires only for platform-level dismissal events —
//! NOT for backdrop taps. Backdrop-tap dismissal is composition-
//! level: callers wire a backdrop child (typically a fullscreen
//! `pressable()`) whose `on_click` flips the open-state signal.
//! The framework never auto-tears-down — the host's reactive state
//! is the source of truth; flipping it drops the surrounding scope
//! and triggers [`Backend::release_portal`].

use crate::{Bound, Element, Ref, RefFill};
use std::rc::Rc;

// =============================================================================
// AnchorTarget + AnchorableHandle (formerly in primitives/overlay)
// =============================================================================

// The data/handle/Ops types of this primitive moved to `runtime-shared`
// (the walker-free half); this file keeps the Element/Bound builder
// surface (and its tests). The wildcard re-export preserves every old
// path.
pub use runtime_shared::primitives::portal::*;

/// Build a [`Element::Portal`] mounting `children` at `target`.
///
/// No defaults for backdrop — that's a caller concern. For the
/// common cases (modal, popover, tooltip) reach for the
/// compositions in [`primitives::overlay`].
#[cfg(feature = "prim-portal")]
pub fn portal(target: PortalTarget, children: Vec<Element>) -> Bound<PortalHandle> {
    Bound::new(Element::Portal {
        children,
        target,
        on_dismiss: None,
        trap_focus: false,
        style: None,
        ref_fill: None,
        accessibility: crate::accessibility::AccessibilityProps::default(),
    })
}

impl Bound<PortalHandle> {
    /// Fires when the platform requests dismissal (Android back,
    /// web Escape, iOS swipe-down). The host flips its open-state
    /// signal in response — the framework doesn't auto-unmount.
    /// Backdrop-tap dismissal is composition-level (a backdrop
    /// `pressable()` child with its own `on_click`).
    pub fn on_dismiss<F: Fn() + 'static>(mut self, f: F) -> Self {
        if let Element::Portal { on_dismiss, .. } = &mut self.primitive {
            // Born batched — see `reactive::cycle`.
            *on_dismiss = Some(Rc::new(move || crate::cycle(|| f())));
        }
        self
    }

    /// When `true`, keyboard / accessibility focus is confined to
    /// the portal subtree until it closes. Default `false`.
    pub fn trap_focus(mut self, t: bool) -> Self {
        if let Element::Portal { trap_focus, .. } = &mut self.primitive {
            *trap_focus = t;
        }
        self
    }

    pub fn bind(mut self, r: Ref<PortalHandle>) -> Self {
        if let Element::Portal { ref_fill, .. } = &mut self.primitive {
            *ref_fill = Some(RefFill::Portal(Box::new(move |h| r.fill(h))));
        }
        self
    }
}
