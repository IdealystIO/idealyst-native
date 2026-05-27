//! `Badge` — small pill-shaped status indicator, built on the
//! extensible Tone + Variant trait surface.
//!
//! ```ignore
//! use idea_ui::extensible::badge::{badge, BadgeProps};
//! use idea_theme::extensible::{tone, variant};
//!
//! ui! {
//!     Badge(
//!         label = "New",
//!         tone = tone::Success,
//!         variant = variant::Soft,
//!     )
//! }
//! ```
//!
//! Badge has no Size or Shape axis — it's intrinsically small with a
//! fixed pill radius (those values live in the base stylesheet). Just
//! Tone + Variant. The four built-in variants all compose; `Ghost`
//! makes a transparent borderless chip which is rarely useful but
//! not invalid.

use std::rc::Rc;

use runtime_core::{text, IntoPrimitive, Primitive, StyleApplication, StyleRules};

use idea_theme::extensible::{tone, variant, ResolutionCtx, Tone, Variant};
use idea_theme::theme::IdeaThemeRef;

use crate::stylesheets::Badge as BadgeSheet;

pub struct BadgeProps {
    pub label: String,
    pub tone: Rc<dyn Tone>,
    pub variant: Rc<dyn Variant>,
}

impl Default for BadgeProps {
    fn default() -> Self {
        Self {
            label: String::new(),
            // Neutral/Soft = the most generic "this is a status chip"
            // look. Matches the default of the closed-enum Badge.
            tone: Rc::new(tone::Neutral),
            variant: Rc::new(variant::Soft),
        }
    }
}

pub fn badge(props: &BadgeProps) -> Primitive {
    let label = props.label.clone();
    let tone = props.tone.clone();
    let variant = props.variant.clone();

    // Cache key: same (tone, variant) pair shares a class. Badge has
    // no Size/Shape axis so the key is two parts not four.
    let cache_key = format!("badge+{}+{}", variant.key(), tone.key());

    let style = move || {
        // Touch the active theme so the apply-style Effect subscribes
        // to swaps.
        let _ = idea_theme::active_theme()
            .downcast_ref::<IdeaThemeRef>()
            .expect("idea-ui: no IdeaTheme installed — call install_idea_theme(...) first");

        let var = variant.clone();
        let tn = tone.clone();
        let compute = move || -> StyleRules {
            let theme = idea_theme::active_theme();
            let theme_ref = theme
                .downcast_ref::<IdeaThemeRef>()
                .expect("idea-ui: no IdeaTheme installed");
            // No `modifier_defaults` — Badge's padding/font/radius live
            // in the base stylesheet. The variant only contributes the
            // tone-driven skeleton (bg, color, optional border).
            let ctx = ResolutionCtx {
                theme: theme_ref,
                tone: &*tn,
            };
            var.render(&ctx)
        };

        StyleApplication::new(BadgeSheet::sheet()).with_computed(cache_key.clone(), compute)
    };

    text(label).with_style(style).into_primitive()
}
