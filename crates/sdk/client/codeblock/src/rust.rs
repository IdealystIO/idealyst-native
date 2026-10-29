//! A small Rust tokenizer for the two code surfaces: [`decorations`] for a
//! [`code_editor`](crate::code_editor) and [`spans`] for a
//! [`code_block`](crate::code_block).
//!
//! It is a lexer, not a parser — enough to tint what helps a reader scan:
//! comments (line and nested block), literals (strings — raw and byte
//! forms included — chars, numbers, lifetimes) and keywords. Everything else — identifiers, types,
//! punctuation, whitespace — is [`Token::Plain`] and renders in the
//! surface's own ink.
//!
//! [`tokens`] is the single source of truth: its ranges tile the input
//! exactly (every byte in one range, in order), and every range boundary
//! sits on an ASCII byte or the ends, so slicing never splits a UTF-8
//! character. Both outputs are derived from it.

use std::ops::Range;

use runtime_shared::Color;

use crate::decoration::Decoration;

/// What a range of source is, for tinting. `#[non_exhaustive]` so a finer
/// kind (types, macros) can be added without breaking a `match`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Token {
    /// `// …` and `/* … */` (nested).
    Comment,
    /// Strings, chars, numbers and lifetimes.
    Literal,
    /// Rust keywords, including the reserved ones.
    Keyword,
    /// Identifiers, types, punctuation, whitespace.
    Plain,
}

/// The colors each [`Token`] renders in. `ink` is [`Token::Plain`]'s, used
/// by [`spans`] only — [`decorations`] leaves plain text undecorated so it
/// takes the editor's own text color.
///
/// `#[non_exhaustive]` so a color for a new [`Token`] kind can be added
/// without breaking callers. Start a custom palette from [`Palette::LIGHT`] or
/// [`Palette::DARK`] and set the fields you want:
/// `let p = Palette { keyword: "#b31d28", ..Palette::LIGHT };` does not compile
/// outside this crate, `let mut p = Palette::LIGHT; p.keyword = "#b31d28";` does.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Palette {
    /// [`Token::Plain`] — identifiers, types, punctuation.
    pub ink: &'static str,
    /// [`Token::Comment`].
    pub comment: &'static str,
    /// [`Token::Literal`].
    pub literal: &'static str,
    /// [`Token::Keyword`].
    pub keyword: &'static str,
}

impl Palette {
    /// A restrained palette for a light surface.
    pub const LIGHT: Palette = Palette {
        ink: "#1f2328",
        comment: "#8c959f",
        literal: "#1f6e5f",
        keyword: "#5a4fcf",
    };

    /// The same roles for a dark surface.
    pub const DARK: Palette = Palette {
        ink: "#e6e8ee",
        comment: "#8b94a3",
        literal: "#5eead4",
        keyword: "#c4b5fd",
    };

    /// The color `token` renders in.
    pub fn color(&self, token: Token) -> &'static str {
        match token {
            Token::Comment => self.comment,
            Token::Literal => self.literal,
            Token::Keyword => self.keyword,
            Token::Plain => self.ink,
        }
    }
}

/// Tokenize `src` into ranges that tile it exactly. Adjacent ranges may
/// share a kind; nothing merges them.
pub fn tokens(src: &str) -> Vec<(Range<usize>, Token)> {
    let mut out = Vec::new();
    let bytes = src.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        // Multi-byte openers first, so `/` and `'` are not taken as
        // punctuation.
        let (end, kind) = if b == b'/' && bytes.get(i + 1) == Some(&b'/') {
            (line_end(bytes, i), Token::Comment)
        } else if b == b'/' && bytes.get(i + 1) == Some(&b'*') {
            (block_comment_end(bytes, i), Token::Comment)
        } else if b == b'"' {
            (string_end(bytes, i), Token::Literal)
        } else if let Some(end) = prefixed_literal_end(bytes, i) {
            // Before identifiers: `r"…"` / `b"…"` start like one.
            (end, Token::Literal)
        } else if b == b'\'' {
            // A char literal (`'x'`, `'\n'`, `'\u{1F600}'`) or a lifetime
            // (`'static`); a stray quote is plain.
            match char_literal_end(bytes, i).or_else(|| lifetime_end(bytes, i)) {
                Some(end) => (end, Token::Literal),
                None => (i + 1, Token::Plain),
            }
        } else if b.is_ascii_digit() {
            (number_end(bytes, i), Token::Literal)
        } else if is_ident_start(b) {
            let end = ident_end(bytes, i);
            let kind = if is_keyword(&src[i..end]) { Token::Keyword } else { Token::Plain };
            (end, kind)
        } else if b.is_ascii_whitespace() {
            (ws_end(bytes, i), Token::Plain)
        } else {
            (punct_end(bytes, i), Token::Plain)
        };
        out.push((i..end, kind));
        i = end;
    }
    out
}

