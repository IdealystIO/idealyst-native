//! A file's SHAPE: everything a hot patch cannot change.
//!
//! The overlay tier asks "did anything outside the `ui!` bodies move?"
//! ([`crate::file_skeleton`]). The subsecond tier asks a different and
//! strictly weaker question: **did anything outside the FUNCTION bodies
//! move?** A jump table rebinds function addresses and nothing else, so
//! a save that only rewrites statements inside functions can be applied
//! to a running process, while a save that changes a signature, a
//! struct, a `static`, an attribute or the set of items in the file
//! cannot.
//!
//! That distinction is not a performance tier — it is a safety
//! boundary. A patch dylib is linked from freshly-emitted objects for
//! the edited crate and spliced into a process where everything else is
//! the OLD build. If the edit changed a props struct's fields, the
//! patched code computes offsets the rest of the process does not
//! agree with, and the framework's own generic instantiations over that
//! type — compiled into a framework rlib that is NOT re-emitted — keep
//! the old layout. The result is silent memory corruption, not a stale
//! render. Rebuild-and-respawn is the only correct answer for those,
//! and [`shape_of`] is how the dev loop tells them apart.
//!
//! # What "blanked" means
//!
//! Every function body in the file — free functions, methods, trait
//! default bodies, nested `fn` items, closures' enclosing fns — is
//! replaced by `{}`. Everything else survives verbatim as tokens:
//! signatures, generics, where clauses, attributes, items, fields,
//! variants, `const` and `static` initializers, `use` statements,
//! macro items at item position.
//!
//! Tokens rather than text, because whitespace and comments must not
//! decide whether a save is patchable — reformatting a signature is a
//! rebuild either way, but reindenting a function should not be. Text
//! is what [`crate::skeleton_of`] compares, and it is right to: there,
//! the question really is "did any byte outside a site move".
//!
//! # Deliberately conservative edges
//!
//! - A `const fn` body is blanked like any other, but a `const` ITEM's
//!   initializer is not: a const's value is baked into every use site
//!   across the whole graph, and only the patched crate is re-emitted.
//! - A `static` / `thread_local!` initializer likewise. The patch dylib
//!   gets its own copy of a static; the running process keeps reading
//!   the original. Treating that as a rebuild is the honest answer.
//! - A doc comment on a function is an attribute, so editing one
//!   rebuilds. Conservative, and it matches what the overlay tier does
//!   with the same edit today.
//! - A macro invocation in item position is opaque to `syn`, so any edit
//!   inside one changes the shape and rebuilds. Correct in general: it
//!   can expand to items.
//!
//! # The one macro whose values are blanked: `stylesheet!`
//!
//! `stylesheet!` expands to items — a builder struct, one enum per
//! `variant` axis, a setter per axis and per `override`, a
//! `<name>_style()` fn — but every item is derived from the sheet's
//! SIGNATURE: its name and vocabulary type, its axes and their arm names
//! and `#[default]` markers, its `override` names and types, its
//! `state` / `breakpoint` / `container` / `compound` keys, and its
//! `transitions` block. The rule VALUES land only inside the
//! `<name>_style()` body (a `StyleRules { … }` literal) and in string
//! literals inside the builder's methods (the premint class name, which
//! hashes the whole invocation). So a value edit is a body edit, and
//! [`blank_stylesheet_values`] blanks exactly that: the contents of every
//! rules block, which is always the brace group right after a
//! parenthesized binding (`base(t) { … }`, `small(t) { … }`,
//! `state hovered(t) { … }`), and the `transitions { … }` block, whose
//! entries become `*_transition` VALUES in the base `StyleRules` literal
//! (`emit_rules_struct_with_transitions`) with no type, setter or enum per
//! entry — so a changed duration or easing, or an added or removed entry,
//! is a body edit too. Other structural braces follow an identifier or `>`
//! and are kept, as are the keys in parentheses: which state or
//! breakpoint, a container's threshold, a compound's axis/value pairs.
//!
//! A premint session bakes that class-name hash into a stylesheet built
//! at session start, so there a value edit must still rebuild; see
//! [`stylesheet_tokens`], which the decider compares when premint is on.

use quote::ToTokens;
use syn::visit_mut::VisitMut;

