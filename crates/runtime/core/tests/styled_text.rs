//! Styled text runs — walker + theme-cohort behavior.
//!
//! Covers the `TextSource::Styled` build path end to end against the
//! mock backend: run hand-off at create time, paragraph style
//! application, theme-swap re-realization through the shared cohort,
//! cleanup on unmount, and the plain-text lowering for non-Text
//! consumers (button labels).

mod common;

use common::mock_backend::Event;
use common::runtime::TestRuntime;

use runtime_core::styled_text::{TextRun, TextRunStyle};
use std::rc::Rc;

use runtime_core::{
    install_tokens, styled_text, update_tokens, Color, IntoElement, StyleApplication, StyleRules,
    StyleSheet, TokenEntry, TokenValue, Tokenized, VariantSet,
};

fn chip_runs() -> Vec<TextRun> {
    vec![
        TextRun::plain("the "),
        TextRun::styled(
            "ui!",
            TextRunStyle {
                background: Some(Tokenized::token(
                    "test-chip-bg",
                    Color("#eee".into()),
                )),
                ..Default::default()
            },
        ),
        TextRun::plain(" macro"),
    ]
}

/// The walker's `Styled` arm hands the full run list to
/// `create_styled_text` (not a pre-flattened string).
#[test]
fn walker_hands_runs_to_create_styled_text() {
    let rt = TestRuntime::new();
    let _owner = rt.render(styled_text(chip_runs()).into_element());
    let events = rt.events();
    assert!(
        events.contains(&Event::CreateStyledText {
            plain: "the ui! macro".to_string(),
            styled_runs: 1,
        }),
        "expected CreateStyledText with the full run list; got {events:?}",
    );
}

/// A styled text node's `.with_style(...)` is the paragraph style —
/// it must flow through the regular apply-style path on the SAME node
/// the runs were created on.
#[test]
fn paragraph_style_applies_to_the_styled_node() {
    let rt = TestRuntime::new();
    let sheet = Rc::new(StyleSheet::new(|_vs: &VariantSet| StyleRules::default()));
    let app = StyleApplication::new(sheet);
    let _owner = rt.render(
        styled_text(chip_runs())
            .with_style(app)
            .into_element(),
    );
    let events = rt.events();
    assert!(
        events.iter().any(|e| matches!(e, Event::CreateStyledText { .. })),
        "styled text created; got {events:?}",
    );
    assert!(
        events.iter().any(|e| matches!(e, Event::ApplyStyle { .. })),
        "paragraph style must reach apply_style; got {events:?}",
    );
}

/// Theme-swap re-realization: on a non-cascade backend (the mock,
/// like every native backend), a token update re-fires the theme
/// cohort, which must call `update_styled_text` with the runs so
/// natively-resolved run colors track the new theme values. This is
/// the regression test for "chips keep their mount-time colors after
/// a dark-mode toggle on native".
#[test]
fn regression_token_update_rerealizes_styled_runs_on_native() {
    install_tokens(&[TokenEntry {
        name: "test-chip-bg",
        value: TokenValue::Color(Color("#aaa".into())),
    }]);

    let rt = TestRuntime::new();
    let owner = rt.render(styled_text(chip_runs()).into_element());

    let before = rt.events();
    assert!(
        !before.iter().any(|e| matches!(e, Event::UpdateStyledText { .. })),
        "no re-realization before any token change; got {before:?}",
    );

    update_tokens(&[TokenEntry {
        name: "test-chip-bg",
        value: TokenValue::Color(Color("#bbb".into())),
    }]);

    let after = rt.events();
    assert!(
        after.iter().any(|e| matches!(
            e,
            Event::UpdateStyledText { plain, .. } if plain == "the ui! macro"
        )),
        "token update must re-realize the styled runs via the cohort; got {after:?}",
    );

    // Unmount, then swap again: the cohort entry must be gone (no
    // further update on a dead node).
    drop(owner);
    let count_after_drop = rt
        .events()
        .iter()
        .filter(|e| matches!(e, Event::UpdateStyledText { .. }))
        .count();
    update_tokens(&[TokenEntry {
        name: "test-chip-bg",
        value: TokenValue::Color(Color("#ccc".into())),
    }]);
    let count_after_swap = rt
        .events()
        .iter()
        .filter(|e| matches!(e, Event::UpdateStyledText { .. }))
        .count();
    assert_eq!(
        count_after_drop, count_after_swap,
        "unmounted styled text must not re-realize on later token updates",
    );
}

/// A `TextSource::Styled` used as a button label lowers to the
/// concatenated plain text — styled runs are an `Element::Text`
/// capability, but the words must never be lost elsewhere.
#[test]
fn styled_source_as_button_label_lowers_to_plain_text() {
    use runtime_core::{Element, TextSource};
    use std::rc::Rc;

    let rt = TestRuntime::new();
    let label = TextSource::Styled(Rc::new(chip_runs()));
    let _owner = rt.render(Element::Button {
        label,
        on_click: runtime_core::IntoAction::into_action(|| {}),
        leading_icon: None,
        trailing_icon: None,
        style: None,
        ref_fill: None,
        disabled: None,
        accessibility: Default::default(),
        #[cfg(feature = "robot")]
        test_id: None,
    });
    let events = rt.events();
    assert!(
        events.contains(&Event::CreateButton { label: "the ui! macro".to_string() }),
        "styled button label must lower to plain concat; got {events:?}",
    );
}
