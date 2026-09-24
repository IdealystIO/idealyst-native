//! `prefer-component` — a free function that returns an idealyst
//! `Element` but isn't a `#[component]` is almost always a component
//! written outside the framework's paradigm.
//!
//! `fn user_row(name: String, active: bool) -> Element` works, but it is
//! called positionally (`user_row(name, true)`) instead of through `ui!`
//! struct-literal dispatch (`UserRow(name = …, active = true)`), its
//! params aren't `Reactive<T>`, and every new option grows the positional
//! signature — the `tone, variant, label, …` shape CLAUDE.md §9.5 warns
//! about. `#[component]` is the fix: the params become inline props and
//! the call site moves into the macro.
//!
//! Flagged:
//! - any such fn with parameters — including a hand-rolled
//!   `fn Card(props: &CardProps) -> Element` that skipped the attribute;
//! - a zero-arg fn called from two or more sites in the file (reuse is
//!   the §9.5 promotion trigger).
//!
//! Deliberately NOT flagged, because each is a sanctioned shape:
//! - a fn used as a **value** anywhere in the file (`start_in("#app",
//!   reg, app)`, a route/screen table, a `flat_list` render callback) —
//!   the consumer wants a `fn(..) -> Element`, and `#[component]` on an
//!   entry fn is a known inference trap;
//! - a zero-arg fn called from at most one site — §9.5's file-local
//!   one-off helper (and a `pub` zero-arg fn is most likely a screen or
//!   entry point referenced from another file, which this per-file
//!   engine can't see);
//! - methods (`impl` / trait items: `From<…> for Element`, builders);
//! - `#[test]` fns, anything under a `#[cfg(test)]` module, and whole
//!   files with a `#[test]` fn outside such a module (an integration
//!   test under `tests/`) — fixtures;
//! - a fn whose body neither invokes `ui!` / `jsx!` nor calls a
//!   primitive constructor (`view(…)`, `text(…)`, …) — Element plumbing
//!   (`finalize_switch(el: Element, …)` restyling a built node, a
//!   Registry-payload builder), not a tree an author composes;
//! - a return type that isn't the idealyst `Element` — qualified through
//!   a non-framework path, or in a file that imports some other `Element`
//!   (`web_sys::Element` is the common collision). The `ui!` gate above
//!   is the positive evidence; this only rules out the collision.
//!
//! Needs whole-file facts (call counts, value uses, imports), so unlike
//! the node-local rules it runs its own walk from [`check_file`].

use std::collections::{HashMap, HashSet};

use proc_macro2::{TokenStream, TokenTree};
use syn::visit::Visit;

use crate::diagnostic::RawDiag;
use crate::rules::component_case::{has_component_attr, is_pascal_case, to_pascal_case};
use crate::rules::last_segment;
use crate::rules::prefer_ui::constructor;

pub(crate) const RULE: &str = "prefer-component";

/// Path segments that place an `Element` in the idealyst framework.
/// Narrower than `prefer_ui::FRAMEWORK_ROOTS` on purpose: `prelude`
/// alone would claim `gstreamer::prelude::*`.
const ELEMENT_ROOTS: &[&str] =
    &["runtime_core", "runtime_vocabulary", "runtime_scene", "idealyst", "glue", "idea_ui"];

/// Attributes that mark a fn as an FFI / wasm entry point, not a
/// component (test attributes are [`is_test_attr`]).
const EXEMPT_ATTRS: &[&str] = &["wasm_bindgen", "no_mangle", "export_name"];

pub(crate) fn check_file(file: &syn::File, out: &mut Vec<RawDiag>) {
    let facts = FileFacts::scan(file);
    if facts.foreign_element_import || facts.integration_test_file {
        return;
    }
    let mut walker = FnWalker { facts: &facts, test_depth: 0, out };
    walker.visit_file(file);
}

struct FnWalker<'a> {
    facts: &'a FileFacts,
    /// Nesting depth of `#[cfg(test)]` modules; fns inside are fixtures.
    test_depth: usize,
    out: &'a mut Vec<RawDiag>,
}

