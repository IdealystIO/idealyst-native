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
use std::any::Any;
use std::rc::Rc;

// =============================================================================
// AnchorTarget + AnchorableHandle (formerly in primitives/overlay)
// =============================================================================

/// Type-erased handle to an anchor target. Constructed via
/// [`AnchorTarget::from`] on any `Ref<H>` whose handle type
/// implements [`AnchorableHandle`].
///
/// The erasure here lets a single [`PortalTarget::Anchor`] accept any
/// primitive's ref without itself being generic. Backends query the
/// target through the [`AnchorableHandle::rect`] trait method, which
/// downcasts the type-erased node back to its concrete backend type
/// at the call site.
#[derive(Clone)]
pub struct AnchorTarget {
    inner: Rc<dyn AnchorTargetInner>,
}

impl AnchorTarget {
    pub fn from<H: AnchorableHandle + 'static>(r: Ref<H>) -> Self {
        Self { inner: Rc::new(AnchorTargetRef(r)) }
    }

    /// Resolve to a viewport-relative rect, or `None` if the
    /// underlying ref hasn't been filled yet (its primitive hasn't
    /// mounted) or the backend can't measure this handle type.
    pub fn rect(&self) -> Option<ViewportRect> {
        self.inner.rect()
    }
}

trait AnchorTargetInner {
    fn rect(&self) -> Option<ViewportRect>;
}

struct AnchorTargetRef<H: AnchorableHandle>(Ref<H>);
impl<H: AnchorableHandle> AnchorTargetInner for AnchorTargetRef<H> {
    fn rect(&self) -> Option<ViewportRect> {
        let handle = self.0.get()?;
        Some(handle.rect())
    }
}

/// Marker trait every primitive handle implements (or doesn't) to opt
/// into being used as an anchor target. The `rect()` method goes
/// through the handle's existing `*Ops` trait — backends implement
/// the position measurement once per primitive kind.
pub trait AnchorableHandle: Clone + 'static {
    fn rect(&self) -> ViewportRect;
}

/// Viewport-relative rect, in CSS pixels (or the backend's
/// equivalent point unit). Origin is top-left of the viewport.
#[derive(Copy, Clone, Debug, PartialEq, Default)]
pub struct ViewportRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

// =============================================================================
// Placement model
// =============================================================================

/// Where a viewport-anchored portal sits in the window.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ViewportPlacement {
    Center,
    Top,
    Bottom,
    Left,
    Right,
    FullScreen,
}

impl Default for ViewportPlacement {
    fn default() -> Self {
        ViewportPlacement::Center
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ElementSide {
    Above,
    Below,
    Start,
    End,
}

impl Default for ElementSide {
    fn default() -> Self {
        ElementSide::Below
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ElementAlign {
    Start,
    Center,
    End,
}

impl Default for ElementAlign {
    fn default() -> Self {
        ElementAlign::Start
    }
}

// =============================================================================
// Target
// =============================================================================

/// Where a portal mounts in the host tree, plus the positioning
/// intent the backend uses to lay out the portal's frame within that
/// target.
#[derive(Clone)]
pub enum PortalTarget {
    /// Viewport-rooted, positioned per the embedded placement.
    Viewport(ViewportPlacement),
    /// Anchored to an element, positioned per side / align / offset.
    Anchor {
        target: AnchorTarget,
        side: ElementSide,
        align: ElementAlign,
        offset: f32,
    },
    /// Named slot (reserved for future use).
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

/// Build a [`Element::Portal`] mounting `children` at `target`.
///
/// No defaults for backdrop — that's a caller concern. For the
/// common cases (modal, popover, tooltip) reach for the
/// compositions in [`primitives::overlay`].
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
            *on_dismiss = Some(Rc::new(f));
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
