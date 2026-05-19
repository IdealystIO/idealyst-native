//! Portal primitive — render a subtree at a different location in the
//! host tree, escaping the parent's layout and clipping context. The
//! lowest-level "floating UI" capability backends provide.
//!
//! Portals are the only render-elsewhere primitive in the framework.
//! Modals, popovers, dropdowns, tooltips, sheets, alerts — all
//! decompose into `portal()` + positioning + (optional) backdrop +
//! (optional) dismissal. The framework ships those as compositions in
//! [`primitives::overlay`]; authors building novel floating UX
//! reach for `portal()` directly.
//!
//! # Cross-platform mapping
//!
//! Each backend implements `Backend::create_portal` against its
//! native window-level mount API:
//!
//! - **Web**: append a `<div>` to `document.body` (escapes
//!   `overflow:hidden` ancestors and stacking contexts). For an
//!   element-anchored portal the div remains body-mounted and the
//!   backend updates its `transform` from the anchor's bounding rect.
//! - **iOS**: `UIWindow`-level `addSubview:` against the key window,
//!   or a window-spanning `UIView` overlay container.
//! - **Android**: window-level `WindowManager.addView` or a
//!   `Dialog`-hosted view.
//! - **wgpu / native skins**: top-of-stack rectangle inserted into
//!   the renderer's scene graph at root z.
//! - **Roku**: a `Group` parented to the root scene above all other
//!   content.
//!
//! Backends are free to leverage native popover APIs
//! (`UIPopoverPresentationController`, `PopupWindow`,
//! `<dialog popover>`) when the [`PortalTarget`] permits — for
//! example a [`PortalTarget::Anchor`] portal with appropriate flags
//! could route to a native popover on iOS rather than a custom
//! positioned `UIView`. The trait surface deliberately leaves room
//! for that without requiring it.
//!
//! # Stacking
//!
//! Portals stack freely. The framework doesn't enforce a "one at a
//! time" rule — opening a second portal while the first is mounted
//! layers it on top. Backends order by mount order (z-index on web,
//! addSubview order on iOS, attachment order on Android).
//!
//! Platform dismiss events (Android back, web Escape, iOS swipe-down)
//! are routed to the topmost portal whose `on_dismiss` is set. The
//! framework doesn't auto-tear-down — the host is expected to flip
//! its open-state signal in response, which causes the surrounding
//! `when`/`switch` branch to drop the portal's scope.
//!
//! # Anchoring
//!
//! [`PortalTarget::Anchor`] holds a type-erased [`AnchorTarget`] —
//! same shape as the existing element-anchor mechanism in
//! [`primitives::overlay`]. The backend treats the anchor's
//! `rect()` as live; on each scroll / layout / orientation event the
//! backend re-queries it and repositions the portal contents.
//!
//! Placement math (preferred side, flip-on-overflow, edge clamping)
//! is **user-space**: callers either compose with a positioning
//! utility or wire raw `transform` updates inside their portal
//! children. The framework's contract here is "give me a live rect
//! and a place to mount" — nothing about preferred sides or fallback
//! flips. Compositions like [`primitives::overlay::anchored_overlay`]
//! own those decisions.

use crate::{Bound, Primitive, Ref, RefFill};
use crate::primitives::overlay::AnchorTarget;
use std::any::Any;
use std::rc::Rc;

// =============================================================================
// Target
// =============================================================================

/// Where a portal mounts in the host tree.
#[derive(Clone)]
pub enum PortalTarget {
    /// Mount at the viewport root. Equivalent to "body portal" on
    /// web, key-window `addSubview` on iOS, top-level
    /// `WindowManager.addView` on Android. The common case for
    /// modals, full-screen sheets, drawers, alerts.
    Viewport,
    /// Mount as a viewport-rooted overlay whose position tracks an
    /// anchor element. The backend re-queries `target.rect()` on
    /// every scroll / layout / orientation event and updates the
    /// portal's position to match. The common case for popovers,
    /// tooltips, dropdowns, context menus.
    Anchor(AnchorTarget),
    /// Mount into a named container previously registered with the
    /// backend. Hosts a "slot" mechanism for custom routing — e.g.
    /// a toast region, a global snackbar host, an embed slot.
    /// Reserved for future use; backends may unimplemented! this
    /// until there's a concrete consumer.
    Named(&'static str),
}

// =============================================================================
// Handle + ops
// =============================================================================

#[derive(Clone)]
pub struct PortalHandle {
    node: Rc<dyn Any>,
    #[allow(dead_code)]
    ops: &'static dyn PortalOps,
}

impl PortalHandle {
    pub fn new(node: Rc<dyn Any>, ops: &'static dyn PortalOps) -> Self {
        Self { node, ops }
    }

    pub fn node(&self) -> &dyn Any {
        &*self.node
    }
}

pub trait PortalOps {}

// =============================================================================
// Constructor + builder
// =============================================================================

/// Build a [`Primitive::Portal`] mounting `children` at `target`.
///
/// Returns a [`Bound<PortalHandle>`] supporting `.on_dismiss(...)`,
/// `.trap_focus(...)`, `.with_style(...)`, `.bind(...)`. No defaults
/// for positioning or backdrop — those are caller concerns. For the
/// common cases ship a composition (modal, popover, tooltip) on
/// top.
pub fn portal(target: PortalTarget, children: Vec<Primitive>) -> Bound<PortalHandle> {
    Bound::new(Primitive::Portal {
        children,
        target,
        on_dismiss: None,
        trap_focus: false,
        style: None,
        ref_fill: None,
    })
}

impl Bound<PortalHandle> {
    /// Fires when the platform requests dismissal (Android back,
    /// web Escape, iOS swipe-down). The host flips its open-state
    /// signal in response — the framework doesn't auto-unmount.
    pub fn on_dismiss<F: Fn() + 'static>(mut self, f: F) -> Self {
        if let Primitive::Portal { on_dismiss, .. } = &mut self.primitive {
            *on_dismiss = Some(Rc::new(f));
        }
        self
    }

    /// When `true`, keyboard / accessibility focus is confined to
    /// the portal subtree until it closes. Default `false`.
    pub fn trap_focus(mut self, t: bool) -> Self {
        if let Primitive::Portal { trap_focus, .. } = &mut self.primitive {
            *trap_focus = t;
        }
        self
    }

    pub fn bind(mut self, r: Ref<PortalHandle>) -> Self {
        if let Primitive::Portal { ref_fill, .. } = &mut self.primitive {
            *ref_fill = Some(RefFill::Portal(Box::new(move |h| r.fill(h))));
        }
        self
    }
}
