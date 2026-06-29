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
//! through scrolls / reactive relayouts / window resizes. This mirrors
//! the iOS backend's `CADisplayLink` tracker (`backend-ios-mobile`);
//! `raf_loop` is macOS's frame driver (it already drives the color
//! transitions in `imp::transitions`).
//!
//! Before this existed, the macOS backend dropped the anchor metadata
//! entirely and laid the content out at the container's top-left — the
//! "tooltip / popover renders in the corner" bug.

use objc2::msg_send;
use objc2::rc::Retained;
use objc2_app_kit::NSView;
use objc2_foundation::{CGPoint, CGRect};

use runtime_core::primitives::portal::{
    AnchorTarget, ElementAlign, ElementSide, ViewportRect,
};
use runtime_core::scheduling::RafLoop;
use runtime_core::{
    AlignItems, FlexDirection, JustifyContent, Length, Position, StyleRules, Tokenized,
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
}

#[derive(Clone)]
pub(crate) struct AnchorSpec {
    pub(crate) target: AnchorTarget,
    pub(crate) side: ElementSide,
    pub(crate) align: ElementAlign,
    pub(crate) offset: f32,
}

pub(crate) type PortalInstances = std::collections::HashMap<usize, PortalEntry>;

/// `StyleRules` for an anchored-portal CONTAINER. Neutral flex — the
/// content child positions itself via `position: absolute` + insets, so
/// the container just needs to span the viewport (where the absolute
/// coordinates live) without stretching or centering the child. The
/// viewport-placement path keeps using `portal_container_style` in
/// `imp::mod`; this is only for the anchor case.
pub(crate) fn container_style_for_anchor() -> StyleRules {
    StyleRules {
        flex_direction: Some(FlexDirection::Column),
        justify_content: Some(JustifyContent::FlexStart),
        align_items: Some(AlignItems::FlexStart),
        ..Default::default()
    }
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
/// Returns the loop handle; the caller stores it on the `PortalEntry` so
/// `release_portal` (or a re-track) can drop it, which stops the loop.
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
) -> RafLoop {
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

    runtime_core::scheduling::raf_loop(move || {
        frames = frames.saturating_add(1);
        let reveal = |p: &NSView| {
            let _: () = unsafe { msg_send![p, setHidden: false] };
        };

        let trigger = target.rect().filter(|t| t.width > 0.0 || t.height > 0.0);
        let pop_frame: CGRect = unsafe { msg_send![&*popover, frame] };
        let pop_w = pop_frame.size.width as f32;
        let pop_h = pop_frame.size.height as f32;

        let Some(trigger) = trigger else {
            // Trigger not measurable yet. Safety net: if it never resolves
            // (an anchor handle type with no rect impl), reveal anyway after a
            // short grace so the content can't stay invisible forever — it
            // shows at the `child_style_for_anchor` position, the pre-fix
            // behavior, rather than vanishing.
            if !revealed && frames > ANCHOR_REVEAL_GRACE_FRAMES {
                revealed = true;
                reveal(&popover);
            }
            return;
        };
        // Taffy hasn't sized the popover yet — `Above` / `Start` origin
        // math depends on the popover size, so defer to a later frame.
        if pop_w <= 0.0 || pop_h <= 0.0 {
            if !revealed && frames > ANCHOR_REVEAL_GRACE_FRAMES {
                revealed = true;
                reveal(&popover);
            }
            return;
        }

        // Shared measured align/side geometry (runtime_core) — one
        // definition across web / iOS / Android / macOS (CLAUDE.md §7).
        // The tracker re-pins to the requested side without flip/clamp,
        // matching the iOS tracker; web layers flip+clamp on top.
        let (top, left) = runtime_core::primitives::portal::anchor_top_left(
            trigger, side, align, offset, (pop_w, pop_h),
        );

        // Compare against the LIVE frame rather than a stored "last written"
        // value: the layout pass writes `setFrame:` on every registered view
        // between frames, so reading the current origin is what makes the
        // tracker self-correct against it.
        let cur_left = pop_frame.origin.x as f32;
        let cur_top = pop_frame.origin.y as f32;
        let in_place = (cur_top - top).abs() < ANCHOR_TRACKER_EPSILON
            && (cur_left - left).abs() < ANCHOR_TRACKER_EPSILON;
        if !in_place {
            let new_frame = CGRect {
                origin: CGPoint { x: left as f64, y: top as f64 },
                size: pop_frame.size,
            };
            let _: () = unsafe { msg_send![&*popover, setFrame: new_frame] };
        }
        // First frame where trigger + size are known and the position is
        // applied: reveal. (Reveal even when `in_place`, e.g. an exact
        // `Below`/`Start` placement that needs no correction, so the content
        // doesn't stay hidden.)
        if !revealed {
            revealed = true;
            reveal(&popover);
        }
    })
}

/// Frames the tracker waits for a measurable trigger before revealing the
/// content anyway (so a never-resolving anchor can't leave it invisible).
/// ~0.25s at 60fps — long enough to never trip in the normal measured case,
/// short enough to be unnoticeable as a fallback.
const ANCHOR_REVEAL_GRACE_FRAMES: u32 = 15;
