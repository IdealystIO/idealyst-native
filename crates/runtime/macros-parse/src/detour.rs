//! What in a file a NATIVE in-place patch cannot carry, beyond its shape.
//!
//! The dev session patches a running server by redirecting its functions
//! in place (`build_runtime_server::hotpatch::detour`): each of the
//! binary's functions of the edited crate branches to the patch's copy.
//! [`crate::shape_of`] already proves that no signature, type or item
//! moved. A body-only edit can still be one a redirect does not carry,
//! because the old body lives on somewhere a redirect does not reach:
//!
//! - **generic** functions (on the fn, on the `impl` or `trait` around
//!   it, `impl Trait` arguments) and **trait default methods**: every
//!   instantiation is its own copy, compiled in whichever crate named the
//!   types, and a patch redirects none of them;
//! - **`#[inline]`**, **`const fn`**: copied into callers, or evaluated
//!   into constants, in other crates;
//! - **`async fn`** and **`impl Trait` returns**: the hidden type's layout
//!   comes from the body, and code that is not re-emitted — a dependent
//!   crate, a future already in flight — holds it at the old size;
//! - **closures and `async` blocks**: one built before the patch — a
//!   route handler registered at startup, a spawned task, a middleware —
//!   keeps calling its old body, while the session would report the edit
//!   applied;
//! - **items declared inside a function body** (a local struct, an
//!   `impl`, a nested fn's signature): the shape blanks bodies, so it
//!   does not see them, and a value built from the old one can outlive
//!   the patch;
//! - **`fn main`**: it has already run, and does not run again;
//! - **`#[no_mangle]` / `#[export_name]` / `extern` functions**: a patch
//!   only redirects Rust-mangled functions.
//!
//! This is the client-blind counterpart of [`crate::downstream`]: there a
//! `#[server]` body or a `cfg(feature = "server")` item is not the
//! client's business and is skipped; here the server IS the program, so
//! nothing is.
//!
//! A macro call inside a body is walked when its arguments read as
//! expressions or statements (`vec![…]`, `format!`); one that reads as
//! neither (`tokio::select! { … }`) is recorded whole when it may hold a
//! closure or an `async` block, so an edit inside it rebuilds.

use std::collections::BTreeMap;

use quote::ToTokens;
use syn::visit::Visit;
use syn::visit_mut::VisitMut;

use crate::downstream::{downstream_sig, has_type_or_const_params};

/// Every part of `text` a native patch cannot carry, keyed by a label
/// naming it (`fn name`, `impl Type::name`, `closure #2 in fn name`, …),
/// valued by its token text. Two versions of a file whose maps are equal
/// differ, if at all, only in what a redirect carries.
///
/// `None` when the file does not parse: "cannot tell", which rebuilds.
pub fn detour_facts(text: &str) -> Option<BTreeMap<String, String>> {
    let file: syn::File = syn::parse_file(text).ok()?;
    let mut out = BTreeMap::new();
    walk_items(&file.items, "", true, &mut out);
    Some(out)
}

