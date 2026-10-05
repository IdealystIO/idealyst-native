//! Width-aware wrapping for painted text.
//!
//! ## One plan, one break function, two consumers
//!
//! Taffy's measure fn and the scene painter must agree EXACTLY on where
//! lines break, or measured heights drift from painted heights (the
//! classic "last line clipped / phantom gap" wrap bug). So both derive
//! their breaks from the same pure function over the same measured data:
//!
//! - A [`WrapPlan`] is built once per (content, font): the text is split
//!   into paragraphs (hard `\n`) and alternating word / space runs, and
//!   each run's advance is measured ONCE with GDI
//!   (`GetTextExtentPoint32W` — integer per-char advances, no kerning
//!   pairs, so run widths sum to exact substring extents).
//! - The Taffy measure fn captures the plan behind an `Rc` and calls
//!   [`WrapPlan::lines_at`] at the probe width. It must not touch the
//!   backend: Taffy runs inside `layout_pass`, while the backend is
//!   already mutably borrowed.
//! - `layout_pass` fills the node's [`WrappedLines`] cache at the final
//!   frame width with the same `lines_at`; `paint_node` only reads it.
//!
//! ## Line limit (`StyleRules::max_lines`)
//!
//! [`WrapPlan::lines_limited`] is the one entry point both consumers use.
//! A limit of `1` lays every paragraph on one unwrapped line (newlines
//! collapse to a space, as CSS `white-space: nowrap` does); `n > 1` wraps
//! and keeps the first `n` lines. A kept line that is cut — wider than
//! the box, or the last one with text dropped after it — ends with `…`
//! (U+2026), cut at a UTF-16 boundary using the per-unit extents every
//! run carries, so the ellipsis lands exactly at the box edge. GDI has a
//! tail ellipsis of its own (`DT_END_ELLIPSIS`), but it re-breaks lines
//! itself and would disagree with the breaks Taffy was measured with —
//! so truncation stays inside the plan, like wrapping.
//!
//! Only ASCII space / tab are break opportunities. NBSP (`\u{a0}`) must
//! NOT break (CSS semantics) — which is why run splitting doesn't use
//! `char::is_whitespace` (true for NBSP).

use runtime_layout::{AvailableSpace, Size};

/// Slack added to every "does it fit" comparison. Taffy hands the final
/// frame width back as the same `f32` the measure fn returned, but it
/// travels through rounding (`compute` → frame) — without slack a line
/// that exactly fit at measure time can lose its last word at paint
/// time.
const FIT_EPS: f32 = 0.5;

/// The ellipsis that ends a cut line, as one UTF-16 unit.
pub(crate) const ELLIPSIS16: u16 = 0x2026;

/// A maximal run of either breakable whitespace (space / tab) or
/// unbreakable content, with its measured advance width.
pub(crate) struct Run {
    pub text16: Vec<u16>,
    pub width: f32,
    /// Cumulative advance after each UTF-16 unit (`extents[i]` = width of
    /// `text16[..=i]`; the last entry is `width`). Only read to cut a line
    /// for `max_lines`.
    pub extents: Vec<f32>,
    pub is_space: bool,
}

/// One paragraph — the text between hard `\n`s — as measured runs.
pub(crate) struct Para {
    pub runs: Vec<Run>,
}

/// The reusable wrap input for one (content, font) pair.
pub(crate) struct WrapPlan {
    pub paras: Vec<Para>,
    /// Natural single-line glyph height of the font (GDI `cy`).
    pub font_height: f32,
    /// Widest paragraph laid out on one line (max-content width).
    pub single_line: f32,
    /// Widest unbreakable run (min-content width).
    pub longest_word: f32,
    /// Every paragraph on one line, joined by a space — what a
    /// `max_lines: 1` text shows before it is cut.
    pub joined_width: f32,
    /// Advance of one ASCII space (the paragraph joint of `max_lines: 1`).
    pub space_width: f32,
    /// Advance of `…` in this font.
    pub ellipsis_width: f32,
}

/// One wrapped output line, ready to draw.
pub(crate) struct WrapLine {
    pub text16: Vec<u16>,
    pub width: f32,
    /// Cumulative advance after each UTF-16 unit, as on [`Run`].
    pub extents: Vec<f32>,
}

impl WrapLine {
    fn empty() -> Self {
        WrapLine { text16: Vec::new(), width: 0.0, extents: Vec::new() }
    }

