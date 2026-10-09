//! A `macro_rules!` interpreter, for the declarative macros that expand
//! to catalog macros.
//!
//! Apps declare catalog entries through `macro_rules!` more often than one
//! would guess: idea-theme's `tone!` / `variant!` emit
//! `#[derive(IdealystSchema)]` markers, and an app's own helper macro may
//! stamp out several `#[props] #[derive(IdealystSchema)]` structs. A
//! source scan that skipped those would drop real entries, so the scanner
//! expands them — only them: a macro is expanded when its body, or a macro
//! its body invokes, mentions a catalog macro (see [`MacroTable`]); every
//! other invocation (`diesel::table!`, `stylesheet!`, …) is left alone.
//!
//! The matcher is the usual one: literal tokens, `$name:fragment`
//! captures, `$( … ) sep op` repetitions, nested groups. Fragments are
//! parsed with `syn`, so `expr`/`ty`/`path`/… end where rustc's would.
//! Rules are tried in order and the first that matches the whole input
//! wins; repetitions are matched greedily, one iteration at a time on a
//! fork, which is how well-formed macros are written (rustc rejects the
//! locally-ambiguous ones). An input no rule matches is an error, and the
//! scanner treats it as "cannot scan this workspace", never as "no
//! entries".
//!
//! Captured `expr`/`ty`/… fragments are substituted inside an invisible
//! (`None`-delimited) group, which is how rustc keeps a substituted
//! `$e:expr` one expression; `tt`, `ident`, `lifetime` and `literal`
//! captures are substituted as their bare tokens. Tokens keep their
//! spans: captured ones point at the invocation, the macro body's at the
//! definition — the same split rustc makes, which is what the `composes`
//! edge lines read.

use std::collections::HashMap;

use proc_macro2::{Delimiter, Group, Ident, Spacing, Span, TokenStream, TokenTree};
use syn::parse::{ParseStream, Parser};

/// One `macro_rules!` definition.
#[derive(Debug, Clone)]
pub struct MacroDef {
    pub name: String,
    rules: Vec<Rule>,
    /// Why the rules could not be read, for a definition kept anyway (so
    /// invoking it refuses the invoking crate instead of silently
    /// expanding to nothing).
    broken: Option<String>,
    /// What `$crate` expands to: the defining crate's name as seen from
    /// the invocation (`crate` for a macro defined in the scanned crate).
    pub crate_path: String,
    /// The raw body, for deciding whether expanding it matters.
    pub body: TokenStream,
}

#[derive(Debug, Clone)]
struct Rule {
    matcher: Vec<Matcher>,
    transcriber: TokenStream,
}

#[derive(Debug, Clone)]
enum Matcher {
    Token(TokenTree),
    Group(Delimiter, Vec<Matcher>),
    Var(String, Frag),
    Rep { body: Vec<Matcher>, sep: Option<TokenTree>, op: char },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Frag {
    Ident,
    Tt,
    Lifetime,
    Literal,
    Block,
    Expr,
    Ty,
    Path,
    Pat,
    PatParam,
    Stmt,
    Item,
    Meta,
    Vis,
}

impl Frag {
    fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "ident" => Frag::Ident,
            "tt" => Frag::Tt,
            "lifetime" => Frag::Lifetime,
            "literal" => Frag::Literal,
            "block" => Frag::Block,
            "expr" | "expr_2021" => Frag::Expr,
            "ty" => Frag::Ty,
            "path" => Frag::Path,
            "pat" => Frag::Pat,
            "pat_param" => Frag::PatParam,
            "stmt" => Frag::Stmt,
            "item" => Frag::Item,
            "meta" => Frag::Meta,
            "vis" => Frag::Vis,
            _ => return None,
        })
    }

    /// Whether a capture of this kind is substituted as bare tokens rather
    /// than inside an invisible group.
    fn is_bare(self) -> bool {
        matches!(self, Frag::Ident | Frag::Tt | Frag::Lifetime | Frag::Literal | Frag::Vis)
    }
}

/// What a matcher captured.
#[derive(Debug, Clone)]
enum Capture {
    One(TokenStream, Frag),
    Many(Vec<Capture>),
}

type Bindings = HashMap<String, Capture>;