/// `code_editor` decorations for `src`: one color range per non-plain
/// token. Pass as `code_editor(..).decorate(|s| rust::decorations(s, &Palette::LIGHT))`.
pub fn decorations(src: &str, palette: &Palette) -> Vec<Decoration> {
    tokens(src)
        .into_iter()
        .filter(|(_, kind)| *kind != Token::Plain)
        .map(|(range, kind)| Decoration::color(range, palette.color(kind)))
        .collect()
}

/// `code_block` runs for `src`: every token with its color, plain ones in
/// `palette.ink`. The runs re-concatenate to `src`.
pub fn spans(src: &str, palette: &Palette) -> Vec<(String, Color)> {
    tokens(src)
        .into_iter()
        .map(|(range, kind)| (src[range].to_string(), Color(palette.color(kind).into())))
        .collect()
}

fn line_end(bytes: &[u8], start: usize) -> usize {
    let mut i = start;
    while i < bytes.len() && bytes[i] != b'\n' {
        i += 1;
    }
    // Include the trailing newline so subsequent runs start at a
    // line boundary — keeps the round-trip exact.
    if i < bytes.len() {
        i + 1
    } else {
        i
    }
}

fn block_comment_end(bytes: &[u8], start: usize) -> usize {
    // Rust allows nested block comments — track depth.
    let mut i = start + 2;
    let mut depth = 1;
    while i + 1 < bytes.len() && depth > 0 {
        if bytes[i] == b'/' && bytes[i + 1] == b'*' {
            depth += 1;
            i += 2;
        } else if bytes[i] == b'*' && bytes[i + 1] == b'/' {
            depth -= 1;
            i += 2;
        } else {
            i += 1;
        }
    }
    if depth > 0 { bytes.len() } else { i }
}

fn string_end(bytes: &[u8], start: usize) -> usize {
    let mut i = start + 1;
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'\\' && i + 1 < bytes.len() {
            // Skip the escaped character verbatim — `\"` shouldn't
            // close the string. Escapes aren't validated: this tints,
            // it doesn't check.
            i += 2;
            continue;
        }
        if c == b'"' {
            return i + 1;
        }
        i += 1;
    }
    bytes.len()
}

/// A literal with a prefix: raw strings (`r"…"`, `r#"…"#` with any number
/// of `#`), byte strings (`b"…"`, `br#"…"#`) and byte chars (`b'x'`).
/// `None` for anything else — including a raw identifier (`r#type`), which
/// lexes as plain.
///
/// A raw string ends only at `"` followed by as many `#` as opened it, and
/// has no escapes. Lexing `r#"a"b"#` as a plain `"` string instead closes it
/// at the inner `"`, and the closing `"#` then opens a string that tints the
/// rest of the file as a literal.
fn prefixed_literal_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut i = start;
    let byte = bytes.get(i) == Some(&b'b');
    if byte {
        i += 1;
        match bytes.get(i) {
            Some(b'"') => return Some(string_end(bytes, i)),
            Some(b'\'') => return char_literal_end(bytes, i),
            _ => {}
        }
    }
    if bytes.get(i) != Some(&b'r') {
        return None;
    }
    i += 1;
    let mut hashes = 0;
    while bytes.get(i) == Some(&b'#') {
        hashes += 1;
        i += 1;
    }
    if bytes.get(i) != Some(&b'"') {
        return None;
    }
    i += 1;
    while i < bytes.len() {
        let close = i + 1 + hashes;
        if bytes[i] == b'"' && close <= bytes.len() && bytes[i + 1..close].iter().all(|&c| c == b'#') {
            return Some(close);
        }
        i += 1;
    }
    Some(bytes.len())
}

