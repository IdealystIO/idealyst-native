//! Painted colored-runs leaf — the Windows realization of the
//! `codeblock` SDK's single-node contract.
//!
//! Every other backend renders a `code_block(...)` as ONE native rich
//! text widget (NSAttributedString label on iOS/macOS, SpannableString
//! TextView on Android, Pango AttrList label on GTK). This backend's
//! equivalent is a single painted node: the pre-tokenized `(text,
//! color)` runs are split into lines, each run's advance measured once
//! in the monospace font, and the scene painter draws them
//! left-to-right per line with per-run colors. No wrapping — code
//! lines keep their authored line structure (same as every sibling
//! handler; they pair no-wrap with horizontal scrolling), and the
//! author-styled outer view clips overflow.
//!
//! The pure line-splitting lives in [`split_lines`] so tests can pin
//! it without GDI: spans may contain embedded `\n` (tokenizers often
//! emit whitespace runs spanning line breaks), and a run's color must
//! survive the split.

use runtime_core::color::{parse_or, Rgba};
use runtime_core::Color;

use crate::font;

/// One measured, colored run on one line.
pub(crate) struct CodeRun {
    pub text16: Vec<u16>,
    pub width: f32,
    pub color: Rgba,
}

/// The painted visual for a colored-runs leaf.
pub(crate) struct CodeVisual {
    /// Runs grouped by line, in draw order.
    pub lines: Vec<Vec<CodeRun>>,
    /// Monospace font the runs are measured in and drawn with.
    pub font_key: font::FontKey,
    /// Line advance (the font's natural height).
    pub line_height: f32,
}

/// A span fragment after line splitting, pre-measurement.
pub(crate) struct RawRun {
    pub text: String,
    pub color: Rgba,
}

/// Split `(text, color)` spans into lines on `\n`, preserving each
/// fragment's color. Empty fragments between consecutive newlines
/// produce empty lines (a blank line in the source stays blank);
/// fragments never contain `\n` afterward.
pub(crate) fn split_lines(spans: &[(String, Color)]) -> Vec<Vec<RawRun>> {
    let mut lines: Vec<Vec<RawRun>> = vec![Vec::new()];
    for (text, color) in spans {
        let rgba = parse_or(&color.0, Rgba::BLACK);
        let mut first = true;
        for piece in text.split('\n') {
            if !first {
                lines.push(Vec::new());
            }
            first = false;
            if piece.is_empty() {
                continue;
            }
            lines
                .last_mut()
                .expect("lines starts non-empty")
                .push(RawRun { text: piece.to_string(), color: rgba });
        }
    }
    lines
}

/// Measure the split lines into a [`CodeVisual`] plus its intrinsic
/// pixel size `(max line width, line count × line height)`. `measure`
/// returns the advance width of a UTF-16 slice in the monospace font —
/// injected so tests can use synthetic metrics.
pub(crate) fn build_visual(
    spans: &[(String, Color)],
    font_key: font::FontKey,
    line_height: f32,
    mut measure: impl FnMut(&[u16]) -> f32,
) -> (CodeVisual, (i32, i32)) {
    let mut lines: Vec<Vec<CodeRun>> = Vec::new();
    let mut max_w = 0.0_f32;
    for raw_line in split_lines(spans) {
        let mut line = Vec::new();
        let mut w = 0.0_f32;
        for raw in raw_line {
            let text16: Vec<u16> = raw.text.encode_utf16().collect();
            let run_w = measure(&text16);
            w += run_w;
            line.push(CodeRun { text16, width: run_w, color: raw.color });
        }
        max_w = max_w.max(w);
        lines.push(line);
    }
    let h = (lines.len().max(1) as f32) * line_height;
    let visual = CodeVisual { lines, font_key, line_height };
    (visual, (max_w.ceil() as i32, h.ceil() as i32))
}