fn walk_items(items: &[syn::Item], prefix: &str, file_root: bool, out: &mut BTreeMap<String, String>) {
    for item in items {
        match item {
            syn::Item::Fn(f) => {
                let label = format!("{prefix}fn {}", f.sig.ident);
                if pinned(&f.sig, &f.attrs) || (file_root && prefix.is_empty() && f.sig.ident == "main") {
                    insert(out, label.clone(), f.to_token_stream().to_string());
                }
                body(&f.block, &label, out);
            }
            syn::Item::Mod(m) => {
                if let Some((_, inner)) = &m.content {
                    walk_items(inner, &format!("{prefix}{}::", m.ident), false, out);
                }
            }
            syn::Item::Impl(imp) => {
                let generic_impl = has_type_or_const_params(&imp.generics);
                let self_ty = imp.self_ty.to_token_stream().to_string();
                let base = match &imp.trait_ {
                    Some((_, path, _)) => format!("{prefix}impl {} for {self_ty}", path.to_token_stream()),
                    None => format!("{prefix}impl {self_ty}"),
                };
                for it in &imp.items {
                    if let syn::ImplItem::Fn(f) = it {
                        let label = format!("{base}::{}", f.sig.ident);
                        if generic_impl || pinned(&f.sig, &f.attrs) {
                            insert(out, label.clone(), f.to_token_stream().to_string());
                        }
                        body(&f.block, &label, out);
                    }
                }
            }
            syn::Item::Trait(t) => {
                for it in &t.items {
                    if let syn::TraitItem::Fn(f) = it {
                        if let Some(block) = &f.default {
                            let label = format!("{prefix}trait {}::{}", t.ident, f.sig.ident);
                            insert(out, label.clone(), f.to_token_stream().to_string());
                            body(block, &label, out);
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

/// Whether a function's body is one a redirect cannot carry, from its
/// signature and attributes alone.
fn pinned(sig: &syn::Signature, attrs: &[syn::Attribute]) -> bool {
    downstream_sig(sig, attrs)
        || sig.abi.is_some()
        || attrs.iter().any(|a| {
            let p = a.path();
            p.is_ident("no_mangle")
                || p.is_ident("export_name")
                // `#[unsafe(no_mangle)]`, edition 2024's spelling.
                || (p.is_ident("unsafe") && a.meta.to_token_stream().to_string().contains("no_mangle"))
        })
}

/// The closures, `async` blocks and items inside one function body.
fn body(block: &syn::Block, owner: &str, out: &mut BTreeMap<String, String>) {
    struct Walk<'a> {
        owner: &'a str,
        closures: usize,
        asyncs: usize,
        items: usize,
        macros: usize,
        out: &'a mut BTreeMap<String, String>,
    }
    impl<'ast> Visit<'ast> for Walk<'_> {
        fn visit_expr_closure(&mut self, c: &'ast syn::ExprClosure) {
            self.closures += 1;
            insert(self.out, format!("closure #{} in {}", self.closures, self.owner), c.to_token_stream().to_string());
            syn::visit::visit_expr_closure(self, c);
        }
        fn visit_expr_async(&mut self, a: &'ast syn::ExprAsync) {
            self.asyncs += 1;
            insert(self.out, format!("async block #{} in {}", self.asyncs, self.owner), a.to_token_stream().to_string());
            syn::visit::visit_expr_async(self, a);
        }
        fn visit_item(&mut self, item: &'ast syn::Item) {
            self.items += 1;
            let mut shape = item.clone();
            crate::shape::BlankBodies.visit_item_mut(&mut shape);
            insert(self.out, format!("item #{} in {}", self.items, self.owner), shape.to_token_stream().to_string());
            // A nested fn's own body is ordinary code, redirected like
            // any other; its closures and items are still recorded.
            match item {
                syn::Item::Fn(f) => {
                    let label = format!("{} :: fn {}", self.owner, f.sig.ident);
                    if pinned(&f.sig, &f.attrs) {
                        insert(self.out, label.clone(), f.to_token_stream().to_string());
                    }
                    body(&f.block, &label, self.out);
                }
                syn::Item::Impl(_) | syn::Item::Mod(_) | syn::Item::Trait(_) => {
                    walk_items(std::slice::from_ref(item), &format!("{} :: ", self.owner), false, self.out);
                }
                _ => {}
            }
        }
        /// A macro call's arguments are tokens to `syn`. Most read as
        /// expressions (`vec![…]`, `format!`, `assert!`) or statements, and
        /// are walked as such. One that reads as neither (`tokio::select!`,
        /// `ui!`) is opaque: if it may hold a closure or an `async` block,
        /// the whole call is recorded, so any edit inside it rebuilds.
        fn visit_macro(&mut self, m: &'ast syn::Macro) {
            use syn::parse::Parser;
            let exprs = syn::punctuated::Punctuated::<syn::Expr, syn::Token![,]>::parse_terminated.parse2(m.tokens.clone());
            if let Ok(exprs) = exprs {
                for e in &exprs {
                    self.visit_expr(e);
                }
                return;
            }
            if let Ok(stmts) = syn::Block::parse_within.parse2(m.tokens.clone()) {
                for s in &stmts {
                    self.visit_stmt(s);
                }
                return;
            }
            if may_hold_code(m.tokens.clone()) {
                self.macros += 1;
                insert(self.out, format!("macro #{} in {}", self.macros, self.owner), m.to_token_stream().to_string());
            }
        }
    }
    /// Whether opaque tokens may spell a closure (`|`) or an `async`
    /// block. Inside a string literal neither counts: a literal is one
    /// token.
    fn may_hold_code(tokens: proc_macro2::TokenStream) -> bool {
        tokens.into_iter().any(|t| match t {
            proc_macro2::TokenTree::Punct(p) => p.as_char() == '|',
            proc_macro2::TokenTree::Ident(i) => i == "async",
            proc_macro2::TokenTree::Group(g) => may_hold_code(g.stream()),
            proc_macro2::TokenTree::Literal(_) => false,
        })
    }
    let mut w = Walk { owner, closures: 0, asyncs: 0, items: 0, macros: 0, out };
    w.visit_block(block);
}

/// Two parts with one label both count.
fn insert(out: &mut BTreeMap<String, String>, label: String, tokens: String) {
    let mut key = label.clone();
    let mut n = 2;
    while out.contains_key(&key) {
        key = format!("{label} #{n}");
        n += 1;
    }
    out.insert(key, tokens);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(src: &str) -> Vec<String> {
        detour_facts(src).expect("parses").into_keys().collect()
    }

    /// The edit a redirect exists for: statements in a plain fn.
    #[test]
    fn a_plain_body_is_not_recorded() {
        assert!(labels("pub fn label(n: u32) -> String { format!(\"{n}\") }").is_empty());
        assert!(labels("pub struct S; impl S { pub fn m(&self) -> u8 { let x = 1; x } }").is_empty());
    }

    /// The server is the whole program here: a `#[server]` body and a
    /// `cfg(feature = "server")` item count, where the client-side rules
    /// skip them.
    #[test]
    fn server_bodies_count() {
        let src = "#[server] pub async fn ping() -> Result<String, E> { Ok(\"a\".into()) }\n\
                   #[cfg(feature = \"server\")] pub async fn helper() -> u8 { 1 }";
        assert_eq!(labels(src), vec!["fn helper", "fn ping"]);
    }

    #[test]
    fn closures_and_async_blocks_are_recorded_in_order() {
        let src = "fn routes() { let a = |x: u32| x + 1; spawn(async move { tick().await }); let b = || 2; }";
        assert_eq!(
            labels(src),
            vec!["async block #1 in fn routes", "closure #1 in fn routes", "closure #2 in fn routes"]
        );
        let edited = src.replace("x + 1", "x + 2");
        let (a, b) = (detour_facts(src).unwrap(), detour_facts(&edited).unwrap());
        assert_ne!(a["closure #1 in fn routes"], b["closure #1 in fn routes"]);
        assert_eq!(a["closure #2 in fn routes"], b["closure #2 in fn routes"]);
    }

    /// `vec![Box::new(|| 1)]`: the closure is inside a macro call.
    #[test]
    fn a_closure_inside_a_macro_call_is_recorded() {
        assert_eq!(labels("fn r() -> Vec<Box<dyn Fn() -> u8>> { vec![Box::new(|| 1)] }"), vec!["closure #1 in fn r"]);
        // Opaque, and holding an `async` block: recorded whole.
        let select = "fn s() { tokio::select! { _ = async { 1 } => {} } }";
        assert_eq!(labels(select), vec!["macro #1 in fn s"]);
        // Opaque, holding no code: an ordinary statement.
        assert!(labels("fn j() { let _ = json!({ \"a\": 1 }); }").is_empty());
        assert!(labels("fn f() { println!(\"{}|{}\", 1, 2); }").is_empty());
    }

    #[test]
    fn main_at_the_root_is_recorded_and_elsewhere_is_not() {
        assert_eq!(labels("fn main() { serve(); }"), vec!["fn main"]);
        assert!(labels("mod m { fn main() { serve(); } }").is_empty());
    }

    #[test]
    fn unmangled_and_foreign_abi_functions_are_recorded() {
        assert_eq!(labels("#[no_mangle] pub fn a() {}"), vec!["fn a"]);
        assert_eq!(labels("#[unsafe(no_mangle)] pub fn a() {}"), vec!["fn a"]);
        assert_eq!(labels("pub extern \"C\" fn b() {}"), vec!["fn b"]);
        assert_eq!(labels("#[export_name = \"c\"] pub fn c() {}"), vec!["fn c"]);
    }

    /// A struct declared in a body is invisible to the shape, which
    /// blanks bodies; its fields are a layout like any other.
    #[test]
    fn an_item_declared_in_a_body_is_recorded_without_its_fn_bodies() {
        let src = "fn f() { struct Local { a: u8 } impl Local { fn get(&self) -> u8 { self.a } } }";
        let facts = detour_facts(src).unwrap();
        assert!(facts.keys().any(|k| k == "item #1 in fn f"));
        let body_edit = detour_facts(&src.replace("self.a }", "self.a + 1 }")).unwrap();
        assert_eq!(facts["item #1 in fn f"], body_edit["item #1 in fn f"], "a nested method's body is not the item's shape");
        assert_eq!(facts["item #2 in fn f"], body_edit["item #2 in fn f"]);
        let field_edit = detour_facts(&src.replace("a: u8", "a: u16")).unwrap();
        assert_ne!(facts["item #1 in fn f"], field_edit["item #1 in fn f"]);
    }

    #[test]
    fn generic_inline_const_async_and_impl_trait_bodies_are_recorded() {
        for (src, label) in [
            ("pub fn w<T>(t: T) -> T { t }", "fn w"),
            ("#[inline] pub fn i() -> u8 { 1 }", "fn i"),
            ("pub const fn c() -> u8 { 1 }", "fn c"),
            ("pub async fn a() -> u8 { 1 }", "fn a"),
            ("pub fn r() -> impl Fn() -> u8 { || 1 }", "fn r"),
            ("pub struct W<T>(T); impl<T> W<T> { pub fn g(&self) {} }", "impl W < T >::g"),
            ("pub trait T { fn d(&self) -> u8 { 1 } }", "trait T::d"),
        ] {
            assert!(labels(src).contains(&label.to_string()), "{src}: {:?}", labels(src));
        }
    }

    #[test]
    fn a_file_that_does_not_parse_cannot_be_told() {
        assert!(detour_facts("fn broken( {").is_none());
    }
}
