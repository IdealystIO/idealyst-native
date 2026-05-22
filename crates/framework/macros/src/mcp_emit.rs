//! MCP catalog registration emission — phase 1 (with `composes` edges).
//!
//! When `#[component]` runs with the `mcp` feature on, it calls
//! [`emit`] with the parsed function and gets back an `inventory::submit!`
//! invocation that registers a `framework_mcp::ComponentEntry` at module
//! scope. The submission is emitted as a sibling of the component function
//! so the linker-section magic in `inventory` works as expected.
//!
//! Fields recorded: name, module_path, file, line, docs, composes. The
//! `composes` list is built by walking the function body for `ui!` and
//! `jsx!` macro invocations and pulling every component-position ident.
//! Per the spec (§6.3) we deliberately do NOT capture arbitrary
//! expression-position calls — a "component" is something that appears
//! as a child in JSX position, not any function call inside the body.

use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::spanned::Spanned;
use syn::visit::Visit;
use syn::ItemFn;

/// Build the `inventory::submit!` token stream for a `#[component]`
/// function. Failures inside the composes walk degrade gracefully —
/// an unparseable `ui!` body just produces an empty edge list rather
/// than failing the user's build. The user's real `ui!` expansion
/// surfaces any genuine syntax error.
///
/// `methods` carries the parsed `methods!` block (or empty when the
/// component declares none) — each method becomes one
/// `MethodEntry` submission tagged with this component as parent.
/// Animations declared via `animated!(...)` inside the body are
/// captured by [`collect_animations`] and submitted as
/// `AnimationEntry` records.
pub(crate) fn emit(
    item_fn: &ItemFn,
    methods: &[crate::methods_block::MethodInfo],
) -> TokenStream2 {
    let name_str = item_fn.sig.ident.to_string();
    let docs = collect_doc_comments(&item_fn.attrs);
    let composes = collect_composes(&item_fn.block);
    let params = collect_params(&item_fn.sig);
    let animations = collect_animations(&item_fn.block);

    let edges = composes.iter().map(|(name, line)| {
        quote! {
            ::framework_core::__mcp::EdgeRef {
                name: #name,
                line: #line,
            }
        }
    });

    let param_entries = params.iter().map(|(name, ty, short)| {
        quote! {
            ::framework_core::__mcp::ParamSpec {
                name: #name,
                type_str: #ty,
                type_short_name: #short,
            }
        }
    });

    let method_submissions = methods.iter().map(|m| {
        let mname = &m.name;
        let mdocs = &m.docs;
        let param_specs = m.params.iter().map(|(pname, ptype)| {
            // Derive `type_short_name` cheaply — last path segment of
            // the pretty-printed type. We don't have an `&syn::Type`
            // here (the body was rewritten before we ran), but the
            // string we have is `quote!`-stringified so it follows the
            // same conventions as `collect_params`. Pulling the short
            // name from the string is fine for catalog purposes; ties
            // through to `TypeEntry::short_name` for enums/structs.
            let short = pretty_type_short(ptype);
            quote! {
                ::framework_core::__mcp::ParamSpec {
                    name: #pname,
                    type_str: #ptype,
                    type_short_name: #short,
                }
            }
        });
        quote! {
            ::framework_core::__mcp::inventory::submit! {
                ::framework_core::__mcp::MethodEntry {
                    parent_module_path: module_path!(),
                    parent_name: #name_str,
                    name: #mname,
                    docs: #mdocs,
                    params: &[ #(#param_specs),* ],
                    return_type: "",
                }
            }
        }
    });

    let animation_submissions = animations.iter().map(|a| {
        let binding = &a.binding;
        let initial = &a.initial;
        let line = a.line;
        quote! {
            ::framework_core::__mcp::inventory::submit! {
                ::framework_core::__mcp::AnimationEntry {
                    parent_module_path: module_path!(),
                    parent_name: #name_str,
                    binding: #binding,
                    initial: #initial,
                    line: #line,
                }
            }
        }
    });

    quote! {
        ::framework_core::__mcp::inventory::submit! {
            ::framework_core::__mcp::ComponentEntry {
                name: #name_str,
                module_path: module_path!(),
                file: file!(),
                line: line!(),
                docs: #docs,
                composes: &[ #(#edges),* ],
                params: &[ #(#param_entries),* ],
            }
        }
        #(#method_submissions)*
        #(#animation_submissions)*
    }
}

/// Pull the last segment of a pretty-printed type string. `& 'a Foo<T>`
/// → `"Foo"`; `Vec<u8>` → `"Vec"`. Empty when the string has no
/// alphanumeric ident token (tuples, function types, …).
fn pretty_type_short(s: &str) -> String {
    // Trim refs/lifetimes/parens up to the first ident-starting char,
    // then walk until the next non-ident character. Cheap and good
    // enough for catalog purposes — full parsing isn't worth the
    // extra `syn` round-trip.
    let mut chars = s.chars().peekable();
    // Skip leading ref/lifetime/whitespace.
    while let Some(&c) = chars.peek() {
        if c.is_alphabetic() || c == '_' { break; }
        chars.next();
    }
    let mut ident = String::new();
    let mut last_ident = String::new();
    for c in chars {
        if c.is_alphanumeric() || c == '_' {
            ident.push(c);
        } else {
            if !ident.is_empty() {
                last_ident = std::mem::take(&mut ident);
            }
            // After `::` we want the next segment, not the previous
            // one. Reset on `::` boundary.
            if c == ':' || c == '<' {
                last_ident.clear();
            }
        }
    }
    if !ident.is_empty() {
        last_ident = ident;
    }
    last_ident
}

/// Pull each parameter off the fn signature and produce a
/// `(name, type_str)` pair. The name comes from the parameter
/// pattern's binding ident (the common `name: Type` shape). Complex
/// patterns (tuple destructuring, `mut name`, ref patterns) reduce
/// to `"_"` — the catalog records that the slot exists but can't
/// name it. The type is `quote!`-stringified for catalog
/// consumption; spacing follows `quote!`'s defaults (e.g.
/// `"& PlanetProps"` with a space between `&` and the type).
fn collect_params(sig: &syn::Signature) -> Vec<(String, String, String)> {
    let mut out = Vec::with_capacity(sig.inputs.len());
    for arg in &sig.inputs {
        let syn::FnArg::Typed(pat_type) = arg else {
            // `self` / `&self` / `&mut self` — not a value-position
            // parameter. Components are free functions in this
            // framework so this branch shouldn't fire, but skip
            // rather than panic if it does.
            continue;
        };
        let name = match pat_type.pat.as_ref() {
            syn::Pat::Ident(pi) => pi.ident.to_string(),
            _ => "_".to_string(),
        };
        let ty = &*pat_type.ty;
        let type_str = quote! { #ty }.to_string();
        let short = type_short_name(ty).unwrap_or_default();
        out.push((name, type_str, short));
    }
    out
}

/// Unwrap `&T` / `&'a T` / `&mut T` / `Box<T>` / `Option<T>` /
/// `Vec<T>` to find the underlying type's bare ident. Returns `None`
/// for non-path types (tuples, function types, …) or when the
/// outermost path has no segments. For a generic `Foo<T>` this
/// returns `"Foo"`. Used to join `ParamSpec` to `PropsSchemaEntry`
/// at MCP-runtime time.
fn type_short_name(ty: &syn::Type) -> Option<String> {
    match ty {
        syn::Type::Reference(r) => type_short_name(&r.elem),
        syn::Type::Paren(p) => type_short_name(&p.elem),
        syn::Type::Group(g) => type_short_name(&g.elem),
        syn::Type::Path(p) => p.path.segments.last().map(|s| s.ident.to_string()),
        _ => None,
    }
}

/// Pull every `#[doc = "..."]` attribute off the fn and concatenate
/// them with `\n` separators. Returns an empty string literal when
/// the fn has no doc comments. Output is a string literal token, ready
/// to drop into the `inventory::submit!` body.
fn collect_doc_comments(attrs: &[syn::Attribute]) -> proc_macro2::TokenStream {
    let mut lines: Vec<String> = Vec::new();
    for attr in attrs {
        if !attr.path().is_ident("doc") {
            continue;
        }
        // Doc attributes look like `#[doc = "literal"]`. We pull the
        // literal out via meta-name-value parsing; anything that doesn't
        // match that shape is silently skipped.
        if let syn::Meta::NameValue(nv) = &attr.meta {
            if let syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Str(s), .. }) = &nv.value {
                // Rustc inserts a leading space on most doc-comment
                // strings (`/// hello` becomes `" hello"`). Trim one
                // leading space to keep the dumped catalog readable.
                let raw = s.value();
                let stripped = raw.strip_prefix(' ').unwrap_or(&raw).to_string();
                lines.push(stripped);
            }
        }
    }
    let joined = lines.join("\n");
    quote! { #joined }
}

/// Walk the component function's body for `ui!` / `jsx!` macro
/// invocations, parse each one, and accumulate every
/// component-position ident with its source line. The visitor
/// recurses into the rest of the block, so multiple `ui!` calls,
/// `ui!` inside `let`-RHS, inside closures, and `Stmt::Macro` at
/// statement position are all caught.
fn collect_composes(block: &syn::Block) -> Vec<(String, u32)> {
    let mut v = ComposeCollector { edges: Vec::new() };
    v.visit_block(block);
    v.edges
}

struct ComposeCollector {
    edges: Vec<(String, u32)>,
}

impl<'ast> Visit<'ast> for ComposeCollector {
    fn visit_expr_macro(&mut self, node: &'ast syn::ExprMacro) {
        capture_if_ui_or_jsx(&node.mac, &mut self.edges);
        syn::visit::visit_expr_macro(self, node);
    }

    fn visit_stmt_macro(&mut self, node: &'ast syn::StmtMacro) {
        capture_if_ui_or_jsx(&node.mac, &mut self.edges);
        syn::visit::visit_stmt_macro(self, node);
    }
}

/// One [`AnimationEntry`]-ready capture from the component body — a
/// `let <name> = animated!(<initial>);` (or just an inline
/// `animated!(...)` expression). Best-effort: anything that doesn't
/// parse cleanly is silently skipped.
pub(crate) struct AnimationCapture {
    pub binding: String,
    pub initial: String,
    pub line: u32,
}

/// Walk the component body for `animated!(...)` macro invocations.
/// Recognises two shapes:
///
/// 1. `let <name> = animated!(<initial>);` — captures `name` and the
///    initial expression.
/// 2. Bare `animated!(<initial>)` inside any expression — captured
///    with empty `binding`.
///
/// In both cases we record the source line so editor jumps work.
pub(crate) fn collect_animations(block: &syn::Block) -> Vec<AnimationCapture> {
    let mut v = AnimationCollector { items: Vec::new() };
    v.visit_block(block);
    v.items
}

struct AnimationCollector {
    items: Vec<AnimationCapture>,
}

impl<'ast> Visit<'ast> for AnimationCollector {
    fn visit_local(&mut self, node: &'ast syn::Local) {
        // `let <pat> = <init>;` — interested only when the init is a
        // macro invocation of `animated!`.
        if let Some(local_init) = &node.init {
            if let syn::Expr::Macro(em) = local_init.expr.as_ref() {
                if is_animated_macro(&em.mac) {
                    let binding = match &node.pat {
                        syn::Pat::Ident(pi) => pi.ident.to_string(),
                        syn::Pat::Type(pt) => match pt.pat.as_ref() {
                            syn::Pat::Ident(pi) => pi.ident.to_string(),
                            _ => String::new(),
                        },
                        _ => String::new(),
                    };
                    let initial = em.mac.tokens.to_string();
                    let line = em.mac.path.span().start().line as u32;
                    self.items.push(AnimationCapture { binding, initial, line });
                }
            }
        }
        syn::visit::visit_local(self, node);
    }

    fn visit_expr_macro(&mut self, node: &'ast syn::ExprMacro) {
        // Inline `animated!(...)` outside a `let` — record with empty
        // binding so the catalog still reflects the animation.
        if is_animated_macro(&node.mac) {
            // Avoid double-counting: if the parent walker already
            // captured this via `visit_local`, we'll have recorded it
            // there. Detecting that is tricky in a visitor; instead,
            // accept the rare double-count for now — consumers can
            // dedupe on `(binding, line)`. In practice components use
            // the `let` form.
            let initial = node.mac.tokens.to_string();
            let line = node.mac.path.span().start().line as u32;
            // Skip if the binding is already empty AND we'd
            // immediately follow a let-captured one on the same line.
            let dup = self
                .items
                .last()
                .map(|prev| prev.line == line && !prev.binding.is_empty())
                .unwrap_or(false);
            if !dup {
                self.items.push(AnimationCapture {
                    binding: String::new(),
                    initial,
                    line,
                });
            }
        }
        syn::visit::visit_expr_macro(self, node);
    }
}

fn is_animated_macro(mac: &syn::Macro) -> bool {
    mac.path
        .segments
        .last()
        .map(|s| s.ident == "animated")
        .unwrap_or(false)
}

/// If `mac` is a call to `ui!` or `jsx!`, parse its body via the
/// sibling parsers and append the discovered component references.
/// Match by last path segment so qualified calls (`crate::ui!`,
/// `framework_macros::ui!`) are still captured — proc-macros run
/// pre-name-resolution but the author's intent is the same.
///
/// Parsing failures are silently swallowed: the user's real `ui!`
/// expansion will surface any real syntax error; the MCP walk is
/// best-effort and must never block the build.
fn capture_if_ui_or_jsx(mac: &syn::Macro, out: &mut Vec<(String, u32)>) {
    let Some(last) = mac.path.segments.last() else {
        return;
    };
    if last.ident == "ui" {
        if let Ok(parsed) = syn::parse2::<crate::ui::Ui>(mac.tokens.clone()) {
            crate::ui::collect_component_refs(&parsed, out);
        }
    } else if last.ident == "jsx" {
        if let Ok(parsed) = syn::parse2::<crate::jsx::Jsx>(mac.tokens.clone()) {
            crate::jsx::collect_component_refs(&parsed, out);
        }
    }
}
