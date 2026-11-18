//! [`DialogFooter`] — the row of verbs a form dialog ends with.

use runtime_core::{
    component, stylesheet, ui, AlignItems, ChildList, Element, FlexDirection, IdealystSchema, JustifyContent,
};

#[runtime_core::props]
#[derive(IdealystSchema, Default)]
pub struct DialogFooterProps {
    /// The dialog's verbs in reading order: the way out first, the verb
    /// last. In the sheet form this is a column, so the last child is
    /// the one nearest the thumb.
    pub children: Vec<Element>,
}

/// The row a form dialog ends with: Cancel and the verb. A full-width
/// column on a phone and a right-aligned row from `Sm`, by stylesheet,
/// so a rotation restyles the footer and keeps its pressed state.
///
/// Each `Button` inside passes `block = dialog_block()`
/// ([`dialog_block`](super::dialog_block) says why the footer cannot do
/// it for them). The footer adds no bottom padding: the sheet carries
/// the home indicator's inset itself.
///
/// ```ignore
/// DialogFooter {
///     Button(label = "Cancel", variant = variant::Ghost, tone = tone::Neutral,
///            block = dialog_block(), on_click = dismiss.clone())
///     Button(label = "Save", loading = busy, block = dialog_block(), on_click = save.clone())
/// }
/// ```
#[component(children)]
pub fn DialogFooter(props: DialogFooterProps) -> Element {
    // The `#[component(children)]` slot's flatten, not a staged list.
    let mut verbs = Vec::with_capacity(props.children.len());
    for c in props.children {
        ChildList::append_to(c, &mut verbs);
    }
    ui! {
        view(style = DialogFooterRow()) {
            verbs
        }
    }
}

stylesheet! {
    // `sm` apart at both widths: two forms of one decision.
    DialogFooterRow<idea_ui::IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Stretch,
            gap: t.spacing.sm(),
        }
        breakpoint sm(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::FlexEnd,
            gap: t.spacing.sm(),
        }
    }
}
