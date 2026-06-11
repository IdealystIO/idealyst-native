//! Map proc-macro2 `LineColumn`s (1-based line, 0-based char column) onto
//! byte offsets and source-line text.
//!
//! rustc-style JSON diagnostics — what rust-analyzer's flycheck consumes —
//! require `byte_start`/`byte_end` alongside line/column, and the human
//! reporter wants the raw source line to draw a caret under. Both come
//! from the same precomputed line index, so they live together here.

use proc_macro2::LineColumn;

/// A byte/line index over a single file's source text.
pub(crate) struct SourceMap<'a> {
    src: &'a str,
    /// Byte offset of the start of each line. `line_starts[0]` is always 0;
    /// `line_starts[n]` is the byte just past the `n`th `\n`.
    line_starts: Vec<usize>,
}

impl<'a> SourceMap<'a> {
    pub(crate) fn new(src: &'a str) -> Self {
        let mut line_starts = vec![0usize];
        for (i, b) in src.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(i + 1);
            }
        }
        SourceMap { src, line_starts }
    }

    /// Byte offset of a `LineColumn`. proc-macro2 columns count Unicode
    /// scalar values from 0; we advance that many chars into the line and
    /// take the resulting byte index (so non-ASCII source still maps
    /// correctly). A column past the line end clamps to the line end.
    pub(crate) fn byte_offset(&self, lc: LineColumn) -> usize {
        // `line` is 1-based; clamp defensively rather than panic on a
        // span that points one past the end (can happen for end spans).
        let line_idx = lc.line.saturating_sub(1).min(self.line_starts.len() - 1);
        let line_start = self.line_starts[line_idx];
        let line_text = self.line_text_at(line_idx);
        let within = line_text
            .char_indices()
            .nth(lc.column)
            .map(|(b, _)| b)
            .unwrap_or(line_text.len());
        (line_start + within).min(self.src.len())
    }

    fn line_text_at(&self, line_idx: usize) -> &'a str {
        let start = self.line_starts[line_idx];
        let end = self
            .line_starts
            .get(line_idx + 1)
            .map(|&n| n - 1) // drop the trailing '\n'
            .unwrap_or(self.src.len());
        // Guard against a start past end (empty trailing line).
        if start > end {
            return "";
        }
        &self.src[start..end]
    }

    /// The full text of a 1-based line, without its trailing newline.
    pub(crate) fn line_text(&self, line: usize) -> &'a str {
        let idx = line.saturating_sub(1).min(self.line_starts.len() - 1);
        self.line_text_at(idx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lc(line: usize, column: usize) -> LineColumn {
        LineColumn { line, column }
    }

    #[test]
    fn byte_offset_ascii() {
        let src = "let x = 1;\nlet y = 2;\n";
        let map = SourceMap::new(src);
        // line 2, col 0 -> byte just after the first '\n' (offset 11)
        assert_eq!(map.byte_offset(lc(2, 0)), 11);
        // line 2, col 4 -> the 'y'
        assert_eq!(&src[map.byte_offset(lc(2, 4))..map.byte_offset(lc(2, 5))], "y");
    }

    #[test]
    fn byte_offset_unicode_column_is_chars_not_bytes() {
        // "café" — the 'é' is 2 bytes, so a char-column of 4 must land past it.
        let src = "let café = 1;\n";
        let map = SourceMap::new(src);
        // col 8 is the space after "café" (l,e,t,sp,c,a,f,é = 8 chars).
        let off = map.byte_offset(lc(1, 8));
        assert_eq!(&src[off..off + 1], " ");
    }

    #[test]
    fn line_text_strips_newline() {
        let src = "alpha\nbeta\n";
        let map = SourceMap::new(src);
        assert_eq!(map.line_text(1), "alpha");
        assert_eq!(map.line_text(2), "beta");
    }
}
