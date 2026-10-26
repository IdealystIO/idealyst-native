//! Which Android view backs a `scroll_view` of each [`ScrollAxis`], and
//! how its inner content container is laid out. Un-gated like
//! `sticky_compute` so the regression tests run on the host; the JNI half
//! lives in `imp/primitives/scroll_view.rs`.
//!
//! The scroller is an outer view holding one inner `FrameLayout` that
//! receives every child (see `imp/primitives/scroll_view.rs`). The outer
//! is `ScrollView` for [`ScrollAxis::Vertical`], `HorizontalScrollView`
//! for [`ScrollAxis::Horizontal`], and the runtime's
//! `RustTwoAxisScrollView` for [`ScrollAxis::Both`] — a stock
//! `ScrollView` nested in a `HorizontalScrollView` cannot stand in for it:
//! each consumes the drag on its own axis so a diagonal pan moves only
//! one of them, and a sticky child finds only the inner scroller, so a
//! `top` + `left` corner cell can pin on one axis at most.

use runtime_shared::{
    AlignItems, AlignSelf, FlexDirection, FlexWrap, JustifyContent, Length, ScrollAxis, StyleRules,
    Tokenized,
};

/// JNI class name of the outer scroller for `axis`.
pub(crate) fn scroller_class(axis: ScrollAxis) -> &'static str {
    match axis {
        ScrollAxis::Vertical => "android/widget/ScrollView",
        ScrollAxis::Horizontal => "android/widget/HorizontalScrollView",
        ScrollAxis::Both => TWO_AXIS_SCROLLER_CLASS,
    }
}

/// The runtime's two-axis scroller (`RustTwoAxisScrollView.kt`).
pub(crate) const TWO_AXIS_SCROLLER_CLASS: &str = "io/idealyst/runtime/RustTwoAxisScrollView";

/// Every class [`scroller_class`] can return. Code that has to recognise
/// "this view is a framework scroll view" — the sticky ancestor walk, the
/// unmount sweep that drops a scroller's sticky + `on_scroll` bookkeeping —
/// checks against this list, so a scroller shape cannot be created without
/// also being recognised.
pub(crate) const SCROLL_CONTAINER_CLASSES: [&str; 3] = [
    "android/widget/ScrollView",
    "android/widget/HorizontalScrollView",
    TWO_AXIS_SCROLLER_CLASS,
];

/// Layout rules for the inner content container of an `axis` scroller.
///
/// - Every axis floors the inner at the viewport on each axis it scrolls
///   (`min_height` / `min_width: 100%`), as web scroll content fills its
///   scroller: a `flex_grow` child then has the viewport to grow into, so
///   content that fits can still centre itself in it (`margin: auto`, a
///   centring parent). Without the floor the inner is exactly its
///   content's size and nothing in it can grow. `flex_shrink: 0` keeps
///   the floor a floor: taller content still overflows.
/// - Vertical / Both stack children downward (`Column`): the inner's
///   height is the sum of its children, so it overflows the viewport and
///   the scroller has somewhere to go. With Taffy's default `Row` the
///   inner's height collapses to one row.
/// - Horizontal lays children out left to right (`Row`).
/// - Both additionally sizes the inner to its content WIDTH rather than
///   stretching it to the viewport (`align_self: FlexStart`), with the
///   viewport as the floor (`min_width: 100%`). The inner's children
///   then stretch to the widest of them — a wide table's rows all span
///   the full table, the way a block's children span its scrollable
///   overflow on web — instead of each stopping at the viewport edge and
///   clipping its cells (an Android `ViewGroup` clips its children to its
///   own bounds). The inner is a direct child of the X-scrolling outer,
///   so `runtime_layout` already exempts it from the `max-width: 100%`
///   clamp.
pub(crate) fn inner_content_rules(axis: ScrollAxis) -> StyleRules {
    let mut rules = StyleRules::default();
    rules.flex_direction = Some(match axis {
        ScrollAxis::Horizontal => FlexDirection::Row,
        ScrollAxis::Vertical | ScrollAxis::Both => FlexDirection::Column,
    });
    rules.align_items = Some(AlignItems::Stretch);
    // Never shrink below the content: an explicit `min_*` replaces the
    // automatic minimum (the content size), which is what keeps the inner
    // from shrinking to the viewport, and content clipped to the viewport
    // has nothing left to scroll.
    rules.flex_shrink = Some(Tokenized::Literal(0.0));
    let full = || Some(Tokenized::Literal(Length::Percent(100.0)));
    match axis {
        // The inner spans the scroller across its axis whatever the
        // author's `align_items` on the scroll view: that alignment is for
        // the content (`inner_forwarded_rules`), and an inner it centred
        // would hug its content, collapsing any `width: 100%` child.
        ScrollAxis::Vertical => {
            rules.align_self = Some(AlignSelf::Stretch);
            rules.min_height = full();
        }
        ScrollAxis::Horizontal => {
            rules.align_self = Some(AlignSelf::Stretch);
            rules.min_width = full();
        }
        ScrollAxis::Both => {
            rules.align_self = Some(AlignSelf::FlexStart);
            rules.min_width = full();
            rules.min_height = full();
        }
    }
    rules
}

