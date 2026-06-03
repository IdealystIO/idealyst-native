//! Web handler for the `markdown` external.
//!
//! Builds **semantic DOM** through the `Backend` trait (so SSR +
//! hydration share the same shape, codeblock-style): a `<div>` column of
//! `<h1>`/`<p>`/`<pre>`/`<blockquote>`/`<hr>` blocks, lists as indented
//! marker rows, and per-run inline styling. Unlike the native backends —
//! which collapse the whole document into ONE styled-text node for perf
//! — DOM layout is cheap and the semantic tree is accessible, so web
//! keeps real elements. These are still raw backend nodes (not framework
//! reactive `Element`s), so there's no per-node reactive overhead.

use std::rc::Rc;

use runtime_core::accessibility::AccessibilityProps;
use runtime_core::{
    Backend, Color, FlexDirection, FontFamily, FontStyle, FontWeight, Length, StyleRules,
    Tokenized,
};

use crate::ir::{MarkdownDoc, MdBlock, MdRun, MdTheme};

/// Vertical gap between blocks (px).
const BLOCK_GAP: f32 = 12.0;
/// Indent per list nesting level (px).
const LIST_INDENT: f32 = 20.0;

pub(crate) fn build<B: Backend>(doc: &Rc<MarkdownDoc>, backend: &mut B) -> B::Node {
    let a11y = AccessibilityProps::default();
    let theme = &doc.theme;

    let root = backend.create_element("div");
    let mut rs = StyleRules::default();
    rs.flex_direction = Some(FlexDirection::Column);
    rs.gap = Some(Tokenized::Literal(Length::Px(BLOCK_GAP)));
    rs.color = Some(Tokenized::Literal(Color(theme.text.clone())));
    rs.font_size = Some(Tokenized::Literal(Length::Px(theme.base_size)));
    // NB: `line_height` here is absolute px in this framework, not a
    // unitless multiplier — leave it unset so each element gets the
    // browser's proportional `normal` line height (a fixed px value
    // would collapse multi-size text: too tight for headings).
    backend.apply_style(&root, &Rc::new(rs));

    let mut root = root;
    for block in &doc.blocks {
        let node = build_block(backend, block, theme, &a11y);
        backend.insert(&mut root, node);
    }
    root
}

fn build_block<B: Backend>(
    backend: &mut B,
    block: &MdBlock,
    theme: &MdTheme,
    a11y: &AccessibilityProps,
) -> B::Node {
    match block {
        MdBlock::Heading { level, runs } => {
            let tag = format!("h{}", level.clamp(&1, &6));
            let node = backend.create_element(&tag);
            let mut rs = StyleRules::default();
            rs.color = Some(Tokenized::Literal(Color(theme.heading.clone())));
            rs.font_size = Some(Tokenized::Literal(Length::Px(theme.heading_size(*level))));
            rs.font_weight = Some(FontWeight::Bold);
            // Zero the UA heading margins; the column `gap` owns spacing.
            zero_margins(&mut rs);
            backend.apply_style(&node, &Rc::new(rs));
            append_runs(backend, &node, runs, theme, &theme.heading, a11y);
            node
        }
        MdBlock::Paragraph { runs } => {
            let node = backend.create_element("p");
            let mut rs = StyleRules::default();
            zero_margins(&mut rs);
            backend.apply_style(&node, &Rc::new(rs));
            append_runs(backend, &node, runs, theme, &theme.text, a11y);
            node
        }
        MdBlock::CodeBlock { text } => {
            let node = backend.create_element("pre");
            let mut rs = StyleRules::default();
            rs.background = Some(Tokenized::Literal(Color(theme.code_bg.clone())));
            rs.color = Some(Tokenized::Literal(Color(theme.code_fg.clone())));
            rs.font_family = Some(mono_family(theme));
            rs.font_size = Some(Tokenized::Literal(Length::Px(theme.base_size * 0.9)));
            pad_all(&mut rs, 12.0);
            rs.border_top_left_radius = Some(Tokenized::Literal(Length::Px(6.0)));
            rs.border_top_right_radius = Some(Tokenized::Literal(Length::Px(6.0)));
            rs.border_bottom_left_radius = Some(Tokenized::Literal(Length::Px(6.0)));
            rs.border_bottom_right_radius = Some(Tokenized::Literal(Length::Px(6.0)));
            zero_margins(&mut rs);
            backend.apply_style(&node, &Rc::new(rs));
            let mut node = node;
            let txt = backend.create_text(text, a11y);
            backend.insert(&mut node, txt);
            node
        }
        MdBlock::Quote { runs } => {
            let node = backend.create_element("blockquote");
            let mut rs = StyleRules::default();
            rs.color = Some(Tokenized::Literal(Color(theme.quote_fg.clone())));
            rs.font_style = Some(FontStyle::Italic);
            rs.border_left_width = Some(Tokenized::Literal(3.0));
            rs.border_left_color = Some(Tokenized::Literal(Color(theme.muted.clone())));
            rs.padding_left = Some(Tokenized::Literal(Length::Px(12.0)));
            zero_margins(&mut rs);
            backend.apply_style(&node, &Rc::new(rs));
            append_runs(backend, &node, runs, theme, &theme.quote_fg, a11y);
            node
        }
        MdBlock::List { ordered: _, items } => {
            let root = backend.create_element("div");
            let mut rs = StyleRules::default();
            rs.flex_direction = Some(FlexDirection::Column);
            rs.gap = Some(Tokenized::Literal(Length::Px(2.0)));
            backend.apply_style(&root, &Rc::new(rs));
            let mut root = root;
            for item in items {
                let row = backend.create_element("div");
                let mut row_rs = StyleRules::default();
                row_rs.flex_direction = Some(FlexDirection::Row);
                row_rs.margin_left =
                    Some(Tokenized::Literal(Length::Px(item.depth as f32 * LIST_INDENT)));
                backend.apply_style(&row, &Rc::new(row_rs));
                let mut row = row;

                let marker = backend.create_text(&format!("{}  ", item.marker), a11y);
                let mut m_rs = StyleRules::default();
                m_rs.color = Some(Tokenized::Literal(Color(theme.muted.clone())));
                backend.apply_style(&marker, &Rc::new(m_rs));
                backend.insert(&mut row, marker);

                let content = backend.create_element("div");
                append_runs(backend, &content, &item.runs, theme, &theme.text, a11y);
                backend.insert(&mut row, content);

                backend.insert(&mut root, row);
            }
            root
        }
        MdBlock::Rule => {
            let node = backend.create_element("hr");
            let mut rs = StyleRules::default();
            rs.border_top_width = Some(Tokenized::Literal(1.0));
            rs.border_top_color = Some(Tokenized::Literal(Color(theme.muted.clone())));
            zero_margins(&mut rs);
            backend.apply_style(&node, &Rc::new(rs));
            node
        }
    }
}

