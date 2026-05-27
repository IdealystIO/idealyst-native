//! `Typography` — text component driven by the extensible
//! [`TypographyKind`](super::TypographyKind) trait.
//!
//! ```ignore
//! use idea_ui::extensible::{typography, typography_component::TypographyProps};
//!
//! ui! {
//!     Typography(
//!         content = "Welcome".into(),
//!         kind = typography::H1,
//!     )
//! }
//! ```
//!
//! The `kind` axis carries the full per-variant character — font size,
//! weight, line height, letter spacing — so apps add a `SexySubtitle`
//! kind by implementing the trait on a marker type without re-deriving
//! anything.
//!
//! Color: defaults to `theme.colors().text`. Set `muted = true` to use
//! `theme.colors().text_muted`. Intent-colored text (e.g. a Danger
//! heading) is a future axis — for now apps that need it can wrap the
//! call in a colored container.

use runtime_core::{text, IntoPrimitive, Primitive, StyleApplication, StyleRules, TextAlign};

use idea_theme::active_theme;
use idea_theme::extensible::{ResolutionCtx, ToneRef, TypographyKindRef};
use idea_theme::theme::{IdeaTheme, IdeaThemeRef};
use crate::stylesheets::Typography as TypographySheet;

/// Props for the extensible Typography. `kind: TypographyKindRef` so
/// call sites write `kind: typography_kind::H1.into()`.
///
/// Color resolution precedence:
/// 1. `tone: Some(...)` — uses the tone's on-page foreground.
/// 2. `muted: true` — uses the theme's muted text color.
/// 3. Default — uses the theme's primary text color.
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct TypographyProps {
    pub content: String,
    pub kind: TypographyKindRef,
    /// Optional intent-colored text. When `Some`, renders the text in
    /// the tone's `stroke_fg` (the "on page background" intent color).
    pub tone: Option<ToneRef>,
    /// When `true` and `tone` is `None`, use the theme's muted text
    /// color. Ignored when `tone` is `Some`.
    pub muted: bool,
    /// Skipped from DocControls — `TextAlign` is a framework enum
    /// without `VariantEnum`, and the docs-derive heuristic flags any
    /// `*Align` field as a VariantEnum by convention.
    #[cfg_attr(feature = "docs", doc_control(skip))]
    pub align: TextAlign,
}

impl Default for TypographyProps {
    fn default() -> Self {
        Self {
            content: String::new(),
            kind: TypographyKindRef::default(),
            tone: None,
            muted: false,
            align: TextAlign::Left,
        }
    }
}

/// Render Typography. The computed layer pulls font-size, font-weight,
/// line-height, letter-spacing from the `kind` and text-align from the
/// prop. Color is read from the theme directly so it follows theme
/// swap without needing a per-instance token.
pub fn typography(props: &TypographyProps) -> Primitive {
    let content = props.content.clone();
    let kind = props.kind.clone();
    let tone = props.tone.clone();
    let muted = props.muted;
    let align = props.align;

    let tone_key = tone.as_ref().map(|t| t.key()).unwrap_or("_");
    let cache_key = format!(
        "{}+{}+{}+{:?}",
        kind.key(),
        tone_key,
        if muted { "muted" } else { "default" },
        align,
    );

    let style = move || {
        let _ = active_theme()
            .downcast_ref::<IdeaThemeRef>()
            .expect("idea-ui: no IdeaTheme installed — call install_idea_theme(...) first");

        let k = kind.clone();
        let tn = tone.clone();
        let compute = move || -> StyleRules {
            let theme = active_theme();
            let theme_ref = theme
                .downcast_ref::<IdeaThemeRef>()
                .expect("idea-ui: no IdeaTheme installed");
            // Color precedence: tone wins, then muted, then default.
            let color = if let Some(t) = tn.as_ref() {
                let ctx = ResolutionCtx {
                    theme: theme_ref,
                    tone: &**t,
                };
                ctx.tone.stroke_fg(ctx.theme)
            } else if muted {
                theme_ref.colors().text_muted.clone()
            } else {
                theme_ref.colors().text.clone()
            };
            StyleRules {
                font_size: Some(k.font_size()),
                font_weight: Some(k.font_weight()),
                line_height: Some(k.line_height()),
                letter_spacing: Some(k.letter_spacing()),
                color: Some(color),
                text_align: Some(align),
                ..Default::default()
            }
        };

        StyleApplication::new(TypographySheet::sheet()).with_computed(cache_key.clone(), compute)
    };

    text(content).with_style(style).into_primitive()
}
