//! Which `const` VALUES a hot patch can carry.
//!
//! [`crate::shape_of`] blanks a const item's initializer, so editing
//! `const HEADLINE: &str = "…"` no longer reads as a shape change. That is
//! only half the answer: a const's value is copied into every place that
//! uses it, and some of those places are not function bodies a patch
//! re-emits. This module records, per file, what the dev loop needs to
//! tell the two apart, and [`compile_time_reach`] combines those records
//! across a crate.
//!
//! # Why the rule is by USE, not by type
//!
//! The first idea is "integer consts can size an array, string consts
//! cannot". It is wrong: `[u8; PREFIX.len()]` is a legal array length
//! for a `&str` const, `[u8; SCALE as usize]` for an `f32` one,
//! `[u8; SIZES[2]]` for a `&[usize]` one, and a `bool` or `char` can be a
//! const generic argument. So no type is safe on its own, and an integer
//! const that only ever reaches a function body is as safe as a string.
//! What decides it is where the value is READ:
//!
//! - in a **function body** — the patch re-emits every body of the crate
//!   from source, so each one sees the new value;
//! - at **compile time, somewhere that outlives the patch** — an array
//!   length, a repeat count, a const generic argument, an enum
//!   discriminant, a const generic parameter's default, a `const fn`
//!   body, a `const _` assertion — the value has gone into a TYPE or a
//!   LAYOUT, which the rest of the process (the framework's generic
//!   instantiations, compiled into rlibs a patch never re-emits) agrees
//!   on with the old value;
//! - in a **`static`** initializer (an item, a `static` inside a body, a
//!   `thread_local!` / `lazy_static!`-style macro): a static's storage is
//!   not re-initialized by a patch. The patch either defines a FRESH copy
//!   (a static of a crate it re-emits — patched code then reads different
//!   storage from the base code still running) or imports the base's
//!   through `GOT.mem` (`build-web`'s `hotpatch_prepare`), where the old
//!   value already sits. Neither shows the edit honestly.
//!
//! A const read by ANOTHER const inherits that const's fate, so the
//! crate-wide answer is a closure: start from the names read in the
//! compile-time places above, add every name an affected const reads,
//! repeat ([`compile_time_reach`]).
//!
//! # Names, not paths, and why that is sound
//!
//! Without name resolution, a read is recorded as every identifier in the
//! expression — `consts::N`, `Self::N`, `<T as Tr>::N` all record `N`, and
//! the whole expression is walked, so `N` inside `N + 1` or `foo(N)` counts
//! too. Two consts of the same name in different modules are then one
//! name: the cost is a rebuild where a patch was possible, never the
//! reverse. Words inside string literals in those places count as names
//! too, for a `format!("{N}")` that captures one implicitly.
//!
//! What a name cannot see:
//!
//! - **`use … as …`**: `use consts::N as LEN; [u8; LEN]` records `LEN`.
//!   So both sides of every renaming `use` are recorded as a pair
//!   ([`ConstFacts::renames`]) and [`compile_time_reach`] follows them.
//! - **A macro's expansion.** Inside a macro the tokens are opaque, so
//!   every name in an unknown macro — at any position, a body included —
//!   is recorded as a compile-time read: it might expand to a `static`
//!   (`thread_local!`) or an array type. Only macros known to expand to
//!   plain expressions in the enclosing body are exempt: `ui!`, `jsx!`
//!   and std's formatting / assertion macros.
//! - **Sources the scan does not read.** A `#[path]` module or an
//!   `include!` pulls code from outside `src/`, where a read cannot be
//!   seen; a file containing either is [`ConstFacts::opaque`], and in such
//!   a crate every const value edit rebuilds.
//! - **Attribute macros.** A non-builtin attribute's arguments may become
//!   anything, so its names are recorded too, except for the attributes
//!   whose expansion is known: the `#[component]` / `#[prop]` family
//!   (their expressions land in a generated `Default` body), `doc`,
//!   `derive` (whose input is the item's own tokens, already walked), and
//!   the lint / `cfg` / `inline` builtins. The item an attribute macro
//!   annotates is walked as written: a function body under one is taken
//!   to stay a body. That is the assumption the whole hot-patch tier
//!   already makes about a body EDIT under an attribute macro, so a const
//!   read there is no weaker.
//!
//! # Premint
//!
//! A premint session (`dev --premint`) ran every `stylesheet!`'s rules at
//! session start and baked the result into CSS. A const read by a sheet's
//! rules is therefore baked too, though the sheet's own tokens did not
//! move. [`ConstFacts::sheet_reads`] records those names; the decider adds
//! them to the closure's roots in a premint session only — outside one, a
//! sheet's values run inside `<name>_style()`, a body the patch re-emits.
//!
//! # Across crates
//!
//! All of this is about ONE crate. A const of a LIBRARY crate is also
//! inlined into its dependents, which a patch re-emits against the base
//! build's metadata — old value included. That case is
//! `crate::downstream_bodies`'s: every const there is a downstream item.