/// The scroll view's own rules that arrange its CONTENT, moved onto the
/// inner — the node that holds the children. On web a scroll view's
/// `align_items` / `justify_content` / `gap` / `flex_wrap` lay out its
/// children; on Android the author's style lands on the outer, whose only
/// child is the inner, so without this they arranged the inner instead
/// (a centred column collapsed to its narrowest — the kiosk's form
/// picker). Each unset rule falls back to the inner's own default, so a
/// rule removed on a reactive re-apply does not linger (`set_style`
/// merges).
pub(crate) fn inner_forwarded_rules(author: &StyleRules) -> StyleRules {
    let mut rules = StyleRules::default();
    rules.align_items = Some(author.align_items.unwrap_or(AlignItems::Stretch));
    rules.justify_content = Some(author.justify_content.unwrap_or(JustifyContent::FlexStart));
    rules.flex_wrap = Some(author.flex_wrap.unwrap_or(FlexWrap::NoWrap));
    rules.gap = Some(author.gap.clone().unwrap_or(Tokenized::Literal(Length::Px(0.0))));
    rules.row_gap = author.row_gap.clone();
    rules.column_gap = author.column_gap.clone();
    rules
}

// The gesture, fling, clamp and edge-effect half lives in
// `RustTwoAxisScrollView.kt` and needs a live `View` tree, which host
// `cargo test` cannot drive (the same blocker as `imp::sticky`'s
// on-device behaviour). What is host-testable is the layout the native
// scroller is handed and the class set the sticky walk recognises.
#[cfg(test)]
mod tests {
    use super::*;
    use runtime_layout::LayoutTree;

    fn px(v: f32) -> Option<Tokenized<Length>> {
        Some(Tokenized::Literal(Length::Px(v)))
    }

    /// The Android scroll tree: `panel (393×300) → outer (scroll, axis) →
    /// inner (inner_content_rules) → rows`, each row a `Row` of four
    /// 200×100 cells with no width of its own — a 800-wide, `rows×100`-tall
    /// table. Returns (outer, inner, first row) frames.
    fn layout_table(
        axis: ScrollAxis,
        rows: usize,
    ) -> (runtime_layout::Frame, runtime_layout::Frame, runtime_layout::Frame) {
        let mut t = LayoutTree::new();
        let panel = t.new_node();
        let mut pr = StyleRules::default();
        pr.width = px(393.0);
        pr.height = px(300.0);
        t.set_style(panel, &pr);

        let outer = t.new_node();
        t.add_child(panel, outer);
        let inner = t.new_node();
        t.add_child(outer, inner);
        t.set_overflow_scroll_axis(outer, axis);
        t.set_style(inner, &inner_content_rules(axis));

        let mut first_row = None;
        for _ in 0..rows {
            let row = t.new_node();
            let mut rr = StyleRules::default();
            rr.flex_direction = Some(FlexDirection::Row);
            t.set_style(row, &rr);
            for _ in 0..4 {
                let cell = t.new_node();
                let mut cr = StyleRules::default();
                cr.width = px(200.0);
                cr.height = px(100.0);
                cr.flex_shrink = Some(Tokenized::Literal(0.0));
                t.set_style(cell, &cr);
                t.add_child(row, cell);
            }
            t.add_child(inner, row);
            first_row.get_or_insert(row);
        }
        t.compute(panel, 393.0, 852.0);
        (t.frame_of(outer), t.frame_of(inner), t.frame_of(first_row.unwrap()))
    }

