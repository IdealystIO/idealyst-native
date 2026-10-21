//! The docs site's own restyles — installed at boot, before any page
//! renders, so they compose into idea-ui's slot sheets and premint.
//!
//! Each adds a value the pages demonstrate; none changes a built-in look
//! (that is the branded variant's job — `websites/idea-ui-docs-branded`).
//!
//! - The accent-gradient Button variant, which idea-ui ships as an opt-in
//!   recipe rather than a built-in arm.
//! - An app-defined Button variant (`ButtonVariant::Raised`) with a
//!   tone-aware recipe, and the same recipe as an IconButton variant
//!   (`IconButtonVariant::Raised`).
//! - An app-defined Card variant (`CardVariant::Tinted`) and Field
//!   appearance (`FieldAppearance::Pill`), each one restyle arm.
//!
//! The values are named with `axis_values!`, which derives each key from
//! its name. They sit on idea-ui's types, so they arrive through traits —
//! a page imports `crate::restyles::*` to write `ButtonVariant::Raised`.

use idea_theme::extensible::{ResolutionCtx, Variant};
use idea_theme::extensible::variant::no_border;
use idea_ui::{
    ButtonStyles, ButtonVariant, CardStyles, CardVariant, FieldAppearance, FieldStyles, IconButtonStyles,
    IconButtonVariant,
};
use runtime_core::{axis_values, Shadow, StyleRules};

axis_values! {
    /// The docs site's Button variants.
    pub trait DocsButtonVariants for ButtonVariant {
        /// A filled surface lifted by a soft shadow.
        Raised,
    }
}

axis_values! {
    /// The docs site's IconButton variants.
    pub trait DocsIconButtonVariants for IconButtonVariant {
        /// The `Raised` recipe on an IconButton.
        Raised,
    }
}

axis_values! {
    /// The docs site's Card variants.
    pub trait DocsCardVariants for CardVariant {
        /// The primary tint, with a primary edge.
        Tinted,
    }
}

axis_values! {
    /// The docs site's Field appearances.
    pub trait DocsFieldAppearances for FieldAppearance {
        /// A fully rounded, filled input.
        Pill,
    }
}

/// The `ButtonVariant::Raised` recipe — the tone's fill and foreground, plus a drop
/// shadow. Rendered over every registered tone, so `tone = …` picks the
/// fill as it does for the built-ins.
pub struct Raised;

impl Variant for Raised {
    fn key(&self) -> &'static str {
        ButtonVariant::Raised.key()
    }
    fn render(&self, ctx: &ResolutionCtx) -> StyleRules {
        let mut s = no_border();
        s.background = Some(ctx.tone.fill_bg(ctx.theme));
        s.color = Some(ctx.tone.fill_fg(ctx.theme));
        s.shadow = Some(Shadow { x: 0.0, y: 3.0, blur: 10.0, color: ctx.theme.effects().shadow.clone() });
        s
    }
}

/// Install the docs site's restyles. Call once at boot, before the first
/// render.
pub fn register() {
    ButtonStyles::restyle(|s| {
        s.tone_variant(ButtonVariant::Gradient, &idea_ui::variant::Gradient);
        s.tone_variant(ButtonVariant::Raised, &Raised);
    });
    IconButtonStyles::restyle(|s| s.tone_variant(IconButtonVariant::Raised, &Raised));
    CardStyles::restyle(|s| {
        s.root().arm(CardVariant::Tinted, |t| {
            let edge = Some(t.intent.primary.solid_bg());
            StyleRules {
                background: Some(t.intent.primary.soft_bg()),
                border_top_color: edge.clone(),
                border_right_color: edge.clone(),
                border_bottom_color: edge.clone(),
                border_left_color: edge,
                ..Default::default()
            }
        });
    });
    FieldStyles::restyle(|s| {
        s.input().arm(FieldAppearance::Pill, |t| {
            let pill = Some(t.radius.pill());
            StyleRules {
                background: Some(t.color.surface_alt()),
                border_top_left_radius: pill.clone(),
                border_top_right_radius: pill.clone(),
                border_bottom_left_radius: pill.clone(),
                border_bottom_right_radius: pill,
                padding_left: Some(t.spacing.lg()),
                padding_right: Some(t.spacing.lg()),
                ..Default::default()
            }
        });
    });
}
