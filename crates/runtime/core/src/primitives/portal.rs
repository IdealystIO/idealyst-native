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
// Anchored-placement resolver (shared by every backend)
// =============================================================================

/// The resolved placement of an anchored overlay: the side actually used
/// (which may differ from the requested side after a collision flip) and
/// the overlay's visual top-left in viewport coordinates.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct AnchorPlacement {
    /// The side the overlay was actually placed on. Equal to the requested
    /// side unless [`resolve_anchored_placement`] flipped it to fit.
    pub side: ElementSide,
    /// Visual top-left x (left edge) in viewport pixels.
    pub x: f32,
    /// Visual top-left y (top edge) in viewport pixels.
    pub y: f32,
}

/// Resolve where an anchored overlay's top-left should sit, given the
/// measured sizes. This is the ONE placement algorithm every backend
/// shares — web, iOS, and Android each used to carry their own copy of
/// this geometry, and they had drifted (the same author intent producing
/// subtly different placement per platform, violating CLAUDE.md §7). The
/// pure math lives here; backends supply the measured rects and apply the
/// result with their native positioning API.
///
/// Inputs are all in viewport pixels (origin = top-left of the viewport):
/// - `trigger` — the anchor element's rect.
/// - `content` — the overlay's measured `(width, height)`.
/// - `viewport` — the viewport's `(width, height)`.
/// - `requested_side` / `align` / `offset` — the author's intent.
/// - `edge_gap` — minimum gutter kept between the overlay and every
///   viewport edge when clamping.
///
/// The algorithm: (1) flip `requested_side` to its opposite if it doesn't
/// fit and the opposite has more room; (2) compute the visual top-left for
/// the chosen side + align using the measured content size; (3) clamp the
/// top-left so the whole content rect stays inside the viewport minus
/// `edge_gap`.
pub fn resolve_anchored_placement(
    trigger: ViewportRect,
    content: (f32, f32),
    viewport: (f32, f32),
    requested_side: ElementSide,
    align: ElementAlign,
    offset: f32,
    edge_gap: f32,
) -> AnchorPlacement {
    let side = pick_anchor_side(requested_side, trigger, content, viewport, offset);
    let (y, x) = anchor_top_left(trigger, side, align, offset, content);
    let (y, x) = clamp_into_viewport(y, x, content, viewport, edge_gap);
    AnchorPlacement { side, x, y }
}

/// The overlay's visual top-left `(top, left)` for a given side + align +
/// offset and measured `content` size — the measured align/side geometry
/// WITHOUT any collision flip or viewport clamp.
///
/// This is the shared piece every backend's anchored-overlay placement is
/// built on. [`resolve_anchored_placement`] composes it with flip + clamp;
/// backends that don't (yet) do a measured flip/clamp pass — the iOS
/// display-link tracker, the Android popup (which passes `content = (0,0)`
/// for its unmeasured initial placement) — call this directly so the
/// align/side math is defined in exactly one place (CLAUDE.md §7).
pub fn anchor_top_left(
    rect: ViewportRect,
    side: ElementSide,
    align: ElementAlign,
    offset: f32,
    content: (f32, f32),
) -> (f32, f32) {
    let (ow, oh) = content;
    let cross_h = |align: ElementAlign| match align {
        ElementAlign::Start => rect.x,
        ElementAlign::Center => rect.x + rect.width / 2.0 - ow / 2.0,
        ElementAlign::End => rect.x + rect.width - ow,
    };
    let cross_v = |align: ElementAlign| match align {
        ElementAlign::Start => rect.y,
        ElementAlign::Center => rect.y + rect.height / 2.0 - oh / 2.0,
        ElementAlign::End => rect.y + rect.height - oh,
    };
    match side {
        ElementSide::Below => (rect.y + rect.height + offset, cross_h(align)),
        ElementSide::Above => (rect.y - offset - oh, cross_h(align)),
        ElementSide::Start => (cross_v(align), rect.x - offset - ow),
        ElementSide::End => (cross_v(align), rect.x + rect.width + offset),
    }
}