    /// A two-axis scroller's content keeps its natural size on BOTH axes
    /// inside a viewport bounded on both: the inner spans the 800-wide
    /// table and every row spans it too, so no cell is clipped by a row
    /// that stopped at the 393 viewport edge.
    #[test]
    fn two_axis_scroll_content_spans_the_table_width_not_the_viewport() {
        let (outer, inner, row) = layout_table(ScrollAxis::Both, 10);
        assert!((outer.width - 393.0).abs() < 0.5, "viewport width bounded, got {}", outer.width);
        assert!((outer.height - 300.0).abs() < 0.5, "viewport height bounded, got {}", outer.height);
        assert!((inner.width - 800.0).abs() < 0.5, "inner spans the table width, got {}", inner.width);
        assert!((inner.height - 1000.0).abs() < 0.5, "inner spans the table height, got {}", inner.height);
        assert!((row.width - 800.0).abs() < 0.5, "a row spans the table width, got {}", row.width);
    }

    /// Content narrower than the viewport still fills it on a two-axis
    /// scroller, as it does on a vertical one: `min_width: 100%` floors the
    /// content-sized inner at the viewport width.
    #[test]
    fn two_axis_scroll_content_narrower_than_the_viewport_fills_it() {
        let mut t = LayoutTree::new();
        let panel = t.new_node();
        let mut pr = StyleRules::default();
        pr.width = px(393.0);
        pr.height = px(300.0);
        t.set_style(panel, &pr);
        let outer = t.new_node();
        t.add_child(panel, outer);
        let inner = t.new_node();
        t.add_child(outer, inner);
        t.set_overflow_scroll_axis(outer, ScrollAxis::Both);
        t.set_style(inner, &inner_content_rules(ScrollAxis::Both));
        let row = t.new_node();
        let mut rr = StyleRules::default();
        rr.height = px(40.0);
        t.set_style(row, &rr);
        t.add_child(inner, row);
        t.compute(panel, 393.0, 852.0);
        assert!((t.frame_of(inner).width - 393.0).abs() < 0.5, "got {}", t.frame_of(inner).width);
        assert!((t.frame_of(row).width - 393.0).abs() < 0.5, "got {}", t.frame_of(row).width);
    }

    /// A vertical scroller keeps clipping its content to the viewport
    /// width — only `Both` sizes the inner to its content horizontally.
    #[test]
    fn vertical_scroll_content_stays_viewport_wide() {
        let (outer, inner, _) = layout_table(ScrollAxis::Vertical, 10);
        assert!((outer.height - 300.0).abs() < 0.5, "got {}", outer.height);
        assert!((inner.width - 393.0).abs() < 0.5, "got {}", inner.width);
        assert!((inner.height - 1000.0).abs() < 0.5, "got {}", inner.height);
    }

