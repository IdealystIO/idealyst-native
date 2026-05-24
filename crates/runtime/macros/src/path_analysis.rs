//! Dotted-path analysis used by the `#[component]` reactivity rewriter.
//!
//! A "field path" is a sequence of identifiers like `count` or `props.value`
//! or `state.user.name`. The rewriter cares about three operations:
//!
//! 1. **Collect** all paths in an expression matching some predicate
//!    (e.g. "is the receiver of a parameterless `.get()` call" or
//!    "has a root ident in the function's parameter list").
//! 2. **Substitute** a path with a fresh local identifier, so a closure
//!    can capture cloned data instead of borrowed parents.
//! 3. **Match** a token-level shape, since `format!`-style macros hide
//!    their contents as opaque tokens that `syn` won't walk.

use proc_macro2::{Span, TokenStream as TokenStream2, TokenTree};
use quote::{quote, ToTokens};
use syn::visit::{self, Visit};
use syn::visit_mut::{self, VisitMut};
use syn::{Expr, ExprMethodCall, Ident};

/// A dotted path like `props.value` represented as its segments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FieldPath {
    pub(crate) segments: Vec<String>,
}

impl FieldPath {
    /// `__rc_props_value` for `props.value`. A single-segment path produces
    /// `__rc_count` for `count`. The leading `__rc_` keeps these names from
    /// colliding with user identifiers.
    pub(crate) fn local_ident(&self) -> Ident {
        let mangled = format!("__rc_{}", self.segments.join("_"));
        Ident::new(&mangled, Span::call_site())
    }

