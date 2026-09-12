//! `prefer-ui-macro` — flag elements built by hand instead of through the
//! `ui!` / `jsx!` macro.
//!
//! Four hand-built shapes are caught:
//!
//! - `runtime_core::view(…)`, `glue::text(…)`, `builders::view()`, … — a
//!   call to one of the framework's primitive constructors qualified by a
//!   framework path segment. These are the free functions `ui!` lowers
//!   its tags to; calling them directly is *exactly* the hand-built form
//!   the rule exists for. (Historically only a `builder::` segment was
//!   recognised, so `runtime_core::view(vec![…])` — the spelling the
//!   framework's own docs use for the positional constructor — sailed
//!   through unflagged.)
//! - `view(…)`, `text(…)`, … written bare, when the file imports that
//!   name from the framework (`use runtime_core::view;` or a glob
//!   `use runtime_core::*;`) and nothing in the file shadows it — a local
//!   `fn text`, a `let text = …`, a closure/fn parameter named `text`.
//!   This is the only place the rule reasons from an unqualified ident,
//!   and it only does so with import evidence in hand.
//! - `BuildElement::build(props)` — the trait call the macro emits for
//!   component dispatch; writing it by hand is the manual form.
//! - `Element::View { … }`, `Element::Text { … }`, … — a struct literal of
//!   a primitive `Element` variant (the old core's shape; kept so legacy
//!   sources still get the pointer).
//!
//! The old core's `Element::External { … }` / `Element::Component { … }`
//! literals are deliberately **not** flagged: `Component` is the macro's
//! own wrapper and `External` was the pre-`Registry` extension seam —
//! both legitimate hand-written forms in the sources that still carry
//! them.
//!
//! Also deliberately NOT flagged: the reactive control-flow glue
//! (`when`, `switch`, `dynamic`, `each_keyed`). Those are how `ui!`
//! lowers `if` / `match` / `for`, but they're also the sanctioned
//! library-level escape for shapes the macro can't express (a `switch`
//! over a tuple scrutinee, a `dynamic` rebuilt from a `Ref`), and a
//! component library reaches for them on purpose. Flagging them would
//! trade precision for noise.
//!
//! Because `syn` never descends into `ui! { … }` token streams, every node
//! this rule sees is genuinely outside the macro.

use std::collections::{HashMap, HashSet};

use syn::visit::Visit;

use crate::diagnostic::RawDiag;
use crate::rules::{has_segment, last_segment, nth_from_end};

pub(crate) const RULE: &str = "prefer-ui-macro";

/// Element variants that are legitimate to construct by hand — the
/// extension escape hatches — and so are exempt from this rule.
const EXEMPT_VARIANTS: &[&str] = &["External", "Component"];

/// The free constructor functions `ui!` lowers its primitive tags to —
/// every snake_case tag the macro accepts (`canonical_primitive` in
/// `runtime-macros/src/primitives.rs`) plus the glue siblings that share
/// the same positional-constructor shape (`text_area`, `pressable`, the
/// `image_*` / `external_link` variants). All reachable as
/// `runtime_core::<name>` / `runtime_vocabulary::glue::<name>` (or, for
/// `slider` / `activity_indicator` / `graphics`, under
/// `runtime_core::primitives::<name>::<name>`).
const PRIMITIVE_CONSTRUCTORS: &[&str] = &[
    "view",
    "text",
    "button",
    "icon",
    "image",
    "image_asset",
    "image_from",
    "link",
    "external_link",
    "overlay",
    "anchored_overlay",
    "presence",
    "scroll_view",
    "text_area",
    "text_input",
    "flat_list",
    "toggle",
    "slider",
    "activity_indicator",
    "graphics",
    "pressable",
];

/// Path segments that mark a path as pointing INTO the framework. Any
/// one of these anywhere in a constructor call's path (or in a `use`
/// path that brings a constructor into scope) is the evidence the rule
/// needs: `runtime_core::view`, `idealyst::runtime_core::view`,
/// `runtime_vocabulary::glue::view`, `runtime_vocabulary::builders::view`,
/// `runtime_core::prelude::*`, `runtime_core::primitives::slider::slider`.
const FRAMEWORK_ROOTS: &[&str] = &[
    "runtime_core",
    "runtime_vocabulary",
    "idealyst",
    "glue",
    "builders",
    "builder",
    "prelude",
    "primitives",
];

/// The canonical constructor `name` spells, if it is one.
fn constructor(name: &str) -> Option<&'static str> {
    PRIMITIVE_CONSTRUCTORS.iter().copied().find(|c| *c == name)
}

fn has_framework_root(path: &syn::Path) -> bool {
    FRAMEWORK_ROOTS.iter().any(|root| has_segment(path, root))
}

