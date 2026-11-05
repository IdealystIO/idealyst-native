//! Portal positioning on macOS — the anchored-overlay (tooltip /
//! popover / dropdown) half of the portal primitive.
//!
//! `create_portal` mounts a viewport-spanning container into the host
//! window's `contentView` and registers it as its own Taffy root (see
//! `imp::mod`). For [`PortalTarget::Viewport`] the container's flex
//! style places the single content child in the requested region —
//! that path needs nothing here.
//!
//! For [`PortalTarget::Anchor`] the container uses a NEUTRAL flex style
//! and the content child is positioned ABSOLUTELY against the trigger's
//! viewport rect: an initial inset style on `insert` (so the first
//! paint lands near the anchor, not top-left), then a `raf_loop` that
//! re-pins the child's frame origin every frame — tracking the anchor
//! through scrolls / reactive relayouts / window resizes. A layout pass keeps
//! a placed child's origin and re-places it itself, so a resize lands in one
//! commit. This mirrors
//! the iOS backend's `CADisplayLink` tracker (`backend-ios-mobile`);
//! `raf_loop` is macOS's frame driver (it already drives the color
//! transitions in `imp::transitions`).
//!
//! Before this existed, the macOS backend dropped the anchor metadata
//! entirely and laid the content out at the container's top-left — the
//! "tooltip / popover renders in the corner" bug.

use std::cell::RefCell;
use std::rc::Rc;

use objc2::msg_send;
use objc2::rc::Retained;
use objc2_app_kit::NSView;
use objc2_foundation::{CGPoint, CGRect};

use runtime_shared::primitives::portal::{
    AnchorTarget, AnchoredPlacer, ElementAlign, ElementSide, ViewportRect, ANCHOR_EDGE_GAP,
};
use runtime_shared::scheduling::RafLoop;
use runtime_shared::{
    Length, Position, StyleRules, Tokenized,
};

/// Per-anchored-portal state, keyed by the container view's pointer.
/// Viewport portals get NO entry — they need no tracking.
pub(crate) struct PortalEntry {
    /// The anchor descriptor: trigger + side / align / offset. Used to
    /// compute the content child's initial absolute style and to re-pin
    /// it each frame.
    pub(crate) anchor: AnchorSpec,
    /// Live frame loop re-pinning the content child. `None` until the
    /// first content child is inserted; replaced when a later child
    /// (the content, after a backdrop) re-claims the tracker. Dropping
    /// it stops the loop (see `RafLoop`), which is how `release_portal`
    /// and re-tracking tear the old loop down.
    pub(crate) tracker: Option<RafLoop>,
    /// The placement the tracker and the layout pass share. Replaced with the
    /// tracker.
    pub(crate) pin: Option<Rc<RefCell<AnchorPin>>>,
}

impl PortalEntry {
    /// The content view whose origin belongs to [`AnchorPin`], once it has
    /// placed it. Before that the layout pass still writes the fallback: a
    /// trigger that never measures reveals the content there.
    pub(crate) fn pinned_content(&self) -> Option<usize> {
        self.pin.as_ref().and_then(|p| {
            let p = p.borrow();
            p.placed.then(|| Retained::as_ptr(&p.popover) as usize)
        })
    }
}

/// An anchored panel's placement: the content view, its trigger, and the shared
/// placer, which remembers the side it settled on.
pub(crate) struct AnchorPin {
    popover: Retained<NSView>,
    target: AnchorTarget,
    placer: AnchoredPlacer,
    placed: bool,
}

/// What [`AnchorPin::place`] could do this time.
enum Placement {
    Placed,
    /// The trigger or the panel has no size yet.
    Unmeasured,
    /// The container has no size yet.
    NoViewport,
}

impl AnchorPin {
    /// Re-resolve the placement against the current trigger rect and panel
    /// size, and move the panel if it is not there already.
    fn place(&mut self) -> Placement {
        let trigger = self.target.rect().filter(|t| t.width > 0.0 || t.height > 0.0);
        let pop_frame: CGRect = unsafe { msg_send![&*self.popover, frame] };
        let pop_w = pop_frame.size.width as f32;
        let pop_h = pop_frame.size.height as f32;

        let Some(trigger) = trigger else {
            return Placement::Unmeasured;
        };
        // Taffy hasn't sized the popover yet — `Above` / `Start` origin
        // math depends on the popover size, so defer to a later frame.
        if pop_w <= 0.0 || pop_h <= 0.0 {
            return Placement::Unmeasured;
        }

        // The container spans the window from its origin, so its bounds
        // ARE the viewport the placement clamps against. Not laid out yet
        // → skip this frame: clamping against a zero viewport would pin the
        // popover into the top-left corner.
        let Some(viewport) = container_viewport(&self.popover) else {
            return Placement::NoViewport;
        };

        // The shared placement (runtime_shared) — the same flip + clamp web,
        // iOS and Linux run, re-resolved with the CURRENT popover size so a
        // content resize re-places (CLAUDE.md §7).
        let placed = self.placer.place(trigger, (pop_w, pop_h), viewport);
        let (top, left) = (placed.y, placed.x);

        // Compare against the LIVE frame: the trigger's rect or the panel's
        // size may have moved it since the last write.
        let cur_left = pop_frame.origin.x as f32;
        let cur_top = pop_frame.origin.y as f32;
        let in_place = (cur_top - top).abs() < ANCHOR_TRACKER_EPSILON
            && (cur_left - left).abs() < ANCHOR_TRACKER_EPSILON;
        if !in_place {
            let new_frame = CGRect {
                origin: CGPoint { x: left as f64, y: top as f64 },
                size: pop_frame.size,
            };
            let _: () = unsafe { msg_send![&*self.popover, setFrame: new_frame] };
        }
        self.placed = true;
        Placement::Placed
    }