use std::collections::{BTreeMap, BTreeSet};

use proc_macro2::{TokenStream, TokenTree};
use quote::ToTokens;
use syn::visit::Visit;

/// One const whose value the shape does not hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConstValue {
    /// The const's own name, as a read of it is recorded.
    pub name: String,
    /// Its initializer, as token text.
    pub value: String,
}

/// What one file says about const values. See the module docs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConstFacts {
    /// Every const whose initializer [`crate::shape_of`] blanks, by a
    /// label unique in the file (`const NAME`, `impl Type::const NAME`,
    /// `trait Name::const NAME`, prefixed with enclosing `mod`s).
    pub values: BTreeMap<String, ConstValue>,
    /// Per const NAME (any const, a body's included), the names its
    /// initializer reads — the edges [`compile_time_reach`] follows.
    pub reads: BTreeMap<String, BTreeSet<String>>,
    /// Names read somewhere a value reaches a type, a layout or a static.
    pub compile_time: BTreeSet<String>,
    /// Names read by a `stylesheet!`'s tokens — compile-time only in a
    /// premint session.
    pub sheet_reads: BTreeSet<String>,
    /// `(alias, original)` for every `use … as alias`.
    pub renames: BTreeSet<(String, String)>,
    /// The file pulls in source the scan cannot read (`#[path]`,
    /// `include!`).
    pub opaque: bool,
}

/// The const facts of `text`. `None` when it does not parse.
pub fn const_facts(text: &str) -> Option<ConstFacts> {
    let file: syn::File = syn::parse_file(text).ok()?;
    let mut c = Collector {
        facts: ConstFacts::default(),
        in_body: 0,
        labels: Vec::new(),
    };
    c.visit_file(&file);
    Some(c.facts)
}

/// Every name whose value reaches compile time anywhere in the crate,
/// given every file's facts. `None` means "cannot tell" — some file is
/// opaque — and the caller must treat every const as reaching it.
pub fn compile_time_reach<'a>(
    files: impl IntoIterator<Item = &'a ConstFacts>,
    premint: bool,
) -> Option<BTreeSet<String>> {
    let mut reach: BTreeSet<String> = BTreeSet::new();
    let mut reads: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    let mut renames: Vec<(&str, &str)> = Vec::new();
    for f in files {
        if f.opaque {
            return None;
        }
        reach.extend(f.compile_time.iter().cloned());
        if premint {
            reach.extend(f.sheet_reads.iter().cloned());
        }
        for (name, names) in &f.reads {
            reads
                .entry(name)
                .or_default()
                .extend(names.iter().map(String::as_str));
        }
        renames.extend(f.renames.iter().map(|(a, o)| (a.as_str(), o.as_str())));
    }
    let mut stack: Vec<String> = reach.iter().cloned().collect();
    while let Some(name) = stack.pop() {
        let mut next: Vec<&str> = reads.get(name.as_str()).cloned().unwrap_or_default();
        next.extend(renames.iter().filter(|(a, _)| *a == name).map(|(_, o)| *o));
        for n in next {
            if reach.insert(n.to_string()) {
                stack.push(n.to_string());
            }
        }
    }
    Some(reach)
}

