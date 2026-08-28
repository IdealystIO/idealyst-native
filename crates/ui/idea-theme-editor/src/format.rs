//! Token values ↔ text, and the flat JSON save format.
//!
//! Every control in the editor edits TEXT, because that is what a text
//! input holds and what a save file carries. This module is the one
//! place that converts between that text and the [`TokenValue`] the
//! framework actually installs, so a round trip through a save file
//! cannot quietly change a value's type.

use idea_theme::{TokenValue, Tokenized};
use runtime_core::{Color, Length};

/// The payload a token carries, as the editor models it.
///
/// [`idea_theme::TokenKind`] covers the two kinds the vocabulary can
/// declare; this adds `Number`, which the vocabulary never emits but
/// [`TokenValue`] does — an extension token (a `tone!`'s
/// `tokens = [...]` block) is free to install one, and the editor
/// enumerates the live world, not just the vocabulary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DraftKind {
    /// A color string, passed through to the backend verbatim.
    Color,
    /// A CSS-ish length: `12px`, `50%`, `auto`, `full`.
    Length,
    /// A bare number.
    Number,
}

impl DraftKind {
    /// The kind matching a live value — how an extension token, which
    /// has no descriptor to declare a kind, gets one.
    pub fn of(value: &TokenValue) -> Self {
        match value {
            TokenValue::Color(_) => Self::Color,
            TokenValue::Length(_) => Self::Length,
            TokenValue::Number(_) => Self::Number,
        }
    }
}

impl From<idea_theme::TokenKind> for DraftKind {
    fn from(k: idea_theme::TokenKind) -> Self {
        match k {
            idea_theme::TokenKind::Color => Self::Color,
            idea_theme::TokenKind::Length => Self::Length,
        }
    }
}

/// Why a piece of text could not become a token value (or a save file
/// could not be read).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParseError {
    /// The control was left empty. Rejected rather than defaulted: a
    /// blank color would paint transparent and read as "the editor
    /// broke", not as "I cleared this".
    Empty,
    /// Not a length the framework can express.
    BadLength(String),
    /// Not a number.
    BadNumber(String),
    /// The save file is not a flat JSON object of string values.
    BadJson(String),
    /// The save file names a token the live theme doesn't have and the
    /// vocabulary doesn't declare, so there is no kind to parse it as.
    UnknownToken(String),
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(f, "value is empty"),
            Self::BadLength(t) => write!(f, "`{t}` is not a length (try `12px`, `50%`, `auto`, `full`)"),
            Self::BadNumber(t) => write!(f, "`{t}` is not a number"),
            Self::BadJson(m) => write!(f, "not a flat JSON object of string values: {m}"),
            Self::UnknownToken(n) => write!(f, "no token named `{n}` in this theme"),
        }
    }
}

impl std::error::Error for ParseError {}

// ---------------------------------------------------------------------------
// Value ↔ text
// ---------------------------------------------------------------------------

/// Render a length the way the editor (and the save file) spells it.
///
/// Mirrors idea-theme's private `length_display`, extended to round
/// trip: `Full` prints `full` rather than a number, because
/// `radius-pill` IS `Length::Full` and printing it as a number is how
/// the pill became a magic 999px in the first place.
pub fn format_length(l: Length) -> String {
    match l {
        Length::Px(v) => format!("{v}px"),
        Length::Percent(v) => format!("{v}%"),
        Length::Auto => "auto".to_string(),
        Length::Full => "full".to_string(),
    }
}

/// Render a token value as the text a control shows.
pub fn format_value(v: &TokenValue) -> String {
    match v {
        TokenValue::Color(c) => c.0.clone(),
        TokenValue::Length(l) => format_length(*l),
        TokenValue::Number(n) => format!("{n}"),
    }
}

/// Read a length back. Accepts what [`format_length`] writes, plus a
/// bare number (read as px) so typing `12` into a spacing control does
/// the obvious thing.
pub fn parse_length(text: &str) -> Result<Length, ParseError> {
    let t = text.trim();
    if t.is_empty() {
        return Err(ParseError::Empty);
    }
    let lower = t.to_ascii_lowercase();
    if lower == "auto" {
        return Ok(Length::Auto);
    }
    if lower == "full" {
        return Ok(Length::Full);
    }
    if let Some(n) = lower.strip_suffix("px") {
        return n.trim().parse::<f32>().map(Length::Px).map_err(|_| ParseError::BadLength(t.into()));
    }
    if let Some(n) = lower.strip_suffix('%') {
        return n
            .trim()
            .parse::<f32>()
            .map(Length::Percent)
            .map_err(|_| ParseError::BadLength(t.into()));
    }
    // Bare number → px. The scales are all px, so this is the common
    // typing path, not a fallback.
    lower.parse::<f32>().map(Length::Px).map_err(|_| ParseError::BadLength(t.into()))
}

