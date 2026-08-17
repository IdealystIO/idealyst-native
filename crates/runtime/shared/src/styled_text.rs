//! Styled text runs — inline-styled ranges inside a single text
//! primitive.
//!
//! A paragraph that mixes prose with inline code (or any other
//! per-range emphasis) is ONE text node whose content is a list of
//! [`TextRun`]s. Inline wrapping is delegated to each backend's own
//! text engine — `NSAttributedString` on Apple, `SpannableString` on
//! Android, nested `<span>`s on web/SSR, cosmic-text rich spans on the
//! GPU backend — which is the only place mixed-style text can wrap
//! *through* run boundaries as a single paragraph. The framework's
//! layout tree (Taffy) has no inline formatting context, so modeling
//! runs as sibling text nodes can never converge across backends;
//! modeling them inside one node always can (CLAUDE.md §7).
//!
//! [`TextRunStyle`] is deliberately a narrow subset of `StyleRules`:
//! only properties every backend's attributed-text mechanism supports
//! uniformly (font family/weight/style/size, foreground, background,
//! underline). No padding, no corner radius — a chip with rounded
//! corners can't be expressed by `NSAttributedString`/`Spannable`
//! without custom drawing, and a web-only nicety would violate the
//! convergence rule.
//! Colors and sizes are `Tokenized` so they participate in theming:
//! web emits them as `var(--token)` (theme swaps ride the CSS
//! cascade); native backends resolve them at realize time and get
//! re-realized through the theme cohort (see `walker::text`).
//!
//! Reactive content is out of scope for styled runs: the run list is
//! static per node. Reactive text rides the `Bound`/`JsBinding`
//! machinery, which is orthogonal to per-range styling — a component
//! that needs live styled text rebuilds the node (e.g. under
//! `switch`), the same recipe every structural change uses.

use crate::style::{Color, FontFamily, FontStyle, FontWeight, Length, Tokenized};

/// One run of a styled text node: a string plus an optional style
/// delta. `style: None` renders with the node's own (paragraph)
/// style, exactly as if the run were part of a plain text node.
#[derive(Clone, Debug)]
pub struct TextRun {
    pub text: String,
    pub style: Option<TextRunStyle>,
}

impl TextRun {
    /// A run rendered entirely with the paragraph's style.
    pub fn plain(text: impl Into<String>) -> Self {
        Self { text: text.into(), style: None }
    }

    /// A run with per-range style deltas layered over the paragraph
    /// style.
    pub fn styled(text: impl Into<String>, style: TextRunStyle) -> Self {
        Self { text: text.into(), style: Some(style) }
    }
}

/// Per-run style deltas. Every field is optional; `None` inherits the
/// text node's own resolved style. This is the *entire* per-run
/// vocabulary — see the module docs for why it stays narrow.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TextRunStyle {
    pub font_family: Option<FontFamily>,
    pub font_weight: Option<FontWeight>,
    pub font_style: Option<FontStyle>,
    pub font_size: Option<Tokenized<Length>>,
    pub color: Option<Tokenized<Color>>,
    pub background: Option<Tokenized<Color>>,
    pub underline: Option<RunUnderline>,
}

impl TextRunStyle {
    /// Does this style change anything? Backends may skip attribute
    /// work for a run whose style is present but empty.
    pub fn is_empty(&self) -> bool {
        self.font_family.is_none()
            && self.font_weight.is_none()
            && self.font_style.is_none()
            && self.font_size.is_none()
            && self.color.is_none()
            && self.background.is_none()
            && self.underline.is_none()
    }
}

/// A per-run underline: line style plus an optional dedicated color.
///
/// `color: None` draws the line in the run's own text color, which is
/// what a "this is a link" underline wants; an explicit color is what
/// diagnostics want (a red mark under otherwise normally-colored text).
/// Tokenized like the other colors, so a themed underline swaps with
/// the palette (web emits `var(--token)`; native re-realizes through
/// the theme cohort).
#[derive(Clone, Debug, PartialEq)]
pub struct RunUnderline {
    pub style: UnderlineStyle,
    pub color: Option<Tokenized<Color>>,
}

impl RunUnderline {
    /// A solid underline in the run's own text color.
    pub fn solid() -> Self {
        Self { style: UnderlineStyle::Solid, color: None }
    }

    /// Builder: give the underline its own color.
    pub fn colored(mut self, color: Tokenized<Color>) -> Self {
        self.color = Some(color);
        self
    }
}

/// Underline line styles.
///
/// Three patterns, chosen because every backend's text engine draws all
/// three natively: CSS `text-decoration-style` on web/SSR, the
/// `NSUnderlineStylePattern*` bits on Apple, the framework's own span on
/// Android, and tiled rects off the existing run geometry on the GPU
/// backend.
///
/// Wavy is deliberately absent. Apple's public `NSUnderlineStyle` has no
/// wavy pattern (the spell-check squiggle comes from a private
/// spelling-state attribute, not a line style), so shipping it would
/// mean either custom glyph-run drawing on two toolkits or a squiggle on
/// web that silently flattens to a straight line on native — a style
/// that means something different per platform is exactly what
/// CLAUDE.md §7 forbids.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum UnderlineStyle {
    #[default]
    Solid,
    Dotted,
    Dashed,
}

