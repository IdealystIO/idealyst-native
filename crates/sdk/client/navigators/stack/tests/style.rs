//! A stack navigator's root and its outlet take an author style.

#![cfg(not(target_arch = "wasm32"))]

use host_mock::Harness;
use runtime_shared::{Length, StyleRules, Tokenized};
use runtime_vocabulary::builders::{text, view};
use runtime_vocabulary::glue::IntoElement;
use stack_navigator::{navigator_outlet, Route, Screen, StackBuilder, StackNavigator};

const HOME: Route<()> = Route::<()>::new("home", "/");

fn width(px: f32) -> StyleRules {
    StyleRules { width: Some(Tokenized::Literal(Length::Px(px))), ..Default::default() }
}

/// Regression: neither the SDK builders nor the outlet had a style
/// setter, so a navigator could not be sized except by wrapping it.
#[test]
fn regression_navigator_root_and_outlet_take_a_style() {
    let h = Harness::new();
    let _r = h.mount(
        StackNavigator::new(&HOME)
            .screen(HOME, |_| Screen::new(view().child(text().content("HOME")).build()))
            .style(width(321.0))
            .layout(|_nav| view().child(navigator_outlet().style(width(123.0))).build())
            .into_element(),
    );
    h.world.flush();
    let log = h.take_log().join("\n");
    assert!(log.contains("apply_style n0 width=Literal(Px(321.0))"), "the root's style applies:\n{log}");
    assert!(log.contains("width=Literal(Px(123.0))"), "the outlet's style applies:\n{log}");
}
