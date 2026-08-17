//! The decoration model — the primitive's contract with whatever
//! produces the styling.
//!
//! A [`Decoration`] is a **byte range into the editor's current value**
//! plus a style delta. That is the entire vocabulary: the primitive
//! never tokenizes, never looks at the text, never knows what a
//! "keyword" or a "string literal" is. A tree-sitter grammar, a regex
//! sweep, a compiler's diagnostic list and a hand-written matcher all
//! emit the same thing — ranges — so any of them plugs in without the
//! primitive growing a notion of "language".
//!
//! Byte ranges (not char indices, not line/column) because that is what
//! Rust text tooling already speaks: `str::find`, `regex::Match::range`,
//! `tree_sitter::Node::byte_range` and rustc's `Span` are all byte
//! offsets into the source. Converting to each backend's index space
//! (UTF-16 on Apple and Android, DOM text offsets on web) is the
//! handler's job, not the author's.
//!
//! ## Layering, not partitioning
//!
//! Decorations may overlap freely and are applied in list order: a
//! later decoration's `Some` fields win over an earlier one's, field by
//! field. This is what makes the two producers compose without either
//! knowing about the other — a syntax highlighter emits colors for the
//! whole buffer, a diagnostics pass emits red underlines over some of
//! the same ranges, and the underline lands on top *without* clearing
//! the keyword's color. A partitioning model (each byte belongs to one
//! decoration) can't express that, and would force the author to merge
//! the two lists themselves.
//!
//! [`flatten`] turns the overlapping list into the non-overlapping,
//! gap-free run list every backend's attributed-text mechanism wants.

use std::collections::BTreeSet;
use std::ops::Range;

use runtime_shared::{Color, FontStyle, FontWeight};

/// One decorated range of the editor's value.
///
/// `range` is in **bytes**, into the value the editor currently holds.
/// Out-of-bounds and mid-character ranges are not an error — see
/// [`flatten`] for how they're normalized — because a decoration
/// producer may legitimately lag the buffer by a frame (an async
/// diagnostics pass) and a panic there would take the app down for a
/// keystroke.
#[derive(Clone, Debug, PartialEq)]
pub struct Decoration {
    /// Byte range into the editor's value.
    pub range: Range<usize>,
    /// The style delta applied to that range.
    pub style: DecorationStyle,
}

impl Decoration {
    /// A decoration over `range` with `style`.
    pub fn new(range: Range<usize>, style: DecorationStyle) -> Self {
        Self { range, style }
    }

    /// The common case: color a range and change nothing else.
    pub fn color(range: Range<usize>, color: impl Into<String>) -> Self {
        Self::new(range, DecorationStyle::default().with_color(color))
    }

    /// The other common case: underline a range (a diagnostic) without
    /// touching its color.
    pub fn underline(range: Range<usize>, underline: Underline) -> Self {
        Self::new(range, DecorationStyle::default().with_underline(underline))
    }
}

/// Per-range style deltas. Every field is optional; `None` inherits
/// whatever the editor's own resolved text style is.
///
/// This is the *entire* per-range vocabulary, and it is deliberately
/// the intersection of what every backend's editable attributed-text
/// mechanism can express uniformly (CLAUDE.md §7): CSS declarations on
/// web/SSR, `NSAttributedString` attributes on Apple, `Spannable` spans
/// on Android. No padding, no corner radius, no per-range font size —
/// a code editor's rows have to stay on a single baseline grid or the
/// caret drifts away from the glyphs it is supposed to sit between.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DecorationStyle {
    /// Foreground (glyph) color.
    pub color: Option<Color>,
    /// Background color painted behind the range's glyphs.
    pub background: Option<Color>,
    /// Weight delta — the editor's own weight when `None`.
    pub font_weight: Option<FontWeight>,
    /// Italic/normal delta.
    pub font_style: Option<FontStyle>,
    /// Underline, with its own line style and (optionally) its own
    /// color, so an error squiggle can be red under blue text.
    pub underline: Option<Underline>,
}

impl DecorationStyle {
    /// Builder: set the foreground color.
    pub fn with_color(mut self, color: impl Into<String>) -> Self {
        self.color = Some(Color(color.into()));
        self
    }

    /// Builder: set the background color.
    pub fn with_background(mut self, color: impl Into<String>) -> Self {
        self.background = Some(Color(color.into()));
        self
    }

    /// Builder: set the font weight.
    pub fn with_weight(mut self, weight: FontWeight) -> Self {
        self.font_weight = Some(weight);
        self
    }

    /// Builder: make the range italic.
    pub fn italic(mut self) -> Self {
        self.font_style = Some(FontStyle::Italic);
        self
    }

    /// Builder: set the underline.
    pub fn with_underline(mut self, underline: Underline) -> Self {
        self.underline = Some(underline);
        self
    }