/// Build a visual measuring runs with GDI in `hfont` (one DC + font
/// selection for the whole build — same shape as `wrap::build_gdi`).
/// Also derives the real line height from the font ("Mg" sample),
/// falling back to `fallback_height`.
pub(crate) fn build_gdi(
    spans: &[(String, Color)],
    hfont: windows::Win32::Graphics::Gdi::HFONT,
    font_key: font::FontKey,
    fallback_height: f32,
) -> (CodeVisual, (i32, i32)) {
    use windows::Win32::Foundation::{HWND, SIZE};
    use windows::Win32::Graphics::Gdi::{
        GetDC, GetTextExtentPoint32W, ReleaseDC, SelectObject, HGDIOBJ,
    };
    unsafe {
        let dc = GetDC(HWND(std::ptr::null_mut()));
        if dc.is_invalid() {
            return build_visual(spans, font_key, fallback_height, |_| 0.0);
        }
        let prev = SelectObject(dc, HGDIOBJ(hfont.0));
        let mut line_height = fallback_height;
        let sample: Vec<u16> = "Mg".encode_utf16().collect();
        let mut size = SIZE::default();
        if GetTextExtentPoint32W(dc, &sample, &mut size).as_bool() {
            line_height = size.cy as f32;
        }
        let out = build_visual(spans, font_key, line_height, |s: &[u16]| {
            let mut sz = SIZE::default();
            if GetTextExtentPoint32W(dc, s, &mut sz).as_bool() {
                sz.cx as f32
            } else {
                0.0
            }
        });
        SelectObject(dc, prev);
        ReleaseDC(HWND(std::ptr::null_mut()), dc);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spans(v: &[(&str, &str)]) -> Vec<(String, Color)> {
        v.iter().map(|(t, c)| (t.to_string(), Color((*c).into()))).collect()
    }

    #[test]
    fn spans_split_on_embedded_newlines_keeping_color() {
        let s = spans(&[("fn ", "#ff0000"), ("main() {\n    body\n}", "#00ff00")]);
        let lines = split_lines(&s);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].len(), 2, "'fn ' + 'main() {{' share line 1");
        assert_eq!(lines[0][0].text, "fn ");
        assert_eq!(lines[0][1].text, "main() {");
        assert_eq!(lines[1][0].text, "    body", "indent preserved");
        assert_eq!(lines[2][0].text, "}");
        // The green run's color survives on every fragment.
        assert_eq!(lines[0][1].color, lines[1][0].color);
        assert_eq!(lines[1][0].color, lines[2][0].color);
        assert_ne!(lines[0][0].color, lines[0][1].color);
    }

    #[test]
    fn blank_lines_stay_blank() {
        let s = spans(&[("a\n\nb", "#000000")]);
        let lines = split_lines(&s);
        assert_eq!(lines.len(), 3);
        assert!(lines[1].is_empty());
    }

    #[test]
    fn intrinsic_is_longest_line_by_line_count() {
        // 10 px per UTF-16 unit.
        let s = spans(&[("short\nlonger line", "#000000")]);
        let key = font::FontKey {
            family: "Consolas".into(),
            size_px: 13,
            weight: 400,
            italic: false,
        };
        let (vis, (w, h)) = build_visual(&s, key, 16.0, |t| t.len() as f32 * 10.0);
        assert_eq!(vis.lines.len(), 2);
        assert_eq!(w, 110, "widest line: 'longer line' = 11 units");
        assert_eq!(h, 32, "2 lines x 16px");
        assert_eq!(vis.lines[1][0].width, 110.0);
    }

    #[test]
    fn empty_span_list_is_one_empty_line() {
        let (vis, (w, h)) = build_visual(
            &[],
            font::FontKey { family: "Consolas".into(), size_px: 13, weight: 400, italic: false },
            16.0,
            |t| t.len() as f32 * 10.0,
        );
        assert_eq!(vis.lines.len(), 1);
        assert_eq!((w, h), (0, 16));
    }
}
