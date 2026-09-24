//! Function bodies a DEPENDENT crate compiles from this crate's metadata.
//!
//! A hot patch re-emits the edited crates from source and splices the
//! result into a running program. For the app crate that is the whole
//! story: nothing depends on it. A library crate of the app's workspace
//! is different. Most of its functions are compiled once, into its own
//! objects, and every dependent calls that one copy — so re-emitting the
//! library (and its dependents, which call it directly) replaces them.
//! But for some functions rustc hands the BODY to each dependent, through
//! the crate's metadata, and the dependent compiles its own copy:
//!
//! - **generic** functions (type or const parameters, on the function or
//!   on the `impl` / `trait` around it, or `impl Trait` in an argument):
//!   each instantiation is compiled by whichever crate names the concrete
//!   types, very often a dependent;
//! - **trait default methods**, generic over `Self`, so compiled per
//!   implementor;
//! - **`#[inline]` / `#[inline(always)]`** functions, which rustc
//!   compiles locally in every crate that calls them (incremental builds
//!   turn off inferring this for unmarked functions, but an explicit
//!   attribute always counts);
//! - **`impl Trait` in return position**: the hidden type's methods — a
//!   returned closure's body, say — are instantiated where they are
//!   called;
//! - **`async fn`**: the future is such a hidden type;
//! - **`const fn`**: evaluated at compile time inside the dependent's own
//!   constants.
//!
//! A replay of a dependent reads the library's metadata from the BASE
//! build, which still carries the old bodies — the replay only emits
//! objects, never new metadata. So an edit to one of these bodies in a
//! library would reach the library's own callers and silently miss every
//! dependent's copy. The dev loop rebuilds such an edit instead; this
//! module is how it recognizes one.
//!
//! The answer is per ITEM, keyed by a readable label, so the rebuild can
//! name the function that forced it.

use std::collections::BTreeMap;

use quote::ToTokens;

/// Every function in `text` whose body a dependent may compile, keyed by
/// a label (`fn name`, `impl Type::name`, `trait Name::name`, prefixed
/// with enclosing `mod`s), valued by the function's token text.
///
/// `None` when the file does not parse — the caller must treat that as
/// "cannot tell", which means rebuild.
pub fn downstream_bodies(text: &str) -> Option<BTreeMap<String, String>> {
    let file: syn::File = syn::parse_file(text).ok()?;
    let mut out = BTreeMap::new();
    collect(&file.items, "", &mut out);
    Some(out)
}