/// Whether [`crate::shape_of`] blanks this const's initializer — the one
/// predicate both it and [`const_facts`] use, so the value a shape no
/// longer holds is always a value the facts track.
///
/// Not for `const _` (it exists for what its initializer DOES — an
/// assertion, a registration — and is never read by name), and not for
/// an initializer that declares items (`= { static S: … ; &S }`): those
/// items are part of the shape like any other.
pub(crate) fn blanks_value(ident: &syn::Ident, expr: &syn::Expr) -> bool {
    ident != "_" && !declares_items(expr)
}

fn declares_items(expr: &syn::Expr) -> bool {
    struct Finder(bool);
    impl<'ast> Visit<'ast> for Finder {
        fn visit_item(&mut self, _: &'ast syn::Item) {
            self.0 = true;
        }
    }
    let mut f = Finder(false);
    f.visit_expr(expr);
    f.0
}

/// Macros whose expansion is an expression evaluated in the enclosing
/// body: nothing in them reaches a type, a layout or a static.
const EXPRESSION_MACROS: &[&str] = &[
    "ui",
    "jsx",
    "format",
    "format_args",
    "print",
    "println",
    "eprint",
    "eprintln",
    "write",
    "writeln",
    "vec",
    "panic",
    "assert",
    "assert_eq",
    "assert_ne",
    "debug_assert",
    "debug_assert_eq",
    "debug_assert_ne",
    "unreachable",
    "todo",
    "unimplemented",
    "matches",
    "dbg",
];

/// Attributes whose arguments are known not to reach compile time; see
/// the module docs.
const INERT_ATTRS: &[&str] = &[
    "doc",
    "component",
    "prop",
    "props",
    "method",
    "derive",
    "allow",
    "warn",
    "deny",
    "expect",
    "forbid",
    "cfg",
    "cfg_attr",
    "inline",
    "must_use",
    "cold",
    "track_caller",
    "deprecated",
    "non_exhaustive",
    "automatically_derived",
];

struct Collector {
    facts: ConstFacts,
    /// Non-zero inside a function or closure body: a const declared
    /// there is a body edit, not a tracked value.
    in_body: usize,
    labels: Vec<String>,
}

impl Collector {
    fn label(&self, own: String) -> String {
        let mut label = self.labels.concat();
        label.push_str(&own);
        let mut key = label.clone();
        let mut n = 2;
        while self.facts.values.contains_key(&key) {
            key = format!("{label} #{n}");
            n += 1;
        }
        key
    }

    fn record_const(&mut self, label: String, ident: &syn::Ident, expr: &syn::Expr) {
        let name = ident.to_string();
        let reads = names_in(expr.to_token_stream());
        if self.in_body == 0 && blanks_value(ident, expr) {
            self.facts.values.insert(
                self.label(label),
                ConstValue {
                    name: name.clone(),
                    value: expr.to_token_stream().to_string(),
                },
            );
        }
        if ident == "_" {
            // An assertion or a registration: whatever it reads is read
            // at compile time.
            self.facts.compile_time.extend(reads);
        } else {
            self.facts.reads.entry(name).or_default().extend(reads);
        }
    }

    fn compile_time(&mut self, tokens: TokenStream) {
        self.facts.compile_time.extend(names_in(tokens));
    }

    fn body<T>(&mut self, f: impl FnOnce(&mut Self) -> T) -> T {
        self.in_body += 1;
        let out = f(self);
        self.in_body -= 1;
        out
    }
}

impl<'ast> Visit<'ast> for Collector {
    fn visit_item_const(&mut self, c: &'ast syn::ItemConst) {
        self.record_const(format!("const {}", c.ident), &c.ident, &c.expr);
        syn::visit::visit_item_const(self, c);
    }

    fn visit_impl_item_const(&mut self, c: &'ast syn::ImplItemConst) {
        self.record_const(format!("const {}", c.ident), &c.ident, &c.expr);
        syn::visit::visit_impl_item_const(self, c);
    }

    fn visit_trait_item_const(&mut self, c: &'ast syn::TraitItemConst) {
        if let Some((_, expr)) = &c.default {
            self.record_const(format!("const {}", c.ident), &c.ident, expr);
        }
        syn::visit::visit_trait_item_const(self, c);
    }

    fn visit_item_mod(&mut self, m: &'ast syn::ItemMod) {
        self.labels.push(format!("{}::", m.ident));
        syn::visit::visit_item_mod(self, m);
        self.labels.pop();
    }