/// `'x'` / `'\n'` / `'\u{1F600}'`. Returns the byte index past the
/// closing `'`, or `None` if the input doesn't look like a char
/// literal (so the caller can try [`lifetime_end`] instead).
fn char_literal_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut i = start + 1;
    if i >= bytes.len() {
        return None;
    }
    if bytes[i] == b'\\' {
        i += 1;
        // Skip a minimum-viable escape body. Unicode escape
        // `\u{...}` runs until `}`; everything else is a single
        // char after the backslash.
        if i < bytes.len() && bytes[i] == b'u' && i + 1 < bytes.len() && bytes[i + 1] == b'{' {
            i += 2;
            while i < bytes.len() && bytes[i] != b'}' {
                i += 1;
            }
            if i < bytes.len() {
                i += 1;
            }
        } else if i < bytes.len() {
            i += 1;
        }
    } else {
        i += 1;
    }
    if i < bytes.len() && bytes[i] == b'\'' {
        Some(i + 1)
    } else {
        None
    }
}

/// `'static`, `'a`. Returns the byte index past the last ident
/// char. Caller pre-checks that the position starts with `'`.
fn lifetime_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut i = start + 1;
    if i >= bytes.len() || !is_ident_start(bytes[i]) {
        return None;
    }
    while i < bytes.len() && is_ident_continue(bytes[i]) {
        i += 1;
    }
    Some(i)
}

fn number_end(bytes: &[u8], start: usize) -> usize {
    let mut i = start;
    // Hex / binary / octal prefix.
    if bytes[i] == b'0' && i + 1 < bytes.len() {
        match bytes[i + 1] {
            b'x' | b'X' => {
                i += 2;
                while i < bytes.len()
                    && (bytes[i].is_ascii_hexdigit() || bytes[i] == b'_')
                {
                    i += 1;
                }
                return i;
            }
            b'b' | b'B' => {
                i += 2;
                while i < bytes.len()
                    && (bytes[i] == b'0' || bytes[i] == b'1' || bytes[i] == b'_')
                {
                    i += 1;
                }
                return i;
            }
            b'o' | b'O' => {
                i += 2;
                while i < bytes.len()
                    && (bytes[i] >= b'0' && bytes[i] <= b'7' || bytes[i] == b'_')
                {
                    i += 1;
                }
                return i;
            }
            _ => {}
        }
    }
    while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'_') {
        i += 1;
    }
    // Fractional part.
    if i < bytes.len() && bytes[i] == b'.' && i + 1 < bytes.len() && bytes[i + 1].is_ascii_digit() {
        i += 1;
        while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'_') {
            i += 1;
        }
    }
    // Exponent.
    if i < bytes.len() && (bytes[i] == b'e' || bytes[i] == b'E') {
        i += 1;
        if i < bytes.len() && (bytes[i] == b'+' || bytes[i] == b'-') {
            i += 1;
        }
        while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'_') {
            i += 1;
        }
    }
    // Suffix (`u32`, `f64`, …). Eat any trailing ident-continue
    // chars to keep it as part of the number span.
    while i < bytes.len() && is_ident_continue(bytes[i]) {
        i += 1;
    }
    i
}

fn ident_end(bytes: &[u8], start: usize) -> usize {
    let mut i = start;
    while i < bytes.len() && is_ident_continue(bytes[i]) {
        i += 1;
    }
    i
}

fn ws_end(bytes: &[u8], start: usize) -> usize {
    let mut i = start;
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    i
}

fn punct_end(bytes: &[u8], start: usize) -> usize {
    let mut i = start;
    while i < bytes.len() {
        let b = bytes[i];
        if b.is_ascii_whitespace()
            || b.is_ascii_alphanumeric()
            || b == b'_'
            || b == b'"'
            || b == b'\''
            || b == b'/'
        {
            break;
        }
        i += 1;
    }
    // At least one byte — even if the loop broke on the first
    // iteration via the `/` guard above (e.g. when we encountered
    // an isolated `/` that isn't followed by `/` or `*`).
    if i == start {
        i + 1
    } else {
        i
    }
}

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

