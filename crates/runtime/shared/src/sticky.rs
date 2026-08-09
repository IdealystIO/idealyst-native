//! The backend-independent core of [`Position::Sticky`](crate::Position):
//! which edges an element pins on, and the pin arithmetic itself.
//!
//! # Why this lives here
//!
//! Every backend that implements sticky does the same two things —
//! read the pin thresholds off `StyleRules`, then translate the child
//! by the scroll overshoot — and until this module existed each one
//! carried its own copy of both, complete with its own duplicated
//! tests. Four copies of one formula is exactly the shape CLAUDE.md §7
//! warns about: the backends are supposed to diverge in *mechanism*
//! (UIKit `CGAffineTransform`, AppKit `setFrameOrigin:`, Android
//! `setTranslationX/Y`, CSS `position: sticky`) and converge in
//! *behavior*. Behavior converges only if there is one implementation
//! of the behavior.
//!
//! Web is deliberately not a consumer: the browser owns pinning
//! natively once `position: sticky` plus the insets are emitted (see
//! `css::rules_to_css`), so there is nothing for the framework to
//! compute. That is the reference the native backends are matched
//! against.
//!
//! # Axis coverage
//!
//! All four edges. Leading (`top` / `left`) pins the element at the
//! scrollport's leading edge once the content scrolls past it; trailing
//! (`bottom` / `right`) pins the element's far edge a threshold inside
//! the scrollport's trailing edge while the content hasn't yet scrolled
//! it into natural view — the CSS behavior for a pinned footer row or a
//! right-frozen table column. Trailing pins need two extents the
//! leading formula does not: the scrollport's size and the element's
//! own size, which is why [`translate`] takes both.
//!
//! When both edges of one axis are declared and the scrollport is too
//! small to satisfy both, the leading edge wins — matching the CSS
//! clamp order for left-to-right content.
//!
//! # Paint order
//!
//! CSS paints positioned elements above static siblings, so a pinned
//! element (frozen table column, pinned header) draws over the content
//! that slides beneath it. The native backends reproduce this by
//! raising a view at sticky-registration time — `layer.zPosition` on
//! iOS/macOS, `setTranslationZ` on Android, an implicit sibling-sort z
//! in the wgpu walker — and restoring it on deregister. Web needs
//! nothing (the browser owns both the pin and the paint order). This
//! raise changes paint order only, not hit-test order; the overlap
//! region is content the pin visually covers, which is the acceptable
//! divergence documented in each backend's sticky module.

use crate::{Length, StyleRules};

/// Per-edge sticky pin thresholds, in px / points, resolved from an
/// element's [`StyleRules`].
///
/// `None` on an edge means "don't pin on this edge" — the element
/// scrolls normally there. That distinction is what makes a frozen
/// column expressible: `left: Some(0.0)` with everything else `None`
/// pins horizontally while scrolling freely with the content
/// vertically, and `right: Some(0.0)` freezes a column at the
/// scrollport's far edge the same way.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct StickyInsets {
    /// Vertical leading pin threshold, from `StyleRules::top`.
    pub top: Option<f32>,
    /// Horizontal leading pin threshold, from `StyleRules::left`.
    pub left: Option<f32>,
    /// Vertical trailing pin threshold, from `StyleRules::bottom`.
    pub bottom: Option<f32>,
    /// Horizontal trailing pin threshold, from `StyleRules::right`.
    pub right: Option<f32>,
}

impl StickyInsets {
    /// Resolve the thresholds an element declares.
    ///
    /// Percent and `Auto` insets resolve to `0.0` rather than being
    /// dropped: a percentage pin offset would have to resolve against
    /// the scrollport, which isn't known at `apply_style` time, and
    /// silently ignoring the side would make the element pin on no edge
    /// at all — a worse failure than pinning at the edge.
    ///
    /// **The no-side default is vertical.** With no inset set at all,
    /// this reports `top: Some(0.0)` so a bare `position: Sticky` keeps
    /// pinning to the leading edge, which is what
    /// [`Position::Sticky`](crate::Position)'s documentation promises
    /// and what every native backend did before per-edge support
    /// existed. (CSS itself treats an inset-less sticky as `relative`;
    /// that web-vs-native divergence predates this module and is
    /// documented on the enum, not silently introduced here.)
    pub fn from_style(style: &StyleRules) -> Self {
        let resolve = |inset: &Option<crate::Tokenized<Length>>| {
            inset.as_ref().map(|t| match t.resolve() {
                Length::Px(v) => v,
                _ => 0.0,
            })
        };

        let top = resolve(&style.top);
        let left = resolve(&style.left);
        let bottom = resolve(&style.bottom);
        let right = resolve(&style.right);
        match (top, left, bottom, right) {
            (None, None, None, None) => Self {
                top: Some(0.0),
                left: None,
                bottom: None,
                right: None,
            },
            _ => Self { top, left, bottom, right },
        }
    }