/// What the file says about bare constructor idents: which of them were
/// imported from the framework, and which idents are shadowed by a local
/// definition. Built once per file by [`FileContext::scan`] and consulted
/// for every bare `view(…)` / `text(…)` call.
#[derive(Default)]
pub(crate) struct FileContext {
    /// Constructors imported by name, keyed by the LOCAL name → the
    /// framework constructor it is (`use runtime_core::view;` records
    /// `view → view`; `use runtime_core::{text as t, …}` records
    /// `t → text`).
    named_imports: HashMap<String, &'static str>,
    /// A glob import from a framework root (`use runtime_core::*;`,
    /// `use runtime_core::prelude::*;`) brings EVERY constructor in.
    framework_glob: bool,
    /// Idents bound locally anywhere in the file — `fn text`, `let view`,
    /// a parameter or closure argument. A bare call to one of these is
    /// the local, not the framework constructor (a local item wins over a
    /// glob import; a `let` shadows a named import), so it's never
    /// flagged. Coarse (file-wide, not scope-aware) on purpose: a false
    /// negative costs one missed warning, a false positive costs trust.
    shadowed: HashSet<String>,
}

impl FileContext {
    pub(crate) fn scan(file: &syn::File) -> Self {
        let mut cx = FileContext::default();
        cx.visit_file(file);
        cx
    }

    /// The framework constructor a bare call `name(…)` resolves to, as
    /// far as this file's imports say — `None` when it's the author's
    /// own `name`.
    fn bare_call_constructor(&self, name: &str) -> Option<&'static str> {
        if self.shadowed.contains(name) {
            return None;
        }
        if let Some(ctor) = self.named_imports.get(name) {
            return Some(ctor);
        }
        if self.framework_glob {
            return constructor(name);
        }
        None
    }

    /// Walk one `use` tree, carrying whether a framework root has been
    /// seen on the path so far. `use runtime_core::{view, text as t}` →
    /// `named_imports = {view, t}`; `use runtime_core::*` → glob.
    fn record_use_tree(&mut self, tree: &syn::UseTree, under_framework: bool) {
        match tree {
            syn::UseTree::Path(p) => {
                let under = under_framework || FRAMEWORK_ROOTS.contains(&p.ident.to_string().as_str());
                self.record_use_tree(&p.tree, under);
            }
            syn::UseTree::Group(g) => {
                for item in &g.items {
                    self.record_use_tree(item, under_framework);
                }
            }
            syn::UseTree::Name(n) if under_framework => {
                if let Some(ctor) = constructor(&n.ident.to_string()) {
                    self.named_imports.insert(ctor.to_string(), ctor);
                }
            }
            syn::UseTree::Rename(r) if under_framework => {
                if let Some(ctor) = constructor(&r.ident.to_string()) {
                    self.named_imports.insert(r.rename.to_string(), ctor);
                }
            }
            syn::UseTree::Glob(_) if under_framework => {
                self.framework_glob = true;
            }
            syn::UseTree::Name(_) | syn::UseTree::Rename(_) | syn::UseTree::Glob(_) => {}
        }
    }
}

impl<'ast> Visit<'ast> for FileContext {
    fn visit_item_use(&mut self, node: &'ast syn::ItemUse) {
        self.record_use_tree(&node.tree, false);
        syn::visit::visit_item_use(self, node);
    }

    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        self.shadowed.insert(node.sig.ident.to_string());
        syn::visit::visit_item_fn(self, node);
    }

    // `PatIdent` is every binding site at once: `let view = …`, a fn
    // parameter, a closure argument, a `match` arm binding.
    fn visit_pat_ident(&mut self, node: &'ast syn::PatIdent) {
        self.shadowed.insert(node.ident.to_string());
        syn::visit::visit_pat_ident(self, node);
    }
}

