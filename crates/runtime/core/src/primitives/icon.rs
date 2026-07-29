//! Icon primitive.
//!
//! Renders vector icon data as an inline SVG on web, `CAShapeLayer` on
//! iOS, `VectorDrawable` on Android. Icon data is `&'static` so only
//! icons actually referenced by application code end up in the binary —
//! the linker (with LTO) drops unreferenced `IconData` constants.
//!
//! ## Stroke animation
//!
//! Icons support stroke-draw animations: the path progressively draws
//! itself from 0% to 100% (or any range). This works natively on all
//! platforms:
//! - Web: `stroke-dasharray` + `stroke-dashoffset` with CSS transition
//! - iOS: `CAShapeLayer.strokeEnd` with `CABasicAnimation`
//! - Android: `ObjectAnimator` on `trimPathEnd`
//!
//! Two modes:
//! - **Reactive stroke progress** — `icon(X).stroke(|| signal.get())`
//!   gives programmatic control over how much of the path is drawn.
//! - **Animate-in on mount** — `icon(X).draw_in(500, Easing::EaseOut)`
//!   plays the draw-on effect when the icon first mounts.
//!
//! Platforms that don't support stroke animation ignore it — the icon
//! still renders fully drawn.

use crate::style::Easing;
use crate::{Bound, Element, Ref, RefFill};
use std::rc::Rc;
use rustc_hash::FxHashMap;

// ---------------------------------------------------------------------------
// IconData — the static, const-constructible icon definition
// ---------------------------------------------------------------------------

// The data/handle/Ops types of this primitive moved to `runtime-shared`
// (the walker-free half); this file keeps the Element/Bound builder
// surface (and its tests). The wildcard re-export preserves every old
// path.
pub use runtime_shared::primitives::icon::*;

/// Construct an `Icon` primitive from icon data.
///
/// ```ignore
/// use icons_lucide::SEARCH;
///
/// // Basic usage
/// icon(SEARCH)
///
/// // With draw-in animation on mount
/// icon(SEARCH).draw_in(500, Easing::EaseOut)
///
/// // With reactive stroke progress (e.g. tied to scroll)
/// icon(SEARCH).stroke(|| scroll_progress.get())
/// ```
#[cfg(feature = "prim-icon")]
pub fn icon(data: IconData) -> Bound<IconHandle> {
    Bound::new(Element::Icon {
        data,
        data_fn: None,
        color: None,
        stroke: None,
        draw_in: None,
        style: None,
        ref_fill: None,
        accessibility: crate::accessibility::AccessibilityProps::default(),
        #[cfg(feature = "robot")]
        test_id: None,
    })
}

impl Bound<IconHandle> {
    /// Set a reactive color for the icon. When `None` (the default),
    /// the icon inherits `currentColor` on web or the nearest text
    /// color on native platforms.
    pub fn color<F: Fn() -> crate::style::Color + 'static>(mut self, f: F) -> Self {
        if let Element::Icon { color, .. } = &mut self.primitive {
            *color = Some(Box::new(f));
        }
        self
    }

    /// Set a reactive icon geometry. When the closure's signals change, the
    /// rendered glyph swaps in place (no node rebuild) — e.g. an icon that
    /// toggles between two glyphs. The icon mounts at the closure's initial
    /// value. Static icons just pass `data` to [`icon`] and skip this.
    pub fn data<F: Fn() -> IconData + 'static>(mut self, f: F) -> Self {
        if let Element::Icon { data, data_fn, .. } = &mut self.primitive {
            *data = f();
            *data_fn = Some(Box::new(f));
        }
        self
    }

    /// Reactive stroke progress (0.0 to 1.0). Controls how much of the
    /// icon's path is visibly drawn. Useful for binding to scroll
    /// position, loading progress, or gesture state.
    ///
    /// When set, the icon mounts at the initial value of the closure
    /// and updates reactively as signals change.
    pub fn stroke<F: Fn() -> f32 + 'static>(mut self, f: F) -> Self {
        if let Element::Icon { stroke, .. } = &mut self.primitive {
            *stroke = Some(Box::new(f));
        }
        self
    }

    /// Configure a stroke animation that plays on mount.
    ///
    /// ```ignore
    /// icon(SEARCH).animate(StrokeAnimation::new(600, Easing::EaseOut))
    /// icon(MENU).animate(StrokeAnimation::new(800, Easing::EaseInOut).looping())
    /// icon(X).animate(StrokeAnimation::new(1000, Easing::Linear).range(0.2, 0.8).reverse())
    /// ```
    ///
    /// For ongoing programmatic control, use `.stroke()` with a
    /// reactive signal, or `.bind()` and call handle methods.
    pub fn animate(mut self, anim: StrokeAnimation) -> Self {
        if let Element::Icon { draw_in, .. } = &mut self.primitive {
            *draw_in = Some(anim);
        }
        self
    }

    /// Shorthand for `.animate(StrokeAnimation::new(duration_ms, easing))`.
    pub fn draw_in(self, duration_ms: u32, easing: Easing) -> Self {
        self.animate(StrokeAnimation::new(duration_ms, easing))
    }

    /// Pin the icon to a `size × size` point square.
    ///
    /// A raw `icon(data)` has no intrinsic content size, so under a flex
    /// parent it collapses to a 0×0 box (invisible, and un-hittable).
    /// Sizing is therefore set through the style system — but the
    /// builder surface should agree with the `ui!` struct-literal form,
    /// where `Icon(data = …, size = …)` already works. `.size(n)` is the
    /// builder equivalent: shorthand for a `width: n, height: n,
    /// flex_shrink: 0` style.
    ///
    /// ```ignore
    /// icon(SEARCH).size(20.0).color(|| theme_color())
    /// ```
    ///
    /// Composes with the rest of the builder. It applies a style, so a
    /// subsequent `.with_style(...)` that also sets width/height would
    /// override the pinned square — set size last, or fold it into the
    /// custom sheet.
    pub fn size(self, size: f32) -> Self {
        self.with_style(icon_size_sheet(size))
    }

    /// Bind to a `Ref<IconHandle>` so the parent can call
    /// `animate_stroke()`, `set_stroke_progress()`, or `replay()`
    /// imperatively.
    pub fn bind(mut self, r: Ref<IconHandle>) -> Self {
        if let Element::Icon { ref_fill, .. } = &mut self.primitive {
            *ref_fill = Some(RefFill::Icon(Box::new(move |h| r.fill(h))));
        }
        self
    }
}

