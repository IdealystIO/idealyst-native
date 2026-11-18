//! [`Toolbar`] — the row under a screen's header — and the controls
//! made for it: [`ToolbarTabs`], [`ToolbarSearch`], [`ToolbarButton`].

use runtime_core::{
    component, stylesheet, ui, AlignItems, Element, FlexDirection, FlexWrap, IdealystSchema, JustifyContent, Length,
};

mod button;
mod search;
mod tabs;

pub use button::{ToolbarButton, ToolbarButtonProps};
pub use search::{ToolbarSearch, ToolbarSearchProps, SEARCH_FIELD_MAX_WIDTH, SEARCH_POPOVER_WIDTH};
pub use tabs::{ToolbarTabs, ToolbarTabsProps};

/// The least room the `start` slot keeps before the `end` slot wraps
/// under it: one tab and the "N more" button.
pub const TOOLBAR_START_MIN: f32 = 200.0;

#[runtime_core::props]
#[derive(IdealystSchema, Default)]
pub struct ToolbarProps {
    /// SLOT, at the left: where the reader is among the screen's views.
    /// [`ToolbarTabs`], or a search field where the screen has no tabs.
    pub start: Option<Element>,
    /// SLOT, at the right: the controls that scope the body.
    /// [`ToolbarSearch`], [`ToolbarButton`]s, a date control, a view
    /// switch.
    pub end: Option<Element>,
    /// SLOT, a line of its own under the row: what does not fit a row
    /// at any width (a date window a phone needs at full width). Empty
    /// costs no height.
    pub below: Option<Element>,
}

/// The toolbar row (rule `toolbar-row`): navigation at the left, the
/// controls that scope the body at the right. Pass it to
/// `Page(toolbar = …)`, which pins it under the header in the page
/// gutter.
///
/// One line while both ends fit. When the controls would leave `start`
/// less than [`TOOLBAR_START_MIN`], they wrap to a line of their own
/// under it, still at the right. The row decides that from the widths
/// by itself, so it holds on a phone, in a narrow pane, and in a wide
/// window with many controls alike, and nothing is rebuilt when it
/// happens.
///
/// It holds only what scopes the body. A verb that writes belongs in
/// the header's actions.
///
/// ```ignore
/// Toolbar(
///     start = Some(ui! { ToolbarTabs(tabs = tabs, active = rx!(tab.get()), on_change = pick) }),
///     end = Some(ui! {
///         ToolbarSearch(value = search, placeholder = "Search headings".to_string())
///         ToolbarButton(label = "Filter".to_string(), icon = Some(LIST_FILTER), count = rx!(filters_on()), on_press = open_filters)
///     }),
/// )
/// ```
#[component]
pub fn Toolbar(props: ToolbarProps) -> Element {
    let ToolbarProps { start, end, below } = props;
    ui! {
        view(style = ToolbarColumn(), test_id = "toolbar") {
            view(style = ToolbarRow()) {
                if let Some(start) = start {
                    view(style = ToolbarStart(), test_id = "toolbar-start") { start }
                }
                if let Some(end) = end {
                    view(style = ToolbarEnd(), test_id = "toolbar-end") { end }
                }
            }
            if let Some(below) = below {
                below
            }
        }
    }
}

stylesheet! {
    ToolbarColumn<idea_ui::IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: t.spacing.sm(),
            min_width: Length::Px(0.0),
        }
    }
}

stylesheet! {
    // A wrapping row of two items. `start` asks for its minimum and
    // grows; when the row cannot give it that beside `end`, `end` wraps.
    ToolbarRow<idea_ui::IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            flex_wrap: FlexWrap::Wrap,
            align_items: AlignItems::Center,
            column_gap: t.spacing.md(),
            row_gap: t.spacing.sm(),
            min_width: Length::Px(0.0),
        }
    }
}

stylesheet! {
    // A row, so what is in it is sized by its own content and a
    // `ToolbarTabs` inside can measure the room it has.
    ToolbarStart<idea_ui::IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            flex_grow: 1.0,
            flex_shrink: 1.0,
            flex_basis: Length::Px(TOOLBAR_START_MIN),
            min_width: Length::Px(0.0),
        }
    }
}

stylesheet! {
    // Pushed to the right end, on the first line or its own. It wraps
    // within itself only once it is wider than the whole row.
    ToolbarEnd<idea_ui::IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            flex_wrap: FlexWrap::Wrap,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::FlexEnd,
            column_gap: t.spacing.sm(),
            row_gap: t.spacing.sm(),
            flex_shrink: 1.0,
            max_width: Length::pct(100.0),
            margin_left: Length::Auto,
        }
    }
}
