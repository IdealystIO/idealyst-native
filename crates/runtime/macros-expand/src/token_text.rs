//! Token streams as text, exactly as rustc prints them.
//!
//! The catalog records several things as source text — a param's type
//! (`type_str`), an `animated!` initial value — produced by stringifying
//! tokens inside the macro. Inside rustc, `TokenStream::to_string()` is
//! rustc's own token printer (`rustc_ast_pretty`'s `tts_to_string`), and
//! the catalog has always carried its output: `Rc < dyn Fn(String) >`,
//! `& FooProps`, and — past 78 columns — a line break.
//!
//! The catalog scanner runs this same expansion outside rustc, where
//! `proc-macro2` falls back to its own printer, which spaces differently
//! (`Fn (String)`) and never wraps. So [`token_text`] keeps rustc's
//! printer when there is one and otherwise reproduces it:
//!
//! - **Spacing** ([`space_between`]): a space after every token printed
//!   `Alone`, except `x.y`, `$x`, `x,` / `x;` / `x.`, `f(…)` (an
//!   identifier — not a keyword other than `fn`/`Self`/`pub` — before a
//!   parenthesis), and `#[…]`. A `Joint` punct never takes one. Tokens
//!   reach the macro through the proc-macro bridge, which keeps `Joint`
//!   only between operator characters, so that is the only spacing rustc
//!   sees too.
//! - **Line breaking**: each of those spaces is a break in an Oppen
//!   pretty-printer with a 78-column margin. A break is taken when the
//!   text up to the next break at the same or an outer level does not fit
//!   on the line. `(…)` / `[…]` open an inconsistent box at the current
//!   indentation, `{…}` a consistent one indented by 4 — what rustc's
//!   `print_mac_common` does.
//!
//! Pinned against real rustc output by the catalog-scan parity tests,
//! which compare a compiled catalog with the scanned one field by field.

use proc_macro2::{Delimiter, Spacing, TokenStream, TokenTree};

/// `tokens` as rustc's `TokenStream::to_string()` prints them.
pub(crate) fn token_text(tokens: &TokenStream) -> String {
    if proc_macro::is_available() {
        // Inside rustc: its own printer, the reference.
        return tokens.to_string();
    }
    rustc_style(tokens)
}

/// The emulation, callable from tests inside or outside rustc.
pub(crate) fn rustc_style(tokens: &TokenStream) -> String {
    let mut pp = Vec::new();
    print_tts(tokens, &mut pp);
    layout(&pp)
}

/// Rustc's pretty-printer margin and the floor its line width never
/// drops below after a break (`rustc_ast_pretty::pp::{MARGIN, MIN_SPACE}`).
const MARGIN: isize = 78;
const MIN_SPACE: isize = 60;
/// `rustc_ast_pretty::pprust::state::INDENT_UNIT`.
const INDENT_UNIT: isize = 4;

#[derive(Debug, Clone)]
enum Pp {
    Text(String),
    Break { offset: isize },
    Begin { offset: isize, consistent: bool },
    End,
}

fn print_tts(tokens: &TokenStream, out: &mut Vec<Pp>) {
    let tts: Vec<TokenTree> = tokens.clone().into_iter().collect();
    for (i, tt) in tts.iter().enumerate() {
        print_tt(tt, out);
        if i + 1 < tts.len() && spacing(&tts, i) == Spacing::Alone && space_between(&tts, i) {
            out.push(Pp::Break { offset: 0 });
        }
    }
}

fn print_tt(tt: &TokenTree, out: &mut Vec<Pp>) {
    match tt {
        TokenTree::Group(g) => {
            let (open, close) = match g.delimiter() {
                Delimiter::Parenthesis => ("(", ")"),
                Delimiter::Bracket => ("[", "]"),
                Delimiter::None => ("", ""),
                Delimiter::Brace => {
                    let empty = g.stream().is_empty();
                    out.push(Pp::Begin { offset: INDENT_UNIT, consistent: true });
                    out.push(Pp::Text("{".into()));
                    if !empty {
                        out.push(Pp::Break { offset: 0 });
                    }
                    out.push(Pp::Begin { offset: 0, consistent: false });
                    print_tts(&g.stream(), out);
                    out.push(Pp::End);
                    if !empty {
                        out.push(Pp::Break { offset: -INDENT_UNIT });
                    }
                    out.push(Pp::Text("}".into()));
                    out.push(Pp::End);
                    return;
                }
            };
            if !open.is_empty() {
                out.push(Pp::Text(open.into()));
            }
            out.push(Pp::Begin { offset: 0, consistent: false });
            print_tts(&g.stream(), out);
            out.push(Pp::End);
            if !close.is_empty() {
                out.push(Pp::Text(close.into()));
            }
        }
        TokenTree::Ident(i) => out.push(Pp::Text(i.to_string())),
        TokenTree::Literal(l) => out.push(Pp::Text(l.to_string())),
        TokenTree::Punct(p) => out.push(Pp::Text(p.as_char().to_string())),
    }
}