    /// Render the original path back as a token stream: `props.value`.
    pub(crate) fn as_tokens(&self) -> TokenStream2 {
        let parts: Vec<Ident> = self
            .segments
            .iter()
            .map(|s| Ident::new(s, Span::call_site()))
            .collect();
        let head = &parts[0];
        let tail = &parts[1..];
        quote! { #head #(. #tail)* }
    }
}

/// Extracts a `FieldPath` from an expression, or `None` if it doesn't
/// bottom out in a plain identifier.
///
/// Examples:
///   `count`             → Some(["count"])
///   `props.value`       → Some(["props", "value"])
///   `props.a.b.c`       → Some(["props", "a", "b", "c"])
///   `foo().value`       → None (root is a call, not an ident)
///   `t.0`               → None (tuple indexing — out of scope)
pub(crate) fn field_path_of(expr: &Expr) -> Option<FieldPath> {
    match expr {
        Expr::Path(p) if p.qself.is_none() && p.path.segments.len() == 1 => {
            Some(FieldPath {
                segments: vec![p.path.segments[0].ident.to_string()],
            })
        }
        Expr::Field(f) => {
            let mut base = field_path_of(&f.base)?;
            if let syn::Member::Named(id) = &f.member {
                base.segments.push(id.to_string());
                Some(base)
            } else {
                None
            }
        }
        _ => None,
    }
}

// =============================================================================
// Token scanning
// =============================================================================

/// Lexical scan over a token stream. For each top-level position, tries
/// `match_root`; if it matches, pushes the resulting path and skips past
/// the consumed tokens. Recurses into nested groups so reads inside
/// inner macro calls are found too.
pub(crate) fn scan_tokens<M, P>(tokens: TokenStream2, match_root: M, mut push: P)
where
    M: Fn(&[TokenTree], usize) -> Option<(FieldPath, usize)> + Copy,
    P: FnMut(FieldPath),
{
    let trees: Vec<TokenTree> = tokens.into_iter().collect();
    scan_tokens_slice(&trees, match_root, &mut push);
}

fn scan_tokens_slice<M, P>(trees: &[TokenTree], match_root: M, push: &mut P)
where
    M: Fn(&[TokenTree], usize) -> Option<(FieldPath, usize)> + Copy,
    P: FnMut(FieldPath),
{
    let mut i = 0;
    while i < trees.len() {
        if let TokenTree::Ident(_) = &trees[i] {
            if let Some((path, consumed)) = match_root(trees, i) {
                push(path);
                i += consumed;
                continue;
            }
        }
        if let TokenTree::Group(g) = &trees[i] {
            let nested: Vec<TokenTree> = g.stream().into_iter().collect();
            scan_tokens_slice(&nested, match_root, push);
        }
        i += 1;
    }
}

// =============================================================================
// Token-level matchers
// =============================================================================

/// Greedy match: from `start`, consume `IDENT (. IDENT)*` and return the
/// resulting path plus the number of token-trees consumed. Stops before
/// `. IDENT (...)` (a method call) so `props.value` isn't extended to
/// `props.value.get` when the next thing is a call.
pub(crate) fn match_field_path(trees: &[TokenTree], start: usize) -> Option<(FieldPath, usize)> {
    let TokenTree::Ident(root) = &trees[start] else {
        return None;
    };
    let mut segs = vec![root.to_string()];
    let mut j = start + 1;
    loop {
        let dot = matches!(trees.get(j), Some(TokenTree::Punct(p)) if p.as_char() == '.');
        if !dot {
            break;
        }
        let after = trees.get(j + 1);
        let TokenTree::Ident(_) = after? else { break };
        let followed_by_call = matches!(
            trees.get(j + 2),
            Some(TokenTree::Group(g)) if g.delimiter() == proc_macro2::Delimiter::Parenthesis
        );
        if followed_by_call {
            break;
        }
        if let Some(TokenTree::Ident(id)) = after {
            segs.push(id.to_string());
            j += 2;
        } else {
            break;
        }
    }
    Some((FieldPath { segments: segs }, j - start))
}

/// Match `IDENT (. IDENT)* . get ( )` and return the receiver path
/// (everything before the final `.get()`) plus the total token-trees
/// consumed.
pub(crate) fn match_get_chain(trees: &[TokenTree], start: usize) -> Option<(FieldPath, usize)> {
    let TokenTree::Ident(root) = &trees[start] else {
        return None;
    };
    let mut segs = vec![root.to_string()];
    let mut j = start + 1;
    loop {
        let dot = matches!(trees.get(j), Some(TokenTree::Punct(p)) if p.as_char() == '.');
        if !dot {
            return None;
        }
        let next = trees.get(j + 1)?;
        let is_get = matches!(next, TokenTree::Ident(id) if id == "get");
        let is_call = is_get
            && matches!(
                trees.get(j + 2),
                Some(TokenTree::Group(g))
                    if g.delimiter() == proc_macro2::Delimiter::Parenthesis
                        && g.stream().is_empty()
            );
        if is_call {
            return Some((FieldPath { segments: segs }, j + 3 - start));
        }
        if let TokenTree::Ident(id) = next {
            segs.push(id.to_string());
            j += 2;
            continue;
        }
        return None;
    }
}

// =============================================================================
// Convenience wrappers used by the rewriter
// =============================================================================

/// Returns paths that are receivers of a parameterless `.get()` call,
/// anywhere in `expr` (AST or inside known macro bodies).
pub(crate) fn collect_signal_reads(expr: &Expr) -> Vec<FieldPath> {
    let mut out: Vec<FieldPath> = Vec::new();
    struct AstWalker<'a> {
        out: &'a mut Vec<FieldPath>,
    }
    impl<'ast> Visit<'ast> for AstWalker<'_> {
        fn visit_expr_method_call(&mut self, m: &'ast ExprMethodCall) {
            if m.method == "get" && m.args.is_empty() {
                if let Some(path) = field_path_of(&m.receiver) {
                    if !self.out.contains(&path) {
                        self.out.push(path);
                    }
                }
            }
            visit::visit_expr_method_call(self, m);
        }
        fn visit_macro(&mut self, m: &'ast syn::Macro) {
            scan_tokens(m.tokens.clone(), match_get_chain, |p| {
                if !self.out.contains(&p) {
                    self.out.push(p);
                }
            });
            visit::visit_macro(self, m);
        }
    }

    let mut walker = AstWalker { out: &mut out };
    Visit::visit_expr(&mut walker, expr);
    out
}

