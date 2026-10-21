//! The Ink brand reaches idea-ui through restyles alone: the composed slot
//! sheets carry its look, its app-defined values are the axis defaults,
//! and every restyled slot still premints.

use idea_ui::{
    install_idea_theme, light_theme, BadgeStyles, BadgeVariant, ButtonShape, CardStyles, CardVariant,
    FieldAppearance, FieldStyles, SegmentedControlSelected, SegmentedControlStyles,
};
use idea_ui_docs_branded::brand::{self, INK, PAPER};
use runtime_core::{resolve_style, StyleApplication};

fn token(app: &StyleApplication, pick: impl Fn(&runtime_core::StyleRules) -> Option<&'static str>) -> Option<&'static str> {
    pick(&resolve_style(app))
}

#[test]
fn ink_restyles_compose_into_idea_ui_and_premint() {
    idea_theme::testing::with_test_world(|| {
        install_idea_theme(light_theme());
        idea_ui_docs::restyles::register();
        brand::install();

        // App-defined values are the defaults every instance picks up.
        assert_eq!(CardVariant::default(), PAPER);
        assert_eq!(BadgeVariant::default(), INK);
        assert_eq!(FieldAppearance::default(), FieldAppearance::Contained);
        assert_eq!(idea_ui::ButtonShape::default(), ButtonShape::Pill);

        let card = CardStyles::root().set(PAPER).into_style_application();
        assert!(resolve_style(&card).shadow.is_some(), "paper is lifted by a shadow");
        assert!(card.preminted_class_list().is_some(), "a restyled slot premints");

        let badge = BadgeStyles::root().set(INK).into_style_application();
        assert_eq!(token(&badge, |r| r.background.as_ref().and_then(|c| c.name())), Some("color-text"));

        let track = SegmentedControlStyles::track().into_style_application();
        assert_eq!(token(&track, |r| r.background.as_ref().and_then(|c| c.name())), Some("color-text"));
        let selected = SegmentedControlStyles::segment().set(SegmentedControlSelected::On).into_style_application();
        assert_eq!(
            token(&selected, |r| r.background.as_ref().and_then(|c| c.name())),
            Some("intent-primary-solid-bg")
        );

        let input = FieldStyles::input().into_style_application();
        assert_eq!(
            token(&input, |r| r.border_top_left_radius.as_ref().and_then(|c| c.name())),
            Some("radius-lg")
        );
        assert!(input.preminted_class_list().is_some());
    });
}
