//! Shared scrolling surface for the anchored-menu family — `Select`, `Menu`,
//! `SubMenu`, and `Autocomplete` all drop the same themed [`SelectMenu`] panel,
//! so they all shared the same bug: the option rows were placed directly in the
//! panel with no height cap and no scroll container, so a long list (e.g. a
//! category filter listing every category) grew past the bottom of the viewport
//! and the overflow was simply unreachable.
//!
//! [`scrolling_menu_panel`] is the one place that fix lives: it wraps the rows
//! in a height-capped `scroll_view` inside the `SelectMenu` panel — the panel
//! hugs its content until it hits the cap, then the row list scrolls. Mirrors
//! the modal scroller's "content-sized up to a cap, then scroll" shape (see
//! [`crate::components::modal::modal_scroll_sheet`]).

use std::rc::Rc;

use runtime_core::primitives::portal::AnchorTarget;
use runtime_core::stylesheet;
use runtime_core::{
    viewport_size, Element, FlexDirection, IntoElement, Length, StyleApplication, StyleRules,
    StyleSheet, Tokenized
};

use crate::stylesheets::SelectMenu;

/// Default ceiling for the open menu's scrollable row area, in DIPs — roughly
/// eight or nine rows. Enough to browse comfortably without the menu dominating
/// the screen; a longer list scrolls. Capped further by the viewport height on
/// a short screen (see [`menu_max_height`]).
const MENU_MAX_HEIGHT: f32 = 288.0;
/// Breathing room kept above and below the menu so a viewport-tall list can't
/// butt right against the screen edges.
const MENU_VIEWPORT_MARGIN: f32 = 16.0;
/// Never cap the scroll area below this even on a very short viewport — a tiny
/// sliver would be unusable. A menu this tall still scrolls its rows internally.
const MENU_MIN_HEIGHT: f32 = 120.0;

/// The scrollable row area's height cap: the smaller of the fixed default
/// ([`MENU_MAX_HEIGHT`]) and the viewport height minus a margin top and bottom,
/// floored at [`MENU_MIN_HEIGHT`] so even a very short viewport leaves a usable,
/// scrollable menu. This is a `max-height`, not a fixed height — a menu with
/// fewer rows than the cap stays content-sized; only a taller one is clamped
/// here and then scrolls. Pure so the cap is unit-testable without a backend.
pub(crate) fn menu_max_height(viewport_height: f32) -> f32 {
    (viewport_height - MENU_VIEWPORT_MARGIN * 2.0)
        .min(MENU_MAX_HEIGHT)
        .max(MENU_MIN_HEIGHT)
}

/// The inner body view that holds the rows in a column. Carries the inter-row
/// `gap` the `SelectMenu` panel used to supply directly (the panel now has a
/// single child — the scroller — so its own gap no longer spaces the rows), and
/// `flex_shrink: 0` so the body keeps its full content height inside the capped
/// scroller instead of shrinking to fit it — without this there'd be nothing to
/// scroll (same reason as `modal.rs::modal_body_sheet`).
fn menu_body_sheet() -> Rc<StyleSheet> {
    MenuBodySheet::sheet()
}

// `stylesheet!` (LINK-time), not `premint_as`: menu panels construct on
// OPEN, which the premint dump's crawl never does — see `popover.rs`.
stylesheet! {
    MenuBodySheet<()> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            flex_shrink: 0.0,
            // Matches `SelectMenu`'s base `gap` (2px) so row spacing is
            // unchanged.
            gap: Length::Px(2.0),
        }
    }
}

/// The `scroll_view` sheet: content-sized until it hits `max_height` (applied
/// reactively at the call site), then scrolls. Mirrors
/// `modal.rs::modal_scroll_sheet` — `scroll_view` seeds the node with
/// `flex_grow:1 / flex_basis:0` (the "fill a bounded parent" shape), which is
/// WRONG for a content-sized anchored panel: a fill-parent scroller contributes
/// 0 to the panel's intrinsic height, collapsing the whole menu to nothing.
/// Override to the "content-sized up to a cap" shape:
///
/// - `flex_grow:0` + `flex_basis:auto` — the scroller sizes to its body, so the
///   panel sizes to it (a short menu hugs its rows).
/// - `min_height:0` — paired with the scroll node's `overflow:scroll`, lets the
///   reactive `max_height` clamp the scroller BELOW its content height so a long
///   list scrolls internally instead of overflowing the viewport.
fn menu_scroll_sheet() -> Rc<StyleSheet> {
    MenuScrollSheet::sheet()
}

