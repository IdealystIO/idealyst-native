//! Regression #24: an optional-tone prop (`tone: Option<ToneRef>`, which
//! `#[props]` wraps as `Reactive<Option<ToneRef>>`) only accepted
//! `Some(tone::X.into())`. idea-theme's source claimed the orphan rule made
//! that unavoidable; it only forbids the BLANKET impl, so every tone marker
//! now emits concrete `From<Marker>` impls for `Option<ToneRef>` and
//! `Reactive<Option<ToneRef>>`. A bare marker must work in a props literal
//! and at a `ui!` call site, and must paint the same as the `Some(..)` form.

use idea_theme::testing::with_test_world;
use idea_theme::theme::{install_idea_theme, light_theme};
use idea_ui::test_support::{classify, P};
use idea_ui::{tone, Typography, TypographyProps};
use runtime_core::{ui, Element, Reactive, StyleRules};
use std::rc::Rc;

fn text_rules(el: Element) -> Rc<StyleRules> {
    match classify(el) {
        P::Text { style: Some(style), .. } => style.resolve(),
        _ => panic!("Typography renders a styled text node"),
    }
}

#[test]
fn regression_bare_tone_marker_fills_an_optional_tone_prop() {
    with_test_world(|| {
        install_idea_theme(light_theme());

        // Props-literal form: `.into()` straight to `Reactive<Option<ToneRef>>`.
        let bare = TypographyProps {
            content: Reactive::Static("Delete".to_string()),
            tone: tone::Danger.into(),
            ..Default::default()
        };
        assert_eq!(bare.tone.get().map(|t| t.key()), Some("danger"));

        // The long-hand form keeps compiling (no inference ambiguity).
        let explicit = TypographyProps {
            content: Reactive::Static("Delete".to_string()),
            tone: Reactive::Static(Some(tone::Danger.into())),
            ..Default::default()
        };

        // `ui!` struct-literal form: a bare marker, no `Some(..)`.
        let via_ui = ui! { Typography(content = "Delete", tone = tone::Danger) };

        let danger = text_rules(Typography(&explicit)).color.clone();
        assert!(danger.is_some(), "a toned Typography paints a color");
        assert_eq!(text_rules(Typography(&bare)).color, danger);
        assert_eq!(text_rules(via_ui).color, danger);
        assert_ne!(
            text_rules(Typography(&TypographyProps::default())).color,
            danger,
            "the untoned default must differ, or the equality above proves nothing"
        );
    });
}