thread_local! {
    /// Cache of the square-sizing sheets minted by [`icon_size_sheet`],
    /// keyed by integer-encoded size (`px * 100`, rounded) so distinct
    /// sizes get distinct cached sheets without float-key hashing. One
    /// `Rc<StyleSheet>` per size keeps stylesheet registration/class
    /// generation deduped across every icon at that size.
    static ICON_SIZE_SHEETS: std::cell::RefCell<
        FxHashMap<u32, Rc<crate::style::StyleSheet>>,
    > = std::cell::RefCell::new(FxHashMap::default());
}

/// A cached static sheet pinning the icon to a `px × px` square. Icons
/// have no intrinsic content size, so an explicit width/height keeps
/// them from collapsing to a 0×0 box under flex. Shared by the
/// primitive's [`Bound::<IconHandle>::size`] builder and the `idea-ui`
/// `Icon` component so both mint identical, deduped sheets.
pub(crate) fn icon_size_sheet(px: f32) -> Rc<crate::style::StyleSheet> {
    use crate::style::{Length, StyleRules, StyleSheet, Tokenized};
    let key = (px * 100.0).round() as u32;
    ICON_SIZE_SHEETS.with(|m| {
        if let Some(s) = m.borrow().get(&key) {
            return s.clone();
        }
        let sheet = Rc::new(StyleSheet::r#static(StyleRules {
            width: Some(Tokenized::Literal(Length::Px(px))),
            height: Some(Tokenized::Literal(Length::Px(px))),
            flex_shrink: Some(Tokenized::Literal(0.0)),
            ..Default::default()
        }));
        m.borrow_mut().insert(key, sheet.clone());
        sheet
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::{resolve as resolve_style, Length, Tokenized};
    use crate::sources::StyleSource;
    use crate::FillRule;

    const DOT: crate::IconData = crate::IconData {
        view_box: (24, 24),
        paths: &["M12 12h.01"],
        fill_rule: FillRule::NonZero,
        filled: true,
    };

    /// `.size()` is the builder peer of the `ui!` `Icon(size = …)` prop:
    /// it pins a `size × size` square so the icon doesn't collapse to a
    /// 0×0 box under flex. Regression test for the "raw `icon()` has no
    /// `.size()`" papercut (Whiteboard Pro feedback).
    #[test]
    fn size_pins_a_square_style() {
        let el = icon(DOT).size(18.0).primitive;
        let style = match el {
            Element::Icon { style, .. } => style.expect(".size() must attach a style"),
            _ => panic!("icon() builds an Icon element"),
        };
        let app = match style {
            StyleSource::Static(a) => a,
            _ => panic!(".size() uses a cached static sheet"),
        };
        let rules = resolve_style(&app);
        assert_eq!(rules.width, Some(Tokenized::Literal(Length::Px(18.0))));
        assert_eq!(rules.height, Some(Tokenized::Literal(Length::Px(18.0))));
        assert_eq!(rules.flex_shrink, Some(Tokenized::Literal(0.0)));
    }

    /// The same size mints the same `Rc<StyleSheet>` (registration dedup).
    #[test]
    fn same_size_shares_one_sheet() {
        let a = icon_size_sheet(24.0);
        let b = icon_size_sheet(24.0);
        assert!(Rc::ptr_eq(&a, &b), "equal sizes must share one cached sheet");
    }
}