    /// Append `text16` (with its own cumulative `extents`) at the end of
    /// the line.
    fn push(&mut self, text16: &[u16], extents: &[f32], width: f32) {
        let base = self.width;
        self.text16.extend_from_slice(text16);
        self.extents.extend(extents.iter().map(|e| base + e));
        self.width = base + width;
    }
}

/// Line cache stored on a text node: the breaks computed for `width`.
pub(crate) struct WrappedLines {
    pub width: f32,
    pub lines: Vec<WrapLine>,
}

struct RawRun {
    text: String,
    is_space: bool,
}

/// Split into alternating space / word runs. Space = ASCII space or tab
/// ONLY (see module docs re NBSP).
fn split_runs(s: &str) -> Vec<RawRun> {
    let mut out: Vec<RawRun> = Vec::new();
    for c in s.chars() {
        let is_space = c == ' ' || c == '\t';
        match out.last_mut() {
            Some(r) if r.is_space == is_space => r.text.push(c),
            _ => out.push(RawRun { text: c.to_string(), is_space }),
        }
    }
    out
}

impl WrapPlan {
    /// Build from `text`, measuring each run with `extents` (the
    /// cumulative advance in px after each unit of a UTF-16 slice, one
    /// entry per unit). GDI-free so tests can inject synthetic metrics.
    pub(crate) fn build_with(
        text: &str,
        font_height: f32,
        mut extents: impl FnMut(&[u16]) -> Vec<f32>,
    ) -> Self {
        let mut measure = |s: &[u16]| {
            let e = extents(s);
            let w = e.last().copied().unwrap_or(0.0);
            (e, w)
        };
        let mut paras = Vec::new();
        let mut single_line = 0.0_f32;
        let mut longest_word = 0.0_f32;
        let mut joined_width = 0.0_f32;
        let space_width = measure(&[b' ' as u16]).1;
        let ellipsis_width = measure(&[ELLIPSIS16]).1;
        for (i, para_text) in text.split('\n').enumerate() {
            let para_text = para_text.strip_suffix('\r').unwrap_or(para_text);
            let mut runs = Vec::new();
            let mut para_w = 0.0;
            for piece in split_runs(para_text) {
                let text16: Vec<u16> = piece.text.encode_utf16().collect();
                let (extents, width) = measure(&text16);
                para_w += width;
                if !piece.is_space {
                    longest_word = longest_word.max(width);
                }
                runs.push(Run { text16, width, extents, is_space: piece.is_space });
            }
            single_line = single_line.max(para_w);
            joined_width += para_w + if i > 0 { space_width } else { 0.0 };
            paras.push(Para { runs });
        }
        WrapPlan {
            paras,
            font_height,
            single_line,
            longest_word,
            joined_width,
            space_width,
            ellipsis_width,
        }
    }

    /// The lines to draw for a box `max_w` wide under a line limit
    /// (`StyleRules::max_lines`; `None` / `Some(0)` = no limit, which is
    /// exactly [`Self::lines_at`]). See the module docs for the rules.
    pub(crate) fn lines_limited(&self, max_w: f32, max_lines: Option<u32>) -> Vec<WrapLine> {
        match max_lines.filter(|n| *n > 0) {
            None => self.lines_at(max_w),
            Some(1) => vec![self.ellipsize(self.joined_line(), max_w, false)],
            Some(n) => {
                let n = n as usize;
                let mut lines = self.lines_at(max_w);
                let dropped = lines.len() > n;
                lines.truncate(n);
                let last = lines.len().saturating_sub(1);
                lines
                    .into_iter()
                    .enumerate()
                    .map(|(i, l)| self.ellipsize(l, max_w, dropped && i == last))
                    .collect()
            }
        }
    }

    /// Every paragraph on one line, joined by a single space.
    fn joined_line(&self) -> WrapLine {
        let mut line = WrapLine::empty();
        for (i, para) in self.paras.iter().enumerate() {
            if i > 0 {
                line.push(&[b' ' as u16], &[self.space_width], self.space_width);
            }
            for run in &para.runs {
                line.push(&run.text16, &run.extents, run.width);
            }
        }
        line
    }