stylesheet! {
    MenuScrollSheet<()> {
        base(_t) {
            flex_grow: 0.0,
            flex_basis: Length::Auto,
            min_height: Length::Px(0.0),
            flex_direction: FlexDirection::Column,
        }
    }
}

/// Wraps `rows` in a viewport-capped, scrolling [`SelectMenu`] panel. This is
/// the canonical anchored-menu surface — every menu-family component (`Select`,
/// `Menu`, `SubMenu`, `Autocomplete`) builds its dropdown through here so a long
/// option list scrolls within a bounded panel instead of running off-screen.
///
/// Structure: `panel(SelectMenu) → scroll_view(height cap) → body(rows)`. The
/// height cap is applied reactively from `viewport_size()` so it tracks
/// orientation / split-view / window resizes and never exceeds the visible area.
pub(crate) fn scrolling_menu_panel(rows: Vec<Element>) -> Element {
    slotted_menu_panel(rows, None, None)
}

/// The children every slot-capable panel shares: the pinned `header`, the
/// capped row scroller, then the pinned `footer`. "Pinned" means OUTSIDE the
/// scroller — a slot stays put while a long row list scrolls under it, which
/// is the whole reason a search field or an "Add ‹query›" action can live on
/// a menu at all.
fn slotted_panel_children(
    rows: Vec<Element>,
    header: Option<Element>,
    footer: Option<Element>,
) -> Vec<Element> {
    // Received-elements plumbing (the panel's children arrive as params),
    // not ad-hoc child authoring — the legitimate Vec<Element> shape.
    let mut children = Vec::with_capacity(3);
    if let Some(h) = header {
        children.push(h);
    }
    children.push(capped_scroller(rows));
    if let Some(f) = footer {
        children.push(f);
    }
    children
}

/// [`scrolling_menu_panel`] with optional pinned `header`/`footer` slots — the
/// surface behind `Menu`'s slots. The caller composes whatever goes in them and
/// owns what it does: the framework pins the slot and caps the rows, it does not
/// know that a header happens to be a search field.
///
/// A slotted panel carries `preserves_focus(true)`, because a slot is exactly
/// where a caller puts a focusable control — without it the first row press
/// blurs the search box mid-typing. A panel with NO slots is left as it was:
/// nothing on it is focusable, and flipping the mark for every `Select` /
/// `Menu` / `SubMenu` dropdown in the app is a change this does not need to
/// make.
pub(crate) fn slotted_menu_panel(
    rows: Vec<Element>,
    header: Option<Element>,
    footer: Option<Element>,
) -> Element {
    let slotted = header.is_some() || footer.is_some();
    let panel = runtime_core::view(slotted_panel_children(rows, header, footer));
    let panel = if slotted { panel.preserves_focus(true) } else { panel };
    panel.with_style(|| StyleApplication::new(SelectMenu::sheet())).into_element()
}

