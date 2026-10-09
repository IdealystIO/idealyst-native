//! A module's items as token ranges, not syntax trees.
//!
//! Parsing a file with `syn` builds every expression of every function
//! body, and in a large app almost none of them matter to the catalog:
//! only items carrying a catalog macro, `mod` items and macro invocations
//! do. Measured on CrewForge (35 crates), full parsing was half of a scan
//! and re-parsing expansion output most of the rest. So the scanner lexes
//! a file once and splits the token stream into items by shape alone —
//! attributes, then tokens up to the `;` or the `{…}` body that ends the
//! item — and hands an item's tokens to `syn` or a macro only when its
//! attributes or kind say it matters. A component's tokens go to the
//! expansion exactly as written, the way rustc hands them over.
//!
//! An item that does not parse costs only itself, not its file.

use proc_macro2::{Delimiter, Group, TokenStream, TokenTree};
use quote::ToTokens;
use syn::parse::Parser;
use syn::Attribute;

/// One item: its outer attributes, parsed (they decide everything), and
/// the rest of its tokens as written.
#[derive(Clone)]
pub struct TokItem {
    pub attrs: Vec<Attribute>,
    pub rest: Vec<TokenTree>,
    /// Source line of the item's first token (an attribute's `#`).
    pub line: u32,
}