impl MacroDef {
    /// Parse the body of `macro_rules! name { … }`: rules
    /// `(matcher) => {transcriber}` separated by `;`.
    pub fn parse(name: &str, body: TokenStream, crate_path: &str) -> Result<Self, String> {
        let tokens: Vec<TokenTree> = body.clone().into_iter().collect();
        let mut rules = Vec::new();
        let mut i = 0;
        while i < tokens.len() {
            let TokenTree::Group(matcher) = &tokens[i] else {
                return Err(format!("macro `{name}`: expected a rule's matcher group"));
            };
            let arrow = (tokens.get(i + 1), tokens.get(i + 2));
            let (Some(TokenTree::Punct(eq)), Some(TokenTree::Punct(gt))) = arrow else {
                return Err(format!("macro `{name}`: expected `=>` after a matcher"));
            };
            if eq.as_char() != '=' || gt.as_char() != '>' {
                return Err(format!("macro `{name}`: expected `=>` after a matcher"));
            }
            let Some(TokenTree::Group(transcriber)) = tokens.get(i + 3) else {
                return Err(format!("macro `{name}`: expected a rule's transcriber group"));
            };
            rules.push(Rule { matcher: parse_matcher(matcher.stream())?, transcriber: transcriber.stream() });
            i += 4;
            if let Some(TokenTree::Punct(p)) = tokens.get(i) {
                if p.as_char() == ';' {
                    i += 1;
                }
            }
        }
        Ok(MacroDef { name: name.to_string(), rules, broken: None, crate_path: crate_path.to_string(), body })
    }

    /// [`MacroDef::parse`], keeping a definition whose rules do not parse
    /// as one that fails to expand.
    pub fn parse_or_broken(name: &str, body: TokenStream, crate_path: &str) -> Self {
        Self::parse(name, body.clone(), crate_path).unwrap_or_else(|why| MacroDef {
            name: name.to_string(),
            rules: Vec::new(),
            broken: Some(why),
            crate_path: crate_path.to_string(),
            body,
        })
    }

    /// Expand one invocation's input with the first rule that matches it.
    pub fn expand(&self, input: TokenStream) -> Result<TokenStream, String> {
        if let Some(why) = &self.broken {
            return Err(format!("cannot read the rules of `{}!`: {why}", self.name));
        }
        let tokens: Vec<TokenTree> = input.into_iter().collect();
        for rule in &self.rules {
            let mut bindings = Bindings::new();
            if let Some(end) = match_seq(&rule.matcher, &tokens, 0, &mut bindings) {
                if end == tokens.len() {
                    return transcribe(rule.transcriber.clone(), &bindings, &mut Vec::new(), &self.crate_path);
                }
            }
        }
        Err(format!("no rule of `{}!` matches this invocation", self.name))
    }
}

fn parse_matcher(stream: TokenStream) -> Result<Vec<Matcher>, String> {
    let tokens: Vec<TokenTree> = stream.into_iter().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        match &tokens[i] {
            TokenTree::Punct(p) if p.as_char() == '$' => match tokens.get(i + 1) {
                Some(TokenTree::Ident(name)) => {
                    let name = name.to_string();
                    let frag = match (tokens.get(i + 2), tokens.get(i + 3)) {
                        (Some(TokenTree::Punct(colon)), Some(TokenTree::Ident(frag))) if colon.as_char() == ':' => {
                            Frag::parse(&frag.to_string()).ok_or_else(|| format!("unknown fragment specifier `{frag}`"))?
                        }
                        _ => return Err(format!("`${name}` in a matcher has no fragment specifier")),
                    };
                    out.push(Matcher::Var(name, frag));
                    i += 4;
                }
                Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Parenthesis => {
                    let body = parse_matcher(g.stream())?;
                    let (sep, op, used) = rep_suffix(&tokens[i + 2..])?;
                    out.push(Matcher::Rep { body, sep, op });
                    i += 2 + used;
                }
                _ => {
                    out.push(Matcher::Token(tokens[i].clone()));
                    i += 1;
                }
            },
            TokenTree::Group(g) => {
                out.push(Matcher::Group(g.delimiter(), parse_matcher(g.stream())?));
                i += 1;
            }
            tt => {
                out.push(Matcher::Token(tt.clone()));
                i += 1;
            }
        }
    }
    Ok(out)
}

/// The `sep? op` after a `$( … )`: returns (separator, operator, tokens
/// consumed).
fn rep_suffix(rest: &[TokenTree]) -> Result<(Option<TokenTree>, char, usize), String> {
    let op_of = |tt: Option<&TokenTree>| match tt {
        Some(TokenTree::Punct(p)) if matches!(p.as_char(), '*' | '+' | '?') => Some(p.as_char()),
        _ => None,
    };
    if let Some(op) = op_of(rest.first()) {
        return Ok((None, op, 1));
    }
    match (rest.first(), op_of(rest.get(1))) {
        (Some(sep), Some(op)) => Ok((Some(sep.clone()), op, 2)),
        _ => Err("a `$( … )` repetition has no `*`, `+` or `?`".to_string()),
    }
}