    /// Does this delta change anything? Backends skip attribute work
    /// for an empty style, and [`flatten`] uses it to keep undecorated
    /// gaps cheap.
    pub fn is_empty(&self) -> bool {
        self.color.is_none()
            && self.background.is_none()
            && self.font_weight.is_none()
            && self.font_style.is_none()
            && self.underline.is_none()
    }

    /// Layer `over` on top of `self`: every field `over` specifies wins,
    /// every field it leaves `None` keeps `self`'s value.
    ///
    /// This is the whole overlap semantic. Field-wise (rather than
    /// whole-style) replacement is what lets a diagnostics underline sit
    /// on a syntax-colored token without erasing the color.
    pub fn layer(&mut self, over: &DecorationStyle) {
        if let Some(v) = &over.color {
            self.color = Some(v.clone());
        }
        if let Some(v) = &over.background {
            self.background = Some(v.clone());
        }
        if let Some(v) = over.font_weight {
            self.font_weight = Some(v);
        }
        if let Some(v) = over.font_style {
            self.font_style = Some(v);
        }
        if let Some(v) = &over.underline {
            self.underline = Some(v.clone());
        }
    }
}

/// An underline: line style plus an optional dedicated color.
///
/// `color: None` draws the underline in the range's own text color
/// (the editor's default when the decoration sets no `color`), which is
/// what a "this identifier is a link" underline wants. An explicit
/// color is what diagnostics want.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Underline {
    /// Line style.
    pub style: UnderlineStyle,
    /// Line color; `None` follows the text color.
    pub color: Option<Color>,
}

impl Underline {
    /// A solid underline in the text's own color.
    pub fn solid() -> Self {
        Self { style: UnderlineStyle::Solid, color: None }
    }

    /// A dotted underline in the text's own color — the conventional
    /// "warning" mark.
    pub fn dotted() -> Self {
        Self { style: UnderlineStyle::Dotted, color: None }
    }

    /// A dashed underline in the text's own color.
    pub fn dashed() -> Self {
        Self { style: UnderlineStyle::Dashed, color: None }
    }

    /// Builder: give the underline its own color.
    pub fn colored(mut self, color: impl Into<String>) -> Self {
        self.color = Some(Color(color.into()));
        self
    }
}

/// Underline line styles — re-exported from the framework's styled-text
/// substrate rather than redefined here, so a decoration and a
/// read-only styled run mean exactly the same thing by the same type.
/// See [`runtime_shared::UnderlineStyle`] for why the set is three
/// patterns and why wavy is absent.
pub use runtime_shared::UnderlineStyle;

/// One run of the flattened text: a byte range and the fully-merged
/// style that applies to every byte in it. Runs tile the value exactly
/// — consecutive, gap-free, covering `0..text.len()` — so a backend can
/// walk them once and emit its attributed string without consulting the
/// original text length.
#[derive(Clone, Debug, PartialEq)]
pub struct DecoratedRun {
    /// Byte range into the value.
    pub range: Range<usize>,
    /// Merged style; empty for undecorated stretches.
    pub style: DecorationStyle,
}

impl DecoratedRun {
    /// The run's slice of `text`. Always a valid slice for the `text`
    /// the run list was flattened against — [`flatten`] snaps every
    /// boundary to a character boundary.
    pub fn slice<'a>(&self, text: &'a str) -> &'a str {
        &text[self.range.clone()]
    }
}