    /// Fit `line` into `max_w`, ending it with `…` when it is cut.
    /// `more_follows` (the last kept line of a clamped text) adds the
    /// ellipsis even when the line fits, as `-webkit-line-clamp` does.
    /// Never splits a surrogate pair; trailing spaces before the ellipsis
    /// are dropped so a cut at a word gap reads `word…`.
    fn ellipsize(&self, line: WrapLine, max_w: f32, more_follows: bool) -> WrapLine {
        if !more_follows && line.width <= max_w + FIT_EPS {
            return line;
        }
        let budget = max_w - self.ellipsis_width + FIT_EPS;
        // Units kept: the longest prefix whose advance fits the budget.
        let mut keep = line.extents.iter().take_while(|e| **e <= budget).count();
        if keep > 0 && (0xD800..0xDC00).contains(&line.text16[keep - 1]) {
            keep -= 1; // don't strand a high surrogate
        }
        while keep > 0 && matches!(line.text16[keep - 1], 0x20 | 0x09) {
            keep -= 1;
        }
        let mut out = WrapLine::empty();
        let width = if keep > 0 { line.extents[keep - 1] } else { 0.0 };
        out.push(&line.text16[..keep], &line.extents[..keep], width);
        out.push(&[ELLIPSIS16], &[self.ellipsis_width], self.ellipsis_width);
        out
    }

    /// Greedy break: pack words onto a line while they fit in `max_w`,
    /// break before the first word that doesn't. A lone word wider than
    /// `max_w` gets its own line and overflows — there is no
    /// character-level breaking (matches the other native backends).
    /// Trailing spaces never count toward a line's width or draw text;
    /// spaces at a soft break are consumed; spaces at paragraph start
    /// are content (indent) and kept. An empty / all-space paragraph
    /// still occupies one (empty) line so `"a\n\nb"` renders a blank
    /// line between `a` and `b`.
    pub(crate) fn lines_at(&self, max_w: f32) -> Vec<WrapLine> {
        let mut lines = Vec::new();
        for para in &self.paras {
            let first = lines.len();
            let mut start = 0usize;
            while start < para.runs.len() {
                if start > 0 {
                    // Consume the whitespace at a soft break.
                    while start < para.runs.len() && para.runs[start].is_space {
                        start += 1;
                    }
                    if start >= para.runs.len() {
                        break;
                    }
                }
                // Grow the line word by word.
                let mut cum = 0.0_f32;
                let mut end = start; // exclusive, at the last committed word
                let mut line_w = 0.0_f32;
                let mut i = start;
                while i < para.runs.len() {
                    let run = &para.runs[i];
                    cum += run.width;
                    if !run.is_space {
                        if cum > max_w + FIT_EPS && end > start {
                            break; // this word starts the next line
                        }
                        end = i + 1;
                        line_w = cum;
                        if cum > max_w + FIT_EPS {
                            break; // oversized lone word — own line
                        }
                    }
                    i += 1;
                }
                if end == start {
                    // Nothing committed: the remainder is all spaces.
                    break;
                }
                let mut line = WrapLine::empty();
                for run in &para.runs[start..end] {
                    line.push(&run.text16, &run.extents, run.width);
                }
                debug_assert!((line.width - line_w).abs() < 0.01);
                lines.push(line);
                start = end;
            }
            if lines.len() == first {
                lines.push(WrapLine::empty());
            }
        }
        lines
    }
}

/// The Taffy measure contract for wrapped text — mirrors the layout
/// crate's canonical `text_measure` test fns exactly:
/// `MinContent` → longest word, `MaxContent` → single line,
/// `Definite(aw)` → clamp the single-line width into
/// `[longest_word, aw]`; height = wrapped line count × `advance`
/// (the style `line-height` in px, or the font's natural height).
///
/// Under a line limit (`max_lines`, `0` = none) the height is the kept
/// line count from [`WrapPlan::lines_limited`] — at most `n` lines. A
/// one-line text never wraps: its min- and max-content width is the
/// joined line, clamped to a definite width (the layout gives a limited
/// text `overflow: hidden`, so it can still shrink in a row).
pub(crate) fn measure_size(
    plan: &WrapPlan,
    advance: f32,
    max_lines: Option<u32>,
    known: Size<Option<f32>>,
    avail: Size<AvailableSpace>,
) -> Size<f32> {
    let one_line = max_lines == Some(1);
    let w = known.width.unwrap_or_else(|| match avail.width {
        AvailableSpace::MinContent | AvailableSpace::MaxContent if one_line => plan.joined_width,
        AvailableSpace::Definite(aw) if one_line => plan.joined_width.min(aw),
        AvailableSpace::MinContent => plan.longest_word,
        AvailableSpace::MaxContent => plan.single_line,
        AvailableSpace::Definite(aw) => plan.single_line.min(aw).max(plan.longest_word),
    });
    let lines = plan.lines_limited(w, max_lines).len().max(1) as f32;
    Size { width: w, height: known.height.unwrap_or(lines * advance) }
}