/// Match `matchers` against `tokens[pos..]`; the position after the last
/// consumed token, or `None`.
fn match_seq(matchers: &[Matcher], tokens: &[TokenTree], mut pos: usize, b: &mut Bindings) -> Option<usize> {
    for m in matchers {
        pos = match m {
            Matcher::Token(want) => {
                let got = tokens.get(pos)?;
                if !same_token(want, got) {
                    return None;
                }
                pos + 1
            }
            Matcher::Group(delim, inner) => {
                let TokenTree::Group(g) = tokens.get(pos)? else { return None };
                if g.delimiter() != *delim {
                    return None;
                }
                let inner_tokens: Vec<TokenTree> = g.stream().into_iter().collect();
                let end = match_seq(inner, &inner_tokens, 0, b)?;
                if end != inner_tokens.len() {
                    return None;
                }
                pos + 1
            }
            Matcher::Var(name, frag) => {
                let (captured, end) = match_fragment(*frag, tokens, pos)?;
                b.insert(name.clone(), Capture::One(captured, *frag));
                end
            }
            Matcher::Rep { body, sep, op } => {
                let mut iterations: Vec<Bindings> = Vec::new();
                let mut cur = pos;
                loop {
                    if *op == '?' && iterations.len() == 1 {
                        break;
                    }
                    let mut at = cur;
                    if !iterations.is_empty() {
                        if let Some(sep) = sep {
                            match tokens.get(at) {
                                Some(t) if same_token(sep, t) => at += 1,
                                _ => break,
                            }
                        }
                    }
                    let mut ib = Bindings::new();
                    match match_seq(body, tokens, at, &mut ib) {
                        // An iteration must consume something, or a body
                        // that can match nothing would repeat forever.
                        Some(end) if end > at => {
                            iterations.push(ib);
                            cur = end;
                        }
                        _ => break,
                    }
                }
                if *op == '+' && iterations.is_empty() {
                    return None;
                }
                for name in rep_vars(body) {
                    let seq = iterations.iter().map(|ib| ib.get(&name).cloned().unwrap_or(Capture::Many(Vec::new()))).collect();
                    b.insert(name, Capture::Many(seq));
                }
                cur
            }
        };
    }
    Some(pos)
}

/// Every variable bound anywhere inside a repetition body.
fn rep_vars(body: &[Matcher]) -> Vec<String> {
    let mut out = Vec::new();
    for m in body {
        match m {
            Matcher::Var(name, _) => out.push(name.clone()),
            Matcher::Group(_, inner) | Matcher::Rep { body: inner, .. } => out.extend(rep_vars(inner)),
            Matcher::Token(_) => {}
        }
    }
    out
}

fn same_token(a: &TokenTree, b: &TokenTree) -> bool {
    match (a, b) {
        (TokenTree::Punct(x), TokenTree::Punct(y)) => x.as_char() == y.as_char(),
        (TokenTree::Ident(x), TokenTree::Ident(y)) => x == y,
        (TokenTree::Literal(x), TokenTree::Literal(y)) => x.to_string() == y.to_string(),
        (TokenTree::Group(x), TokenTree::Group(y)) => {
            x.delimiter() == y.delimiter() && x.stream().to_string() == y.stream().to_string()
        }
        _ => false,
    }
}