/// Pick the side the overlay anchors on. If the requested side lacks room
/// for the measured content, flip to the opposite — unless the opposite is
/// even tighter (then keep the original and let it overflow, matching what
/// most popover libraries do).
fn pick_anchor_side(
    requested: ElementSide,
    trigger: ViewportRect,
    content: (f32, f32),
    viewport: (f32, f32),
    offset: f32,
) -> ElementSide {
    let (ow, oh) = content;
    let (vw, vh) = viewport;
    let needed = match requested {
        ElementSide::Above | ElementSide::Below => oh + offset,
        ElementSide::Start | ElementSide::End => ow + offset,
    };
    let (have, opposite_have, opposite) = match requested {
        ElementSide::Below => (vh - (trigger.y + trigger.height), trigger.y, ElementSide::Above),
        ElementSide::Above => (trigger.y, vh - (trigger.y + trigger.height), ElementSide::Below),
        ElementSide::Start => (trigger.x, vw - (trigger.x + trigger.width), ElementSide::End),
        ElementSide::End => (vw - (trigger.x + trigger.width), trigger.x, ElementSide::Start),
    };
    if have < needed && opposite_have > have {
        opposite
    } else {
        requested
    }
}

/// Clamp the overlay's `(top, left)` so its full content rect stays inside
/// the viewport with an `edge_gap` gutter on every side.
fn clamp_into_viewport(
    top: f32,
    left: f32,
    content: (f32, f32),
    viewport: (f32, f32),
    edge_gap: f32,
) -> (f32, f32) {
    let (ow, oh) = content;
    let (vw, vh) = viewport;
    let max_left = (vw - edge_gap - ow).max(edge_gap);
    let max_top = (vh - edge_gap - oh).max(edge_gap);
    (top.clamp(edge_gap, max_top), left.clamp(edge_gap, max_left))
}

#[cfg(test)]
mod placement_tests {
    use super::*;

    const VP: (f32, f32) = (1000.0, 800.0);
    const GAP: f32 = 8.0;

    fn trigger() -> ViewportRect {
        ViewportRect { x: 400.0, y: 300.0, width: 100.0, height: 40.0 }
    }

    #[test]
    fn below_start_sits_under_the_trigger() {
        let p = resolve_anchored_placement(
            trigger(), (120.0, 60.0), VP, ElementSide::Below, ElementAlign::Start, 4.0, GAP,
        );
        assert_eq!(p.side, ElementSide::Below);
        assert_eq!(p.y, 300.0 + 40.0 + 4.0); // below the trigger + offset
        assert_eq!(p.x, 400.0); // start-aligned to trigger left
    }

    #[test]
    fn center_align_centers_content_on_trigger() {
        let p = resolve_anchored_placement(
            trigger(), (120.0, 60.0), VP, ElementSide::Below, ElementAlign::Center, 0.0, GAP,
        );
        // trigger center x = 450; content half-width 60 → left 390.
        assert_eq!(p.x, 390.0);
    }

    #[test]
    fn below_flips_to_above_when_it_does_not_fit() {
        // Trigger near the bottom: not enough room below for a tall overlay,
        // but plenty above → flip to Above.
        let low = ViewportRect { x: 400.0, y: 760.0, width: 100.0, height: 30.0 };
        let p = resolve_anchored_placement(
            low, (120.0, 200.0), VP, ElementSide::Below, ElementAlign::Start, 0.0, GAP,
        );
        assert_eq!(p.side, ElementSide::Above);
        assert_eq!(p.y, 760.0 - 200.0); // above: trigger.y - offset - content height
    }

    #[test]
    fn keeps_requested_side_when_opposite_is_tighter() {
        // Trigger near the top with a very tall overlay: Below doesn't quite
        // fit (750 < 760 needed) but Above is far tighter (only 20) → keep
        // the requested Below and let it overflow.
        let near_top = ViewportRect { x: 10.0, y: 20.0, width: 100.0, height: 30.0 };
        let p = resolve_anchored_placement(
            near_top, (120.0, 760.0), VP, ElementSide::Below, ElementAlign::Start, 0.0, GAP,
        );
        assert_eq!(p.side, ElementSide::Below);
    }

    #[test]
    fn clamps_into_viewport_with_gap() {
        // End-aligned far-right trigger would push content off-screen; clamp.
        let right = ViewportRect { x: 980.0, y: 300.0, width: 15.0, height: 40.0 };
        let p = resolve_anchored_placement(
            right, (200.0, 60.0), VP, ElementSide::Below, ElementAlign::Start, 0.0, GAP,
        );
        // max_left = 1000 - 8 - 200 = 792; content can't exceed it.
        assert_eq!(p.x, 792.0);
        assert!(p.x >= GAP && p.y >= GAP);
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