fn is_ident_continue(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Full Rust keyword set (`https://doc.rust-lang.org/reference/keywords.html`)
/// plus the reserved-for-future-use words: strict keywords first, then the
/// reserved ones. A `match` on ~50 strings — no lookup table needed.
fn is_keyword(word: &str) -> bool {
    matches!(
        word,
        "as" | "async"
            | "await"
            | "break"
            | "const"
            | "continue"
            | "crate"
            | "dyn"
            | "else"
            | "enum"
            | "extern"
            | "false"
            | "fn"
            | "for"
            | "if"
            | "impl"
            | "in"
            | "let"
            | "loop"
            | "match"
            | "mod"
            | "move"
            | "mut"
            | "pub"
            | "ref"
            | "return"
            | "self"
            | "Self"
            | "static"
            | "struct"
            | "super"
            | "trait"
            | "true"
            | "type"
            | "union"
            | "unsafe"
            | "use"
            | "where"
            | "while"
            | "yield"
            | "abstract"
            | "become"
            | "box"
            | "do"
            | "final"
            | "macro"
            | "override"
            | "priv"
            | "try"
            | "typeof"
            | "unsized"
            | "virtual"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(src: &str) -> Vec<(&str, Token)> {
        tokens(src).into_iter().map(|(r, k)| (&src[r], k)).collect()
    }

    #[test]
    fn ranges_tile_the_source_exactly() {
        let src = "fn main() {\n    // hi ✓\n    let s = \"a\\\"b\"; let c = '\\n'; /* /* x */ */ 'a: loop {}\n}";
        let mut at = 0;
        for (range, _) in tokens(src) {
            assert_eq!(range.start, at, "ranges are contiguous");
            assert!(range.end > range.start, "no empty range");
            at = range.end;
        }
        assert_eq!(at, src.len());
        let joined: String = spans(src, &Palette::LIGHT).into_iter().map(|(t, _)| t).collect();
        assert_eq!(joined, src);
    }

    #[test]
    fn classifies_comments_literals_and_keywords() {
        let k = kinds("let n = 0x1F; // done");
        assert!(k.contains(&("let", Token::Keyword)));
        assert!(k.contains(&("n", Token::Plain)));
        assert!(k.contains(&("0x1F", Token::Literal)));
        assert!(k.contains(&("// done", Token::Comment)));
        assert!(kinds("&'static str").contains(&("'static", Token::Literal)));
        assert!(kinds("'x'").contains(&("'x'", Token::Literal)));
    }

    #[test]
    fn non_ascii_text_never_splits_a_character() {
        // Every range is sliced, which panics on a non-boundary.
        let src = "text { \"héllo 👋\" } → «done»";
        for (range, _) in tokens(src) {
            let _ = &src[range];
        }
    }

    /// Regression: a raw string containing `"` closed at that quote, and its
    /// closing `"#` opened a new string that tinted everything after it.
    #[test]
    fn regression_raw_and_byte_strings_end_where_rust_ends_them() {
        let src = r####"let s = r#"a"b"#; let n = 1;"####;
        let k = kinds(src);
        assert!(k.contains(&(r##"r#"a"b"#"##, Token::Literal)), "{k:?}");
        assert!(k.contains(&("let", Token::Keyword)) && k.iter().filter(|(t, _)| *t == "let").count() == 2, "{k:?}");
        assert!(k.contains(&("1", Token::Literal)), "code after the raw string is not swallowed: {k:?}");

        assert!(kinds(r###"r"no \ escape""###).contains(&(r###"r"no \ escape""###, Token::Literal)));
        assert!(kinds(r####"br##"x"#y"##"####).contains(&(r####"br##"x"#y"##"####, Token::Literal)));
        assert!(kinds(r#"b"bytes
""#).contains(&(r#"b"bytes
""#, Token::Literal)));
        assert!(kinds("b'x'").contains(&("b'x'", Token::Literal)));
        // A raw identifier is not a string.
        let k = kinds("r#type");
        assert!(k.iter().all(|(_, t)| *t != Token::Literal), "{k:?}");
        // Unterminated: runs to the end, still tiles the input.
        assert_eq!(kinds(r##"r#"open"##).last().map(|(_, t)| *t), Some(Token::Literal));
    }

    #[test]
    fn decorations_skip_plain_text() {
        let decs = decorations("let x", &Palette::LIGHT);
        assert_eq!(decs.len(), 1, "only the keyword is decorated");
        assert_eq!(decs[0].range, 0..3);
    }
}