/// Returns paths whose root identifier is one of `param_idents`, anywhere
/// in `expr` (AST or token-level inside macros).
pub(crate) fn collect_param_paths(expr: &Expr, param_idents: &[String]) -> Vec<FieldPath> {
    let is_param = |p: &FieldPath| {
        !p.segments.is_empty() && param_idents.iter().any(|s| s == &p.segments[0])
    };

    let mut out: Vec<FieldPath> = Vec::new();

    struct AstWalker<'a> {
        out: &'a mut Vec<FieldPath>,
        is_param: &'a dyn Fn(&FieldPath) -> bool,
    }
    impl<'ast> Visit<'ast> for AstWalker<'_> {
        fn visit_expr(&mut self, e: &'ast Expr) {
            if let Some(path) = field_path_of(e) {
                if (self.is_param)(&path) {
                    if !self.out.contains(&path) {
                        self.out.push(path);
                    }
                    return;
                }
            }
            visit::visit_expr(self, e);
        }
        fn visit_macro(&mut self, m: &'ast syn::Macro) {
            scan_tokens(m.tokens.clone(), match_field_path, |p| {
                if (self.is_param)(&p) && !self.out.contains(&p) {
                    self.out.push(p);
                }
            });
            visit::visit_macro(self, m);
        }
    }

    let is_param_fn: &dyn Fn(&FieldPath) -> bool = &is_param;
    let mut walker = AstWalker { out: &mut out, is_param: is_param_fn };
    Visit::visit_expr(&mut walker, expr);
    out
}

// =============================================================================
// Substitution
// =============================================================================

/// Walks the expression and replaces every occurrence of any path in
/// `paths` with its local identifier. Substitutes both at the AST level
/// and inside opaque macro token bodies.
pub(crate) fn substitute_in_expr(expr: &mut Expr, paths: &[FieldPath]) {
    struct Sub<'a> {
        paths: &'a [FieldPath],
    }
    impl VisitMut for Sub<'_> {
        fn visit_expr_mut(&mut self, expr: &mut Expr) {
            if let Some(path) = field_path_of(expr) {
                if let Some(target) = self.paths.iter().find(|p| **p == path) {
                    let local = target.local_ident();
                    *expr = syn::parse2(local.to_token_stream())
                        .expect("local ident must parse as expr");
                    return;
                }
            }
            if let Expr::Macro(em) = expr {
                em.mac.tokens = substitute_in_tokens(em.mac.tokens.clone(), self.paths);
            }
            visit_mut::visit_expr_mut(self, expr);
        }
    }
    Sub { paths }.visit_expr_mut(expr);
}