/// The file with every function body blanked, as token text.
///
/// `None` when the file does not parse — the caller must treat that as
/// "cannot decide", which means rebuild.
pub fn shape_of(text: &str) -> Option<String> {
    let mut file: syn::File = syn::parse_file(text).ok()?;
    BlankBodies.visit_file_mut(&mut file);
    Some(file.to_token_stream().to_string())
}

struct BlankBodies;

/// Every `stylesheet!` invocation in item position, as token text,
/// values included. `None` when the file does not parse.
///
/// What a PREMINT session compares: there the class names the page uses
/// were generated from these tokens at session start, so any change to
/// them, a value included, has to rebuild.
pub fn stylesheet_tokens(text: &str) -> Option<String> {
    let file: syn::File = syn::parse_file(text).ok()?;
    let mut out = String::new();
    for item in &file.items {
        if let syn::Item::Macro(m) = item {
            if is_stylesheet(&m.mac) {
                out.push_str(&m.mac.tokens.to_string());
                out.push('\n');
            }
        }
    }
    Some(out)
}

fn is_stylesheet(mac: &syn::Macro) -> bool {
    mac.path.segments.last().is_some_and(|s| s.ident == "stylesheet")
}

/// Empty the contents of every rules block in a `stylesheet!` body,
/// keeping everything else. See the module docs for why a rules block
/// is exactly "a brace group right after a parenthesized group".
fn blank_stylesheet_values(tokens: proc_macro2::TokenStream) -> proc_macro2::TokenStream {
    use proc_macro2::{Delimiter, Group, TokenTree};
    let mut out: Vec<TokenTree> = Vec::new();
    for tt in tokens {
        let after_parens = matches!(
            out.last(),
            Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Parenthesis
        ) || matches!(out.last(), Some(TokenTree::Ident(i)) if i == "transitions");
        match tt {
            TokenTree::Group(g) if g.delimiter() == Delimiter::Brace && after_parens => {
                out.push(TokenTree::Group(Group::new(
                    Delimiter::Brace,
                    proc_macro2::TokenStream::new(),
                )));
            }
            TokenTree::Group(g) if g.delimiter() == Delimiter::Brace => {
                // A structural block — the sheet body, a `variant` axis:
                // keep it, and look inside for the rules blocks it holds.
                let mut inner = Group::new(Delimiter::Brace, blank_stylesheet_values(g.stream()));
                inner.set_span(g.span());
                out.push(TokenTree::Group(inner));
            }
            other => out.push(other),
        }
    }
    out.into_iter().collect()
}

impl VisitMut for BlankBodies {
    fn visit_item_macro_mut(&mut self, m: &mut syn::ItemMacro) {
        if is_stylesheet(&m.mac) {
            m.mac.tokens = blank_stylesheet_values(std::mem::take(&mut m.mac.tokens));
        }
    }

    fn visit_item_fn_mut(&mut self, f: &mut syn::ItemFn) {
        // Signature and attributes first: a nested `fn` inside the body
        // is about to be discarded with it, and a generic parameter's
        // default could itself contain a block.
        self.visit_signature_mut(&mut f.sig);
        *f.block = empty_block();
    }

    fn visit_impl_item_fn_mut(&mut self, f: &mut syn::ImplItemFn) {
        self.visit_signature_mut(&mut f.sig);
        f.block = empty_block();
    }

    fn visit_trait_item_fn_mut(&mut self, f: &mut syn::TraitItemFn) {
        self.visit_signature_mut(&mut f.sig);
        if f.default.is_some() {
            f.default = Some(empty_block());
        }
    }
}