    fn visit_item_impl(&mut self, imp: &'ast syn::ItemImpl) {
        let self_ty = imp.self_ty.to_token_stream().to_string();
        self.labels.push(match &imp.trait_ {
            Some((_, path, _)) => format!("impl {} for {self_ty}::", path.to_token_stream()),
            None => format!("impl {self_ty}::"),
        });
        syn::visit::visit_item_impl(self, imp);
        self.labels.pop();
    }

    fn visit_item_trait(&mut self, t: &'ast syn::ItemTrait) {
        self.labels.push(format!("trait {}::", t.ident));
        syn::visit::visit_item_trait(self, t);
        self.labels.pop();
    }

    fn visit_item_fn(&mut self, f: &'ast syn::ItemFn) {
        if f.sig.constness.is_some() {
            self.compile_time(f.block.to_token_stream());
        }
        for a in &f.attrs {
            self.visit_attribute(a);
        }
        self.visit_signature(&f.sig);
        self.body(|c| c.visit_block(&f.block));
    }

    fn visit_impl_item_fn(&mut self, f: &'ast syn::ImplItemFn) {
        if f.sig.constness.is_some() {
            self.compile_time(f.block.to_token_stream());
        }
        for a in &f.attrs {
            self.visit_attribute(a);
        }
        self.visit_signature(&f.sig);
        self.body(|c| c.visit_block(&f.block));
    }

    fn visit_trait_item_fn(&mut self, f: &'ast syn::TraitItemFn) {
        if let Some(block) = &f.default {
            if f.sig.constness.is_some() {
                self.compile_time(block.to_token_stream());
            }
        }
        for a in &f.attrs {
            self.visit_attribute(a);
        }
        self.visit_signature(&f.sig);
        if let Some(block) = &f.default {
            self.body(|c| c.visit_block(block));
        }
    }

    fn visit_expr_closure(&mut self, e: &'ast syn::ExprClosure) {
        self.body(|c| syn::visit::visit_expr_closure(c, e));
    }

    fn visit_item_static(&mut self, s: &'ast syn::ItemStatic) {
        self.compile_time(s.to_token_stream());
        syn::visit::visit_item_static(self, s);
    }

    fn visit_type_array(&mut self, a: &'ast syn::TypeArray) {
        self.compile_time(a.len.to_token_stream());
        syn::visit::visit_type_array(self, a);
    }

    fn visit_expr_repeat(&mut self, r: &'ast syn::ExprRepeat) {
        self.compile_time(r.len.to_token_stream());
        syn::visit::visit_expr_repeat(self, r);
    }

    fn visit_generic_argument(&mut self, g: &'ast syn::GenericArgument) {
        match g {
            // `Foo<N>` parses as a TYPE argument; a const argument that is
            // not a literal or a block must be a single-segment path, so
            // that is the one type spelling that can be a const.
            syn::GenericArgument::Type(syn::Type::Path(p))
                if p.qself.is_none() && p.path.segments.len() == 1 =>
            {
                self.facts
                    .compile_time
                    .insert(p.path.segments[0].ident.to_string());
            }
            syn::GenericArgument::Const(e) => self.compile_time(e.to_token_stream()),
            syn::GenericArgument::AssocConst(c) => self.compile_time(c.value.to_token_stream()),
            _ => {}
        }
        syn::visit::visit_generic_argument(self, g);
    }

    fn visit_const_param(&mut self, p: &'ast syn::ConstParam) {
        if let Some(d) = &p.default {
            self.compile_time(d.to_token_stream());
        }
        syn::visit::visit_const_param(self, p);
    }

    fn visit_variant(&mut self, v: &'ast syn::Variant) {
        if let Some((_, d)) = &v.discriminant {
            self.compile_time(d.to_token_stream());
        }
        syn::visit::visit_variant(self, v);
    }

    fn visit_item_use(&mut self, u: &'ast syn::ItemUse) {
        fn walk(t: &syn::UseTree, out: &mut BTreeSet<(String, String)>) {
            match t {
                syn::UseTree::Path(p) => walk(&p.tree, out),
                syn::UseTree::Rename(r) => {
                    out.insert((r.rename.to_string(), r.ident.to_string()));
                }
                syn::UseTree::Group(g) => g.items.iter().for_each(|t| walk(t, out)),
                syn::UseTree::Name(_) | syn::UseTree::Glob(_) => {}
            }
        }
        walk(&u.tree, &mut self.facts.renames);
    }