    /// Content shorter than a vertical viewport still fills it, so a
    /// `flex_grow` child can grow into the viewport and centre what it
    /// holds — the sign-in form's `margin: auto` column sat at the top of
    /// a tablet with the inner only as tall as the form.
    #[test]
    fn vertical_scroll_content_shorter_than_the_viewport_fills_it() {
        let mut t = LayoutTree::new();
        let panel = t.new_node();
        let mut pr = StyleRules::default();
        pr.width = px(393.0);
        pr.height = px(300.0);
        t.set_style(panel, &pr);
        let outer = t.new_node();
        t.add_child(panel, outer);
        let inner = t.new_node();
        t.add_child(outer, inner);
        t.set_overflow_scroll_axis(outer, ScrollAxis::Vertical);
        t.set_style(inner, &inner_content_rules(ScrollAxis::Vertical));
        let grow = t.new_node();
        let mut gr = StyleRules::default();
        gr.flex_grow = Some(Tokenized::Literal(1.0));
        t.set_style(grow, &gr);
        t.add_child(inner, grow);
        let form = t.new_node();
        let mut fr = StyleRules::default();
        fr.height = px(100.0);
        fr.margin_top = Some(Tokenized::Literal(Length::Auto));
        fr.margin_bottom = Some(Tokenized::Literal(Length::Auto));
        t.set_style(form, &fr);
        t.add_child(grow, form);
        t.compute(panel, 393.0, 852.0);
        assert!((t.frame_of(inner).height - 300.0).abs() < 0.5, "got {}", t.frame_of(inner).height);
        assert!((t.frame_of(grow).height - 300.0).abs() < 0.5, "got {}", t.frame_of(grow).height);
        assert!((t.frame_of(form).y - 100.0).abs() < 0.5, "form centred, got y {}", t.frame_of(form).y);
    }

    /// A scroll view's `align_items: Center` centres its content, as on
    /// web, rather than the inner: a `width: 100%` / `max_width: 880`
    /// column fills to its cap and sits in the middle (the kiosk's form
    /// picker collapsed to its narrowest word).
    #[test]
    fn scroll_view_alignment_arranges_the_content_not_the_inner() {
        let mut t = LayoutTree::new();
        let panel = t.new_node();
        let mut pr = StyleRules::default();
        pr.width = px(1200.0);
        pr.height = px(600.0);
        t.set_style(panel, &pr);
        let outer = t.new_node();
        t.add_child(panel, outer);
        let mut author = StyleRules::default();
        author.flex_grow = Some(Tokenized::Literal(1.0));
        author.flex_direction = Some(FlexDirection::Column);
        author.align_items = Some(AlignItems::Center);
        t.set_style(outer, &author);
        let inner = t.new_node();
        t.add_child(outer, inner);
        t.set_overflow_scroll_axis(outer, ScrollAxis::Vertical);
        t.set_style(inner, &inner_content_rules(ScrollAxis::Vertical));
        t.set_style(inner, &inner_forwarded_rules(&author));
        let column = t.new_node();
        t.add_child(inner, column);
        let mut cr = StyleRules::default();
        cr.width = Some(Tokenized::Literal(Length::Percent(100.0)));
        cr.max_width = px(880.0);
        cr.height = px(200.0);
        t.set_style(column, &cr);
        t.compute(panel, 1200.0, 600.0);
        assert!((t.frame_of(inner).width - 1200.0).abs() < 0.5, "inner spans the viewport, got {}", t.frame_of(inner).width);
        assert!((t.frame_of(column).width - 880.0).abs() < 0.5, "column fills to its cap, got {}", t.frame_of(column).width);
        assert!((t.frame_of(column).x - 160.0).abs() < 0.5, "column centred, got x {}", t.frame_of(column).x);
    }

    #[test]
    fn inner_direction_follows_the_scroll_axis() {
        assert_eq!(inner_content_rules(ScrollAxis::Vertical).flex_direction, Some(FlexDirection::Column));
        assert_eq!(inner_content_rules(ScrollAxis::Horizontal).flex_direction, Some(FlexDirection::Row));
        assert_eq!(inner_content_rules(ScrollAxis::Both).flex_direction, Some(FlexDirection::Column));
    }

    /// Every scroller shape the backend creates is one the sticky walk and
    /// the unmount sweep recognise — a two-axis scroller missing from the
    /// list would leave its `top`/`left` sticky children unpinned and leak
    /// its scroll listener on unmount.
    #[test]
    fn every_scroller_class_is_recognised_as_a_scroll_container() {
        for axis in [ScrollAxis::Vertical, ScrollAxis::Horizontal, ScrollAxis::Both] {
            assert!(
                SCROLL_CONTAINER_CLASSES.contains(&scroller_class(axis)),
                "{axis:?} scroller {} is not a recognised scroll container",
                scroller_class(axis)
            );
        }
        assert_eq!(scroller_class(ScrollAxis::Both), TWO_AXIS_SCROLLER_CLASS);
    }
}