/// Append the inline runs of a block as styled text children.
fn append_runs<B: Backend>(
    backend: &mut B,
    parent: &B::Node,
    runs: &[MdRun],
    theme: &MdTheme,
    base_color: &str,
    a11y: &AccessibilityProps,
) {
    let mut parent = parent.clone();
    for run in runs {
        let node = backend.create_text(&run.text, a11y);
        let mut rs = StyleRules::default();
        let color = if run.link.is_some() {
            &theme.link
        } else if run.code {
            &theme.code_fg
        } else {
            base_color
        };
        rs.color = Some(Tokenized::Literal(Color(color.to_string())));
        if run.bold {
            rs.font_weight = Some(FontWeight::Bold);
        }
        if run.italic {
            rs.font_style = Some(FontStyle::Italic);
        }
        if run.strike {
            rs.strikethrough = Some(true);
        }
        if run.link.is_some() {
            rs.underline = Some(true);
        }
        if run.code {
            rs.font_family = Some(mono_family(theme));
            rs.background = Some(Tokenized::Literal(Color(theme.code_bg.clone())));
            rs.padding_left = Some(Tokenized::Literal(Length::Px(4.0)));
            rs.padding_right = Some(Tokenized::Literal(Length::Px(4.0)));
        }
        backend.apply_style(&node, &Rc::new(rs));
        backend.insert(&mut parent, node);
    }
}

fn mono_family(theme: &MdTheme) -> FontFamily {
    FontFamily::System(
        theme
            .mono_family
            .clone()
            .unwrap_or_else(|| "ui-monospace, SFMono-Regular, Menlo, monospace".to_string()),
    )
}

fn zero_margins(rs: &mut StyleRules) {
    rs.margin_top = Some(Tokenized::Literal(Length::Px(0.0)));
    rs.margin_bottom = Some(Tokenized::Literal(Length::Px(0.0)));
}

fn pad_all(rs: &mut StyleRules, px: f32) {
    rs.padding_top = Some(Tokenized::Literal(Length::Px(px)));
    rs.padding_right = Some(Tokenized::Literal(Length::Px(px)));
    rs.padding_bottom = Some(Tokenized::Literal(Length::Px(px)));
    rs.padding_left = Some(Tokenized::Literal(Length::Px(px)));
}