    /// The layout pass's half: place the panel in the pass that sized it, so
    /// the commit after a resize already has the panel on its trigger.
    pub(crate) fn place_after_layout(&mut self) {
        let _ = self.place();
    }
}

#[derive(Clone)]
pub(crate) struct AnchorSpec {
    pub(crate) target: AnchorTarget,
    pub(crate) side: ElementSide,
    pub(crate) align: ElementAlign,
    pub(crate) offset: f32,
    pub(crate) collision_insets: runtime_shared::EdgeInsets,
}

pub(crate) type PortalInstances = std::collections::HashMap<usize, PortalEntry>;

/// `StyleRules` for an anchored-portal CONTAINER. Neutral flex — the
/// content child positions itself via `position: absolute` + insets, so
/// the container just needs to span the viewport (where the absolute
/// coordinates live) without stretching or centering the child. The
/// viewport-placement path keeps using `portal_container_style` in
/// `imp::mod`; this is only for the anchor case.
pub(crate) fn container_style_for_anchor() -> StyleRules {
    runtime_shared::primitives::portal::anchored_container_rules()
}

/// Fallback popover height used before Taffy has measured the content —
/// the initial guess for `Above`, corrected on the tracker's first
/// frame once the real size is known.
const ANCHOR_POPOVER_HEIGHT_FALLBACK: f32 = 100.0;
/// Same idea for the `Start` (overlay-to-the-left) case.
const ANCHOR_POPOVER_WIDTH_FALLBACK: f32 = 200.0;
/// Minimum inset the unmeasured `Above`/`Start` fallbacks clamp to, so
/// the popover can't render off-screen before the tracker corrects it.
const ANCHOR_FALLBACK_MIN_INSET: f32 = 8.0;
/// Inset used for the fully-unmeasured (trigger not yet on screen)
/// defensive fallback.
const ANCHOR_UNMEASURED_INSET: f32 = 16.0;
/// Sub-pixel threshold below which the tracker treats the popover as
/// already in place — avoids per-frame `setFrame:` churn when the
/// anchor isn't moving (both the live frame and the computed origin
/// drift by tiny amounts from AppKit's geometry rounding).
const ANCHOR_TRACKER_EPSILON: f32 = 0.5;

/// Build the absolutely-positioned Taffy style for an anchored portal's
/// content child from the anchor's current viewport rect. Applied once
/// on `insert` so the FIRST paint lands near the anchor; the per-frame
/// tracker then refines `Above`/`Start` (which need the measured popover
/// size) and tracks subsequent movement.
pub(crate) fn child_style_for_anchor(anchor: &AnchorSpec) -> StyleRules {
    // Trigger not measured yet (no window / not mounted) — a safe
    // near-top-left default. Compositions usually only mount the overlay
    // after the trigger is on screen, so this is mostly defensive.
    let rect = match anchor.target.rect() {
        Some(r) if r.width > 0.0 || r.height > 0.0 => r,
        _ => {
            return StyleRules {
                position: Some(Position::Absolute),
                top: Some(Tokenized::Literal(Length::Px(ANCHOR_POPOVER_HEIGHT_FALLBACK))),
                left: Some(Tokenized::Literal(Length::Px(ANCHOR_UNMEASURED_INSET))),
                ..Default::default()
            };
        }
    };

    let mut rules = StyleRules { position: Some(Position::Absolute), ..Default::default() };
    // Initial placement uses the unmeasured-popover-size align —
    // Center/End land slightly off until the tracker's first frame,
    // which runs the same frame and corrects it.
    match anchor.side {
        ElementSide::Below => {
            rules.top = px(rect.y + rect.height + anchor.offset);
            rules.left = px(align_x_unmeasured(&rect, anchor.align));
        }
        ElementSide::Above => {
            // Without the measured height we can't subtract it from
            // `rect.y`; approximate with the fallback. The tracker fixes
            // it on the first frame once Taffy has sized the popover.
            let top = (rect.y - anchor.offset - ANCHOR_POPOVER_HEIGHT_FALLBACK)
                .max(ANCHOR_FALLBACK_MIN_INSET);
            rules.top = px(top);
            rules.left = px(align_x_unmeasured(&rect, anchor.align));
        }
        ElementSide::End => {
            rules.top = px(align_y_unmeasured(&rect, anchor.align));
            rules.left = px(rect.x + rect.width + anchor.offset);
        }
        ElementSide::Start => {
            rules.top = px(align_y_unmeasured(&rect, anchor.align));
            let left = (rect.x - anchor.offset - ANCHOR_POPOVER_WIDTH_FALLBACK)
                .max(ANCHOR_FALLBACK_MIN_INSET);
            rules.left = px(left);
        }
    }
    rules
}

