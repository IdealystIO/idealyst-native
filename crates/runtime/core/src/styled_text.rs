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
//! uniformly (font family/weight/size, foreground, background). No
//! padding, no corner radius — a chip with rounded corners can't be
//! expressed by `NSAttributedString`/`Spannable` without custom
//! drawing, and a web-only nicety would violate the convergence rule.
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

use crate::style::{Color, FontFamily, FontWeight, Length, Tokenized};

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
#[derive(Clone, Debug, Default)]
pub struct TextRunStyle {
    pub font_family: Option<FontFamily>,
    pub font_weight: Option<FontWeight>,
    pub font_size: Option<Tokenized<Length>>,
    pub color: Option<Tokenized<Color>>,
    pub background: Option<Tokenized<Color>>,
}

impl TextRunStyle {
    /// Does this style change anything? Backends may skip attribute
    /// work for a run whose style is present but empty.
    pub fn is_empty(&self) -> bool {
        self.font_family.is_none()
            && self.font_weight.is_none()
            && self.font_size.is_none()
            && self.color.is_none()
            && self.background.is_none()
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
}