/// Read a token value back from a control's text, as `kind`.
///
/// Colors are NOT validated beyond emptiness. The framework passes a
/// color string to the backend verbatim, so every form the platform
/// accepts (`#f0f`, `rgba(…)`, `color-mix(…)` on web) has to survive
/// this function — a hex-only check here would reject values the app
/// can legitimately paint.
pub fn parse_value(kind: DraftKind, text: &str) -> Result<TokenValue, ParseError> {
    let t = text.trim();
    if t.is_empty() {
        return Err(ParseError::Empty);
    }
    match kind {
        DraftKind::Color => Ok(TokenValue::Color(Color(t.to_string()))),
        DraftKind::Length => parse_length(t).map(TokenValue::Length),
        DraftKind::Number => {
            t.parse::<f32>().map(TokenValue::Number).map_err(|_| ParseError::BadNumber(t.into()))
        }
    }
}

/// A token value as an `f32` literal Rust will accept where an `f32` is
/// expected.
///
/// `format!("{}", 12.0f32)` yields `12`, and `theme.spacing.md = 12;`
/// does not compile — an integer literal is not an `f32`. Generated
/// source is pasted into someone else's crate, where a type error is
/// ours and a compile cycle away from being found, so the decimal point
/// is added here.
pub fn rust_f32(v: f32) -> String {
    let s = format!("{v}");
    if s.contains('.') || s.contains('e') || s.contains("inf") || s.contains("NaN") {
        s
    } else {
        format!("{s}.0")
    }
}

/// A color as the `Tokenized<Color>` expression a theme field takes.
pub fn rust_color(c: &Color) -> String {
    format!("Tokenized::Literal(Color({:?}.into()))", c.0)
}

/// A token value as the `TokenValue` expression an `update_tokens` call
/// takes — the fallback for tokens with no theme field to assign.
pub fn rust_token_value(v: &TokenValue) -> String {
    match v {
        TokenValue::Color(c) => format!("TokenValue::Color(Color({:?}.into()))", c.0),
        TokenValue::Length(Length::Px(n)) => {
            format!("TokenValue::Length(Length::Px({}))", rust_f32(*n))
        }
        TokenValue::Length(Length::Percent(n)) => {
            format!("TokenValue::Length(Length::Percent({}))", rust_f32(*n))
        }
        TokenValue::Length(Length::Auto) => "TokenValue::Length(Length::Auto)".to_string(),
        TokenValue::Length(Length::Full) => "TokenValue::Length(Length::Full)".to_string(),
        TokenValue::Number(n) => format!("TokenValue::Number({})", rust_f32(*n)),
    }
}

/// The inline swatch background for a color token: a reference to the
/// token itself, so the swatch re-tints from the live registry the
/// moment an edit commits — no second write, no chance of the swatch
/// and the app disagreeing about what the token is.
pub fn swatch_color(name: &'static str, fallback: &Color) -> Tokenized<Color> {
    Tokenized::token(name, fallback.clone())
}

// ---------------------------------------------------------------------------
// The save format: a flat JSON object of string values
// ---------------------------------------------------------------------------
//
// Hand-rolled rather than serde_json because the shape is fixed and
// tiny (object → string values, one level, no arrays or numbers) and
// no UI crate in this tree pulls serde_json into a bundle. The subset
// is small enough to be pinned by tests; `\u` escapes are REJECTED
// rather than half-supported, so a file this can't represent fails
// loudly instead of loading a corrupted value.