fn px(v: f32) -> Option<Tokenized<Length>> {
    Some(Tokenized::Literal(Length::Px(v)))
}

fn align_x_unmeasured(rect: &ViewportRect, align: ElementAlign) -> f32 {
    match align {
        ElementAlign::Start => rect.x,
        ElementAlign::Center => rect.x + rect.width / 2.0,
        ElementAlign::End => rect.x + rect.width,
    }
}

fn align_y_unmeasured(rect: &ViewportRect, align: ElementAlign) -> f32 {
    match align {
        ElementAlign::Start => rect.y,
        ElementAlign::Center => rect.y + rect.height / 2.0,
        ElementAlign::End => rect.y + rect.height,
    }
}

/// Start a `raf_loop` that re-pins `popover` to the anchor every frame.
/// Returns the loop handle and the shared placement; the caller stores both on
/// the `PortalEntry` so `release_portal` (or a re-track) can drop them, which
/// stops the loop.
///
/// `popover` is the content child's NSView. Its `frame` is in the
/// container's coordinate space — and the container spans the window
/// from its top-left origin, so that space equals window/viewport
/// coordinates, the same space `AnchorTarget::rect()` reports in (the
/// `FlippedView` content view is `isFlipped`, top-left, y-down). No
/// conversion is needed.
pub(crate) fn start_anchor_tracker(
    popover: Retained<NSView>,
    target: AnchorTarget,
    side: ElementSide,
    align: ElementAlign,
    offset: f32,
    collision_insets: runtime_shared::EdgeInsets,
) -> (RafLoop, Rc<RefCell<AnchorPin>>) {
    // Hide until the FIRST measured placement so the content never paints at
    // the unmeasured `child_style_for_anchor` fallback (which lacks the popover
    // size, so Center/Above/End/Start land slightly off) and then visibly jumps.
    // The reveal happens the same frame the trigger + popover are both measured,
    // so it's imperceptible. `revealed` and `frames` persist across frames
    // because the loop closure is `FnMut`.
    //
    // NOTE: anchored portals in this codebase are always SINGLE-child
    // (`anchored_overlay` is always `BackdropMode::None`; a popover's click-away
    // catcher is a SEPARATE viewport portal), so the `RetrackToLatest` path —
    // which drops this loop before it reveals — never strands a hidden sibling.
    // If a multi-child anchored composition is ever added, reveal the superseded
    // child when re-tracking (see `imp::mod::insert`).
    let _: () = unsafe { msg_send![&*popover, setHidden: true] };
    let mut revealed = false;
    let mut frames: u32 = 0;
    // Shared placement (side flip + viewport clamp), held for the tracker's
    // life: it remembers the side it settled on, so when the popover's
    // CONTENT resizes (a menu's header search filtering its rows) the next
    // placement keeps it on the side it is already on — a flipped-above
    // menu that shrinks keeps its bottom on the trigger instead of jumping
    // below it.
    let placer =
        AnchoredPlacer::new(side, align, offset, ANCHOR_EDGE_GAP).with_collision_insets(collision_insets);
    let pin = Rc::new(RefCell::new(AnchorPin { popover, target, placer, placed: false }));
    let shared = Rc::clone(&pin);

    let tracker = runtime_shared::scheduling::raf_loop(move || {
        frames = frames.saturating_add(1);
        let placement = pin.borrow_mut().place();
        if revealed {
            return;
        }
        // Reveal on the first placement. Safety net: if the trigger never
        // resolves (an anchor handle type with no rect impl), reveal anyway
        // after a short grace so the content can't stay invisible forever —
        // it shows at the unadjusted `child_style_for_anchor` position rather
        // than vanishing.
        let reveal = match placement {
            Placement::Placed => true,
            Placement::Unmeasured => frames > ANCHOR_REVEAL_GRACE_FRAMES,
            Placement::NoViewport => false,
        };
        if reveal {
            revealed = true;
            let _: () = unsafe { msg_send![&*pin.borrow().popover, setHidden: false] };
        }
    });
    (tracker, shared)
}

/// The anchored container's bounds size — the viewport the popover is
/// placed and clamped in — or `None` while it has no superview or no size.
fn container_viewport(popover: &NSView) -> Option<(f32, f32)> {
    let container: Option<Retained<NSView>> = unsafe { popover.superview() };
    let bounds: CGRect = unsafe { msg_send![&*container?, bounds] };
    let (w, h) = (bounds.size.width as f32, bounds.size.height as f32);
    (w > 0.0 && h > 0.0).then_some((w, h))
}

/// Frames the tracker waits for a measurable trigger before revealing the
/// content anyway (so a never-resolving anchor can't leave it invisible).
/// ~0.25s at 60fps — long enough to never trip in the normal measured case,
/// short enough to be unnoticeable as a fallback.
const ANCHOR_REVEAL_GRACE_FRAMES: u32 = 15;
