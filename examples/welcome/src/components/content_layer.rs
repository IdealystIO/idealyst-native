//! Absolute, viewport-filling, flex-centered column holding the
//! welcome phrase + subtitle.

use std::rc::Rc;

use runtime_core::{
    component, ui, AlignItems, FlexDirection, JustifyContent, Position, Element, StyleRules,
    StyleSheet,
};

use crate::components::subtitle::{Subtitle, SubtitleProps};
use crate::components::welcome_phrase::{WelcomePhrase, WelcomePhraseProps};
use crate::coordinator::WelcomeRefs;
use crate::style_helpers::{px, static_sheet};

pub struct ContentLayerProps {
    pub refs: WelcomeRefs,
}

#[component]
pub fn ContentLayer(props: &ContentLayerProps) -> Element {
    let refs = props.refs;
    let sheet = sheet();
    ui! {
        View(style = sheet) {
            WelcomePhrase(refs = refs)
            Subtitle(refs = refs)
        }
    }
}

fn sheet() -> Rc<StyleSheet> {
    static_sheet(StyleRules {
        position: Some(Position::Absolute),
        top: Some(px(0.0)),
        left: Some(px(0.0)),
        right: Some(px(0.0)),
        bottom: Some(px(0.0)),
        flex_direction: Some(FlexDirection::Column),
        align_items: Some(AlignItems::Center),
        justify_content: Some(JustifyContent::Center),
        gap: Some(px(14.0)),
        ..Default::default()
    })
}