fn empty_block() -> syn::Block {
    syn::Block {
        brace_token: Default::default(),
        stmts: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn same(a: &str, b: &str) -> bool {
        shape_of(a).unwrap() == shape_of(b).unwrap()
    }

    const BASE: &str = r#"
        use runtime_core::{component, signal, ui, Element};

        #[component]
        fn Badge(label: String, count: i32) -> Element {
            let doubled = count * 2;
            ui! { view { text { "{label} {doubled}" } } }
        }

        pub struct Config { pub retries: u32 }

        pub static LIMIT: u32 = 10;

        pub fn app() -> Element {
            let n = signal(0i32);
            ui! { view { Badge(label = "x".into(), count = 1) } }
        }
    "#;

    // ---------------------------------------------------------------
    // Body-only edits: the shape holds, so the save is hot-patchable.
    // ---------------------------------------------------------------

    #[test]
    fn changing_an_expression_keeps_the_shape() {
        assert!(same(BASE, &BASE.replace("count * 2", "count * 3")));
    }

    #[test]
    fn adding_a_statement_keeps_the_shape() {
        let edited = BASE.replace(
            "let doubled = count * 2;",
            "let tripled = count * 3; let doubled = tripled - count;",
        );
        assert!(same(BASE, &edited));
    }

    /// The dangerous one the decision table names: a new `signal()`
    /// changes what state the component owns, but NOT what the rest of
    /// the process believes about its layout. It is body-only, so it
    /// patches; keeping its state across the patch is the slot
    /// registry's problem, not this one's.
    #[test]
    fn adding_a_signal_call_keeps_the_shape() {
        let edited = BASE.replace(
            "let n = signal(0i32);",
            "let n = signal(0i32); let m = signal(String::new());",
        );
        assert!(same(BASE, &edited));
    }

    #[test]
    fn changing_a_closures_captures_keeps_the_shape() {
        let edited = BASE.replace(
            "let n = signal(0i32);",
            "let n = signal(0i32); let f = move || n.get() + 1; let _ = f();",
        );
        assert!(same(BASE, &edited));
    }

    #[test]
    fn reindenting_a_body_keeps_the_shape() {
        let edited = BASE.replace(
            "            let doubled = count * 2;",
            "let doubled  =  count*2;",
        );
        assert!(same(BASE, &edited));
    }

    /// A free fn edited next to the components that call it. Body-only,
    /// so it patches — and it reaches the running app through those
    /// components, since the whole crate is re-emitted into one patch
    /// dylib and the patched `__*_hot_impl` calls the dylib's copy.
    #[test]
    fn editing_a_free_fn_body_keeps_the_shape() {
        let src = format!("{BASE}\n fn helper(n: u32) -> u32 {{ n + 1 }}");
        let edited = src.replace("n + 1", "n + 2");
        assert!(same(&src, &edited));
    }

    // ---------------------------------------------------------------
    // Shape edits: the process and the patch would disagree about
    // layout, so these must reach rebuild-and-respawn.
    // ---------------------------------------------------------------

    #[test]
    fn changing_a_prop_type_changes_the_shape() {
        assert!(!same(BASE, &BASE.replace("count: i32", "count: u64")));
    }

    #[test]
    fn adding_a_prop_changes_the_shape() {
        assert!(!same(BASE, &BASE.replace("count: i32)", "count: i32, tone: Tone)")));
    }

    #[test]
    fn adding_a_struct_field_changes_the_shape() {
        assert!(!same(BASE, &BASE.replace("pub retries: u32", "pub retries: u32, pub tries: u8")));
    }

    #[test]
    fn changing_a_static_initializer_changes_the_shape() {
        assert!(!same(BASE, &BASE.replace("LIMIT: u32 = 10", "LIMIT: u32 = 20")));
    }

    #[test]
    fn adding_an_item_changes_the_shape() {
        assert!(!same(BASE, &format!("{BASE}\n pub enum Tone {{ Warm }}")));
    }

    #[test]
    fn changing_an_attribute_changes_the_shape() {
        assert!(!same(BASE, &BASE.replace("#[component]", "#[component(children)]")));
    }

    #[test]
    fn renaming_a_component_changes_the_shape() {
        assert!(!same(BASE, &BASE.replace("fn Badge", "fn Chip")));
    }

    #[test]
    fn changing_a_return_type_changes_the_shape() {
        assert!(!same(BASE, &BASE.replace("fn app() -> Element", "fn app() -> Option<Element>")));
    }

    /// `syn` cannot see inside a macro, so an item-position macro's
    /// tokens are part of the shape — any macro but `stylesheet!`, whose
    /// values are known to land only in function bodies.
    #[test]
    fn editing_an_unknown_item_position_macro_changes_the_shape() {
        let src = format!("{BASE}\n widgets! {{ card {{ padding: 8 }} }}");
        let edited = src.replace("padding: 8", "padding: 12");
        assert!(!same(&src, &edited));
    }

    const SHEET: &str = r#"
        stylesheet! {
            pub Banner<IdeaThemeRef> {
                base(t) {
                    padding: 8,
                    background: t.color.surface(),
                }
                variant tone {
                    #[default]
                    calm(t) { border_color: t.color.border() }
                    loud(_t) { border_width: 2 }
                }
                state hovered(t) { background: t.color.hover() }
                breakpoint md(_t) { padding: 16 }
                override width: f32
                transitions { background: 150ms ease_out }
            }
        }
    "#;

    /// The point of the stylesheet blanking: a VALUE edit — a number, a
    /// token path, an added rule, a transition's duration or easing, an
    /// added or removed transition — is body-only, so it can be a hot
    /// patch instead of a rebuild. (Regression: transitions were first
    /// kept as shape, so a transition tweak still rebuilt.)
    #[test]
    fn a_stylesheet_value_edit_is_body_only() {
        let src = format!("{BASE}\n{SHEET}");
        for (from, to) in [
            ("padding: 8,", "padding: 12,"),
            ("t.color.surface()", "t.color.accent()"),
            ("border_width: 2", "border_width: 3, opacity: 0.5"),
            ("breakpoint md(_t) { padding: 16 }", "breakpoint md(_t) { padding: 24 }"),
            ("state hovered(t) { background: t.color.hover() }", "state hovered(t) { }"),
            // Transitions are values folded into the base rules.
            ("background: 150ms ease_out", "background: 600ms ease_out"),
            ("background: 150ms ease_out", "background: 150ms ease_in"),
            (
                "transitions { background: 150ms ease_out }",
                "transitions { background: 150ms ease_out padding: 200ms linear }",
            ),
            ("transitions { background: 150ms ease_out }", "transitions { }"),
        ] {
            let edited = src.replace(from, to);
            assert_ne!(src, edited, "fixture did not contain {from}");
            assert!(same(&src, &edited), "{from} -> {to} should be body-only");
        }
    }

    /// Everything the generated ITEMS are derived from stays shape. A
    /// renamed arm is a renamed enum variant; a moved `#[default]` is a
    /// different `Default` impl; a new override is a new setter.
    #[test]
    fn a_stylesheet_signature_edit_changes_the_shape() {
        let src = format!("{BASE}\n{SHEET}");
        for (from, to) in [
            ("loud(_t)", "shouty(_t)"),
            ("variant tone", "variant mood"),
            ("#[default]\n                    calm", "calm"),
            ("override width: f32", "override width: f64"),
            ("pub Banner<", "pub Ribbon<"),
            ("state hovered(t)", "state pressed(t)"),
            ("breakpoint md(_t)", "breakpoint lg(_t)"),
        ] {
            let edited = src.replace(from, to);
            assert_ne!(src, edited, "fixture did not contain {from}");
            assert!(!same(&src, &edited), "{from} -> {to} must rebuild");
        }
    }

    /// A premint session compares the raw invocation, values and all.
    #[test]
    fn stylesheet_tokens_see_a_value_edit() {
        let src = format!("{BASE}\n{SHEET}");
        let edited = src.replace("padding: 8,", "padding: 12,");
        assert_ne!(stylesheet_tokens(&src), stylesheet_tokens(&edited));
        assert_eq!(stylesheet_tokens(&src), stylesheet_tokens(&src.replace("fn app", "fn app ")));
    }

    // ---------------------------------------------------------------

    #[test]
    fn a_file_that_does_not_parse_has_no_shape() {
        assert!(shape_of("fn broken( {").is_none());
    }

    #[test]
    fn impl_and_trait_bodies_are_blanked_too() {
        let src = "trait T { fn d(&self) -> u8 { 1 } } struct S; impl S { fn m(&self) -> u8 { 2 } }";
        let edited = "trait T { fn d(&self) -> u8 { 9 } } struct S; impl S { fn m(&self) -> u8 { 8 } }";
        assert!(same(src, edited));
        // …but their signatures are not.
        assert!(!same(src, "trait T { fn d(&self) -> u16 { 1 } } struct S; impl S { fn m(&self) -> u8 { 2 } }"));
    }
}