/// Underline geometry, as ratios of the run's font size.
///
/// Web and Apple get their underline drawn by the toolkit and use its
/// own metrics. The two backends that must DRAW the line themselves —
/// the GPU renderer (cosmic-text has no underline) and Android (see
/// `RustUnderlineSpan.kt`, whose Kotlin `companion object` mirrors these
/// values) — share these so the mark comes out the same thickness and
/// pattern on both instead of each picking its own pixel constants.
/// Ratios rather than pixels so the line tracks the text: a fixed 1px
/// dash is a hairline at 24px text and a solid smear at 8px.
pub mod underline_geometry {
    /// Line thickness as a fraction of font size (floored at 1px by the
    /// caller). ~1px at a 14px body size, which is what the platform
    /// underlines draw.
    pub const THICKNESS_RATIO: f32 = 0.07;
    /// Distance from the baseline down to the line's top edge.
    pub const BASELINE_GAP_RATIO: f32 = 0.12;
    /// Dot: on-length and off-length, in multiples of the thickness.
    pub const DOT_ON: f32 = 1.0;
    pub const DOT_OFF: f32 = 1.5;
    /// Dash: on-length and off-length, in multiples of the thickness.
    pub const DASH_ON: f32 = 3.0;
    pub const DASH_OFF: f32 = 2.5;

    /// The `(on, off)` dash lengths in px for a style at a given
    /// thickness. `Solid` reports `None` — one unbroken run.
    pub fn dash_lengths(style: super::UnderlineStyle, thickness: f32) -> Option<(f32, f32)> {
        match style {
            super::UnderlineStyle::Solid => None,
            super::UnderlineStyle::Dotted => Some((DOT_ON * thickness, DOT_OFF * thickness)),
            super::UnderlineStyle::Dashed => Some((DASH_ON * thickness, DASH_OFF * thickness)),
        }
    }
}

/// The concatenated plain text of a run list — the graceful default
/// for backends without a styled-text realization (`Backend`'s
/// default `create_styled_text` lowers to `create_text` with this),
/// and the value robot/introspection surfaces report for the node.
/// Same words on every platform; styling is the only thing that
/// degrades.
pub fn plain_text_of(runs: &[TextRun]) -> String {
    let mut out = String::with_capacity(runs.iter().map(|r| r.text.len()).sum());
    for r in runs {
        out.push_str(&r.text);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_concatenates_all_runs() {
        let runs = vec![
            TextRun::plain("the "),
            TextRun::styled("ui!", TextRunStyle { font_weight: Some(FontWeight::Bold), ..Default::default() }),
            TextRun::plain(" macro"),
        ];
        assert_eq!(plain_text_of(&runs), "the ui! macro");
    }

    #[test]
    fn empty_style_reports_empty() {
        assert!(TextRunStyle::default().is_empty());
        assert!(!TextRunStyle { font_weight: Some(FontWeight::Bold), ..Default::default() }.is_empty());
    }

    /// Every field must count towards `is_empty`, or a backend skips the
    /// attribute work for a run that genuinely carries style — the bug
    /// shape that hid `underline`/`font_style` when they were added.
    #[test]
    fn regression_every_field_defeats_is_empty() {
        let cases: Vec<TextRunStyle> = vec![
            TextRunStyle { font_family: Some(FontFamily::System("Menlo".into())), ..Default::default() },
            TextRunStyle { font_weight: Some(FontWeight::Bold), ..Default::default() },
            TextRunStyle { font_style: Some(FontStyle::Italic), ..Default::default() },
            TextRunStyle { font_size: Some(Tokenized::Literal(Length::Px(11.0))), ..Default::default() },
            TextRunStyle { color: Some(Tokenized::Literal(Color("#f00".into()))), ..Default::default() },
            TextRunStyle { background: Some(Tokenized::Literal(Color("#eee".into()))), ..Default::default() },
            TextRunStyle { underline: Some(RunUnderline::solid()), ..Default::default() },
        ];
        for (i, style) in cases.iter().enumerate() {
            assert!(!style.is_empty(), "field {i} does not defeat is_empty: {style:?}");
        }
    }

    #[test]
    fn underline_defaults_to_solid_and_inherits_the_text_color() {
        let u = RunUnderline::solid();
        assert_eq!(u.style, UnderlineStyle::Solid);
        assert!(u.color.is_none(), "no color means the run's own text color");
        let colored = RunUnderline::solid().colored(Tokenized::Literal(Color("#c00".into())));
        assert_eq!(colored.color, Some(Tokenized::Literal(Color("#c00".into()))));
    }
}
