//! Token streams as their author wrote them.
//!
//! The catalog shows code to people: a recipe's source, a prop example's
//! value. Pretty-printing the parsed tokens loses the author's layout, and
//! `prettyplease` can't format inside a macro call, so a `ui!` body comes
//! out flow-wrapped into one paragraph. [`verbatim`] rebuilds the text
//! from the source instead: each top-level token's own source text (a
//! delimited group's covers everything inside it, comments included),
//! placed by its span's line and column. Stable Rust can't join two spans
//! into one, so the text between tokens is reconstructed from positions
//! rather than read.
//!
//! Inside rustc this needs `Span::source_text` and line/column info (both
//! stable; the crate's `rust-version` already requires the latter). The
//! catalog scanner runs the same expansion outside rustc, where
//! proc-macro2's `span-locations` source map answers the same questions
//! about the parsed files, so both produce the same text. Tokens with no
//! source (built by another macro) give `None`, and callers fall back to
//! a pretty-printer.

use proc_macro2::{TokenStream, TokenTree};

/// `tokens` as written: the source text from the first token to the
/// last, with lines after the first dedented by their common margin.
/// `None` when any token has no source text.
pub(crate) fn verbatim(tokens: &TokenStream) -> Option<String> {
    let mut out = String::new();
    let mut prev_end: Option<proc_macro2::LineColumn> = None;
    for tt in tokens.clone() {
        let span = match &tt {
            TokenTree::Group(g) => g.span(),
            other => other.span(),
        };
        let text = span.source_text()?;
        let (start, end) = (span.start(), span.end());
        if let Some(prev) = prev_end {
            if start.line > prev.line {
                for _ in prev.line..start.line {
                    out.push('\n');
                }
                out.push_str(&" ".repeat(start.column));
            } else if start.line == prev.line && start.column > prev.column {
                out.push_str(&" ".repeat(start.column - prev.column));
            } else if start.line < prev.line || start.column < prev.column {
                // Out of order: not one contiguous stretch of source.
                return None;
            }
        }
        out.push_str(&text);
        prev_end = Some(end);
    }
    if prev_end.is_none() {
        return None;
    }
    Some(dedent_after_first(&out))
}

/// Lines after the first lose their shared leading whitespace: the first
/// line starts at its token, the rest carry the file's indentation.
fn dedent_after_first(text: &str) -> String {
    let mut lines = text.lines();
    let Some(first) = lines.next() else { return String::new() };
    let rest: Vec<&str> = lines.collect();
    let margin = rest
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    let mut out = first.trim_end().to_string();
    for line in rest {
        out.push('\n');
        out.push_str(line.get(margin..).unwrap_or("").trim_end());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    /// Outside rustc, spans of parsed text carry their source (the
    /// scanner's case): line breaks and the author's indentation inside a
    /// macro body survive, and the file's outer indentation is removed.
    #[test]
    fn keeps_the_authors_layout_inside_macro_bodies() {
        let src = "ui! {\n            Stack(gap = StackGap::Md) {\n                // the header\n                Typography(content = \"Profile\")\n                Typography(content = \"Details\", muted = true)\n            }\n        }";
        let tokens = TokenStream::from_str(src).unwrap();
        assert_eq!(
            verbatim(&tokens).unwrap(),
            "ui! {\n    Stack(gap = StackGap::Md) {\n        // the header\n        Typography(content = \"Profile\")\n        Typography(content = \"Details\", muted = true)\n    }\n}"
        );
    }

    #[test]
    fn spacing_between_top_level_tokens_follows_their_columns() {
        let tokens = TokenStream::from_str("vec![Crumb::new(\"Home\"),   Crumb::new(\"Docs\")]").unwrap();
        assert_eq!(verbatim(&tokens).unwrap(), "vec![Crumb::new(\"Home\"),   Crumb::new(\"Docs\")]");
        let tokens = TokenStream::from_str("pub fn a()\n    -> Element { x }").unwrap();
        assert_eq!(verbatim(&tokens).unwrap(), "pub fn a()\n-> Element { x }");
    }

    #[test]
    fn generated_tokens_have_no_source() {
        let tokens = quote::quote!(vec![1, 2]);
        assert_eq!(verbatim(&tokens), None);
        assert_eq!(verbatim(&TokenStream::new()), None);
    }
}