    fn visit_attribute(&mut self, a: &'ast syn::Attribute) {
        let name = a
            .path()
            .segments
            .last()
            .map(|s| s.ident.to_string())
            .unwrap_or_default();
        if name == "path" {
            self.facts.opaque = true;
        }
        if !INERT_ATTRS.contains(&name.as_str()) {
            self.compile_time(a.meta.to_token_stream());
        }
    }

    fn visit_macro(&mut self, m: &'ast syn::Macro) {
        let name = m
            .path
            .segments
            .last()
            .map(|s| s.ident.to_string())
            .unwrap_or_default();
        if name == "include" {
            self.facts.opaque = true;
        }
        if name == "stylesheet" {
            self.facts.sheet_reads.extend(names_in(m.tokens.clone()));
        } else if !EXPRESSION_MACROS.contains(&name.as_str()) {
            self.compile_time(m.tokens.clone());
        }
    }
}

/// Every identifier in `tokens`, and every identifier-shaped word inside
/// its string literals.
fn names_in(tokens: TokenStream) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    fn walk(tokens: TokenStream, out: &mut BTreeSet<String>) {
        for tt in tokens {
            match tt {
                TokenTree::Ident(i) => {
                    let s = i.to_string();
                    out.insert(s.strip_prefix("r#").map(str::to_string).unwrap_or(s));
                }
                TokenTree::Group(g) => walk(g.stream(), out),
                TokenTree::Literal(l) => {
                    let s = l.to_string();
                    if s.contains('"') {
                        for word in s.split(|c: char| !(c.is_alphanumeric() || c == '_')) {
                            if word.starts_with(|c: char| c.is_alphabetic() || c == '_') {
                                out.insert(word.to_string());
                            }
                        }
                    }
                }
                TokenTree::Punct(_) => {}
            }
        }
    }
    walk(tokens, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reach(src: &str, premint: bool) -> Option<BTreeSet<String>> {
        compile_time_reach([&const_facts(src).expect("parses")], premint)
    }

    fn reaches(src: &str, name: &str) -> bool {
        reach(src, false).is_none_or(|r| r.contains(name))
    }

    #[test]
    fn a_const_read_only_in_bodies_does_not_reach_compile_time() {
        let src = r#"
            const HEADLINE: &str = "hi";
            const RETRIES: u32 = 3;
            fn f() -> String { let n = RETRIES + 1; format!("{HEADLINE} {n}") }
            fn g() -> Element { ui! { text { HEADLINE } } }
        "#;
        assert!(!reaches(src, "HEADLINE"));
        assert!(!reaches(src, "RETRIES"));
        let facts = const_facts(src).unwrap();
        assert_eq!(facts.values["const HEADLINE"].value, "\"hi\"");
    }

    #[test]
    fn an_integer_const_sizing_an_array_reaches_compile_time() {
        assert!(reaches("const N: usize = 4; struct S { a: [u8; N] }", "N"));
        assert!(reaches(
            "const N: usize = 4; fn f(a: [u8; consts::N]) {}",
            "N"
        ));
        assert!(reaches(
            "const N: usize = 4; fn f() { let a = [0u8; N]; }",
            "N"
        ));
    }

    /// The reason the rule is by use: a non-integer const can size a type
    /// too.
    #[test]
    fn a_string_const_whose_length_sizes_an_array_reaches_compile_time() {
        assert!(reaches(
            r#"const P: &str = "abc"; struct S { a: [u8; P.len()] }"#,
            "P"
        ));
    }

    #[test]
    fn const_generic_arguments_defaults_and_discriminants_reach_compile_time() {
        assert!(reaches(
            "const FLAG: bool = true; type W = Wrap<FLAG>;",
            "FLAG"
        ));
        assert!(reaches(
            "const N: usize = 1; type W = Wrap<{ N + 1 }>;",
            "N"
        ));
        assert!(reaches(
            "const N: usize = 1; struct W<const M: usize = N>;",
            "N"
        ));
        assert!(reaches("const N: isize = 1; enum E { A = N, B }", "N"));
        assert!(reaches(
            "const N: usize = 1; const _: () = assert!(N > 0);",
            "N"
        ));
        assert!(reaches(
            "const N: usize = 1; const fn size() -> usize { N }",
            "N"
        ));
    }

    #[test]
    fn a_static_initializer_reaches_compile_time_in_every_spelling() {
        assert!(reaches(r#"const H: &str = "a"; static S: &str = H;"#, "H"));
        assert!(reaches(
            r#"const H: &str = "a"; static S: LazyLock<String> = LazyLock::new(|| format!("{H}!"));"#,
            "H"
        ));
        assert!(reaches(
            r#"const H: &str = "a"; fn f() { static S: &str = H; }"#,
            "H"
        ));
        assert!(reaches(
            r#"const H: &str = "a"; thread_local! { static S: String = H.to_string(); }"#,
            "H"
        ));
        assert!(reaches(
            r#"const H: &str = "a"; fn f() { thread_local! { static S: String = H.to_string(); } }"#,
            "H"
        ));
    }

    /// A const read by a const that reaches compile time reaches it too,
    /// through a renaming `use` as well.
    #[test]
    fn the_reach_is_closed_over_consts_and_renames() {
        let src = "const BASE: usize = 2; const N: usize = BASE * 2; struct S { a: [u8; N] }";
        assert!(reaches(src, "BASE"));
        let src = "const BASE: usize = 2; const N: usize = BASE * 2; fn f() -> usize { N }";
        assert!(!reaches(src, "BASE"));
        let src = "use crate::consts::WIDTH as W; struct S { a: [u8; W] }";
        assert!(reaches(src, "WIDTH"));
    }

    #[test]
    fn the_reach_spans_files() {
        let a = const_facts(r#"pub const N: usize = 3; pub const H: &str = "x";"#).unwrap();
        let b = const_facts("struct S { a: [u8; crate::a::N] }").unwrap();
        let r = compile_time_reach([&a, &b], false).unwrap();
        assert!(r.contains("N"));
        assert!(!r.contains("H"));
    }

    #[test]
    fn an_unknown_macro_anywhere_counts_but_ui_and_format_do_not() {
        assert!(reaches(
            r#"const H: &str = "a"; fn f() { my_macro!(H); }"#,
            "H"
        ));
        assert!(!reaches(
            r#"const H: &str = "a"; fn f() { println!("{}", H); }"#,
            "H"
        ));
    }

    #[test]
    fn a_path_module_or_include_makes_the_crate_opaque() {
        assert!(reach(r#"#[path = "../x.rs"] mod x;"#, false).is_none());
        assert!(reach(r#"include!(concat!(env!("OUT_DIR"), "/gen.rs"));"#, false).is_none());
    }

    /// A sheet's rules run inside `<name>_style()` — a body — except in a
    /// premint session, which baked them into CSS at session start.
    #[test]
    fn a_const_read_by_a_stylesheet_reaches_compile_time_only_under_premint() {
        let src = r#"
            const PAD: f32 = 8.0;
            stylesheet! { pub Card<IdeaThemeRef> { base(_t) { padding: PAD } } }
        "#;
        assert!(!reach(src, false).unwrap().contains("PAD"));
        assert!(reach(src, true).unwrap().contains("PAD"));
    }

    #[test]
    fn assoc_consts_are_tracked_with_their_impl_in_the_label() {
        let src = r#"
            struct X;
            impl X { const Y: &str = "a"; }
            trait T { const Z: u8 = 1; const W: u8; }
            mod m { pub const V: &str = "b"; }
        "#;
        let labels: Vec<_> = const_facts(src).unwrap().values.into_keys().collect();
        assert_eq!(
            labels,
            ["impl X::const Y", "m::const V", "trait T::const Z"]
        );
    }

    #[test]
    fn body_consts_and_underscore_consts_are_not_tracked_values() {
        let src = r#"fn f() { const L: &str = "a"; } const _: () = ();"#;
        assert!(const_facts(src).unwrap().values.is_empty());
    }
}