pub(crate) fn check_call(call: &syn::ExprCall, cx: &FileContext, out: &mut Vec<RawDiag>) {
    let syn::Expr::Path(path_expr) = &*call.func else {
        return;
    };
    let path = &path_expr.path;
    let Some(name) = last_segment(path) else {
        return;
    };

    // `builder::…` / `builders::…` — the raw builder layer, qualified.
    // Kept as its own arm (ahead of the constructor-name check) so a
    // builder-layer fn that ISN'T in the constructor list — `builders::
    // virtual_grid()`, say — still gets the pointer.
    if has_segment(path, "builder") || has_segment(path, "builders") {
        out.push(
            RawDiag::new(
                RULE,
                format!("building an element through `{}` bypasses the `ui!` macro", render(path)),
                span_of(path_expr),
            )
            .with_help("compose the tree inside `ui! { … }` (or `jsx! { … }`) instead"),
        );
        return;
    }

    // A primitive constructor: qualified into the framework, or bare with
    // import evidence (possibly under a renamed local name).
    let ctor = if path.segments.len() > 1 {
        constructor(&name).filter(|_| has_framework_root(path))
    } else {
        cx.bare_call_constructor(&name)
    };
    if let Some(ctor) = ctor {
        out.push(
            RawDiag::new(
                RULE,
                format!(
                    "calling the `{ctor}` constructor by hand (`{}(…)`) bypasses the `ui!` macro",
                    render(path)
                ),
                span_of(path_expr),
            )
            .with_help(format!(
                "write it as a tag inside `ui! {{ … }}` — `{ctor}(…) {{ … }}` — so the \
                 macro owns reconciliation and reactive-scope inference"
            )),
        );
        return;
    }

    if name == "build" && nth_from_end(path, 1).as_deref() == Some("BuildElement") {
        out.push(
            RawDiag::new(
                RULE,
                "calling `BuildElement::build` by hand bypasses the `ui!` macro",
                span_of(path_expr),
            )
            .with_help("render the component inside `ui! { … }` so reconciliation tracks it"),
        );
    }
}

pub(crate) fn check_struct(node: &syn::ExprStruct, out: &mut Vec<RawDiag>) {
    // Looking for `Element::Variant { … }`: the owner segment is `Element`
    // and the last segment is the variant name.
    if nth_from_end(&node.path, 1).as_deref() != Some("Element") {
        return;
    }
    let Some(variant) = last_segment(&node.path) else {
        return;
    };
    if EXEMPT_VARIANTS.contains(&variant.as_str()) {
        return;
    }
    out.push(
        RawDiag::new(
            RULE,
            format!("constructing `Element::{variant}` by hand bypasses the `ui!` macro"),
            span_of_struct(node),
        )
        .with_help("write this element inside `ui! { … }` (or `jsx! { … }`) instead"),
    );
}

/// `a::b::c` as the author wrote it, for the message.
fn render(path: &syn::Path) -> String {
    path.segments.iter().map(|s| s.ident.to_string()).collect::<Vec<_>>().join("::")
}

fn span_of(path_expr: &syn::ExprPath) -> proc_macro2::Span {
    use syn::spanned::Spanned;
    path_expr.path.span()
}