/// Parse one fragment at `tokens[pos..]`: its tokens and the position
/// after them.
fn match_fragment(frag: Frag, tokens: &[TokenTree], pos: usize) -> Option<(TokenStream, usize)> {
    let take = |n: usize| -> Option<(TokenStream, usize)> {
        (pos + n <= tokens.len()).then(|| (tokens[pos..pos + n].iter().cloned().collect(), pos + n))
    };
    match frag {
        Frag::Tt => take(1),
        Frag::Ident => match tokens.get(pos)? {
            TokenTree::Ident(i) if i != "_" => take(1),
            _ => None,
        },
        Frag::Lifetime => match (tokens.get(pos)?, tokens.get(pos + 1)?) {
            (TokenTree::Punct(p), TokenTree::Ident(_)) if p.as_char() == '\'' => take(2),
            _ => None,
        },
        Frag::Literal => match tokens.get(pos)? {
            TokenTree::Literal(_) => take(1),
            TokenTree::Ident(i) if i == "true" || i == "false" => take(1),
            TokenTree::Punct(p) if p.as_char() == '-' => match tokens.get(pos + 1)? {
                TokenTree::Literal(_) => take(2),
                _ => None,
            },
            _ => None,
        },
        _ => {
            // Parse with `syn` against the remaining tokens and count what
            // it left: fragments always end on a token-tree boundary.
            let rest: TokenStream = tokens[pos..].iter().cloned().collect();
            let total = tokens.len() - pos;
            let parser = |input: ParseStream| -> syn::Result<usize> {
                match frag {
                    Frag::Block => drop(input.parse::<syn::Block>()?),
                    Frag::Expr => drop(input.parse::<syn::Expr>()?),
                    Frag::Ty => drop(input.parse::<syn::Type>()?),
                    Frag::Path => drop(input.parse::<syn::TypePath>()?),
                    Frag::Pat => drop(syn::Pat::parse_multi_with_leading_vert(input)?),
                    Frag::PatParam => drop(syn::Pat::parse_single(input)?),
                    Frag::Stmt => drop(input.parse::<syn::Stmt>()?),
                    Frag::Item => drop(input.parse::<syn::Item>()?),
                    Frag::Meta => drop(input.parse::<syn::Meta>()?),
                    Frag::Vis => drop(input.parse::<syn::Visibility>()?),
                    Frag::Tt | Frag::Ident | Frag::Lifetime | Frag::Literal => unreachable!("matched above"),
                }
                let left: TokenStream = input.parse()?;
                Ok(left.into_iter().count())
            };
            let left = parser.parse2(rest).ok()?;
            take(total - left)
        }
    }
}

/// Substitute `bindings` into a transcriber. `idx` is the current index
/// in each enclosing repetition.
fn transcribe(stream: TokenStream, b: &Bindings, idx: &mut Vec<usize>, crate_path: &str) -> Result<TokenStream, String> {
    let tokens: Vec<TokenTree> = stream.into_iter().collect();
    let mut out = TokenStream::new();
    let mut i = 0;
    while i < tokens.len() {
        match &tokens[i] {
            TokenTree::Punct(p) if p.as_char() == '$' => match tokens.get(i + 1) {
                Some(TokenTree::Ident(name)) if name == "crate" => {
                    out.extend(crate_tokens(crate_path, name.span()));
                    i += 2;
                }
                Some(TokenTree::Ident(name)) => {
                    match lookup(b, &name.to_string(), idx) {
                        Some((ts, frag)) if frag.is_bare() => out.extend(ts),
                        Some((ts, _)) => out.extend([TokenTree::Group(Group::new(Delimiter::None, ts))]),
                        // Not a capture: rustc would reject the macro;
                        // keep the tokens so the error is visible.
                        None => out.extend([tokens[i].clone(), tokens[i + 1].clone()]),
                    }
                    i += 2;
                }
                Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Parenthesis => {
                    let (sep, _op, used) = rep_suffix(&tokens[i + 2..])?;
                    let count = rep_count(g.stream(), b, idx)
                        .ok_or_else(|| "a `$( … )` in a transcriber repeats no captured variable".to_string())?;
                    for n in 0..count {
                        if n > 0 {
                            if let Some(sep) = &sep {
                                out.extend([sep.clone()]);
                            }
                        }
                        idx.push(n);
                        let piece = transcribe(g.stream(), b, idx, crate_path);
                        idx.pop();
                        out.extend(piece?);
                    }
                    i += 2 + used;
                }
                _ => {
                    out.extend([tokens[i].clone()]);
                    i += 1;
                }
            },
            TokenTree::Group(g) => {
                let mut ng = Group::new(g.delimiter(), transcribe(g.stream(), b, idx, crate_path)?);
                ng.set_span(g.span());
                out.extend([TokenTree::Group(ng)]);
                i += 1;
            }
            tt => {
                out.extend([tt.clone()]);
                i += 1;
            }
        }
    }
    Ok(out)
}

/// `$crate` as tokens: `crate`, or `::the_crate`.
fn crate_tokens(crate_path: &str, span: Span) -> TokenStream {
    if crate_path == "crate" {
        return TokenStream::from_iter([TokenTree::Ident(Ident::new("crate", span))]);
    }
    let mut colons = proc_macro2::Punct::new(':', Spacing::Joint);
    colons.set_span(span);
    let mut second = proc_macro2::Punct::new(':', Spacing::Alone);
    second.set_span(span);
    TokenStream::from_iter([
        TokenTree::Punct(colons),
        TokenTree::Punct(second),
        TokenTree::Ident(Ident::new(crate_path, span)),
    ])
}

/// The capture `name` at the current repetition depth.
fn lookup(b: &Bindings, name: &str, idx: &[usize]) -> Option<(TokenStream, Frag)> {
    let mut cap = b.get(name)?;
    let mut depth = 0;
    loop {
        match cap {
            Capture::One(ts, frag) => return Some((ts.clone(), *frag)),
            Capture::Many(items) => {
                cap = items.get(*idx.get(depth)?)?;
                depth += 1;
            }
        }
    }
}