fn collect(items: &[syn::Item], prefix: &str, out: &mut BTreeMap<String, String>) {
    for item in items {
        match item {
            syn::Item::Fn(f) => {
                if downstream_sig(&f.sig, &f.attrs) {
                    insert(out, format!("{prefix}fn {}", f.sig.ident), f.to_token_stream().to_string());
                }
            }
            syn::Item::Mod(m) => {
                if let Some((_, inner)) = &m.content {
                    collect(inner, &format!("{prefix}{}::", m.ident), out);
                }
            }
            syn::Item::Impl(imp) => {
                let generic_impl = has_type_or_const_params(&imp.generics);
                let self_ty = imp.self_ty.to_token_stream().to_string();
                let label = match &imp.trait_ {
                    Some((_, path, _)) => {
                        format!("{prefix}impl {} for {self_ty}", path.to_token_stream())
                    }
                    None => format!("{prefix}impl {self_ty}"),
                };
                for it in &imp.items {
                    if let syn::ImplItem::Fn(f) = it {
                        if generic_impl || downstream_sig(&f.sig, &f.attrs) {
                            insert(
                                out,
                                format!("{label}::{}", f.sig.ident),
                                f.to_token_stream().to_string(),
                            );
                        }
                    }
                }
            }
            syn::Item::Trait(t) => {
                // A default body is generic over `Self`: every implementor
                // gets its own instantiation, compiled by whichever crate
                // names the implementor.
                for it in &t.items {
                    if let syn::TraitItem::Fn(f) = it {
                        if f.default.is_some() {
                            insert(
                                out,
                                format!("{prefix}trait {}::{}", t.ident, f.sig.ident),
                                f.to_token_stream().to_string(),
                            );
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

/// Two items with one label (a `cfg`-gated pair, say) both count; the
/// second gets a suffix rather than overwriting the first.
fn insert(out: &mut BTreeMap<String, String>, label: String, tokens: String) {
    let mut key = label.clone();
    let mut n = 2;
    while out.contains_key(&key) {
        key = format!("{label} #{n}");
        n += 1;
    }
    out.insert(key, tokens);
}

/// Whether a function with this signature and these attributes has its
/// body compiled by dependents. See the module docs for each rule.
fn downstream_sig(sig: &syn::Signature, attrs: &[syn::Attribute]) -> bool {
    has_type_or_const_params(&sig.generics)
        || sig.constness.is_some()
        || sig.asyncness.is_some()
        || mentions_impl_trait_sig(sig)
        || inline_hint(attrs)
}

/// Lifetime parameters do not count: lifetimes are erased before
/// codegen, so a lifetime-generic function is compiled once, in its own
/// crate, like any other.
fn has_type_or_const_params(generics: &syn::Generics) -> bool {
    generics
        .params
        .iter()
        .any(|p| !matches!(p, syn::GenericParam::Lifetime(_)))
}

/// `#[inline]` or `#[inline(always)]`. `#[inline(never)]` is the one
/// spelling that keeps the body in its own crate.
fn inline_hint(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| {
        if !a.path().is_ident("inline") {
            return false;
        }
        match &a.meta {
            syn::Meta::Path(_) => true,
            syn::Meta::List(list) => list.tokens.to_string().trim() != "never",
            syn::Meta::NameValue(_) => true,
        }
    })
}

fn mentions_impl_trait_sig(sig: &syn::Signature) -> bool {
    struct Finder(bool);
    impl<'ast> syn::visit::Visit<'ast> for Finder {
        fn visit_type_impl_trait(&mut self, _: &'ast syn::TypeImplTrait) {
            self.0 = true;
        }
    }
    let mut f = Finder(false);
    for input in &sig.inputs {
        if let syn::FnArg::Typed(pt) = input {
            syn::visit::Visit::visit_type(&mut f, &pt.ty);
        }
    }
    if let syn::ReturnType::Type(_, ty) = &sig.output {
        syn::visit::Visit::visit_type(&mut f, ty);
    }
    f.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(src: &str) -> Vec<String> {
        downstream_bodies(src).expect("parses").into_keys().collect()
    }

    #[test]
    fn a_plain_function_stays_in_its_own_crate() {
        assert!(labels("pub fn helper(n: u32) -> u32 { n + 1 }").is_empty());
        assert!(labels("pub struct S; impl S { pub fn m(&self) -> u8 { 1 } }").is_empty());
        assert!(labels("impl std::fmt::Display for S { fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { Ok(()) } }").is_empty());
    }

    #[test]
    fn a_lifetime_only_generic_is_not_downstream() {
        assert!(labels("pub fn first<'a>(s: &'a str) -> &'a str { s }").is_empty());
    }

    #[test]
    fn generics_are_downstream_on_the_fn_and_on_the_impl() {
        assert_eq!(labels("pub fn wrap<T: Clone>(t: T) -> Vec<T> { vec![t] }"), vec!["fn wrap"]);
        assert_eq!(labels("pub fn n<const N: usize>() -> usize { N }"), vec!["fn n"]);
        assert_eq!(
            labels("pub struct W<T>(T); impl<T> W<T> { pub fn get(&self) -> &T { &self.0 } }"),
            vec!["impl W < T >::get"]
        );
    }

    #[test]
    fn impl_trait_in_either_position_is_downstream() {
        assert_eq!(labels("pub fn run(f: impl Fn()) { f() }"), vec!["fn run"]);
        assert_eq!(labels("pub fn make() -> impl Fn() -> u8 { || 1 }"), vec!["fn make"]);
    }

    #[test]
    fn inline_const_and_async_are_downstream_but_inline_never_is_not() {
        assert_eq!(labels("#[inline] pub fn a() -> u8 { 1 }"), vec!["fn a"]);
        assert_eq!(labels("#[inline(always)] pub fn b() -> u8 { 1 }"), vec!["fn b"]);
        assert!(labels("#[inline(never)] pub fn c() -> u8 { 1 }").is_empty());
        assert_eq!(labels("pub const fn d() -> u8 { 1 }"), vec!["fn d"]);
        assert_eq!(labels("pub async fn e() -> u8 { 1 }"), vec!["fn e"]);
    }

    #[test]
    fn trait_default_bodies_are_downstream_and_required_methods_are_not() {
        assert_eq!(
            labels("pub trait T { fn req(&self); fn def(&self) -> u8 { 1 } }"),
            vec!["trait T::def"]
        );
    }

    #[test]
    fn nested_modules_prefix_the_label() {
        assert_eq!(labels("mod a { pub mod b { #[inline] pub fn f() {} } }"), vec!["a::b::fn f"]);
    }

    /// The digest the decider compares: a body edit to a downstream fn
    /// changes its entry; the same edit to a plain fn changes nothing.
    #[test]
    fn a_body_edit_moves_only_its_own_entry() {
        let src = "#[inline] pub fn a() -> u8 { 1 } pub fn b() -> u8 { 2 }";
        let before = downstream_bodies(src).unwrap();
        assert_ne!(before, downstream_bodies(&src.replace("{ 1 }", "{ 3 }")).unwrap());
        assert_eq!(before, downstream_bodies(&src.replace("{ 2 }", "{ 4 }")).unwrap());
    }

    #[test]
    fn a_file_that_does_not_parse_has_no_answer() {
        assert!(downstream_bodies("fn broken( {").is_none());
    }
}