/// [`scrolling_menu_panel`] shaped for a combobox (`Autocomplete`): the same
/// capped scroller, but the panel additionally
///
/// - **matches the anchor's width** — `min_width` tracks the anchoring
///   input's measured width (re-read on viewport resize), so filtering the
///   rows down doesn't shrink the panel to its shortest label and make it
///   jump around under the field. Rows longer than the field still widen the
///   panel (it's a floor, not a fixed width), matching a native select.
/// - **preserves the input's focus** — `.preserves_focus(true)`, so pressing
///   a row (or dragging the scrollbar) doesn't blur the input; the
///   component's close-on-blur only fires on genuine focus departure and a
///   row press can never unmount the row before its own click lands.
/// - **pins optional `header`/`footer` slots** as direct panel children
///   above/below the capped scroller — a slot (e.g. an "Add ‹query›"
///   action) stays visible while the rows scroll, and being inside the
///   focus-preserving panel its presses never blur the input either. The
///   panel's own `gap` spaces slots from the row area like any children.
pub(crate) fn combobox_menu_panel(
    rows: Vec<Element>,
    anchor: AnchorTarget,
    header: Option<Element>,
    footer: Option<Element>,
) -> Element {
    let viewport = viewport_size();
    runtime_core::view(slotted_panel_children(rows, header, footer))
        .preserves_focus(true)
        .with_style(move || {
            // Reading the viewport subscribes this style: a window resize
            // re-measures the anchor (its width can change with layout)
            // while the menu is open. `rect()` itself is an imperative
            // measure, not a reactive source.
            let _ = viewport.get();
            let app = StyleApplication::new(SelectMenu::sheet());
            match anchor.rect() {
                Some(r) if r.width > 0.0 => {
                    let w = r.width;
                    // Measured from the anchor — continuous, so inline.
                    app.with_inline(StyleRules {
                        min_width: Some(Tokenized::Literal(Length::Px(w))),
                        ..Default::default()
                    })
                }
                _ => app,
            }
        })
        .into_element()
}

