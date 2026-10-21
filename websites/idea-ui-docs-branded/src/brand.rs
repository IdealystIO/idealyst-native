//! "Ink" — an alternate design language for idea-ui, installed entirely
//! through component restyles. This module is the canonical example of
//! branding idea-ui: every change below is a `restyle`, an app-defined axis
//! value (`axis_values!`), or an axis default (`set_default`), registered
//! once at boot. Nothing is forked or wrapped.
//!
//! The language: ink (the theme's text color) as the accent surface,
//! filled fields with soft corners, pill-shaped buttons, borderless paper
//! cards, and ink table headers. It is written against theme TOKENS, so
//! it follows the light/dark/cyber theme switch like the stock look does —
//! "ink" is light on a dark theme.
//!
//! Two of its values are app-defined: `CardVariant::Paper` (the default
//! Card variant, from [`InkCardVariants`]) and `BadgeVariant::Ink` (the
//! default Badge variant, from [`InkBadgeVariants`]), so every Card and
//! Badge on the docs pages shows them. It also restyles the docs site's own
//! app values (`CardVariant::Tinted`, `ButtonVariant::Raised`) — a later
//! restyle composes over an earlier one.

use idea_ui::components::table::{TablePinned, TableRowHovered, TableStyles, TableVariant};
use idea_ui::{
    BadgeStyles, BadgeVariant, ButtonShape, ButtonStyles, ButtonVariant, CardStyles, CardVariant, FieldAppearance, FieldRing,
    FieldStyles, SegmentedControlSelected, SegmentedControlStyles,
};
use idea_ui_docs::restyles::{DocsButtonVariants, DocsCardVariants};
use runtime_core::{axis_values, Color, FontWeight, Length, Shadow, StateBits, StyleRules, Tokenized};

axis_values! {
    /// Ink's Card variants.
    pub trait InkCardVariants for CardVariant {
        /// The variant every Card takes by default under Ink: a borderless
        /// paper surface lifted by a soft shadow.
        Paper,
    }
}

axis_values! {
    /// Ink's Badge variants.
    pub trait InkBadgeVariants for BadgeVariant {
        /// The variant every Badge takes by default under Ink: ink fill,
        /// paper text.
        Ink,
    }
}

type Px = Option<Tokenized<f32>>;
type Len = Option<Tokenized<Length>>;
type Col = Option<Tokenized<Color>>;

/// All four corner radii.
fn corners(r: Len) -> StyleRules {
    StyleRules {
        border_top_left_radius: r.clone(),
        border_top_right_radius: r.clone(),
        border_bottom_left_radius: r.clone(),
        border_bottom_right_radius: r,
        ..Default::default()
    }
}

/// All four border colors.
fn edge(c: Col) -> StyleRules {
    StyleRules {
        border_top_color: c.clone(),
        border_right_color: c.clone(),
        border_bottom_color: c.clone(),
        border_left_color: c,
        ..Default::default()
    }
}

/// All four border widths.
fn edge_width(w: Px) -> StyleRules {
    StyleRules {
        border_top_width: w.clone(),
        border_right_width: w.clone(),
        border_bottom_width: w.clone(),
        border_left_width: w,
        ..Default::default()
    }
}

fn none() -> Px {
    Some(Tokenized::Literal(0.0))
}

/// Install the Ink restyles. Call once at boot, before the first render —
/// `idea_ui_docs::app_with(brand::install)` does.
pub fn install() {
    // Fields: filled (Contained is the default appearance), soft corners,
    // and an ink focus border instead of the theme's focus-ring color.
    FieldStyles::set_default(FieldAppearance::Contained);
    FieldStyles::restyle(|s| {
        s.input()
            .base(|t| corners(Some(t.radius.lg())))
            .arm(FieldRing::On, |t| edge(Some(t.color.text())))
            .state(StateBits::FOCUSED, |t| edge(Some(t.color.text())));
    });

    // SegmentedControl: an ink track, the selected segment filled with the
    // primary color, and labels that read on both.
    SegmentedControlStyles::restyle(|s| {
        s.track().base(|t| StyleRules {
            background: Some(t.color.text()),
            ..corners(Some(t.radius.pill())).merge(&edge(Some(t.color.text())))
        });
        s.segment()
            .base(|t| corners(Some(t.radius.pill())))
            .arm(SegmentedControlSelected::On, |t| StyleRules {
                background: Some(t.intent.primary.solid_bg()),
                ..edge(Some(t.intent.primary.solid_bg()))
            });
        s.label()
            .base(|t| StyleRules { color: Some(t.color.surface()), ..Default::default() })
            .arm(SegmentedControlSelected::On, |t| StyleRules {
                color: Some(t.intent.primary.solid_text()),
                ..Default::default()
            });
    });

    // Buttons: pill-shaped by default, with a heavier label. The docs
    // site's `Raised` value trades its shadow for an ink underline.
    ButtonStyles::set_default(ButtonShape::Pill);
    ButtonStyles::restyle(|s| {
        s.label().base(|_| StyleRules { font_weight: Some(FontWeight::SemiBold), ..Default::default() });
        s.root().arm(ButtonVariant::Raised, |t| StyleRules {
            shadow: Some(Shadow { x: 0.0, y: 3.0, blur: 0.0, color: t.color.text() }),
            ..Default::default()
        });
    });

    // Cards: borderless paper by default; the docs site's `Tinted` value
    // becomes a primary wash with an ink spine.
    CardStyles::set_default(CardVariant::Paper);
    CardStyles::restyle(|s| {
        s.root()
            .base(|t| corners(Some(t.radius.lg())))
            .arm(CardVariant::Paper, |t| StyleRules {
                background: Some(t.color.surface()),
                shadow: Some(Shadow { x: 0.0, y: 6.0, blur: 18.0, color: t.effect.shadow() }),
                ..edge_width(none())
            })
            .arm(CardVariant::Tinted, |t| StyleRules {
                background: Some(t.intent.primary.soft_bg()),
                border_left_width: Some(Tokenized::Literal(4.0)),
                border_left_color: Some(t.color.text()),
                ..edge_width(none())
            });
    });

    // Badges: ink by default.
    BadgeStyles::set_default(BadgeVariant::Ink);
    BadgeStyles::restyle(|s| {
        s.root().arm(BadgeVariant::Ink, |t| StyleRules {
            background: Some(t.color.text()),
            color: Some(t.color.surface()),
            ..edge_width(none())
        });
    });

    // Tables: borderless frame, an ink header band with paper text. The
    // header's `row_hovered` arms state its background (a resting arm must,
    // so a released hover clears), so the band is restyled there — and in
    // the `plain` variant's compounds, which clear the band (and outrank
    // the arms): under Ink every header is ink, so its paper text reads.
    TableStyles::restyle(|s| {
        let ink = |t: &idea_ui::IdeaTokens| StyleRules { background: Some(t.color.text()), ..Default::default() };
        s.frame().base(|_| edge_width(none()));
        s.head_cell()
            .arm(TableRowHovered::Off, ink)
            .arm(TableRowHovered::On, ink)
            .compound((TableVariant::Plain, TableRowHovered::Off), ink)
            .compound((TableVariant::Plain, TablePinned::Left, TableRowHovered::Off), ink)
            .compound((TableVariant::Plain, TablePinned::Right, TableRowHovered::Off), ink);
        s.head_text().base(|t| StyleRules { color: Some(t.color.surface()), ..Default::default() });
    });
}
