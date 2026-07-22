//! `Card` — the shared surface container every panel is built on.

use runtime_core::{component, ui, ChildList, Element, StyleRules, StyleSheet, Tokenized};
use std::rc::Rc;

use crate::style_helpers::{col, px, static_sheet};

#[derive(Default)]
pub struct CardProps {
    /// Panel contents. Incoming fragments are flattened before rendering.
    pub children: Vec<Element>,
}

/// A rounded, padded, bordered surface that groups related content.
///
/// Every dashboard panel (Header, Toolbar, each TaskRow) wraps itself in
/// a `Card` so the whole app shares one surface treatment. Pass the
/// panel body as children.
#[component(children)]
pub fn Card(props: CardProps) -> Element {
    let sheet = card_sheet();
    let mut children: Vec<Element> = Vec::with_capacity(props.children.len());
    for c in props.children {
        ChildList::append_to(c, &mut children);
    }
    ui! {
        view(style = sheet) {
            children
        }
    }
}

fn card_sheet() -> Rc<StyleSheet> {
    static_sheet(StyleRules {
        gap: Some(px(8.0)),
        padding_top: Some(px(16.0)),
        padding_bottom: Some(px(16.0)),
        padding_left: Some(px(16.0)),
        padding_right: Some(px(16.0)),
        background: Some(col("#ffffff")),
        border_top_left_radius: Some(px(12.0)),
        border_top_right_radius: Some(px(12.0)),
        border_bottom_left_radius: Some(px(12.0)),
        border_bottom_right_radius: Some(px(12.0)),
        border_top_width: Some(Tokenized::Literal(1.0)),
        border_right_width: Some(Tokenized::Literal(1.0)),
        border_bottom_width: Some(Tokenized::Literal(1.0)),
        border_left_width: Some(Tokenized::Literal(1.0)),
        border_top_color: Some(col("#e4e6ef")),
        border_right_color: Some(col("#e4e6ef")),
        border_bottom_color: Some(col("#e4e6ef")),
        border_left_color: Some(col("#e4e6ef")),
        ..Default::default()
    })
}
