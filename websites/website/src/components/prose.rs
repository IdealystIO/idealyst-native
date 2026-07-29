//! `Prose` — paragraph text with inline-code rendering.
//!
//! Drop-in for `Typography` wherever body copy mentions code: a
//! backtick pair in `content` (`` "the `ui!` macro" ``) renders as a
//! monospace chip instead of literal backtick characters.
//!
//! Rendering goes through the framework's styled-text primitive
//! (`runtime_core::styled_text` + `TextRun`): the paragraph is ONE
//! text node whose runs carry per-range deltas, realized by each
//! backend's own text engine — nested `<span>`s on web/SSR,
//! `NSAttributedString` on iOS/macOS, `SpannableString` on Android,
//! cosmic-text rich spans on the GPU renderer. Mixed prose/code wraps
//! as a single paragraph on every platform; this component just
//! parses backticks and picks the chip style.
//!
//! The paragraph style is the same Typography-sheet application
//! `Typography` itself uses (kind/color axes), attached to the styled
//! text node — so a `Prose` with no backticks and a `Typography` are
//! visually identical, and the chip runs inherit the paragraph's
//! size/line-height/color from that base.

use runtime_core::{
    component, Color, Element, FontFamily, Reactive, StyleApplication, Tokenized,
};
// `styled_text` + the run types resolve on BOTH cores: old-core root
// re-exports, or the glue facade's `styled_text(runs)` mirror over the
// vocabulary text builder (same old-core `TextRun` type either way).
use runtime_core::{styled_text, TextRun, TextRunStyle};

use idea_ui::{
    installed_typography_sheet, Typography, TypographyKindRef, TypographyProps,
};

/// One run of a parsed paragraph: either plain prose or the inside of
/// a backtick pair.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Segment {
    Text(String),
    Code(String),
}

/// Split `input` on backtick pairs. An unmatched backtick (and
/// everything after it) stays literal text, and an empty pair
/// (` `` `) is dropped — so ordinary prose that happens to contain a
/// stray backtick never loses characters.
pub(crate) fn segments(input: &str) -> Vec<Segment> {
    let mut out = Vec::new();
    let mut rest = input;
    while let Some(open) = rest.find('`') {
        let Some(close_rel) = rest[open + 1..].find('`') else {
            // Unmatched backtick — the remainder (including the
            // backtick itself) is literal.
            break;
        };
        if open > 0 {
            out.push(Segment::Text(rest[..open].to_string()));
        }
        let code = &rest[open + 1..open + 1 + close_rel];
        if !code.is_empty() {
            out.push(Segment::Code(code.to_string()));
        }
        rest = &rest[open + 1 + close_rel + 1..];
    }
    if !rest.is_empty() {
        out.push(Segment::Text(rest.to_string()));
    }
    out
}

/// The inline-code chip deltas: mono stack + `color-surface-alt`
/// background. Colors are theme tokens, so chips track theme swaps on
/// every backend (web via `var()` cascade, native via the styled-text
/// cohort re-realization). Everything typographic (size, line-height,
/// color, weight) is deliberately ABSENT so the chip inherits the
/// paragraph's kind — one style works across Body, BodyLg, and any
/// other kind without restating the type scale.
fn code_run_style() -> TextRunStyle {
    TextRunStyle {
        font_family: Some(FontFamily::System(
            "ui-monospace, SFMono-Regular, Menlo, monospace".into(),
        )),
        background: Some(Tokenized::token(
            "color-surface-alt",
            Color("#f4eedb".into()),
        )),
        ..Default::default()
    }
}

// Mirrors the `Typography` props this site's body copy actually uses
// (`content` / `kind` / `muted`) so converting a call site is a tag
// rename. `#[props]` wraps the fields `Reactive<T>` with the same
// call-site coercions Typography gets (`kind = typography_kind::BodyLg`
// works unchanged).
#[runtime_core::props]
pub struct ProseProps {
    /// Paragraph text; backtick pairs mark inline code. When the
    /// content contains code, the value is read once at build time —
    /// the marketing site's prose is static copy. Backtick-free
    /// content passes through to `Typography` untouched, reactivity
    /// included.
    pub content: Reactive<String>,
    /// Typographic role of the paragraph, as in `Typography`.
    pub kind: TypographyKindRef,
    /// Muted text color, as in `Typography`.
    pub muted: bool,
}

impl Default for ProseProps {
    fn default() -> Self {
        Self {
            content: Reactive::Static(String::new()),
            kind: Reactive::Static(TypographyKindRef::default()),
            muted: Reactive::Static(false),
        }
    }
}