impl<'ast> Visit<'ast> for FnWalker<'_> {
    fn visit_item_mod(&mut self, node: &'ast syn::ItemMod) {
        let is_test = node.attrs.iter().any(is_cfg_test);
        self.test_depth += is_test as usize;
        syn::visit::visit_item_mod(self, node);
        self.test_depth -= is_test as usize;
    }

    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        if self.test_depth == 0 {
            check_fn(node, self.facts, self.out);
        }
        syn::visit::visit_item_fn(self, node);
    }

    // `impl` / trait fns are methods (`From<…> for Element`, builder
    // `.build()`), never components — don't descend into them. Nested
    // free fns inside method bodies are rare enough to not warrant the walk.
    fn visit_item_impl(&mut self, _: &'ast syn::ItemImpl) {}
    fn visit_item_trait(&mut self, _: &'ast syn::ItemTrait) {}
}

fn check_fn(item: &syn::ItemFn, facts: &FileFacts, out: &mut Vec<RawDiag>) {
    let sig = &item.sig;
    if has_component_attr(&item.attrs) || sig.ident == "main" || !returns_element(&sig.output) {
        return;
    }
    let exempt_attr = |a: &syn::Attribute| {
        is_test_attr(a) || last_segment(a.path()).is_some_and(|s| EXEMPT_ATTRS.contains(&s.as_str()))
    };
    if item.attrs.iter().any(exempt_attr) {
        return;
    }
    let name = sig.ident.to_string();
    if facts.value_uses.contains(&name) || !composes_tree(&item.block) {
        return;
    }

    let calls = facts.calls.get(&name).copied().unwrap_or(0);
    let rename = if is_pascal_case(&name) {
        String::new()
    } else {
        format!(" and rename to `{}`", to_pascal_case(&name))
    };
    let tag = if is_pascal_case(&name) { name.clone() } else { to_pascal_case(&name) };

    let (message, help) = if sig.inputs.is_empty() {
        if calls < 2 {
            return;
        }
        (
            format!("`{name}` builds an `Element` and is called from {calls} sites — it's a component without `#[component]`"),
            format!("annotate with `#[component]`{rename}, then call it as `{tag}()` inside `ui!`"),
        )
    } else if takes_props_struct(sig) {
        (
            format!("`{name}` takes a props struct and returns `Element` — a hand-rolled component missing `#[component]`"),
            format!("annotate with `#[component]`{rename}; the macro supplies the `BuildElement` glue `ui!` dispatch needs"),
        )
    } else {
        // Name the first real param in the suggested call site.
        let arg = sig
            .inputs
            .iter()
            .find_map(|a| match a {
                syn::FnArg::Typed(pt) => match pt.pat.as_ref() {
                    syn::Pat::Ident(pi) => Some(pi.ident.to_string()),
                    _ => None,
                },
                syn::FnArg::Receiver(_) => None,
            })
            .unwrap_or_else(|| "arg".to_string());
        (
            format!("`{name}` returns `Element` from positional arguments — likely a component outside the `#[component]` paradigm"),
            format!(
                "annotate with `#[component]`{rename}: the params become inline props, and call sites move into `ui!` as `{tag}({arg} = …)`"
            ),
        )
    };
    out.push(RawDiag::new(RULE, message, sig.ident.span()).with_help(help));
}

/// `-> Element` / `-> runtime_core::Element` — the bare type, not
/// `Option<Element>` / `Vec<Element>` (those are fragments, not a node).
/// A qualified path must go through the framework; `web_sys::Element`
/// is someone else's type.
fn returns_element(output: &syn::ReturnType) -> bool {
    let syn::ReturnType::Type(_, ty) = output else { return false };
    let syn::Type::Path(tp) = ty.as_ref() else { return false };
    if tp.qself.is_some() {
        return false;
    }
    let path = &tp.path;
    let Some(last) = path.segments.last() else { return false };
    if last.ident != "Element" || !matches!(last.arguments, syn::PathArguments::None) {
        return false;
    }
    path.segments.len() == 1
        || path.segments.iter().any(|s| ELEMENT_ROOTS.contains(&s.ident.to_string().as_str()))
}

/// The pre-`#[component]` component shape: one param typed `FooProps` /
/// `&FooProps`.
fn takes_props_struct(sig: &syn::Signature) -> bool {
    if sig.inputs.len() != 1 {
        return false;
    }
    let Some(syn::FnArg::Typed(pt)) = sig.inputs.first() else { return false };
    let mut ty = pt.ty.as_ref();
    while let syn::Type::Reference(r) = ty {
        ty = r.elem.as_ref();
    }
    matches!(ty, syn::Type::Path(tp) if last_segment(&tp.path).is_some_and(|s| s.ends_with("Props")))
}

/// Whether the body composes a tree anywhere (closures and nested
/// blocks included): a `ui!` / `jsx!` invocation, or a hand-built
/// primitive constructor call — the latter is a component outside the
/// paradigm twice over, so it must not escape this rule.
fn composes_tree(block: &syn::Block) -> bool {
    struct Find(bool);
    impl<'ast> Visit<'ast> for Find {
        fn visit_macro(&mut self, m: &'ast syn::Macro) {
            self.0 |= m.path.is_ident("ui") || m.path.is_ident("jsx");
        }
        fn visit_expr_call(&mut self, c: &'ast syn::ExprCall) {
            if let syn::Expr::Path(p) = c.func.as_ref() {
                self.0 |= last_segment(&p.path).is_some_and(|s| constructor(&s).is_some());
            }
            syn::visit::visit_expr_call(self, c);
        }
    }
    let mut f = Find(false);
    f.visit_block(block);
    f.0
}

/// `#[test]`, `#[tokio::test]`, `#[wasm_bindgen_test]`, …
fn is_test_attr(attr: &syn::Attribute) -> bool {
    last_segment(attr.path()).is_some_and(|s| s == "test" || s == "wasm_bindgen_test")
}

fn is_cfg_test(attr: &syn::Attribute) -> bool {
    attr.path().is_ident("cfg")
        && attr
            .parse_args::<syn::Meta>()
            .map(|m| m.path().is_ident("test"))
            .unwrap_or(false)
}

/// Whole-file facts the per-fn check consults.
#[derive(Default)]
struct FileFacts {
    /// Bare `name(…)` call sites per ident — in the AST and inside macro
    /// token streams (`ui! { … { helper() } }` is where most calls live).
    calls: HashMap<String, usize>,
    /// Idents used as a value (not called): `start_in(.., app)`,
    /// `.map(row)`, `render = row` inside a macro.
    value_uses: HashSet<String>,
    /// `use <elsewhere>::Element` — `Element` in this file isn't ours.
    foreign_element_import: bool,
    /// A `#[test]` fn outside any `#[cfg(test)]` module: the file itself
    /// is test code (`tests/*.rs`), so its Element fns are fixtures.
    integration_test_file: bool,
    /// Scan-time nesting depth of `#[cfg(test)]` modules.
    test_depth: usize,
}

impl FileFacts {
    fn scan(file: &syn::File) -> Self {
        let mut facts = FileFacts::default();
        facts.visit_file(file);
        facts
    }

    fn record_use_tree(&mut self, tree: &syn::UseTree, under_framework: bool) {
        match tree {
            syn::UseTree::Path(p) => {
                let under = under_framework || ELEMENT_ROOTS.contains(&p.ident.to_string().as_str());
                self.record_use_tree(&p.tree, under);
            }
            syn::UseTree::Group(g) => {
                for item in &g.items {
                    self.record_use_tree(item, under_framework);
                }
            }
            syn::UseTree::Name(n) if n.ident == "Element" => self.foreign_element_import |= !under_framework,
            syn::UseTree::Rename(r) if r.rename == "Element" => self.foreign_element_import |= !under_framework,
            _ => {}
        }
    }

    /// Token-level scan of a macro body: `ident(` is a call, a bare
    /// `ident` is a value use. Idents after `.` / `::` (methods, paths)
    /// and before `=` / `:` (prop and field names) are neither. Coarse
    /// on purpose — an over-counted value use only suppresses a warning.
    fn scan_tokens(&mut self, tokens: TokenStream) {
        let trees: Vec<TokenTree> = tokens.into_iter().collect();
        for (i, tt) in trees.iter().enumerate() {
            match tt {
                TokenTree::Group(g) => self.scan_tokens(g.stream()),
                TokenTree::Ident(id) => {
                    let punct = |j: usize, c: char| {
                        j < i && matches!(&trees[j], TokenTree::Punct(p) if p.as_char() == c)
                    };
                    // `x.ident` / `a::ident` — a method, field, or path tail.
                    let after_path = punct(i.wrapping_sub(1), '.')
                        || (punct(i.wrapping_sub(1), ':') && punct(i.wrapping_sub(2), ':'));
                    if after_path {
                        continue;
                    }
                    match trees.get(i + 1) {
                        Some(TokenTree::Group(g)) if g.delimiter() == proc_macro2::Delimiter::Parenthesis => {
                            *self.calls.entry(id.to_string()).or_default() += 1;
                        }
                        Some(TokenTree::Punct(p)) if matches!(p.as_char(), '=' | ':' | '!') => {}
                        _ => {
                            self.value_uses.insert(id.to_string());
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

impl<'ast> Visit<'ast> for FileFacts {
    fn visit_item_mod(&mut self, node: &'ast syn::ItemMod) {
        let is_test = node.attrs.iter().any(is_cfg_test);
        self.test_depth += is_test as usize;
        syn::visit::visit_item_mod(self, node);
        self.test_depth -= is_test as usize;
    }

    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        self.integration_test_file |= self.test_depth == 0 && node.attrs.iter().any(is_test_attr);
        syn::visit::visit_item_fn(self, node);
    }

    fn visit_item_use(&mut self, node: &'ast syn::ItemUse) {
        self.record_use_tree(&node.tree, false);
    }

    fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
        if let syn::Expr::Path(p) = node.func.as_ref() {
            if let Some(id) = p.path.get_ident() {
                *self.calls.entry(id.to_string()).or_default() += 1;
                // Visit only the args — the callee path isn't a value use.
                for arg in &node.args {
                    self.visit_expr(arg);
                }
                return;
            }
        }
        syn::visit::visit_expr_call(self, node);
    }

    fn visit_expr_path(&mut self, node: &'ast syn::ExprPath) {
        if let Some(id) = node.path.get_ident() {
            self.value_uses.insert(id.to_string());
        }
        syn::visit::visit_expr_path(self, node);
    }

    fn visit_macro(&mut self, node: &'ast syn::Macro) {
        self.scan_tokens(node.tokens.clone());
        syn::visit::visit_macro(self, node);
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::{lint_source, Config, Diagnostic};

    const PRELUDE: &str = "use runtime_core::{Element, view};\n";

    fn lint(body: &str) -> Vec<Diagnostic> {
        let src = format!("{PRELUDE}{body}");
        lint_source(&src, Path::new("t.rs"), &Config::default())
            .unwrap()
            .into_iter()
            .filter(|d| d.rule == super::RULE)
            .collect()
    }

    #[test]
    fn flags_positional_element_fn() {
        let d = lint("fn user_row(name: String, active: bool) -> Element { ui! { view() } }");
        assert_eq!(d.len(), 1, "{d:?}");
        let help = d[0].help.as_deref().unwrap();
        assert!(help.contains("#[component]") && help.contains("UserRow(name = …)"), "{help}");
    }

    #[test]
    fn flags_pascal_case_fn_missing_attr() {
        let d = lint("fn UserRow(name: String) -> Element { ui! { view() } }");
        assert_eq!(d.len(), 1, "{d:?}");
        assert!(!d[0].help.as_deref().unwrap().contains("rename"), "{d:?}");
    }

    #[test]
    fn flags_hand_rolled_props_component() {
        let d = lint("pub fn Card(props: &CardProps) -> Element { jsx! { <view /> } }");
        assert_eq!(d.len(), 1, "{d:?}");
        assert!(d[0].message.contains("hand-rolled"), "{}", d[0].message);
    }

    #[test]
    fn flags_qualified_framework_return() {
        let d = lint("fn row(n: i32) -> runtime_core::Element { ui! { view() } }");
        assert_eq!(d.len(), 1, "{d:?}");
    }

    #[test]
    fn element_plumbing_without_ui_macro_is_clean() {
        // Post-processing an already-built node isn't composing a tree.
        let d = lint("fn finalize_switch(el: Element, props: &ButtonProps) -> Element { el }");
        assert!(d.is_empty(), "{d:?}");
    }

    #[test]
    fn ui_macro_in_nested_closure_counts() {
        let d = lint("fn row(n: i32) -> Element { dynamic(move || ui! { view() }) }");
        assert_eq!(d.len(), 1, "{d:?}");
    }

    #[test]
    fn hand_built_primitive_body_is_flagged() {
        let d = lint("fn header(title: String) -> Element { view(vec![text(title).into_element()]).into_element() }");
        assert_eq!(d.len(), 1, "{d:?}");
    }

    #[test]
    fn component_attr_is_clean() {
        assert!(lint("#[component]\nfn UserRow(name: String) -> Element { ui! { view() } }").is_empty());
    }

    #[test]
    fn zero_arg_one_off_helper_is_clean() {
        // CLAUDE.md §9.5: a no-prop helper called from one place is fine.
        let d = lint("fn phones() -> Element { ui! { view() } }\nfn page() -> Element { ui! { view() { phones() } } }");
        assert!(d.is_empty(), "{d:?}");
    }

    #[test]
    fn zero_arg_reused_helper_is_flagged() {
        let d = lint(
            "fn divider() -> Element { ui! { view() } }\n\
             fn page() -> Element { ui! { view() { divider() text { \"a\" } divider() } } }",
        );
        assert_eq!(d.len(), 1, "{d:?}");
        assert!(d[0].message.contains("2 sites"), "{}", d[0].message);
    }

    #[test]
    fn fn_used_as_value_is_clean() {
        // Entry fns and screens are consumed as `fn() -> Element` values.
        let d = lint(
            "pub fn app() -> Element { ui! { view() } }\n\
             fn row(item: &Item) -> Element { ui! { view() } }\n\
             pub fn start() { start_in(\"#app\", register, app); }\n\
             fn list() -> Element { ui! { flat_list(items = items, render = row) } }",
        );
        assert!(d.is_empty(), "{d:?}");
    }

    #[test]
    fn methods_and_tests_are_clean() {
        let d = lint(
            "impl From<Bound> for Element { fn from(b: Bound) -> Element { ui! { view() } } }\n\
             #[test]\nfn fixture_a(n: i32) -> Element { ui! { view() } }\n\
             #[cfg(test)]\nmod tests { use super::*; fn fixture_b(n: i32) -> Element { ui! { view() } } }",
        );
        assert!(d.is_empty(), "{d:?}");
    }

    #[test]
    fn integration_test_file_is_clean() {
        let d = lint("fn fixture(n: i32) -> Element { ui! { view() } }\n#[test]\nfn t() {}");
        assert!(d.is_empty(), "{d:?}");
    }

    #[test]
    fn unit_test_module_does_not_exempt_the_file() {
        let d = lint(
            "fn row(n: i32) -> Element { ui! { view() } }\n\
             #[cfg(test)]\nmod tests { #[test]\nfn t() {} }",
        );
        assert_eq!(d.len(), 1, "{d:?}");
    }

    #[test]
    fn fragments_are_not_elements() {
        assert!(lint("fn rows(n: i32) -> Vec<Element> { ui! { view() } }").is_empty());
        assert!(lint("fn maybe(n: i32) -> Option<Element> { ui! { view() } }").is_empty());
    }

    #[test]
    fn foreign_element_type_is_clean() {
        // `web_sys::Element` in a file that also renders with `ui!`.
        let src = "use web_sys::Element;\nfn node(doc: &Document) -> Element { ui! { view() } }\nfn f() { ui! { view() }; }";
        let d = lint_source(src, Path::new("t.rs"), &Config::default()).unwrap();
        assert!(d.iter().all(|d| d.rule != super::RULE), "{d:?}");
        let d = lint_source("fn node(doc: &Document) -> web_sys::Element { ui! { view() } }\nfn f() { ui! { view() }; }", Path::new("t.rs"), &Config::default()).unwrap();
        assert!(d.iter().all(|d| d.rule != super::RULE), "{d:?}");
    }

    #[test]
    fn crate_prelude_element_is_flagged() {
        let src = "use crate::prelude::*;\nfn badge(n: i32) -> Element { ui! { text { \"{n}\" } } }";
        let d = lint_source(src, Path::new("t.rs"), &Config::default()).unwrap();
        assert_eq!(d.iter().filter(|d| d.rule == super::RULE).count(), 1, "{d:?}");
    }
}