/// The shared "content-sized up to a viewport-aware cap, then scroll" row
/// scroller both panel shapes wrap (see the module docs for why the cap
/// exists). Reactive height cap: re-resolves the `max_height` when the
/// viewport changes. `with_computed`'s cache key folds identical caps across
/// menu instances onto one backend class (mirrors `modal.rs`'s scroller).
fn capped_scroller(rows: Vec<Element>) -> Element {
    let body = runtime_core::view(rows)
        .with_style(|| StyleApplication::new(menu_body_sheet()))
        .into_element();

    let viewport = viewport_size();
    runtime_core::primitives::scroll_view::scroll_view(vec![body])
        .with_style(move || {
            let max_h = menu_max_height(viewport.get().height);
            StyleApplication::new(menu_scroll_sheet()).with_inline(StyleRules {
                max_height: Some(Tokenized::Literal(Length::Px(max_h))),
                ..Default::default()
            })
        })
        .into_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{classify, P, TStyle};
    use idea_theme::testing::with_test_world;

    #[test]
    fn max_height_caps_to_fixed_default_on_a_roomy_viewport() {
        with_test_world(|| {
            // A tall desktop viewport: the menu is bounded by the fixed default,
            // not the (much larger) viewport, so it never dominates the screen.
            assert_eq!(menu_max_height(1080.0), MENU_MAX_HEIGHT);
    });
    }

    #[test]
    fn regression_max_height_caps_to_viewport_on_a_short_screen() {
        with_test_world(|| {
            // The bug: the menu had NO height cap, so a 40-option list grew past the
            // viewport bottom and the overflow was unreachable. On a short viewport
            // the cap must come from the viewport (minus a margin) — smaller than
            // the fixed default — so the menu can never exceed the visible area.
            let short = 260.0;
            let h = menu_max_height(short);
            assert_eq!(h, short - MENU_VIEWPORT_MARGIN * 2.0);
            assert!(h < MENU_MAX_HEIGHT, "a short viewport caps below the fixed default");
            assert!(h < short, "the menu fits within the visible viewport");
    });
    }

    #[test]
    fn max_height_never_shrinks_below_min_on_a_tiny_viewport() {
        with_test_world(|| {
            // A pathologically short viewport still leaves a usable, scrollable menu
            // rather than collapsing to an unusable sliver.
            assert_eq!(menu_max_height(80.0), MENU_MIN_HEIGHT);
    });
    }

    /// A test anchor whose rect is fixed — stands in for the mounted input
    /// wrapper the combobox panel measures.
    #[derive(Clone)]
    struct FixedAnchor;
    impl runtime_core::primitives::portal::AnchorableHandle for FixedAnchor {
        fn rect(&self) -> runtime_core::ViewportRect {
            runtime_core::ViewportRect { x: 10.0, y: 10.0, width: 240.0, height: 32.0 }
        }
    }

    /// REGRESSION (Autocomplete): the dropdown wrapped its content, so
    /// filtering the rows resized the panel under the field ("jumps all
    /// over in size"), and row presses blurred the input (no
    /// focus-preservation), which with close-on-blur would unmount a row
    /// before its own click landed. The combobox panel must floor its
    /// width at the anchor's measured width AND carry the
    /// `preserves_focus` mark.
    #[test]
    fn regression_combobox_panel_floors_width_at_anchor_and_preserves_focus() {
        with_test_world(|| {
            idea_theme::theme::install_idea_theme(idea_theme::theme::light_theme());

            let anchor_ref: runtime_core::Ref<FixedAnchor> = runtime_core::Ref::new();
            anchor_ref.fill(FixedAnchor);
            let panel = combobox_menu_panel(
                vec![runtime_core::text("A".to_string()).into_element()],
                AnchorTarget::from(anchor_ref),
                None,
                None,
            );

            let P::View { preserves_focus, style, mut children, .. } = classify(panel) else {
                panic!("the combobox panel is a View (the SelectMenu surface)");
            };
            assert!(
                preserves_focus,
                "row presses must not blur the anchoring input (close-on-blur safety)"
            );
            assert!(
                !children.is_empty()
                    && matches!(classify(children.remove(0)), P::ScrollView { .. }),
                "the combobox panel wraps the same capped scroller as the plain panel"
            );

            // The panel style must resolve the anchor's width as `min_width` —
            // a floor (rows longer than the field still widen it), not a fixed
            // width.
            let app = match style.expect("the panel carries the SelectMenu style") {
                TStyle::AppFn(f) => f(),
                _ => panic!("the combobox panel style is reactive (anchor width + viewport cap)"),
            };
            let resolved = runtime_core::resolve_style(&app);
            let min_width = resolved
                .min_width
                .clone()
                .expect("a measurable anchor must produce a min_width floor")
                .resolve();
            assert_eq!(min_width, Length::Px(240.0));
    });
    }

    /// The combobox panel's `header`/`footer` slots must land as direct
    /// panel children AROUND the capped scroller — pinned, so an
    /// "Add ‹query›" footer stays visible while a long row list scrolls.
    /// If a refactor moves a slot inside the scroller it scrolls away with
    /// the rows and this fails. The panel must also keep its
    /// `preserves_focus` mark so slot presses don't blur the anchoring
    /// input (close-on-blur safety, same as rows).
    #[test]
    fn combobox_panel_pins_header_and_footer_outside_the_scroller() {
        with_test_world(|| {
            idea_theme::theme::install_idea_theme(idea_theme::theme::light_theme());

            let anchor_ref: runtime_core::Ref<FixedAnchor> = runtime_core::Ref::new();
            anchor_ref.fill(FixedAnchor);
            let panel = combobox_menu_panel(
                vec![runtime_core::text("Row".to_string()).into_element()],
                AnchorTarget::from(anchor_ref),
                Some(runtime_core::text("Header".to_string()).into_element()),
                Some(runtime_core::text("Footer".to_string()).into_element()),
            );

            let P::View { preserves_focus, mut children, .. } = classify(panel) else {
                panic!("the combobox panel is a View (the SelectMenu surface)");
            };
            assert!(preserves_focus, "slot presses must not blur the anchoring input");
            assert_eq!(children.len(), 3, "panel children are [header, scroller, footer]");
            match classify(children.remove(0)) {
                P::Text { text, .. } => assert_eq!(text.as_deref(), Some("Header")),
                _ => panic!("the header slot is the panel's first child, outside the scroller"),
            }
            assert!(
                matches!(classify(children.remove(0)), P::ScrollView { .. }),
                "the row scroller sits between the pinned slots"
            );
            match classify(children.remove(0)) {
                P::Text { text, .. } => assert_eq!(text.as_deref(), Some("Footer")),
                _ => panic!("the footer slot is the panel's last child, outside the scroller"),
            }
    });
    }

    /// The slot-capable panel pins `header`/`footer` around the capped
    /// scroller and marks itself focus-preserving — the two things a search
    /// field on a menu needs. If a refactor moves a slot inside the scroller
    /// it scrolls away with the rows; if it drops `preserves_focus`, the
    /// first row press blurs the box mid-typing.
    #[test]
    fn slotted_panel_pins_slots_outside_the_scroller_and_preserves_focus() {
        with_test_world(|| {
            idea_theme::theme::install_idea_theme(idea_theme::theme::light_theme());

            let panel = slotted_menu_panel(
                vec![runtime_core::text("Row".to_string()).into_element()],
                Some(runtime_core::text("Header".to_string()).into_element()),
                Some(runtime_core::text("Footer".to_string()).into_element()),
            );

            let P::View { preserves_focus, mut children, .. } = classify(panel) else {
                panic!("the slotted panel is a View (the SelectMenu surface)");
            };
            assert!(preserves_focus, "a row press must not blur a focusable header slot");
            assert_eq!(children.len(), 3, "panel children are [header, scroller, footer]");
            match classify(children.remove(0)) {
                P::Text { text, .. } => assert_eq!(text.as_deref(), Some("Header")),
                _ => panic!("the header slot is the panel's first child, outside the scroller"),
            }
            assert!(
                matches!(classify(children.remove(0)), P::ScrollView { .. }),
                "the row scroller sits between the pinned slots"
            );
            match classify(children.remove(0)) {
                P::Text { text, .. } => assert_eq!(text.as_deref(), Some("Footer")),
                _ => panic!("the footer slot is the panel's last child, outside the scroller"),
            }
    });
    }

    /// A panel with NO slots is left exactly as it was — one scroller child,
    /// and NOT focus-preserving. `preserves_focus` is deliberately scoped to
    /// slotted panels: flipping it for every `Select` / `Menu` / `SubMenu`
    /// dropdown in the app is a change this feature does not need to make.
    #[test]
    fn a_slotless_panel_is_unchanged_and_does_not_preserve_focus() {
        with_test_world(|| {
            idea_theme::theme::install_idea_theme(idea_theme::theme::light_theme());

            let panel = slotted_menu_panel(
                vec![runtime_core::text("Row".to_string()).into_element()],
                None,
                None,
            );

            let P::View { preserves_focus, children, .. } = classify(panel) else {
                panic!("the panel is a View (the SelectMenu surface)");
            };
            assert!(!preserves_focus, "a slotless menu panel keeps its original focus behaviour");
            assert_eq!(children.len(), 1, "the panel holds a single scroller child");
    });
    }

    /// Regression: the anchored menu panel must wrap its rows in a `scroll_view`
    /// so a long option list scrolls instead of overflowing the viewport. The
    /// panel is `view(SelectMenu) → scroll_view → body(rows)`; if a refactor
    /// drops the scroller and puts rows straight in the panel again, the
    /// off-screen-overflow bug returns and this fails.
    #[test]
    fn menu_panel_wraps_rows_in_a_scroll_view() {
        with_test_world(|| {
            let panel = scrolling_menu_panel(vec![
                runtime_core::text("A".to_string()).into_element(),
                runtime_core::text("B".to_string()).into_element(),
            ]);

            let mut panel_children = match classify(panel) {
                P::View { children, .. } => children,
                _ => panic!("the menu panel must be a View (the SelectMenu surface)"),
            };
            assert_eq!(panel_children.len(), 1, "the panel holds a single scroller child");
            let mut body = match classify(panel_children.remove(0)) {
                P::ScrollView { children, .. } => children,
                _ => panic!("the panel's child must be a ScrollView wrapping the rows"),
            };
            assert_eq!(body.len(), 1, "the scroller holds a single body view");
            let rows = match classify(body.remove(0)) {
                P::View { children, .. } => children,
                _ => panic!("the scroller's child must be the body View holding the rows"),
            };
            assert_eq!(rows.len(), 2, "both rows live inside the scrolling body");
    });
    }
}