/// Flatten an overlapping decoration list into consecutive, gap-free
/// runs covering all of `text`.
///
/// Normalization, in order:
///
/// 1. **Clamp** each range to `0..text.len()`. A decoration produced
///    against an older, longer buffer (async diagnostics arriving after
///    the user hit backspace) is truncated, not dropped and not a
///    panic.
/// 2. **Snap** to character boundaries: `start` moves down, `end` moves
///    up, so a range that lands inside a multi-byte character covers
///    the whole character rather than producing an invalid slice.
/// 3. **Drop** ranges that are empty or inverted after clamping.
/// 4. **Layer** overlapping decorations in list order via
///    [`DecorationStyle::layer`].
/// 5. **Coalesce** adjacent runs whose merged styles are equal, so an
///    unchanged buffer produces the shortest run list the backends can
///    diff against.
///
/// The sweep is O(b log b + b·k) for `b` boundaries and `k` decorations
/// simultaneously active at a point — for a syntax highlighter `k` is
/// 1, and 2 where a diagnostic overlaps a token, so this stays linear
/// in practice even when the buffer is retokenized on every keystroke.
pub fn flatten(text: &str, decorations: &[Decoration]) -> Vec<DecoratedRun> {
    let len = text.len();
    if len == 0 {
        return Vec::new();
    }

    // 1–3: normalize into (start, end, index) keeping the original list
    // order in `index`, which is what defines layering precedence.
    let mut spans: Vec<(usize, usize, usize)> = Vec::with_capacity(decorations.len());
    for (idx, deco) in decorations.iter().enumerate() {
        let start = snap_down(text, deco.range.start.min(len));
        let end = snap_up(text, deco.range.end.min(len));
        if start < end && !deco.style.is_empty() {
            spans.push((start, end, idx));
        }
    }

    if spans.is_empty() {
        return vec![DecoratedRun { range: 0..len, style: DecorationStyle::default() }];
    }

    // Boundaries: every span edge plus the two document edges.
    let mut bounds: Vec<usize> = Vec::with_capacity(spans.len() * 2 + 2);
    bounds.push(0);
    bounds.push(len);
    for (s, e, _) in &spans {
        bounds.push(*s);
        bounds.push(*e);
    }
    bounds.sort_unstable();
    bounds.dedup();

    // Sweep: `by_start` / `by_end` are the same spans ordered by each
    // edge, walked with two cursors so activation and deactivation are
    // both O(1) amortized per boundary.
    let mut by_start: Vec<usize> = (0..spans.len()).collect();
    by_start.sort_unstable_by_key(|&i| spans[i].0);
    let mut by_end: Vec<usize> = (0..spans.len()).collect();
    by_end.sort_unstable_by_key(|&i| spans[i].1);
    let (mut si, mut ei) = (0usize, 0usize);

    // Active spans, ordered by ORIGINAL list index so the merge below
    // applies them in author order regardless of sweep order.
    let mut active: BTreeSet<usize> = BTreeSet::new();

    let mut runs: Vec<DecoratedRun> = Vec::with_capacity(bounds.len());
    for w in bounds.windows(2) {
        let (from, to) = (w[0], w[1]);
        while si < by_start.len() && spans[by_start[si]].0 <= from {
            active.insert(spans[by_start[si]].2);
            si += 1;
        }
        while ei < by_end.len() && spans[by_end[ei]].1 <= from {
            active.remove(&spans[by_end[ei]].2);
            ei += 1;
        }

        let mut style = DecorationStyle::default();
        for &idx in &active {
            style.layer(&decorations[idx].style);
        }

        // 5: coalesce with the previous run when the style is identical.
        match runs.last_mut() {
            Some(prev) if prev.style == style => prev.range.end = to,
            _ => runs.push(DecoratedRun { range: from..to, style }),
        }
    }
    runs
}