/// Token-stream substitution: for each `IDENT (. IDENT)*` sequence that
/// matches a path in `paths`, replace with the corresponding local.
/// Recurses into nested groups.
pub(crate) fn substitute_in_tokens(tokens: TokenStream2, paths: &[FieldPath]) -> TokenStream2 {
    let trees: Vec<TokenTree> = tokens.into_iter().collect();
    let mut out = Vec::with_capacity(trees.len());
    let mut i = 0;
    while i < trees.len() {
        if let TokenTree::Ident(_) = &trees[i] {
            if let Some((path, consumed)) = match_field_path(&trees, i) {
                if paths.contains(&path) {
                    let local = path.local_ident();
                    out.push(TokenTree::Ident(local));
                    i += consumed;
                    continue;
                }
            }
        }
        if let TokenTree::Group(g) = &trees[i] {
            let sub = substitute_in_tokens(g.stream(), paths);
            let new_group = proc_macro2::Group::new(g.delimiter(), sub);
            out.push(TokenTree::Group(new_group));
            i += 1;
            continue;
        }
        out.push(trees[i].clone());
        i += 1;
    }
    out.into_iter().collect()
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;

    fn parse_expr(tokens: TokenStream2) -> Expr {
        syn::parse2(tokens).expect("parse expr")
    }

    fn paths_to_strings(paths: &[FieldPath]) -> Vec<String> {
        paths.iter().map(|p| p.segments.join(".")).collect()
    }

    #[test]
    fn field_path_of_handles_simple_ident() {
        let e = parse_expr(quote! { count });
        let p = field_path_of(&e).unwrap();
        assert_eq!(p.segments, vec!["count"]);
    }

    #[test]
    fn field_path_of_handles_field_chain() {
        let e = parse_expr(quote! { props.value });
        let p = field_path_of(&e).unwrap();
        assert_eq!(p.segments, vec!["props", "value"]);

        let e = parse_expr(quote! { state.user.name });
        let p = field_path_of(&e).unwrap();
        assert_eq!(p.segments, vec!["state", "user", "name"]);
    }

    #[test]
    fn field_path_of_rejects_calls() {
        let e = parse_expr(quote! { foo().value });
        assert!(field_path_of(&e).is_none());
    }

    #[test]
    fn field_path_of_rejects_tuple_indexing() {
        let e = parse_expr(quote! { t.0 });
        assert!(field_path_of(&e).is_none());
    }

    #[test]
    fn local_ident_mangles_path() {
        let p = FieldPath { segments: vec!["props".into(), "value".into()] };
        assert_eq!(p.local_ident().to_string(), "__rc_props_value");
        let p = FieldPath { segments: vec!["count".into()] };
        assert_eq!(p.local_ident().to_string(), "__rc_count");
    }

    #[test]
    fn collect_signal_reads_finds_ast_get() {
        // `count.get()` — a `.get()` on a plain ident.
        let e = parse_expr(quote! { count.get() + 1 });
        let paths = collect_signal_reads(&e);
        assert_eq!(paths_to_strings(&paths), vec!["count"]);
    }

    #[test]
    fn collect_signal_reads_finds_field_get() {
        let e = parse_expr(quote! { props.value.get() });
        let paths = collect_signal_reads(&e);
        assert_eq!(paths_to_strings(&paths), vec!["props.value"]);
    }

    #[test]
    fn collect_signal_reads_finds_get_inside_format() {
        // The .get() is inside a format! macro body — opaque to syn but
        // visible to our token-level scanner.
        let e = parse_expr(quote! { format!("v={}", count.get()) });
        let paths = collect_signal_reads(&e);
        assert_eq!(paths_to_strings(&paths), vec!["count"]);
    }

    #[test]
    fn collect_signal_reads_deduplicates() {
        let e = parse_expr(quote! { count.get() + count.get() });
        let paths = collect_signal_reads(&e);
        assert_eq!(paths.len(), 1);
    }

    #[test]
    fn collect_param_paths_only_keeps_param_rooted() {
        let e = parse_expr(quote! { props.label.len() + other.value });
        let paths = collect_param_paths(&e, &["props".to_string()]);
        assert_eq!(paths_to_strings(&paths), vec!["props.label"]);
    }

    #[test]
    fn collect_param_paths_finds_paths_in_format() {
        let e = parse_expr(quote! { format!("{}: {}", props.label, props.value.get()) });
        let mut paths = collect_param_paths(&e, &["props".to_string()]);
        paths.sort_by(|a, b| a.segments.cmp(&b.segments));
        let s = paths_to_strings(&paths);
        assert!(s.contains(&"props.label".to_string()));
        assert!(s.contains(&"props.value".to_string()));
    }

    #[test]
    fn substitute_in_expr_replaces_ast_paths() {
        let mut e = parse_expr(quote! { props.value + 1 });
        let p = FieldPath { segments: vec!["props".into(), "value".into()] };
        substitute_in_expr(&mut e, &[p]);
        let out = e.to_token_stream().to_string();
        assert!(out.contains("__rc_props_value"));
        assert!(!out.contains("props . value"));
    }

    #[test]
    fn substitute_in_expr_replaces_inside_macro() {
        let mut e = parse_expr(quote! { format!("v={}", props.value) });
        let p = FieldPath { segments: vec!["props".into(), "value".into()] };
        substitute_in_expr(&mut e, &[p]);
        let out = e.to_token_stream().to_string();
        assert!(out.contains("__rc_props_value"), "substituted in macro body, got: {}", out);
    }

    #[test]
    fn match_get_chain_handles_simple() {
        let tokens: Vec<TokenTree> = quote! { count.get() }.into_iter().collect();
        let (path, _) = match_get_chain(&tokens, 0).unwrap();
        assert_eq!(path.segments, vec!["count"]);
    }

    #[test]
    fn match_get_chain_handles_field_chain() {
        let tokens: Vec<TokenTree> = quote! { props.value.get() }.into_iter().collect();
        let (path, _) = match_get_chain(&tokens, 0).unwrap();
        assert_eq!(path.segments, vec!["props", "value"]);
    }

    #[test]
    fn match_get_chain_rejects_non_get() {
        let tokens: Vec<TokenTree> = quote! { count.set(5) }.into_iter().collect();
        assert!(match_get_chain(&tokens, 0).is_none());
    }
}