    /// True when the element pins on no edge. Never true for insets
    /// produced by [`from_style`](Self::from_style) (the vertical
    /// default guarantees at least one edge), but a hand-constructed
    /// value can be empty and backends should treat it as "nothing to
    /// register".
    pub fn is_empty(self) -> bool {
        self.top.is_none()
            && self.left.is_none()
            && self.bottom.is_none()
            && self.right.is_none()
    }
}

/// The leading-edge pin translation for one axis.
///
/// `natural` is the child's offset in the scroll container's *content*
/// space, `threshold` the pin offset from the scrollport's leading
/// edge, `scroll` the container's current offset — all on the same
/// axis, all in the same units. The result is what the backend adds to
/// the child's natural position so it renders at `scroll + threshold`
/// once the content has scrolled past that point, and `0.0` while it
/// hasn't.
#[inline]
pub fn axis_translate(natural: f32, threshold: f32, scroll: f32) -> f32 {
    let pinned = scroll + threshold;
    // Strictly greater: at exactly `pinned == natural` the child is
    // already sitting on the pin line and needs no translation.
    // Using `>=` here made the child snap one device pixel early —
    // the boundary case the backends' regression tests pin down.
    if pinned > natural {
        pinned - natural
    } else {
        0.0
    }
}

/// The trailing-edge pin translation for one axis — the mirror of
/// [`axis_translate`] for `bottom` / `right`.
///
/// The child's far edge (`natural + extent`) must stay at least
/// `threshold` inside the scrollport's trailing edge
/// (`scroll + viewport`). While the layout would place it further out
/// than that, the child is pulled *back* (a negative translation) so it
/// renders parked at the trailing pin line; once the content scrolls
/// far enough that its natural position satisfies the constraint, the
/// translation is `0.0` and it rides the content — CSS `bottom:` /
/// `right:` sticky.
#[inline]
pub fn axis_translate_trailing(
    natural: f32,
    extent: f32,
    threshold: f32,
    scroll: f32,
    viewport: f32,
) -> f32 {
    let pinned = scroll + viewport - threshold - extent;
    // Strictly less, mirroring the leading boundary: at exactly
    // `pinned == natural` the child already sits on the pin line.
    if pinned < natural {
        pinned - natural
    } else {
        0.0
    }
}

/// The combined pin translation for one axis with optional leading and
/// trailing thresholds. `None` on a threshold means the element doesn't
/// pin on that edge.
///
/// When both edges are declared and the scrollport is too small to
/// satisfy both constraints at once, the leading edge wins — the CSS
/// clamp order for left-to-right content.
#[inline]
pub fn axis_pin(
    leading: Option<f32>,
    trailing: Option<f32>,
    natural: f32,
    extent: f32,
    scroll: f32,
    viewport: f32,
) -> f32 {
    if let Some(t) = leading {
        let dx = axis_translate(natural, t, scroll);
        if dx != 0.0 {
            return dx;
        }
    }
    if let Some(t) = trailing {
        return axis_translate_trailing(natural, extent, t, scroll, viewport);
    }
    0.0
}