/// Move `i` down to the nearest character boundary.
fn snap_down(text: &str, mut i: usize) -> usize {
    while i > 0 && !text.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Move `i` up to the nearest character boundary.
fn snap_up(text: &str, mut i: usize) -> usize {
    let len = text.len();
    while i < len && !text.is_char_boundary(i) {
        i += 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    fn red() -> DecorationStyle {
        DecorationStyle::default().with_color("#f00")
    }
    fn blue() -> DecorationStyle {
        DecorationStyle::default().with_color("#00f")
    }
    fn squiggle() -> DecorationStyle {
        DecorationStyle::default().with_underline(Underline::dotted().colored("#c00"))
    }

    /// The run list must tile the whole value with no gaps and no
    /// overlaps — backends walk it as the complete document.
    fn assert_tiles(text: &str, runs: &[DecoratedRun]) {
        let mut at = 0;
        for r in runs {
            assert_eq!(r.range.start, at, "gap or overlap before {:?}", r.range);
            assert!(r.range.start < r.range.end, "empty run {:?}", r.range);
            at = r.range.end;
        }
        assert_eq!(at, text.len(), "runs stop short of the value");
    }

    #[test]
    fn undecorated_text_is_one_plain_run() {
        let runs = flatten("fn main() {}", &[]);
        assert_eq!(runs.len(), 1);
        assert!(runs[0].style.is_empty());
        assert_tiles("fn main() {}", &runs);
    }

    #[test]
    fn empty_text_produces_no_runs() {
        assert!(flatten("", &[Decoration::color(0..4, "#f00")]).is_empty());
    }

    #[test]
    fn gaps_between_decorations_become_plain_runs() {
        let text = "fn main";
        let runs = flatten(text, &[Decoration::color(0..2, "#f00")]);
        assert_tiles(text, &runs);
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].slice(text), "fn");
        assert_eq!(runs[0].style.color, Some(Color("#f00".into())));
        assert_eq!(runs[1].slice(text), " main");
        assert!(runs[1].style.is_empty());
    }

    #[test]
    fn later_decorations_layer_over_earlier_ones_field_by_field() {
        // The whole point of the model: a diagnostic underline over a
        // syntax-colored token keeps the token's color.
        let text = "let x";
        let runs = flatten(
            text,
            &[
                Decoration::new(0..3, red()),
                Decoration::new(0..3, squiggle()),
            ],
        );
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].slice(text), "let");
        assert_eq!(
            runs[0].style.color,
            Some(Color("#f00".into())),
            "the underline must not clear the syntax color"
        );
        assert_eq!(runs[0].style.underline, Some(Underline::dotted().colored("#c00")));
    }

    #[test]
    fn same_field_conflicts_resolve_to_the_later_decoration() {
        let text = "abcd";
        let runs = flatten(text, &[Decoration::new(0..4, red()), Decoration::new(0..4, blue())]);
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].style.color, Some(Color("#00f".into())));
    }

    #[test]
    fn partial_overlap_splits_into_three_runs() {
        let text = "abcdef";
        let runs = flatten(text, &[Decoration::new(0..4, red()), Decoration::new(2..6, squiggle())]);
        assert_tiles(text, &runs);
        assert_eq!(runs.len(), 3);
        // 0..2 red only, 2..4 red + underline, 4..6 underline only.
        assert_eq!(runs[0].slice(text), "ab");
        assert!(runs[0].style.underline.is_none());
        assert_eq!(runs[1].slice(text), "cd");
        assert_eq!(runs[1].style.color, Some(Color("#f00".into())));
        assert!(runs[1].style.underline.is_some());
        assert_eq!(runs[2].slice(text), "ef");
        assert!(runs[2].style.color.is_none());
    }

    #[test]
    fn adjacent_equal_styles_coalesce() {
        let text = "abcdef";
        let runs = flatten(text, &[Decoration::new(0..3, red()), Decoration::new(3..6, red())]);
        assert_eq!(runs.len(), 1, "two touching identical decorations are one run");
        assert_eq!(runs[0].range, 0..6);
    }

    /// A decoration producer that lags the buffer (async diagnostics
    /// landing after a backspace) must truncate, never panic.
    #[test]
    fn regression_stale_ranges_past_the_end_are_clamped_not_panicked() {
        let text = "fn";
        let runs = flatten(text, &[Decoration::color(0..999, "#f00")]);
        assert_tiles(text, &runs);
        assert_eq!(runs[0].range, 0..2);
    }

    #[test]
    fn ranges_wholly_past_the_end_are_dropped() {
        let text = "fn";
        let runs = flatten(text, &[Decoration::color(50..60, "#f00")]);
        assert_eq!(runs.len(), 1);
        assert!(runs[0].style.is_empty());
    }

    #[test]
    fn inverted_and_empty_ranges_are_dropped() {
        let text = "abc";
        #[allow(clippy::reversed_empty_ranges)]
        let runs = flatten(
            text,
            &[Decoration::color(2..1, "#f00"), Decoration::color(1..1, "#0f0")],
        );
        assert_eq!(runs.len(), 1);
        assert!(runs[0].style.is_empty());
    }

    /// Mid-character byte offsets must widen to whole characters —
    /// slicing a `String` at a non-boundary is a panic, and a producer
    /// working in char indices instead of bytes hits this immediately.
    #[test]
    fn regression_mid_character_ranges_snap_to_character_boundaries() {
        let text = "héllo"; // 'é' occupies bytes 1..3
        let runs = flatten(text, &[Decoration::color(2..4, "#f00")]);
        assert_tiles(text, &runs);
        // Snapped outward to 1..4 — slicing must not panic.
        let decorated: Vec<&str> = runs
            .iter()
            .filter(|r| !r.style.is_empty())
            .map(|r| r.slice(text))
            .collect();
        assert_eq!(decorated, vec!["él"]);
    }

    #[test]
    fn empty_styles_do_not_split_runs() {
        // A decoration that changes nothing must not fragment the run
        // list — backends pay per run.
        let text = "abcdef";
        let runs = flatten(text, &[Decoration::new(2..4, DecorationStyle::default())]);
        assert_eq!(runs.len(), 1);
    }

    #[test]
    fn nested_decorations_restore_the_outer_style_after_the_inner_one_ends() {
        let text = "abcdef";
        let runs = flatten(text, &[Decoration::new(0..6, red()), Decoration::new(2..4, blue())]);
        assert_tiles(text, &runs);
        assert_eq!(runs.len(), 3);
        assert_eq!(runs[0].style.color, Some(Color("#f00".into())));
        assert_eq!(runs[1].style.color, Some(Color("#00f".into())));
        assert_eq!(
            runs[2].style.color,
            Some(Color("#f00".into())),
            "the outer decoration must resume after the inner one closes"
        );
    }

    #[test]
    fn out_of_order_decorations_still_tile_correctly() {
        // Producers are not required to emit ranges in order.
        let text = "abcdefgh";
        let runs = flatten(
            text,
            &[
                Decoration::color(6..8, "#f00"),
                Decoration::color(0..2, "#0f0"),
                Decoration::color(3..5, "#00f"),
            ],
        );
        assert_tiles(text, &runs);
        assert_eq!(runs.len(), 5);
    }
}
