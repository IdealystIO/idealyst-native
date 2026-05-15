//! Overlay primitive — render a subtree above the rest of the UI,
//! escaping the parent's layout/clip, optionally anchored to either
//! the viewport or another primitive's bounds.
//!
//! This is the platform abstraction every floating-UI affordance
//! (modal, drawer, popover, tooltip, dropdown, context menu) builds
//! on. Each backend implements it against its native window-level
//! presentation API — web portals to `<body>` with
//! `position: fixed`; iOS uses `UIView`/window-level addSubview or
//! `UIViewController` presentation; Android uses `Dialog` /
//! `PopupWindow`. The contract this module defines is what's stable
//! across all of them.
//!
//! # Stacking
//!
//! Overlays stack freely — opening a second overlay while the first
//! is still mounted layers it on top. The framework doesn't enforce
//! any "only one overlay at a time" rule; backends are responsible
//! for ordering the rendered layers by mount order (z-index on web,
//! addSubview order on iOS, attachment order on Android).
//!
//! Dismiss events from the platform (back button on Android, escape
//! key on web, swipe-down on iOS) are routed to the topmost overlay
//! only. The framework's walker maintains the stack via the natural
//! mount order — each new `Primitive::Overlay` mount pushes; each
//! cleanup pops.
//!
//! # Anchoring
//!
//! Two flavors:
//!
//! - [`OverlayAnchor::Viewport`] — positioned relative to the
//!   viewport (centered, edge-pinned, full-screen). The common case
//!   for modals, drawers, and sheets.
//! - [`OverlayAnchor::Element`] — positioned relative to another
//!   primitive's rendered bounds. The common case for popovers,
//!   tooltips, dropdowns, context menus. Requires the anchor's
//!   `Ref<H>` so the backend can query its native position.
//!
//! The element-anchored path requires backends to expose a way to
//! measure a node's viewport-relative rect. The framework reaches
//! into the node via the [`Anchorable`] marker (impl'd by every
//! visible-primitive handle in the backend impl) and an `ops.rect()`
//! method, the same shape used for other imperative handle APIs.
//!
//! # Dismiss
//!
//! When the platform fires a "dismiss me" event (Escape, back gesture,
//! click-outside on a `Dismiss` backdrop), the backend calls the
//! `on_dismiss` callback the framework handed it. The host is
//! expected to flip its open-state signal in that callback — which
//! causes the surrounding `when`/`switch` branch to flip and the
//! Overlay's scope to drop. Backends do NOT auto-tear-down the
//! overlay on dismiss; the host's reactive state is the source of
//! truth.
//!
//! # Animation
//!
//! Out of scope for v1. Overlays mount/unmount instantly. A future
//! `Presence` primitive can hold a child subtree alive for a
//! configurable duration after its `when` condition flips, letting
//! exit transitions on stylesheets actually play.

use crate::{Bound, Primitive, Ref, RefFill};
use std::any::Any;
use std::rc::Rc;

// =============================================================================
// Placement model
// =============================================================================

/// How an overlay is positioned. See [`OverlayAnchor::Viewport`] for
/// "centered modal" / "edge-pinned drawer" use cases, and
/// [`OverlayAnchor::Element`] for popovers / tooltips that follow a
/// trigger element.
#[derive(Clone)]
pub enum OverlayAnchor {
    Viewport(ViewportPlacement),
    Element(ElementAnchor),
}