/// Both axes at once — the shape every backend's per-scroll tick wants.
///
/// - `natural`: the child's `(x, y)` origin in the scroll container's
///   content space (Taffy frames summed up to the scroll node — never
///   the live view's frame, which the pin itself may have moved).
/// - `size`: the child's `(width, height)` — needed only by trailing
///   pins; pass the Taffy frame extent.
/// - `scroll`: the container's current `(x, y)` offset.
/// - `viewport`: the scrollport's `(width, height)` — the visible
///   extent, not the content extent; needed only by trailing pins.
///
/// The result is the `(dx, dy)` the backend adds to the child's
/// natural position.
#[inline]
pub fn translate(
    insets: StickyInsets,
    natural: (f32, f32),
    size: (f32, f32),
    scroll: (f32, f32),
    viewport: (f32, f32),
) -> (f32, f32) {
    (
        axis_pin(insets.left, insets.right, natural.0, size.0, scroll.0, viewport.0),
        axis_pin(insets.top, insets.bottom, natural.1, size.1, scroll.1, viewport.1),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Tokenized;

    fn px(v: f32) -> Option<Tokenized<Length>> {
        Some(Tokenized::Literal(Length::Px(v)))
    }

    /// A left-frozen table column: an element with `left` set must pin
    /// horizontally while scrolling freely vertically. This was
    /// impossible on every backend before the per-axis thresholds — the
    /// pin math only ever read `top`.
    #[test]
    fn regression_sticky_left_never_pins_horizontally() {
        let insets = StickyInsets::from_style(&StyleRules {
            left: px(0.0),
            ..Default::default()
        });
        assert_eq!(insets.left, Some(0.0));
        assert_eq!(insets.top, None, "a left-only sticky must not also pin vertically");

        // Natural x of 100, scrolled 400 right: the child renders
        // pinned at the scrollport's left edge.
        let (dx, dy) = translate(insets, (100.0, 100.0), (80.0, 40.0), (400.0, 400.0), (600.0, 800.0));
        assert!(
            ((100.0 + dx) - 400.0).abs() < 1e-5,
            "pinned rendered x must equal scroll_x + threshold"
        );
        assert_eq!(dy, 0.0, "vertical must scroll freely when `top` is unset");
    }

    /// A RIGHT-frozen table column — the trailing-edge mirror. The
    /// element's far edge parks `threshold` inside the scrollport's
    /// right edge while its natural position lies beyond it, and
    /// releases once the content scrolls it into natural view. This was
    /// the "trailing edges are web-only" gap: native backends ignored
    /// `right` entirely.
    #[test]
    fn regression_sticky_right_never_pinned_natively() {
        let insets = StickyInsets::from_style(&StyleRules {
            right: px(0.0),
            ..Default::default()
        });
        assert_eq!(insets.right, Some(0.0));
        assert_eq!(insets.top, None, "a right-only sticky must not also pin vertically");

        // Column at x=900, 100 wide, in a 600-wide scrollport at
        // scroll 0: natural far edge (1000) is 400 past the visible
        // right edge (600), so the pin pulls it back to park at
        // x = 600 - 100 = 500.
        let (dx, dy) = translate(insets, (900.0, 0.0), (100.0, 40.0), (0.0, 0.0), (600.0, 800.0));
        assert!(
            ((900.0 + dx) - 500.0).abs() < 1e-5,
            "pinned rendered far edge must sit at the scrollport's right edge"
        );
        assert_eq!(dy, 0.0);

        // Scrolled far enough right (scroll_x = 400): the natural
        // position is exactly at the pin line — no translation, it
        // rides the content from here.
        let (dx, _) = translate(insets, (900.0, 0.0), (100.0, 40.0), (400.0, 0.0), (600.0, 800.0));
        assert_eq!(dx, 0.0, "once naturally visible the column must ride the content");
    }

    /// A pinned footer row (`bottom: 0`): parks at the scrollport's
    /// bottom edge while the content hasn't scrolled it into natural
    /// view, releases when it has.
    #[test]
    fn regression_sticky_bottom_never_pinned_natively() {
        let insets = StickyInsets::from_style(&StyleRules {
            bottom: px(8.0),
            ..Default::default()
        });
        assert_eq!(insets.bottom, Some(8.0));
        assert_eq!(insets.top, None, "a bottom-only sticky must not gain the vertical default");

        // Row at y=1200, 50 tall, 800-tall scrollport, scroll 0: far
        // edge (1250) is far past the visible bottom (800); parks at
        // y = 800 - 8 - 50 = 742.
        let (dx, dy) = translate(insets, (0.0, 1200.0), (200.0, 50.0), (0.0, 0.0), (600.0, 800.0));
        assert_eq!(dx, 0.0);
        assert!(((1200.0 + dy) - 742.0).abs() < 1e-5);

        // Scrolled past it (scroll_y = 500): 1200 < 500 + 800 - 8 - 50
        // = 1242 — naturally visible, no pin.
        let (_, dy) = translate(insets, (0.0, 1200.0), (200.0, 50.0), (0.0, 500.0), (600.0, 800.0));
        assert_eq!(dy, 0.0);
    }

    /// A frozen header and a frozen column in the same grid: both axes
    /// pin independently off one element's insets.
    #[test]
    fn both_axes_pin_independently() {
        let insets = StickyInsets::from_style(&StyleRules {
            top: px(8.0),
            left: px(4.0),
            ..Default::default()
        });
        let (dx, dy) = translate(insets, (50.0, 60.0), (100.0, 40.0), (300.0, 500.0), (600.0, 800.0));
        assert!(((50.0 + dx) - 304.0).abs() < 1e-5);
        assert!(((60.0 + dy) - 508.0).abs() < 1e-5);
    }

    /// Both edges on one axis: the leading edge wins when the
    /// scrollport can't satisfy both — the CSS clamp order. (A 100-wide
    /// element with `left: 0; right: 0` in a 60-wide scrollport: the
    /// leading pin holds, the trailing constraint is unsatisfiable.)
    #[test]
    fn leading_edge_wins_when_both_edges_conflict() {
        let insets = StickyInsets {
            left: Some(0.0),
            right: Some(0.0),
            top: None,
            bottom: None,
        };
        // Scrolled past the element: leading pin engages.
        let (dx, _) = translate(insets, (100.0, 0.0), (100.0, 40.0), (400.0, 0.0), (60.0, 800.0));
        assert!(((100.0 + dx) - 400.0).abs() < 1e-5, "leading pin must win the conflict");
    }

    /// The pre-existing vertical behavior must survive: a bare
    /// `position: Sticky` with no side keeps pinning to the top edge.
    /// Making the thresholds per-edge `Option`s is exactly the change
    /// that could have silently turned this into "pins on no edge".
    #[test]
    fn regression_bare_sticky_still_pins_vertically() {
        let insets = StickyInsets::from_style(&StyleRules::default());
        assert_eq!(insets.top, Some(0.0));
        assert!(!insets.is_empty());
        let (_, dy) = translate(insets, (100.0, 100.0), (80.0, 40.0), (0.0, 500.0), (600.0, 800.0));
        assert!(((100.0 + dy) - 500.0).abs() < 1e-5);
    }

    /// Scrolling back above the pin point un-pins on both axes.
    #[test]
    fn regression_sticky_unpins_on_scroll_back() {
        let insets = StickyInsets {
            top: Some(32.0),
            left: Some(16.0),
            bottom: None,
            right: None,
        };
        let pinned = translate(insets, (100.0, 100.0), (80.0, 40.0), (500.0, 500.0), (600.0, 800.0));
        assert!(pinned.0 > 0.0 && pinned.1 > 0.0);
        assert_eq!(
            translate(insets, (100.0, 100.0), (80.0, 40.0), (0.0, 0.0), (600.0, 800.0)),
            (0.0, 0.0)
        );
    }

    /// The off-by-one-pixel boundary: at exactly the pin point the
    /// translation is still zero — on BOTH edge kinds. The trailing
    /// mirror uses strictly-less for the same reason leading uses
    /// strictly-greater.
    #[test]
    fn regression_sticky_pins_one_pixel_early_at_the_boundary() {
        assert_eq!(axis_translate(100.0, 32.0, 68.0), 0.0);
        assert_eq!(axis_translate(100.0, 32.0, 68.5), 0.5);
        // Trailing: pinned = scroll + viewport - threshold - extent =
        // 0 + 600 - 0 - 100 = 500. natural == pinned → no translate;
        // natural half a pixel past → -0.5.
        assert_eq!(axis_translate_trailing(500.0, 100.0, 0.0, 0.0, 600.0), 0.0);
        assert_eq!(axis_translate_trailing(500.5, 100.0, 0.0, 0.0, 600.0), -0.5);
    }

    /// Percent / Auto insets degrade to a zero threshold rather than
    /// dropping the axis — an element with `left: 50%` still freezes,
    /// it just freezes at the edge.
    #[test]
    fn percent_inset_degrades_to_edge_not_to_unpinned() {
        let insets = StickyInsets::from_style(&StyleRules {
            left: Some(Tokenized::Literal(Length::Percent(50.0))),
            ..Default::default()
        });
        assert_eq!(insets.left, Some(0.0));
    }
}
