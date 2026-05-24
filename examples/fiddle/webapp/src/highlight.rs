//! Tiny hand-rolled Rust tokenizer for the fiddle's editor pane.
//!
//! Not a real parser — just enough to produce colored runs we can
//! feed to the [`runtime_core::code_block`] primitive. Recognizes:
//!
//! - Line comments (`// …`) and block comments (`/* … */`, nested).
//! - Strings (`"…"`, with `\…` escapes) and chars (`'…'`).
//! - Lifetimes (`'static`, `'a`).
//! - Numbers (`123`, `0x1F`, `0b10`, `1_000`, `1.5e10`).
//! - Keywords (the standard Rust set).
//! - Identifiers (split into "looks like a type" via uppercase
//!   first letter, vs. ordinary ident).
//! - Punctuation, whitespace.
//!
//! Output is a flat `Vec<(String, Color)>` — consecutive runs of
//! the same color stay separate so the consumer can decide whether
//! to coalesce. The render side ([`runtime_core::CodeBlock`])
//! emits one `<span>` per tuple, which is fine for thousand-token
//! files; if it becomes a bottleneck the coalescing pass goes here.

use runtime_core::Color;

/// Subdued three-accent palette. Most code reads as default-ink
/// black; only the categories whose color actually helps you scan
/// (literals vs. identifiers vs. comments) get their own tint.
///
/// - **Default ink** — identifiers, types, punctuation, even the
///   whitespace runs. The eye groups them as "the code" without
///   needing per-token differentiation.
/// - **Comment** — flat gray so commented-out chunks visually
///   recede.
/// - **Literal** — strings, chars, numbers, lifetimes. A single
///   teal so anywhere data is embedded in source is consistent;
///   the eye doesn't have to parse three separate hues.
/// - **Keyword** — a subtle purple-blue for control flow (`fn`,
///   `let`, `match`, `if`, …). Different enough from default ink
///   to scan, muted enough not to "shout".
const C_DEFAULT: &str = "#1f2328";
const C_COMMENT: &str = "#8c959f";
const C_LITERAL: &str = "#1f6e5f";
const C_KEYWORD: &str = "#5a4fcf";

/// Tokenize a Rust source string into `(text, color)` runs that
/// re-concatenate to the original input. Round-trip is exact —
/// every byte of `src` appears once in some span.
pub fn highlight_rust(src: &str) -> Vec<(String, Color)> {
    let mut out: Vec<(String, Color)> = Vec::new();
    let bytes = src.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];

        // Multi-char openers come first so single-char prefixes
        // (`/`, `'`) don't get classified as punctuation.
        if b == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
            let end = line_end(bytes, i);
            push(&mut out, &src[i..end], C_COMMENT);
            i = end;
            continue;
        }
        if b == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
            let end = block_comment_end(bytes, i);
            push(&mut out, &src[i..end], C_COMMENT);
            i = end;
            continue;
        }
        if b == b'"' {
            let end = string_end(bytes, i);
            push(&mut out, &src[i..end], C_LITERAL);
            i = end;
            continue;
        }
        if b == b'\'' {
            // Either a char literal (`'x'` / `'\n'` / `'\u{1F600}'`)
            // or a lifetime (`'static`, `'a`). Char literals are
            // string-style; lifetimes share the literal accent
            // since they're also "data attached to a binding."
            if let Some(end) = char_literal_end(bytes, i) {
                push(&mut out, &src[i..end], C_LITERAL);
                i = end;
                continue;
            }
            if let Some(end) = lifetime_end(bytes, i) {
                push(&mut out, &src[i..end], C_LITERAL);
                i = end;
                continue;
            }
            // Stray `'` — fold into default ink so the round-trip
            // is still exact.
            push(&mut out, "'", C_DEFAULT);
            i += 1;
            continue;
        }
        if b.is_ascii_digit() {
            let end = number_end(bytes, i);
            push(&mut out, &src[i..end], C_LITERAL);
            i = end;
            continue;
        }
        if is_ident_start(b) {
            let end = ident_end(bytes, i);
            let word = &src[i..end];
            // Two-bucket policy: keywords get the accent; everything
            // else (idents, types, the bool literals `true`/`false`
            // — which `is_keyword` classifies as keywords for the
            // parser, but read as default-ink data here) stays
            // default. Identifier types staying default keeps the
            // page calm; if the user really wants types differentiated
            // they'll tell us.
            let color = if is_keyword(word) { C_KEYWORD } else { C_DEFAULT };
            push(&mut out, word, color);
            i = end;
            continue;
        }
        if b.is_ascii_whitespace() {
            let end = ws_end(bytes, i);
            push(&mut out, &src[i..end], C_DEFAULT);
            i = end;
            continue;
        }
        // Punctuation runs (`=>` / `..=` / `::`) stay default —
        // we don't accent them because that's where the
        // GitHub-style palette starts to look "rainbow."
        let end = punct_end(bytes, i);
        push(&mut out, &src[i..end], C_DEFAULT);
        i = end;
    }
    out
}

fn push(out: &mut Vec<(String, Color)>, text: &str, color_hex: &str) {
    out.push((text.to_string(), Color(color_hex.to_string())));
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
            // close the string. We don't validate the escape
            // sequence further; the cargo compile catches malformed
            // ones if the user runs.
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
/// plus the reserved-for-future-use words. Kept as a sorted match —
/// `phf` would be cleaner but isn't worth a dep for ~50 entries
/// against a ~10k-char user file.
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