/// Paragraph text where backtick pairs render as inline code chips.
#[component]
pub fn Prose(props: &ProseProps) -> Element {
    let segs = segments(&props.content.get());
    if !segs.iter().any(|s| matches!(s, Segment::Code(_))) {
        // No inline code — behave exactly like Typography, keeping a
        // reactive `content` live.
        return Typography(&TypographyProps {
            content: props.content.clone(),
            kind: props.kind.clone(),
            muted: props.muted.clone(),
            ..Default::default()
        });
    }
    build(segs, props.kind.get(), props.muted.get())
}

/// Lower parsed segments to one styled-text node carrying the
/// paragraph's Typography-sheet style (same axes Typography uses).
fn build(segs: Vec<Segment>, kind: TypographyKindRef, muted: bool) -> Element {
    let runs: Vec<TextRun> = segs
        .into_iter()
        .map(|seg| match seg {
            Segment::Text(t) => TextRun::plain(t),
            Segment::Code(c) => TextRun::styled(c, code_run_style()),
        })
        .collect();

    let color_key = if muted { "muted" } else { "default" };
    let paragraph_style = StyleApplication::new(installed_typography_sheet())
        .with("kind", kind.key().to_string())
        .with("color", color_key.to_string());

    runtime_core::IntoElement::into_element(styled_text(runs).with_style(paragraph_style))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "old-core")]
    use idea_ui::{install_idea_theme, light_theme};
    #[cfg(feature = "old-core")]
    use runtime_core::{styled_text::plain_text_of, TextSource};

    fn text_seg(s: &str) -> Segment {
        Segment::Text(s.to_string())
    }
    fn code_seg(s: &str) -> Segment {
        Segment::Code(s.to_string())
    }

    #[test]
    fn segments_plain_text_is_one_run() {
        assert_eq!(segments("no code here"), vec![text_seg("no code here")]);
    }

    #[test]
    fn segments_splits_backtick_pairs() {
        assert_eq!(
            segments("the `ui!` macro and `Element` type"),
            vec![
                text_seg("the "),
                code_seg("ui!"),
                text_seg(" macro and "),
                code_seg("Element"),
                text_seg(" type"),
            ]
        );
    }

    #[test]
    fn segments_code_at_edges() {
        assert_eq!(
            segments("`lazy!` carves a subtree"),
            vec![code_seg("lazy!"), text_seg(" carves a subtree")]
        );
        assert_eq!(
            segments("goes through `Element::External`"),
            vec![text_seg("goes through "), code_seg("Element::External")]
        );
    }

    #[test]
    fn segments_unmatched_backtick_stays_literal() {
        assert_eq!(
            segments("a stray ` backtick"),
            vec![text_seg("a stray ` backtick")]
        );
        // A pair plus a stray: the pair parses, the stray stays.
        assert_eq!(
            segments("`code` then a stray ` here"),
            vec![code_seg("code"), text_seg(" then a stray ` here")]
        );
    }

    #[test]
    fn segments_empty_pair_is_dropped() {
        assert_eq!(segments("a `` b"), vec![text_seg("a "), text_seg(" b")]);
    }

    #[test]
    fn segments_empty_input() {
        assert_eq!(segments(""), Vec::<Segment>::new());
    }

    /// Backticked prose lowers to ONE styled-text node (every backend
    /// wraps it as a single paragraph) with the paragraph style
    /// attached and the code runs styled.
    ///
    /// Old-core-only: matches the old `Element::Text` enum shape (the
    /// new core's `Element::Item` payload is opaque; the styled-runs
    /// lowering there is pinned by the vocabulary's own text tests).
    #[cfg(feature = "old-core")]
    #[test]
    fn code_prose_lowers_to_styled_text_runs() {
        install_idea_theme(light_theme());
        let el = build(segments("the `ui!` macro"), TypographyKindRef::default(), false);
        let Element::Text { source: TextSource::Styled(runs), style, .. } = el else {
            panic!("prose with code renders one styled text node");
        };
        assert!(style.is_some(), "paragraph carries the typography style");
        assert_eq!(plain_text_of(&runs), "the ui! macro");
        assert_eq!(runs.len(), 3);
        assert!(runs[0].style.is_none() && runs[2].style.is_none());
        let chip = runs[1].style.as_ref().expect("code run is styled");
        assert!(chip.font_family.is_some(), "code run gets the mono stack");
        assert!(chip.background.is_some(), "code run gets the chip background");
    }

    /// Backtick-free content goes straight to Typography — identical
    /// output to a direct Typography call site.
    ///
    /// Old-core-only: matches the old `Element::Text` enum shape.
    #[cfg(feature = "old-core")]
    #[test]
    fn plain_content_delegates_to_typography() {
        install_idea_theme(light_theme());
        let props = ProseProps {
            content: Reactive::Static("no code at all".to_string()),
            ..Default::default()
        };
        let el = Prose(&props);
        let Element::Text { source: TextSource::Static(s), style, .. } = el else {
            panic!("plain prose renders one text node");
        };
        assert_eq!(s, "no code at all");
        assert!(style.is_some(), "Typography always attaches its style");
    }
}