/// Escape a string into a JSON string literal, quotes included.
fn write_json_string(s: &str, out: &mut String) {
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            // Other control characters can't appear in a token name or
            // value, but emitting them raw would produce invalid JSON,
            // so they go out as the escapes JSON requires.
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Serialize `pairs` as a pretty flat JSON object, in the order given.
///
/// Order is the caller's (descriptor order), not sorted: a save file
/// that reads in the same order the editor lays out is reviewable as a
/// diff, which a hash-ordered one is not.
pub fn write_json(pairs: &[(&str, String)]) -> String {
    let mut out = String::from("{\n");
    for (i, (name, value)) in pairs.iter().enumerate() {
        out.push_str("  ");
        write_json_string(name, &mut out);
        out.push_str(": ");
        write_json_string(value, &mut out);
        if i + 1 < pairs.len() {
            out.push(',');
        }
        out.push('\n');
    }
    out.push('}');
    out
}

/// Parse a flat JSON object of string values, preserving file order.
///
/// Deliberately narrow: anything else in the document — a nested
/// object, an array, a bare number or `null` as a value — is an error
/// rather than something to coerce. A save file is generated by this
/// module; a file that isn't shaped like one is more likely the wrong
/// file than a file to guess at.
pub fn read_json(src: &str) -> Result<Vec<(String, String)>, ParseError> {
    let b: Vec<char> = src.chars().collect();
    let mut i = 0usize;
    let err = |m: &str| ParseError::BadJson(m.to_string());

    let skip_ws = |i: &mut usize| {
        while *i < b.len() && b[*i].is_whitespace() {
            *i += 1;
        }
    };

    // One JSON string literal starting at `i` (which must be `"`).
    let read_string = |i: &mut usize| -> Result<String, ParseError> {
        if *i >= b.len() || b[*i] != '"' {
            return Err(ParseError::BadJson("expected a quoted string".into()));
        }
        *i += 1;
        let mut s = String::new();
        loop {
            if *i >= b.len() {
                return Err(ParseError::BadJson("unterminated string".into()));
            }
            match b[*i] {
                '"' => {
                    *i += 1;
                    return Ok(s);
                }
                '\\' => {
                    *i += 1;
                    if *i >= b.len() {
                        return Err(ParseError::BadJson("trailing escape".into()));
                    }
                    match b[*i] {
                        '"' => s.push('"'),
                        '\\' => s.push('\\'),
                        '/' => s.push('/'),
                        'b' => s.push('\u{8}'),
                        'f' => s.push('\u{c}'),
                        'n' => s.push('\n'),
                        'r' => s.push('\r'),
                        't' => s.push('\t'),
                        'u' => {
                            return Err(ParseError::BadJson(
                                "\\u escapes are not supported — token names and values are \
                                 plain text"
                                    .into(),
                            ))
                        }
                        c => {
                            return Err(ParseError::BadJson(format!("unknown escape `\\{c}`")));
                        }
                    }
                    *i += 1;
                }
                c => {
                    s.push(c);
                    *i += 1;
                }
            }
        }
    };

    skip_ws(&mut i);
    if i >= b.len() || b[i] != '{' {
        return Err(err("expected `{`"));
    }
    i += 1;

    let mut out = Vec::new();
    skip_ws(&mut i);
    if i < b.len() && b[i] == '}' {
        return Ok(out);
    }
    loop {
        skip_ws(&mut i);
        let key = read_string(&mut i)?;
        skip_ws(&mut i);
        if i >= b.len() || b[i] != ':' {
            return Err(err("expected `:` after a key"));
        }
        i += 1;
        skip_ws(&mut i);
        if i < b.len() && b[i] != '"' {
            return Err(ParseError::BadJson(format!(
                "value for `{key}` is not a string — this format stores every token as text"
            )));
        }
        let value = read_string(&mut i)?;
        out.push((key, value));
        skip_ws(&mut i);
        if i >= b.len() {
            return Err(err("unterminated object"));
        }
        match b[i] {
            ',' => i += 1,
            '}' => {
                i += 1;
                break;
            }
            c => return Err(ParseError::BadJson(format!("expected `,` or `}}`, found `{c}`"))),
        }
    }
    skip_ws(&mut i);
    if i != b.len() {
        return Err(err("trailing content after the object"));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn length_round_trips_every_variant() {
        for l in [Length::Px(12.0), Length::Percent(50.0), Length::Auto, Length::Full] {
            let text = format_length(l);
            assert_eq!(parse_length(&text), Ok(l), "`{text}` must round trip");
        }
    }

    /// `radius-pill` is `Length::Full`. Printing it as a number is
    /// exactly how the pill became a magic `999px` that stopped being a
    /// pill above 1998px — so the round trip is asserted by name.
    #[test]
    fn pill_round_trips_as_full_not_a_number() {
        assert_eq!(format_length(Length::Full), "full");
        assert_eq!(parse_length("full"), Ok(Length::Full));
        assert_eq!(parse_length("FULL"), Ok(Length::Full), "case-insensitive");
    }

    #[test]
    fn bare_number_reads_as_px() {
        assert_eq!(parse_length("14"), Ok(Length::Px(14.0)));
        assert_eq!(parse_length(" 14 "), Ok(Length::Px(14.0)));
        assert_eq!(parse_length("14.5px"), Ok(Length::Px(14.5)));
    }

    #[test]
    fn bad_length_is_rejected_not_defaulted() {
        assert_eq!(parse_length("wide"), Err(ParseError::BadLength("wide".into())));
        assert_eq!(parse_length(""), Err(ParseError::Empty));
        assert_eq!(parse_length("  "), Err(ParseError::Empty));
    }

    /// A color is whatever the backend accepts, so the parser passes it
    /// through. A hex-only check here would reject `rgba(…)`, which the
    /// base palette itself uses for `color-overlay`.
    #[test]
    fn color_passes_through_every_platform_form() {
        for text in ["#fff", "#ffffff", "rgba(15, 23, 42, 0.45)", "transparent"] {
            assert_eq!(
                parse_value(DraftKind::Color, text),
                Ok(TokenValue::Color(Color(text.into()))),
            );
        }
        assert_eq!(parse_value(DraftKind::Color, "  #abc  "), Ok(TokenValue::Color(Color("#abc".into()))));
        assert_eq!(parse_value(DraftKind::Color, ""), Err(ParseError::Empty));
    }

    /// The reason `rust_f32` exists: `theme.spacing.md = 12;` does not
    /// compile, and the error would land in the user's crate.
    #[test]
    fn rust_f32_always_emits_a_float_literal() {
        assert_eq!(rust_f32(12.0), "12.0");
        assert_eq!(rust_f32(12.5), "12.5");
        assert_eq!(rust_f32(0.0), "0.0");
        assert_eq!(rust_f32(-3.0), "-3.0");
        for v in [12.0f32, 0.5, 1e10, -0.0] {
            let s = rust_f32(v);
            assert!(s.contains('.') || s.contains('e'), "`{s}` is not an f32 literal");
        }
    }

    #[test]
    fn rust_color_quotes_and_escapes() {
        // `r##`, not `r#`: the value itself contains `"#`, which would
        // close a single-hash raw string early.
        assert_eq!(
            rust_color(&Color("#fff".into())),
            r##"Tokenized::Literal(Color("#fff".into()))"##
        );
        // A quote inside a color string would otherwise close the
        // literal and paste as a syntax error.
        assert!(rust_color(&Color("a\"b".into())).contains(r#"\""#));
    }

    #[test]
    fn json_round_trips_in_order() {
        let pairs = vec![
            ("color-surface", "#ffffff".to_string()),
            ("color-overlay", "rgba(15, 23, 42, 0.45)".to_string()),
            ("spacing-md", "12px".to_string()),
            ("radius-pill", "full".to_string()),
        ];
        let text = write_json(&pairs);
        let back = read_json(&text).expect("round trip");
        let expected: Vec<(String, String)> =
            pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
        assert_eq!(back, expected, "order is preserved so a save file diffs cleanly");
    }

    #[test]
    fn json_escapes_survive() {
        let pairs = vec![("weird\"name", "a\\b\"c\nd".to_string())];
        let back = read_json(&write_json(&pairs)).unwrap();
        assert_eq!(back, vec![("weird\"name".to_string(), "a\\b\"c\nd".to_string())]);
    }

    #[test]
    fn json_empty_object_is_valid() {
        assert_eq!(read_json("{}"), Ok(vec![]));
        assert_eq!(read_json("  {  }  "), Ok(vec![]));
        assert_eq!(write_json(&[]), "{\n}");
        assert_eq!(read_json(&write_json(&[])), Ok(vec![]));
    }

    /// Malformed input fails loudly. A save file is machine-written; a
    /// file that doesn't parse is more likely the WRONG file than one
    /// to guess at, and a silent partial load would leave the theme in
    /// a state no one chose.
    #[test]
    fn malformed_json_is_rejected() {
        for bad in [
            "",
            "[]",
            "{",
            "{\"a\"}",
            "{\"a\": }",
            "{\"a\": 12}",
            "{\"a\": null}",
            "{\"a\": {\"b\": \"c\"}}",
            "{\"a\": \"b\",}",
            "{\"a\": \"b\"} trailing",
            "{\"a\": \"unterminated}",
        ] {
            assert!(read_json(bad).is_err(), "`{bad}` must not parse");
        }
    }

    /// `\u` is rejected with a message rather than silently mangled —
    /// the one escape this subset does not implement.
    #[test]
    fn unsupported_unicode_escape_names_itself() {
        let e = read_json(r#"{"a": "\u0041"}"#).unwrap_err();
        assert!(format!("{e}").contains("\\u"), "the error must name the escape: {e}");
    }

    #[test]
    fn draft_kind_follows_the_live_value() {
        assert_eq!(DraftKind::of(&TokenValue::Color(Color("#fff".into()))), DraftKind::Color);
        assert_eq!(DraftKind::of(&TokenValue::Length(Length::Px(1.0))), DraftKind::Length);
        assert_eq!(DraftKind::of(&TokenValue::Number(1.0)), DraftKind::Number);
    }
}