/// The spacing rustc sees after `tts[i]`. Only a punct carries one; and a
/// punct before a lifetime's `'` is `Alone` to rustc (the lifetime is one
/// non-operator token there), where `proc-macro2`'s lexer marks it
/// `Joint`.
fn spacing(tts: &[TokenTree], i: usize) -> Spacing {
    match &tts[i] {
        TokenTree::Punct(p) if p.spacing() == Spacing::Joint => match tts.get(i + 1) {
            Some(TokenTree::Punct(n)) if n.as_char() == '\'' => Spacing::Alone,
            _ => Spacing::Joint,
        },
        _ => Spacing::Alone,
    }
}

/// Whether the punct at `tts[i]` is a lone `.` (not part of `..`, `...`,
/// `..=`), which is what rustc's `Dot` token is.
fn is_dot(tts: &[TokenTree], i: usize) -> bool {
    let joined_with = |j: Option<usize>| {
        j.and_then(|j| tts.get(j)).is_some_and(|t| matches!(t, TokenTree::Punct(p) if matches!(p.as_char(), '.' | '=')))
    };
    match &tts[i] {
        TokenTree::Punct(p) if p.as_char() == '.' => {
            let joint_after = p.spacing() == Spacing::Joint && joined_with(Some(i + 1));
            let joint_before = i > 0
                && matches!(&tts[i - 1], TokenTree::Punct(q) if q.as_char() == '.' && q.spacing() == Spacing::Joint);
            !joint_after && !joint_before
        }
        _ => false,
    }
}

fn is_punct(tt: &TokenTree) -> bool {
    matches!(tt, TokenTree::Punct(_))
}

fn punct_is(tt: &TokenTree, c: char) -> bool {
    matches!(tt, TokenTree::Punct(p) if p.as_char() == c)
}

/// rustc's `space_between(tt1, tt2)` for `tt1 = tts[i]`, `tt2 = tts[i + 1]`.
fn space_between(tts: &[TokenTree], i: usize) -> bool {
    let (a, b) = (&tts[i], &tts[i + 1]);
    // `.` + NON-PUNCT: `x.y`, `tup.0`
    if is_dot(tts, i) && !is_punct(b) {
        return false;
    }
    // `$` + IDENT: `$e`
    if punct_is(a, '$') && matches!(b, TokenTree::Ident(_)) {
        return false;
    }
    // NON-PUNCT + `,` / `;` / `.`
    if !is_punct(a) && (punct_is(b, ',') || punct_is(b, ';') || is_dot(tts, i + 1)) {
        return false;
    }
    // IDENT (not reserved, or `fn` / `Self` / `pub` / raw) + `(`
    if let (TokenTree::Ident(id), TokenTree::Group(g)) = (a, b) {
        if g.delimiter() == Delimiter::Parenthesis {
            let s = id.to_string();
            if s.starts_with("r#") || !is_reserved(&s) || matches!(s.as_str(), "fn" | "Self" | "pub") {
                return false;
            }
        }
    }
    // `#` + `[`: `#[attr]`
    if punct_is(a, '#') && matches!(b, TokenTree::Group(g) if g.delimiter() == Delimiter::Bracket) {
        return false;
    }
    true
}

/// rustc's reserved identifiers (strict and reserved keywords, edition
/// 2021, plus `_`).
fn is_reserved(s: &str) -> bool {
    matches!(
        s,
        "_" | "as" | "break" | "const" | "continue" | "crate" | "else" | "enum" | "extern" | "false" | "fn"
            | "for" | "if" | "impl" | "in" | "let" | "loop" | "match" | "mod" | "move" | "mut" | "pub"
            | "ref" | "return" | "self" | "Self" | "static" | "struct" | "super" | "trait" | "true"
            | "type" | "unsafe" | "use" | "where" | "while" | "async" | "await" | "dyn" | "abstract"
            | "become" | "box" | "do" | "final" | "macro" | "override" | "priv" | "typeof" | "unsized"
            | "virtual" | "yield" | "try" | "gen"
    )
}