/// How many times a transcriber repetition runs: the length of the
/// repeated captures it names at this depth (they must agree, as rustc
/// requires).
fn rep_count(stream: TokenStream, b: &Bindings, idx: &[usize]) -> Option<usize> {
    let mut count = None;
    let mut names = Vec::new();
    collect_var_names(stream, &mut names);
    for name in names {
        let Some(mut cap) = b.get(&name) else { continue };
        let mut depth = 0;
        let len = loop {
            match cap {
                Capture::One(..) => break None,
                Capture::Many(items) if depth == idx.len() => break Some(items.len()),
                Capture::Many(items) => match items.get(idx[depth]) {
                    Some(c) => {
                        cap = c;
                        depth += 1;
                    }
                    None => break None,
                },
            }
        };
        if let Some(len) = len {
            count = Some(count.map_or(len, |c: usize| c.min(len)));
        }
    }
    count
}

fn collect_var_names(stream: TokenStream, out: &mut Vec<String>) {
    let tokens: Vec<TokenTree> = stream.into_iter().collect();
    for (i, tt) in tokens.iter().enumerate() {
        match tt {
            TokenTree::Punct(p) if p.as_char() == '$' => {
                if let Some(TokenTree::Ident(name)) = tokens.get(i + 1) {
                    out.push(name.to_string());
                }
            }
            TokenTree::Group(g) => collect_var_names(g.stream(), out),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;

    fn squash(ts: TokenStream) -> String {
        ts.to_string().chars().filter(|c| !c.is_whitespace()).collect()
    }

    #[test]
    fn expands_captures_repetitions_and_dollar_crate() {
        let def = MacroDef::parse(
            "state_props",
            quote! {
                ($name:ident) => {
                    #[props]
                    #[derive(IdealystSchema)]
                    struct $name { state: $crate::State }
                };
                ($($name:ident),+ $(,)?) => { $( state_props!($name); )+ };
            },
            "my_crate",
        )
        .unwrap();
        assert_eq!(
            squash(def.expand(quote!(DoorsProps)).unwrap()),
            "#[props]#[derive(IdealystSchema)]structDoorsProps{state:::my_crate::State}"
        );
        assert_eq!(squash(def.expand(quote!(A, B,)).unwrap()), "state_props!(A);state_props!(B);");
    }

    /// The shape of idea-theme's `tone!`: outer attrs, an empty-able
    /// `vis`, `expr` fields, a nested optional repetition.
    #[test]
    fn expands_the_tone_macro_shape() {
        let def = MacroDef::parse(
            "tone",
            quote! {
                (
                    $(#[$meta:meta])*
                    $vis:vis $name:ident using $me:tt {
                        key = $key:literal,
                        fill = $fill:expr
                        $(, tokens = [ $($k:literal => $v:literal),* $(,)? ] )?
                        $(,)?
                    }
                ) => {
                    $(#[$meta])*
                    #[derive(Copy, Clone, ::runtime_core::IdealystSchema)]
                    #[schema(value_of = "ToneRef")]
                    $vis struct $name;
                    const KEYS: &[&str] = &[$($($k),*)?];
                };
            },
            "idea_theme",
        )
        .unwrap();
        let out = def
            .expand(quote! {
                /// Brand tone.
                pub Brand using self {
                    key = "brand",
                    fill = theme.color(1, 2),
                    tokens = ["a" => "#fff", "b" => "#000"],
                }
            })
            .unwrap();
        let s = squash(out.clone());
        assert!(s.starts_with("#[doc=r\"Brandtone.\"]#[derive(Copy,Clone,::runtime_core::IdealystSchema)]"), "{s}");
        assert!(s.contains("pubstructBrand;"), "{s}");
        assert!(s.contains("&[\"a\",\"b\"]"), "{s}");
        // And it parses as items, which is what the scanner does next.
        syn::parse2::<syn::File>(out).unwrap();

        let private = squash(def.expand(quote!(Plain using self { key = "p", fill = 1 })).unwrap());
        assert!(private.contains("structPlain;") && !private.contains("pubstruct"), "{private}");
    }

    #[test]
    fn an_input_no_rule_matches_is_an_error() {
        let def = MacroDef::parse("m", quote! { (a $x:ident) => { $x }; }, "crate").unwrap();
        assert!(def.expand(quote!(b c)).is_err());
        assert!(def.expand(quote!(a c d)).is_err(), "a rule must consume the whole input");
    }
}