/// What an item is, as far as the scan cares.
pub enum Kind<'a> {
    /// `mod name;` (body `None`) or `mod name { … }`.
    Mod { name: String, body: Option<&'a Group> },
    /// `macro_rules! name { … }`.
    MacroRules { name: String, body: &'a Group },
    /// `path!(…)` / `path![…]` / `path! { … }` at item position.
    MacroCall { path: syn::Path, args: &'a Group, line: u32 },
    /// `fn`, `struct`, `enum`, `union` — what attribute macros and
    /// derives attach to.
    Decorable,
    Other,
}

impl TokItem {
    pub fn kind(&self) -> Kind<'_> {
        let toks = strip_vis(&self.rest);
        // Item macros: a path, `!`, an optional name (`macro_rules! x`),
        // then a group.
        if let Some(bang) = toks.iter().position(|t| matches!(t, TokenTree::Punct(p) if p.as_char() == '!')) {
            let head: TokenStream = toks[..bang].iter().map(|t| (*t).clone()).collect();
            if let Ok(path) = syn::Path::parse_mod_style.parse2(head) {
                let line = toks[0].span().start().line as u32;
                if path.is_ident("macro_rules") {
                    if let (Some(TokenTree::Ident(name)), Some(TokenTree::Group(body))) = (toks.get(bang + 1), toks.get(bang + 2)) {
                        return Kind::MacroRules { name: name.to_string(), body };
                    }
                } else if let Some(TokenTree::Group(args)) = toks.get(bang + 1) {
                    return Kind::MacroCall { path, args, line };
                }
            }
            return Kind::Other;
        }
        let words = leading_words(&toks);
        match words.iter().find(|w| !QUALIFIERS.contains(&w.as_str())).map(String::as_str) {
            Some("mod") => {
                let at = toks.iter().position(|t| matches!(t, TokenTree::Ident(i) if i == "mod")).expect("found above");
                let Some(TokenTree::Ident(name)) = toks.get(at + 1) else { return Kind::Other };
                let body = match toks.get(at + 2) {
                    Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Brace => Some(g),
                    _ => None,
                };
                let name = name.to_string();
                Kind::Mod { name: name.strip_prefix("r#").unwrap_or(&name).to_string(), body }
            }
            Some("fn" | "struct" | "enum" | "union") => Kind::Decorable,
            _ => Kind::Other,
        }
    }

    /// The item as tokens: its attributes (as they now are, after any
    /// `cfg_attr` expansion or removal) and the rest as written.
    pub fn tokens(&self) -> TokenStream {
        let mut out = TokenStream::new();
        for a in &self.attrs {
            a.to_tokens(&mut out);
        }
        out.extend(self.rest.iter().cloned());
        out
    }
}

/// Words that may precede an item's keyword: `pub`, `unsafe`, `async`,
/// `extern "C"`, `const fn`, `default`, `auto trait`.
const QUALIFIERS: &[&str] = &["unsafe", "async", "extern", "default", "auto", "safe"];

/// The identifiers an item starts with, up to its first group or
/// punctuation (`const fn foo` → `const fn foo`; `struct Foo<` → `struct
/// Foo`). `const` is dropped when another keyword follows it (`const fn`).
fn leading_words(toks: &[&TokenTree]) -> Vec<String> {
    let mut words = Vec::new();
    for t in toks {
        match t {
            TokenTree::Ident(i) => words.push(i.to_string()),
            TokenTree::Literal(_) => {} // `extern "C"`
            _ => break,
        }
    }
    if words.first().is_some_and(|w| w == "const") && words.get(1).is_some_and(|w| matches!(w.as_str(), "fn" | "unsafe" | "async" | "extern")) {
        words.remove(0);
    }
    words
}

/// The tokens without a leading visibility (`pub`, `pub(crate)`, …).
fn strip_vis(rest: &[TokenTree]) -> Vec<&TokenTree> {
    let mut i = 0;
    if matches!(rest.first(), Some(TokenTree::Ident(id)) if id == "pub") {
        i = 1;
        if matches!(rest.get(1), Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Parenthesis) {
            i = 2;
        }
    }
    rest[i..].iter().collect()
}

/// Whether an item, given the tokens of it seen so far, ends at a `{…}`
/// group (a body) rather than running on to a `;`. The item keyword
/// decides: `fn`, `impl`, `trait`, `mod`, `struct`/`enum`/`union` (with
/// braces), an `extern` block, and item macros end at their braces;
/// `const`/`static`/`type`/`use` run on (`const X: S = S { … };`).
fn brace_ends_item(seen: &[TokenTree]) -> bool {
    let toks = strip_vis(seen);
    if toks.iter().any(|t| matches!(t, TokenTree::Punct(p) if p.as_char() == '!')) {
        return true; // `name! { … }` / `macro_rules! name { … }`
    }
    let words = leading_words(&toks);
    match words.iter().find(|w| !QUALIFIERS.contains(&w.as_str())).map(String::as_str) {
        Some("fn" | "impl" | "trait" | "mod" | "struct" | "enum" | "union") => true,
        // `extern "C" { … }` / `extern crate x;`
        None => words.first().is_some_and(|w| w == "extern"),
        _ => false,
    }
}

/// Split `tokens` (a file, a module body, an expansion) into items.
/// Inner attributes (`#![…]`) are skipped. An attribute that does not
/// parse leaves its item out (it would not compile either).
pub fn split(tokens: TokenStream) -> Vec<TokItem> {
    let tts: Vec<TokenTree> = tokens.into_iter().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < tts.len() {
        // Inner attributes.
        if is_punct(&tts[i], '#') && matches!(tts.get(i + 1), Some(t) if is_punct(t, '!')) && is_bracket(tts.get(i + 2)) {
            i += 3;
            continue;
        }
        let line = tts[i].span().start().line as u32;
        let start = i;
        while i < tts.len() && is_punct(&tts[i], '#') && is_bracket(tts.get(i + 1)) {
            i += 2;
        }
        let attr_tokens: TokenStream = tts[start..i].iter().cloned().collect();
        let body_start = i;
        while i < tts.len() {
            match &tts[i] {
                TokenTree::Punct(p) if p.as_char() == ';' => {
                    i += 1;
                    break;
                }
                TokenTree::Group(g) if g.delimiter() == Delimiter::Brace && brace_ends_item(&tts[body_start..i]) => {
                    i += 1;
                    // `name! { … };` may carry a redundant `;`.
                    if matches!(tts.get(i), Some(t) if is_punct(t, ';')) && tts[body_start..i].iter().any(|t| is_punct(t, '!')) {
                        i += 1;
                    }
                    break;
                }
                _ => i += 1,
            }
        }
        if body_start == i {
            continue; // stray attributes at the end
        }
        let Ok(attrs) = Attribute::parse_outer.parse2(attr_tokens) else { continue };
        out.push(TokItem { attrs, rest: tts[body_start..i].to_vec(), line });
    }
    out
}

fn is_punct(t: &TokenTree, c: char) -> bool {
    matches!(t, TokenTree::Punct(p) if p.as_char() == c)
}

fn is_bracket(t: Option<&TokenTree>) -> bool {
    matches!(t, Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Bracket)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn kinds(src: &str) -> Vec<String> {
        split(TokenStream::from_str(src).unwrap())
            .iter()
            .map(|it| match it.kind() {
                Kind::Mod { name, body } => format!("mod {name}{}", if body.is_some() { "{}" } else { ";" }),
                Kind::MacroRules { name, .. } => format!("macro_rules {name}"),
                Kind::MacroCall { path, .. } => format!("call {}", quote::quote!(#path).to_string().replace(' ', "")),
                Kind::Decorable => format!("decorable@{}", it.line),
                Kind::Other => "other".to_string(),
            })
            .collect()
    }

    #[test]
    fn splits_items_by_shape() {
        let src = r#"
#![allow(dead_code)]
use a::{b, c};
pub(crate) mod x;
mod y { fn inner() {} }
#[component]
pub fn Card(props: &CardProps) -> Element where T: X { ui! { view() } }
const S: Foo = Foo { a: 1 };
const fn k() -> u8 { 1 }
struct Unit;
struct Tuple(u8, String);
#[derive(IdealystSchema)]
pub struct Named { a: u8 }
impl X for Y { fn f() {} }
macro_rules! m { () => {} }
recipe!(Card, fn r() -> Element { ui! { view() } });
doc_scope! { Demo = "Demo" }
inventory::submit! { E { a: 1 } }
extern "C" { fn ext(); }
static T: &[u8] = &[1, 2];
"#;
        assert_eq!(
            kinds(src),
            [
                "other",
                "mod x;",
                "mod y{}",
                "decorable@6",
                "other",
                "decorable@9",
                "decorable@10",
                "decorable@11",
                "decorable@12",
                "other",
                "macro_rules m",
                "call recipe",
                "call doc_scope",
                "call inventory::submit",
                "other",
                "other",
            ]
        );
    }

    /// The tokens an attribute macro receives: the item minus that
    /// attribute, everything else as written.
    #[test]
    fn re_emits_an_item_with_its_remaining_attributes() {
        let mut items = split(TokenStream::from_str("/// Docs.\n#[component]\n#[allow(x)]\nfn A() {}").unwrap());
        let item = &mut items[0];
        item.attrs.remove(1);
        let s = item.tokens().to_string();
        assert!(s.contains("doc") && s.contains("allow") && !s.contains("component") && s.ends_with("fn A () { }"), "{s}");
    }
}