/// Lay out the pretty-printer stream (Oppen's algorithm, as
/// `rustc_ast_pretty::pp` implements it).
fn layout(pp: &[Pp]) -> String {
    let n = pp.len();
    // Width of everything before each position, breaks counting as one.
    let mut pos = vec![0isize; n + 1];
    for (i, t) in pp.iter().enumerate() {
        pos[i + 1] = pos[i]
            + match t {
                Pp::Text(s) => s.len() as isize,
                Pp::Break { .. } => 1,
                _ => 0,
            };
    }
    // Nesting level of each token (a box's Begin/End at its outer level).
    let mut level = vec![0usize; n];
    let mut depth = 0usize;
    for (i, t) in pp.iter().enumerate() {
        if let Pp::End = t {
            depth -= 1;
        }
        level[i] = depth;
        if let Pp::Begin { .. } = t {
            depth += 1;
        }
    }
    // A break's size runs to the next break at its level or shallower
    // (or the end); a box's size runs from its Begin to the next break
    // outside it (or the end). That is when rustc's `check_stack`
    // resolves each.
    let next_break_at_or_above = |from: usize, lvl: usize| -> isize {
        let mut d = lvl;
        for j in from..n {
            match &pp[j] {
                Pp::End => d = d.min(level[j]),
                Pp::Break { .. } if level[j] <= d => return pos[j],
                _ => {}
            }
        }
        pos[n]
    };
    let size: Vec<isize> = (0..n)
        .map(|i| match &pp[i] {
            Pp::Break { .. } => next_break_at_or_above(i + 1, level[i]) - pos[i],
            Pp::Begin { .. } => next_break_at_or_above(i + 1, level[i]) - pos[i],
            _ => 0,
        })
        .collect();

    enum Frame {
        Fits,
        Broken { indent: isize, consistent: bool },
    }
    let mut out = String::new();
    let mut frames: Vec<Frame> = Vec::new();
    let mut space = MARGIN;
    let mut indent: isize = 0;
    let mut pending: isize = 0;
    for (i, t) in pp.iter().enumerate() {
        match t {
            Pp::Text(s) => {
                out.extend(std::iter::repeat(' ').take(pending.max(0) as usize));
                pending = 0;
                out.push_str(s);
                space -= s.len() as isize;
            }
            Pp::Begin { offset, consistent } => {
                if size[i] > space {
                    frames.push(Frame::Broken { indent, consistent: *consistent });
                    indent += offset;
                } else {
                    frames.push(Frame::Fits);
                }
            }
            Pp::End => {
                if let Some(Frame::Broken { indent: outer, .. }) = frames.pop() {
                    indent = outer;
                }
            }
            Pp::Break { offset } => {
                let fits = match frames.last() {
                    Some(Frame::Fits) => true,
                    Some(Frame::Broken { consistent: true, .. }) => false,
                    // The top level is an inconsistent broken box at 0.
                    Some(Frame::Broken { consistent: false, .. }) | None => size[i] <= space,
                };
                if fits {
                    pending += 1;
                    space -= 1;
                } else {
                    out.push('\n');
                    let new_indent = indent + offset;
                    pending = new_indent;
                    space = (MARGIN - new_indent).max(MIN_SPACE);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::rustc_style;
    use quote::quote;
    use std::str::FromStr;

    /// A type as the macro stringifies it: parsed by `syn`, re-emitted
    /// with `quote!` (`type_str`).
    fn ty(s: &str) -> String {
        let ty: syn::Type = syn::parse_str(s).unwrap();
        rustc_style(&quote!(#ty))
    }

    /// Raw source tokens, as `animated!`'s initial value is captured.
    fn raw(s: &str) -> String {
        rustc_style(&proc_macro2::TokenStream::from_str(s).unwrap())
    }

    /// Strings taken from a compiled catalog (rustc's printer).
    #[test]
    fn spacing_matches_rustc() {
        assert_eq!(ty("Rc<dyn Fn(String)>"), "Rc < dyn Fn(String) >");
        assert_eq!(ty("Option<Rc<dyn Fn()>>"), "Option < Rc < dyn Fn() > >");
        assert_eq!(ty("&AvatarProps"), "& AvatarProps");
        assert_eq!(ty("&'a Foo<T>"), "& 'a Foo < T >");
        assert_eq!(ty("::runtime_vocabulary::glue::Reactive<ToneRef>"), ":: runtime_vocabulary :: glue :: Reactive < ToneRef >");
        assert_eq!(ty("[u8; 4]"), "[u8; 4]");
        assert_eq!(raw("x.y.0"), "x.y.0");
        assert_eq!(raw("a..b"), "a .. b");
        assert_eq!(raw("#[doc = \"x\"]"), "#[doc = \"x\"]");
        assert_eq!(raw("let (a, b) = (1, 2)"), "let (a, b) = (1, 2)");
        assert_eq!(raw("0.0_f32"), "0.0_f32");
    }

    /// The one wrapped string in idea-ui's compiled catalog.
    #[test]
    fn long_text_wraps_at_rustcs_margin() {
        assert_eq!(
            ty("Option<runtime_core::Ref<runtime_core::primitives::text_input::TextInputHandle>>"),
            "Option < runtime_core :: Ref < runtime_core :: primitives :: text_input ::\nTextInputHandle > >"
        );
    }

    /// Tokens built by `quote!` (the inline-props `Reactive<…>` wrap)
    /// print the same as parsed ones.
    #[test]
    fn quoted_tokens_print_like_parsed_ones() {
        let inner = quote!(String);
        assert_eq!(rustc_style(&quote!(::runtime_core::Reactive<#inner>)), ":: runtime_core :: Reactive < String >");
    }
}
