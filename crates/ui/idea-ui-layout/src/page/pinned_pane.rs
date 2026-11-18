//! [`PinnedPane`] — a side pane that stays put while the page scrolls.

use runtime_core::{
    component, stylesheet, ui, Element, FlexDirection, IdealystSchema, Length, Position, ScrollbarVisibility,
    StyleRules, Tokenized,
};

use super::PageFrame;

#[runtime_core::props]
#[derive(IdealystSchema)]
pub struct PinnedPaneProps {
    /// The page it pins in.
    #[prop(static)]
    pub frame: PageFrame,
    /// No bar of its own: a short nav that rarely overflows, where a
    /// bar's track would throw off its inset. It still scrolls if the
    /// window is short.
    #[prop(static)]
    pub hide_bar: bool,
    /// The pane's content.
    pub children: Vec<Element>,
}

impl Default for PinnedPaneProps {
    fn default() -> Self {
        Self { frame: PageFrame::new(), hide_bar: false, children: Vec::new() }
    }
}

/// A side pane beside content that scrolls with the page's one scroller,
/// so the scrollbar runs the page's full height (rule `one-scroller`).
/// It pins directly under the page's header and scrolls itself only when
/// it is taller than the room left.
///
/// Its parent must be taller than it (a row stretching its children),
/// or there is nothing to stick within. A list or a panel beside a page
/// is normally a [`SplitView`](crate::SplitView) with `frame`, which
/// pins its side region through the same rules; this is for a pane the
/// layouts do not place.
///
/// ```ignore
/// PinnedPane(frame = frame, hide_bar = true) { nav }
/// ```
#[component(children)]
pub fn PinnedPane(props: PinnedPaneProps) -> Element {
    let frame = props.frame;
    let bar = if props.hide_bar { PinnedPaneScrollBar::Hidden } else { PinnedPaneScrollBar::Shown };
    let children = props.children;
    let style = move || pin_rules(frame, StyleRules::default());
    ui! {
        view(style = style) {
            scroll_view(style = PinnedPaneScroll().bar(bar)) {
                view(style = PinnedPaneBody()) {
                    children
                }
            }
        }
    }
}

/// `rules`, made sticky at the top of `frame`'s scroller and never
/// taller than the room under its header, so the region scrolls itself
/// rather than running off the window's foot.
pub(crate) fn pin_rules(frame: PageFrame, rules: StyleRules) -> StyleRules {
    let header = frame.header_height.get();
    let port = frame.port_height.get();
    StyleRules {
        position: Some(Position::Sticky),
        // The header sits above the scroller, not in it, so the top of
        // the scroller is where a pinned region rests.
        top: Some(Tokenized::Literal(Length::Px(0.0))),
        max_height: (port > header).then(|| Tokenized::Literal(Length::Px(port - header))),
        flex_direction: Some(FlexDirection::Column),
        min_height: Some(Tokenized::Literal(Length::Px(0.0))),
        ..rules
    }
}

stylesheet! {
    PinnedPaneScroll<idea_ui::IdeaThemeRef> {
        base(_t) {
            flex_grow: 1.0,
            flex_shrink: 1.0,
            min_height: Length::Px(0.0),
        }
        variant bar {
            #[default]
            shown(_t) {}
            hidden(_t) {
                scrollbar: ScrollbarVisibility::Hidden,
            }
        }
    }
}

stylesheet! {
    // What the pane scrolls: its content at its own height.
    PinnedPaneBody<idea_ui::IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            flex_shrink: 0.0,
        }
    }
}
