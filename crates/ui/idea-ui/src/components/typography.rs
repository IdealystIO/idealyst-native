//! `Typography` — text component driven by the extensible
//! `TypographyKind` trait.
//!
//! ```ignore
//! ui! { Typography(content = "Welcome".into(), kind = typography_kind::H1) }
//! ```
//!
//! Styling routes through the installed Typography stylesheet (set by
//! `install_idea_theme`). Three axes: `kind` (font characteristics),
//! `color` (default / muted / tone-driven), `align`. Every combination
//! is pre-generated, so apply-style is a className lookup.
//!
//! Color precedence: `tone: Some(...)` wins, then `muted: true`, then
//! the theme's default text color.

use runtime_core::{text, IntoElement, Element, Reactive, StyleApplication, TextAlign};

use idea_theme::extensible::{installed_typography_sheet, ToneRef, TypographyKindRef};

#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct TypographyProps {
    /// Text content. `Reactive<String>` so it can carry live text: a
    /// string literal / `String` is static, a `Signal<String>` or
    /// `rx!(…)` re-renders the text in place when its signals change —
    /// no parent rebuild. The invocation macro coerces all of these via
    /// `.into()`, so call sites are unchanged for the static case.
    pub content: Reactive<String>,
    pub kind: TypographyKindRef,
    /// Optional intent-colored text. When `Some`, overrides `muted`.
    pub tone: Option<ToneRef>,
    /// When `true` and `tone` is `None`, use the theme's muted text color.
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
            content: Reactive::Static(String::new()),
            kind: TypographyKindRef::default(),
            tone: None,
            muted: false,
            align: TextAlign::Left,
        }
    }
}

pub fn typography(props: &TypographyProps) -> Element {
    let content = props.content.clone();
    let kind_key = props.kind.key().to_string();
    let color_key = match (&props.tone, props.muted) {
        (Some(t), _) => t.key().to_string(),
        (None, true) => "muted".to_string(),
        (None, false) => "default".to_string(),
    };
    let align_key = match props.align {
        TextAlign::Left => "left",
        TextAlign::Center => "center",
        TextAlign::Right => "right",
        TextAlign::Justify => "justify",
    }
    .to_string();

    // Static style — applied at build time (before first paint), with
    // theme swaps handled in bulk by the theme cohort. A reactive
    // closure would defer the apply to a per-node Effect, letting the
    // text paint once in the browser-default color before the themed
    // class lands — which the `color` transition then animates (the
    // on-load / on-navigation text flicker). The axis keys are fixed
    // per instance, so nothing needs to be reactive.
    let style = StyleApplication::new(installed_typography_sheet())
        .with("kind", kind_key)
        .with("color", color_key)
        .with("align", align_key);

    text(content).with_style(style).into_element()
}
