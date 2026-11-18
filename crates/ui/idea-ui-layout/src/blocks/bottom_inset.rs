//! [`BottomInset`] — the last thing a scroller holds, so its content
//! ends clear of the bottom safe area.

use runtime_core::{component, stylesheet, ui, Element, FlexDirection, SafeAreaSides};

/// The bottom safe area's height, as a block (rule `safe-areas`). Put it
/// LAST inside a scroller that reaches the screen's bottom edge: the
/// rows pass under the home indicator as they scroll, and the last one
/// comes to rest above it. It takes no height where there is no inset.
///
/// A `Content` page needs none: the page pads its own scroller. It is
/// for the scroller a `Table` or `Blank` page's body brings.
///
/// ```ignore
/// scroll_view(style = Rows()) {
///     for row in rows.get(), key = row.id { Row(row = row.clone()) }
///     BottomInset()
/// }
/// ```
#[component]
pub fn BottomInset() -> Element {
    ui! { view(style = BottomInsetSeat(), safe_area = SafeAreaSides::BOTTOM) {} }
}

stylesheet! {
    // Never squeezed: its whole height is the inset.
    BottomInsetSeat<idea_ui::IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            flex_shrink: 0.0,
        }
    }
}