/// Build a plan measuring runs with GDI in `hfont`: one DC + font
/// selection for the whole build, one `GetTextExtentPoint32W` per run.
/// `fallback_height` stands in when GDI yields no metrics.
pub(crate) fn build_gdi(
    text: &str,
    hfont: windows::Win32::Graphics::Gdi::HFONT,
    fallback_height: f32,
) -> WrapPlan {
    use windows::Win32::Foundation::{HWND, SIZE};
    use windows::core::PCWSTR;
    use windows::Win32::Graphics::Gdi::{
        GetDC, GetTextExtentExPointW, GetTextExtentPoint32W, ReleaseDC, SelectObject, HGDIOBJ,
    };
    unsafe {
        let dc = GetDC(HWND(std::ptr::null_mut()));
        if dc.is_invalid() {
            return WrapPlan::build_with(text, fallback_height, |s| vec![0.0; s.len()]);
        }
        let prev = SelectObject(dc, HGDIOBJ(hfont.0));
        let mut font_height = fallback_height;
        let sample: Vec<u16> = "Mg".encode_utf16().collect();
        let mut size = SIZE::default();
        if GetTextExtentPoint32W(dc, &sample, &mut size).as_bool() {
            font_height = size.cy as f32;
        }
        // `GetTextExtentExPointW` fills the cumulative extent after each
        // unit (`lpDx`) in the same call that sizes the run — the
        // per-unit cut points `max_lines` truncation needs.
        let plan = WrapPlan::build_with(text, font_height, |s: &[u16]| {
            if s.is_empty() {
                return Vec::new();
            }
            let mut dx = vec![0i32; s.len()];
            let mut sz = SIZE::default();
            if GetTextExtentExPointW(
                dc,
                PCWSTR(s.as_ptr()),
                s.len() as i32,
                0,
                None,
                Some(dx.as_mut_ptr()),
                &mut sz,
            )
            .as_bool()
            {
                dx.into_iter().map(|d| d as f32).collect()
            } else {
                vec![0.0; s.len()]
            }
        });
        SelectObject(dc, prev);
        ReleaseDC(HWND(std::ptr::null_mut()), dc);
        plan
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 10 px per UTF-16 unit — every test metric below derives from it.
    fn plan(text: &str) -> WrapPlan {
        WrapPlan::build_with(text, 16.0, |s| (1..=s.len()).map(|i| i as f32 * 10.0).collect())
    }

    fn line_strs(lines: &[WrapLine]) -> Vec<String> {
        lines.iter().map(|l| String::from_utf16_lossy(&l.text16)).collect()
    }

    #[test]
    fn single_line_when_it_fits() {
        let p = plan("aa bb cc"); // 80 px on one line
        let lines = p.lines_at(100.0);
        assert_eq!(line_strs(&lines), ["aa bb cc"]);
        assert_eq!(lines[0].width, 80.0);
        assert_eq!(p.single_line, 80.0);
        assert_eq!(p.longest_word, 20.0);
    }

    #[test]
    fn wraps_at_word_boundaries() {
        let p = plan("aa bb cc");
        let lines = p.lines_at(50.0);
        assert_eq!(line_strs(&lines), ["aa bb", "cc"]);
        assert_eq!(lines[0].width, 50.0);
        assert_eq!(lines[1].width, 20.0);
    }

    /// The website bug this module exists for: text nodes previously got
    /// their single-line extent as a Taffy `min_size`, so paragraphs
    /// could never shrink below one line and ran off the window edge.
    /// With the measure fn, a definite width caps the box and the
    /// height grows by wrapped line count instead.
    #[test]
    fn regression_website_paragraph_wraps_instead_of_overflowing() {
        let text = "the quick brown fox jumps over the lazy dog and keeps \
                    on running until the paragraph is long enough to matter";
        let p = plan(text);
        assert!(p.single_line > 300.0, "premise: text is wider than the box");
        let known = Size { width: None, height: None };
        let avail = Size {
            width: AvailableSpace::Definite(300.0),
            height: AvailableSpace::Definite(600.0),
        };
        let size = measure_size(&p, 20.0, None, known, avail);
        assert_eq!(size.width, 300.0, "box capped at the available width");
        let lines = p.lines_at(300.0);
        assert!(lines.len() > 1, "long text must wrap, got {} line(s)", lines.len());
        assert_eq!(size.height, lines.len() as f32 * 20.0);
        for l in &lines {
            assert!(l.width <= 300.5, "no line exceeds the box: {}", l.width);
        }
        // Nothing lost: the lines re-concatenate to the exact words.
        let joined = line_strs(&lines).join(" ");
        assert_eq!(joined, text.split_whitespace().collect::<Vec<_>>().join(" "));
    }

    #[test]
    fn oversized_word_gets_own_line_and_overflows() {
        let p = plan("hi incomprehensibilities yo"); // word2 = 21 ch = 210 px
        let lines = p.lines_at(100.0);
        assert_eq!(line_strs(&lines), ["hi", "incomprehensibilities", "yo"]);
        assert_eq!(lines[1].width, 210.0, "no char-level breaking: overflows");
    }

    #[test]
    fn hard_newlines_make_paragraphs_and_blank_lines() {
        let p = plan("a\n\nb");
        let lines = p.lines_at(1000.0);
        assert_eq!(line_strs(&lines), ["a", "", "b"]);
    }

    #[test]
    fn soft_break_consumes_spaces_and_trailing_spaces_dont_count() {
        let p = plan("aa  bb   "); // double inner space, trailing run
        let lines = p.lines_at(50.0);
        // Inner spaces (20) push `bb` past 50 → break; break consumes
        // them; trailing spaces neither draw nor count.
        assert_eq!(line_strs(&lines), ["aa", "bb"]);
        assert_eq!(lines[0].width, 20.0);
        assert_eq!(lines[1].width, 20.0);
    }

    #[test]
    fn leading_spaces_are_indent_content() {
        let p = plan("  aa");
        let lines = p.lines_at(100.0);
        assert_eq!(line_strs(&lines), ["  aa"]);
        assert_eq!(lines[0].width, 40.0);
    }

    #[test]
    fn nbsp_does_not_break() {
        let p = plan("a\u{a0}b c"); // NBSP-joined pair is one run
        assert_eq!(p.longest_word, 30.0);
        let lines = p.lines_at(30.0);
        assert_eq!(line_strs(&lines), ["a\u{a0}b", "c"]);
    }

    #[test]
    fn measure_contract_min_max_definite_known() {
        let p = plan("aaaa bb"); // single_line 70, longest 40
        let advance = 16.0;
        let none = Size { width: None, height: None };
        let ms = |avail_w| {
            measure_size(&p, advance, None, none, Size {
                width: avail_w,
                height: AvailableSpace::MaxContent,
            })
        };
        assert_eq!(ms(AvailableSpace::MinContent).width, 40.0);
        assert_eq!(ms(AvailableSpace::MaxContent).width, 70.0);
        assert_eq!(ms(AvailableSpace::MaxContent).height, advance);
        // Definite clamps into [longest_word, avail].
        let d50 = ms(AvailableSpace::Definite(50.0));
        assert_eq!(d50.width, 50.0);
        assert_eq!(d50.height, 2.0 * advance, "wraps to two lines at 50");
        assert_eq!(ms(AvailableSpace::Definite(30.0)).width, 40.0);
        // known dimensions pin both axes.
        let pinned = measure_size(
            &p,
            advance,
            None,
            Size { width: Some(45.0), height: Some(99.0) },
            Size {
                width: AvailableSpace::MaxContent,
                height: AvailableSpace::MaxContent,
            },
        );
        assert_eq!(pinned.width, 45.0);
        assert_eq!(pinned.height, 99.0);
    }

    #[test]
    fn empty_text_is_one_empty_line_of_advance_height() {
        let p = plan("");
        let lines = p.lines_at(100.0);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].text16.is_empty());
        let size = measure_size(
            &p,
            16.0,
            None,
            Size { width: None, height: None },
            Size {
                width: AvailableSpace::MaxContent,
                height: AvailableSpace::MaxContent,
            },
        );
        assert_eq!(size.width, 0.0);
        assert_eq!(size.height, 16.0);
    }

    /// Exactly-fitting content must not lose its last word to float
    /// noise when re-broken at the frame width Taffy hands back.
    #[test]
    fn exact_fit_width_keeps_last_word() {
        let p = plan("aa bb");
        let lines = p.lines_at(50.0 - 0.25); // frame width minus rounding
        assert_eq!(line_strs(&lines), ["aa bb"]);
    }

    // --- max_lines -------------------------------------------------------
    // 10 px per unit, so `…` is 10 px and a 100 px box holds 10 units.

    #[test]
    fn max_lines_none_or_zero_is_plain_wrapping() {
        let p = plan("aa bb cc dd");
        for limit in [None, Some(0)] {
            assert_eq!(line_strs(&p.lines_limited(50.0, limit)), ["aa bb", "cc dd"]);
        }
    }

    #[test]
    fn max_lines_one_is_one_line_cut_with_an_ellipsis() {
        let p = plan("alpha beta gamma delta");
        // 10 units (100 px) + `…` (10 px) fill the 110 px box exactly.
        let lines = p.lines_limited(110.0, Some(1));
        assert_eq!(line_strs(&lines), ["alpha beta\u{2026}"]);
        assert_eq!(lines[0].width, 110.0);
        // A cut right after a word gap drops the space: `alpha…`.
        let lines = p.lines_limited(70.0, Some(1));
        assert_eq!(line_strs(&lines), ["alpha\u{2026}"]);
        assert_eq!(lines[0].width, 60.0);
    }

    #[test]
    fn max_lines_one_joins_paragraphs_and_keeps_a_fitting_line() {
        let p = plan("ab\ncd");
        assert_eq!(line_strs(&p.lines_limited(100.0, Some(1))), ["ab cd"]);
        assert_eq!(p.joined_width, 50.0);
    }

    #[test]
    fn max_lines_one_cuts_inside_a_long_word() {
        let p = plan("incomprehensibilities");
        let lines = p.lines_limited(60.0, Some(1));
        assert_eq!(line_strs(&lines), ["incom\u{2026}"]);
        assert_eq!(lines[0].width, 60.0);
    }

    #[test]
    fn max_lines_two_drops_the_rest_and_marks_the_last_kept_line() {
        let p = plan("aa bb cc dd ee ff");
        assert_eq!(line_strs(&p.lines_at(50.0)), ["aa bb", "cc dd", "ee ff"]);
        // `cc dd…` would be 60 px > 50, so the line is cut back: `cc d…`.
        assert_eq!(line_strs(&p.lines_limited(50.0, Some(2))), ["aa bb", "cc d\u{2026}"]);
        // Exactly two lines at 80 px: nothing dropped, so no ellipsis.
        assert_eq!(line_strs(&p.lines_limited(80.0, Some(2))), ["aa bb cc", "dd ee ff"]);

        let p = plan("aa bbbb ccccc dd");
        assert_eq!(line_strs(&p.lines_at(50.0)), ["aa", "bbbb", "ccccc", "dd"]);
        // A short last kept line has room: the ellipsis is just appended.
        let lines = p.lines_limited(50.0, Some(2));
        assert_eq!(line_strs(&lines), ["aa", "bbbb\u{2026}"]);
        assert_eq!(lines[1].width, 50.0);
    }

    #[test]
    fn max_lines_never_splits_a_surrogate_pair() {
        // U+1F600 is two UTF-16 units; a cut between them must back off.
        let p = plan("ab\u{1F600}cd");
        let lines = p.lines_limited(40.0, Some(1));
        assert_eq!(line_strs(&lines), ["ab\u{2026}"]);
    }

    /// The measured box and the drawn lines agree under a limit: height
    /// is the kept line count, and a one-line text measures one line at
    /// its joined width (clamped to a definite width).
    #[test]
    fn measure_respects_max_lines() {
        let p = plan("aa bb cc dd ee ff"); // joined 170
        let none = Size { width: None, height: None };
        let at = |w, limit| {
            measure_size(&p, 16.0, limit, none, Size {
                width: w,
                height: AvailableSpace::MaxContent,
            })
        };
        assert_eq!(at(AvailableSpace::Definite(50.0), None).height, 3.0 * 16.0);
        assert_eq!(at(AvailableSpace::Definite(50.0), Some(2)).height, 2.0 * 16.0);
        assert_eq!(at(AvailableSpace::Definite(50.0), Some(5)).height, 3.0 * 16.0);
        let one = at(AvailableSpace::Definite(50.0), Some(1));
        assert_eq!((one.width, one.height), (50.0, 16.0));
        assert_eq!(at(AvailableSpace::MaxContent, Some(1)).width, 170.0);
        assert_eq!(at(AvailableSpace::MinContent, Some(1)).height, 16.0);
    }
}