impl Default for OverlayAnchor {
    fn default() -> Self {
        OverlayAnchor::Viewport(ViewportPlacement::Center)
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ViewportPlacement {
    /// Centered in the viewport. Most common for modals.
    Center,
    /// Pinned to the top edge, full width. Banners, page-top sheets.
    Top,
    /// Pinned to the bottom edge, full width. Bottom sheets.
    Bottom,
    /// Pinned to the left edge, full height. Left drawers.
    Left,
    /// Pinned to the right edge, full height. Right drawers.
    Right,
    /// Covers the entire viewport with no padding.
    FullScreen,
}

/// Element-anchored positioning — the overlay follows a specific
/// primitive's rendered bounds. The backend queries the target's
/// rect on mount and again whenever it changes (scroll, layout,
/// resize) so the overlay tracks the trigger.
#[derive(Clone)]
pub struct ElementAnchor {
    /// The primitive to anchor against. Held as a type-erased
    /// reference so the same `ElementAnchor` can target any kind of
    /// primitive (Button, View, Text, etc).
    pub target: AnchorTarget,
    /// Which side of the target the overlay sits on.
    pub side: ElementSide,
    /// How the overlay aligns along the anchor's edge. For
    /// `ElementSide::Below`, `Start` aligns the overlay's left edge
    /// with the anchor's left; `End` aligns rights; `Center` centers
    /// horizontally. Symmetric for the other sides.
    pub align: ElementAlign,
    /// Gap in pixels between the anchor and the overlay.
    pub offset: f32,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ElementSide {
    Above,
    Below,
    Start,
    End,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ElementAlign {
    Start,
    Center,
    End,
}

/// Type-erased handle to an anchor target. Constructed via
/// [`AnchorTarget::from`] on any `Ref<H>` whose handle type
/// implements [`AnchorableHandle`].
///
/// The erasure here lets a single `ElementAnchor` accept any
/// primitive's ref without `ElementAnchor` itself being generic.
/// Backends query the target through the [`AnchorableHandle::rect`]
/// trait method, which downcasts the type-erased node back to its
/// concrete backend type at the call site.
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

/// Internal: type-erased lookup target. One impl per ref type.
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
// Backdrop
// =============================================================================

/// How the overlay's backdrop behaves.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum BackdropMode {
    /// Semi-transparent scrim. Clicks on the scrim fire the
    /// `on_dismiss` callback.
    #[default]
    Dismiss,
    /// Semi-transparent scrim. Clicks on the scrim do NOT dismiss;
    /// the host must drive open/close itself (e.g. via the
    /// overlay's own buttons). Use when dismissal must be
    /// deliberate.
    Opaque,
    /// No scrim at all. The viewport behind the overlay remains
    /// interactive. Use for popovers, tooltips, dropdowns.
    None,
}

// =============================================================================
// Handle + ops
// =============================================================================

#[derive(Clone)]
pub struct OverlayHandle {
    node: Rc<dyn Any>,
    #[allow(dead_code)]
    ops: &'static dyn OverlayOps,
}

impl OverlayHandle {
    pub fn new(node: Rc<dyn Any>, ops: &'static dyn OverlayOps) -> Self {
        Self { node, ops }
    }

    /// Convenience for backends and tests; not stable user API.
    pub fn node(&self) -> &dyn Any {
        &*self.node
    }
}

pub trait OverlayOps {}

// =============================================================================
// Constructor + builder
// =============================================================================

/// Build an Overlay primitive holding the given children. The
/// returned [`Bound<OverlayHandle>`] supports the usual builder
/// methods (`.anchor(...)`, `.backdrop(...)`, `.on_dismiss(...)`,
/// `.trap_focus(...)`, `.with_style(...)`, `.backdrop_style(...)`,
/// `.bind(...)`).
///
/// By default an overlay is centered in the viewport with a
/// dismiss-on-click backdrop and focus trap enabled.
pub fn overlay(children: Vec<Primitive>) -> Bound<OverlayHandle> {
    Bound::new(Primitive::Overlay {
        children,
        anchor: OverlayAnchor::default(),
        backdrop: BackdropMode::default(),
        backdrop_style: None,
        on_dismiss: None,
        trap_focus: true,
        style: None,
        ref_fill: None,
    })
}

impl Bound<OverlayHandle> {
    pub fn anchor(mut self, a: OverlayAnchor) -> Self {
        if let Primitive::Overlay { anchor, .. } = &mut self.primitive {
            *anchor = a;
        }
        self
    }

    pub fn backdrop(mut self, b: BackdropMode) -> Self {
        if let Primitive::Overlay { backdrop, .. } = &mut self.primitive {
            *backdrop = b;
        }
        self
    }

    pub fn backdrop_style<S: crate::IntoStyleSource>(mut self, s: S) -> Self {
        if let Primitive::Overlay { backdrop_style, .. } = &mut self.primitive {
            *backdrop_style = Some(s.into_style_source());
        }
        self
    }

    pub fn on_dismiss<F: Fn() + 'static>(mut self, f: F) -> Self {
        if let Primitive::Overlay { on_dismiss, .. } = &mut self.primitive {
            *on_dismiss = Some(Rc::new(f));
        }
        self
    }

    pub fn trap_focus(mut self, t: bool) -> Self {
        if let Primitive::Overlay { trap_focus, .. } = &mut self.primitive {
            *trap_focus = t;
        }
        self
    }

    pub fn bind(mut self, r: Ref<OverlayHandle>) -> Self {
        if let Primitive::Overlay { ref_fill, .. } = &mut self.primitive {
            *ref_fill = Some(RefFill::Overlay(Box::new(move |h| r.fill(h))));
        }
        self
    }
}