fn span_of_struct(node: &syn::ExprStruct) -> proc_macro2::Span {
    use syn::spanned::Spanned;
    node.path.span()
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;

    /// Route through the shared visitor so the `FileContext` pre-scan in
    /// `mod.rs` is exercised — the bare-ident arm depends on it.
    fn diags(file_tokens: proc_macro2::TokenStream) -> Vec<RawDiag> {
        let file: syn::File = syn::parse2(file_tokens).unwrap();
        crate::rules::collect(&file).into_iter().filter(|d| d.rule == RULE).collect()
    }

    /// The reported bug: `runtime_core::view(vec![…])` — the positional
    /// constructor spelled the way the framework's own docs spell it —
    /// went unflagged, and a consumer project hand-built its whole tree
    /// on the strength of the linter's silence.
    #[test]
    fn regression_qualified_runtime_core_constructor_is_flagged() {
        let out = diags(quote! {
            fn card() -> Element {
                let label = runtime_core::text("hi").into_element();
                runtime_core::view(vec![label]).into_element()
            }
        });
        assert_eq!(out.len(), 2, "{out:?}");
        assert!(out[0].message.contains("runtime_core::text"), "{out:?}");
        assert!(out[1].message.contains("runtime_core::view"), "{out:?}");
        assert!(out[1].help.as_deref().unwrap_or("").contains("view(…) { … }"), "{out:?}");
    }

    #[test]
    fn every_framework_root_spelling_is_flagged() {
        for src in [
            quote! { fn f() { idealyst::runtime_core::view(vec![]); } },
            quote! { fn f() { runtime_vocabulary::glue::text("x"); } },
            quote! { fn f() { glue::scroll_view(vec![]); } },
            quote! { fn f() { runtime_core::primitives::slider::slider(v, |_| {}); } },
            quote! { fn f() { runtime_core::image_from("a.png"); } },
            quote! { fn f() { runtime_core::pressable(vec![], || {}); } },
        ] {
            let out = diags(src.clone());
            assert_eq!(out.len(), 1, "{src} → {out:?}");
        }
    }

    /// The raw builder layer, singular or plural module name.
    #[test]
    fn builder_and_builders_segments_are_flagged() {
        let out = diags(quote! {
            fn f() {
                builder::view(vec![]);
                runtime_vocabulary::builders::view().child(x).build();
                builders::virtual_grid();
            }
        });
        assert_eq!(out.len(), 3, "{out:?}");
    }

    /// Bare `view(…)` with an explicit framework import is the same call
    /// as the qualified form — flag it.
    #[test]
    fn bare_call_with_named_import_is_flagged() {
        let out = diags(quote! {
            use runtime_core::{text, view};
            fn f() -> Element {
                let label = text("hi").into_element();
                view(vec![label]).into_element()
            }
        });
        assert_eq!(out.len(), 2, "{out:?}");
        assert!(out[1].message.contains("`view(…)`"), "{out:?}");
    }

    #[test]
    fn bare_call_with_framework_glob_import_is_flagged() {
        for glob in [
            quote! { use runtime_core::*; },
            quote! { use runtime_core::prelude::*; },
            quote! { use runtime_vocabulary::glue::*; },
        ] {
            let out = diags(quote! {
                #glob
                fn f() -> Element { view(vec![]).into_element() }
            });
            assert_eq!(out.len(), 1, "{glob} → {out:?}");
        }
    }

    /// A `use` inside a fn body counts too — that's where a quick
    /// hand-built helper tends to import from.
    #[test]
    fn fn_local_import_is_seen() {
        let out = diags(quote! {
            fn f() -> Element {
                use runtime_core::view;
                view(vec![]).into_element()
            }
        });
        assert_eq!(out.len(), 1, "{out:?}");
    }

    /// `use runtime_core::text as t;` — the call is spelled `t(…)`.
    #[test]
    fn renamed_import_is_tracked_under_its_local_name() {
        let out = diags(quote! {
            use runtime_core::text as t;
            fn f() -> Element { t("hi").into_element() }
        });
        assert_eq!(out.len(), 1, "{out:?}");
        assert!(out[0].message.contains("`text` constructor"), "{out:?}");
    }

    /// No import evidence → a bare `text(…)` is whatever the author's
    /// own `text` is. Never guess from the ident alone.
    #[test]
    fn bare_call_without_import_is_clean() {
        let out = diags(quote! {
            fn f() { let s = text("hello"); let v = view(3); }
        });
        assert!(out.is_empty(), "{out:?}");
    }

    /// A glob import from somewhere else is not framework evidence.
    #[test]
    fn non_framework_glob_is_clean() {
        let out = diags(quote! {
            use my_helpers::*;
            fn f() { text("hello"); }
        });
        assert!(out.is_empty(), "{out:?}");
    }

    /// A local definition wins over a glob import (Rust's own rule) and a
    /// `let` / parameter shadows a named import — none of those calls
    /// reach the framework constructor.
    #[test]
    fn locally_shadowed_names_are_clean() {
        let out = diags(quote! {
            use runtime_core::*;
            fn text(s: &str) -> String { s.to_string() }
            fn f(view: impl Fn(u32) -> u32) {
                text("local fn");
                view(1);
                let icon = |d| d;
                icon(2);
                let button = make();
                button(3);
            }
        });
        assert!(out.is_empty(), "{out:?}");
    }

    /// The control-flow glue is the sanctioned library-level escape and
    /// is NOT a hand-built element — see the module docs.
    #[test]
    fn control_flow_glue_is_clean() {
        let out = diags(quote! {
            use runtime_core::*;
            fn f() {
                runtime_core::when(move || c.get(), move || a(), move || b());
                runtime_core::switch(move || k.get(), vec![]);
                dynamic(move || build());
            }
        });
        assert!(out.is_empty(), "{out:?}");
    }

    /// Method calls are never constructor calls — `.text(…)` on a
    /// builder, `.view()` on a buffer — and a qualified path whose LAST
    /// segment isn't a constructor is not one either.
    #[test]
    fn method_calls_and_non_constructor_paths_are_clean() {
        let out = diags(quote! {
            use runtime_core::*;
            fn f() {
                b.text("x").view();
                runtime_core::signal(0);
                runtime_core::memo(move || 1);
                runtime_core::fixed_size(20.0);
            }
        });
        assert!(out.is_empty(), "{out:?}");
    }

    /// Inside `ui! { … }` the tags ARE the constructors — invisible to
    /// syn, never flagged, whatever the file imports.
    #[test]
    fn tags_inside_ui_macro_are_clean() {
        let out = diags(quote! {
            use runtime_core::*;
            fn f() -> Element {
                ui! { view() { text { "hi" } scroll_view() { icon(data = D) } } }
            }
        });
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn build_element_build_and_element_literals_still_flagged() {
        let out = diags(quote! {
            fn f() {
                BuildElement::build(props);
                let e = Element::View { children: vec![] };
                let x = Element::External { name: "x" };
            }
        });
        assert_eq!(out.len(), 2, "{out:?}");
    }
}
