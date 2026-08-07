//! The backend-independent core of [`Position::Sticky`](crate::Position):
//! which axes an element pins on, and the pin arithmetic itself.
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
//! Leading-edge pinning on **both** axes: `top` for vertical, `left`
//! for horizontal. Trailing-edge pinning (`bottom` / `right`) is not
//! implemented on the native backends — it needs the scrollport extent
//! and the element's own extent threaded into the tick, which none of
//! the per-backend registries carry today. `bottom` / `right` continue
//! to be ignored natively and honored on web.

use crate::{Length, StyleRules};

/// Per-axis sticky pin thresholds, in px / points, resolved from an
/// element's [`StyleRules`].
///
/// `None` on an axis means "don't pin on this axis" — the element
/// scrolls normally there. That distinction is what makes a frozen
/// column expressible: `left: Some(0.0), top: None` pins horizontally
/// while scrolling freely with the content vertically.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct StickyInsets {
    /// Vertical pin threshold, from `StyleRules::top`.
    pub top: Option<f32>,
    /// Horizontal pin threshold, from `StyleRules::left`.
    pub left: Option<f32>,
}

impl StickyInsets {
    /// Resolve the thresholds an element declares.
    ///
    /// Percent and `Auto` insets resolve to `0.0` rather than being
    /// dropped: a percentage pin offset would have to resolve against
    /// the scrollport, which isn't known at `apply_style` time, and
    /// silently ignoring the side would make the element pin on no axis
    /// at all — a worse failure than pinning at the edge.
    ///
    /// **The no-side default is vertical.** With neither `top` nor
    /// `left` set, this reports `top: Some(0.0)` so a bare
    /// `position: Sticky` keeps pinning to the leading edge, which is
    /// what [`Position::Sticky`](crate::Position)'s documentation
    /// promises and what every native backend did before horizontal
    /// support existed. (CSS itself treats an inset-less sticky as
    /// `relative`; that web-vs-native divergence predates this module
    /// and is documented on the enum, not silently introduced here.)
    pub fn from_style(style: &StyleRules) -> Self {
        let resolve = |inset: &Option<crate::Tokenized<Length>>| {
            inset.as_ref().map(|t| match t.resolve() {
                Length::Px(v) => v,
                _ => 0.0,
            })
        };
        // Trailing-edge insets reach here only on a native backend
        // (web never calls this — the browser owns pinning), and no
        // native backend implements them. Say so once instead of
        // letting the element quietly not pin: this is precisely the
        // silent-degradation class `unsupported` exists for.
        if style.bottom.is_some() && style.top.is_none() {
            crate::unsupported::warn_once(
                "sticky.bottom",
                "position: Sticky with `bottom` and no `top` — native backends pin only to \
                 leading edges (`top` / `left`), so this element will not pin. It pins \
                 normally on web.",
            );
        }
        if style.right.is_some() && style.left.is_none() {
            crate::unsupported::warn_once(
                "sticky.right",
                "position: Sticky with `right` and no `left` — native backends pin only to \
                 leading edges (`top` / `left`), so this element will not pin horizontally. \
                 It pins normally on web.",
            );
        }

        let top = resolve(&style.top);
        let left = resolve(&style.left);
        match (top, left) {
            (None, None) => Self {
                top: Some(0.0),
                left: None,
            },
            _ => Self { top, left },
        }
    }

    /// True when the element pins on neither axis. Never true for
    /// insets produced by [`from_style`](Self::from_style) (the
    /// vertical default guarantees at least one axis), but a
    /// hand-constructed value can be empty and backends should treat
    /// it as "nothing to register".
    pub fn is_empty(self) -> bool {
        self.top.is_none() && self.left.is_none()
    }
}

/// The pin translation for one axis.
///
/// `natural` is the child's offset in the scroll container's *content*
/// space, `threshold` the pin offset from the scrollport's leading
/// edge, `scroll` the container's current offset — all on the same
/// axis, all in the same units. The result is what the backend adds to
/// the child's natural position so it renders at `scroll + threshold`
/// once the content has scrolled past that point, and `0.0` while it
/// hasn't.
///
/// Returns `0.0` for an axis the element doesn't pin on, which is why
/// [`translate_for`] takes `Option<f32>` and this takes a resolved
/// `f32`.
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

/// [`axis_translate`] for an optional threshold: `None` (the element
/// doesn't pin on this axis) yields no translation, so the child
/// scrolls with the content there.
#[inline]
pub fn translate_for(threshold: Option<f32>, natural: f32, scroll: f32) -> f32 {
    match threshold {
        Some(t) => axis_translate(natural, t, scroll),
        None => 0.0,
    }
}

/// Both axes at once — the shape every backend's per-scroll tick wants.
/// `natural` / `scroll` are `(x, y)`; the result is `(dx, dy)`.
#[inline]
pub fn translate(insets: StickyInsets, natural: (f32, f32), scroll: (f32, f32)) -> (f32, f32) {
    (
        translate_for(insets.left, natural.0, scroll.0),
        translate_for(insets.top, natural.1, scroll.1),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Tokenized;

    fn px(v: f32) -> Option<Tokenized<Length>> {
        Some(Tokenized::Literal(Length::Px(v)))
    }

    /// The bug this whole module exists for: a frozen COLUMN. An
    /// element with `left` set must pin horizontally, which was
    /// impossible on every backend — the pin math only ever read
    /// `top`, so `left: 0` produced no horizontal pin anywhere.
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
        let (dx, dy) = translate(insets, (100.0, 100.0), (400.0, 400.0));
        assert!(
            ((100.0 + dx) - 400.0).abs() < 1e-5,
            "pinned rendered x must equal scroll_x + threshold"
        );
        assert_eq!(dy, 0.0, "vertical must scroll freely when `top` is unset");
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
        let (dx, dy) = translate(insets, (50.0, 60.0), (300.0, 500.0));
        assert!(((50.0 + dx) - 304.0).abs() < 1e-5);
        assert!(((60.0 + dy) - 508.0).abs() < 1e-5);
    }

    /// The pre-existing vertical behavior must survive: a bare
    /// `position: Sticky` with no side keeps pinning to the top edge.
    /// Making the thresholds per-axis `Option`s is exactly the change
    /// that could have silently turned this into "pins on no axis".
    #[test]
    fn regression_bare_sticky_still_pins_vertically() {
        let insets = StickyInsets::from_style(&StyleRules::default());
        assert_eq!(insets.top, Some(0.0));
        assert!(!insets.is_empty());
        let (_, dy) = translate(insets, (100.0, 100.0), (0.0, 500.0));
        assert!(((100.0 + dy) - 500.0).abs() < 1e-5);
    }

    /// Scrolling back above the pin point un-pins on both axes.
    #[test]
    fn regression_sticky_unpins_on_scroll_back() {
        let insets = StickyInsets {
            top: Some(32.0),
            left: Some(16.0),
        };
        let pinned = translate(insets, (100.0, 100.0), (500.0, 500.0));
        assert!(pinned.0 > 0.0 && pinned.1 > 0.0);
        assert_eq!(translate(insets, (100.0, 100.0), (0.0, 0.0)), (0.0, 0.0));
    }

    /// The off-by-one-pixel boundary: at exactly the pin point the
    /// translation is still zero.
    #[test]
    fn regression_sticky_pins_one_pixel_early_at_the_boundary() {
        assert_eq!(axis_translate(100.0, 32.0, 68.0), 0.0);
        assert_eq!(axis_translate(100.0, 32.0, 68.5), 0.5);
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
