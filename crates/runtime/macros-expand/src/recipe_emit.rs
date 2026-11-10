//! `recipe!(Component, fn name() -> Element { … })` emission.
//!
//! A **recipe** is a compile-checked usage example for a component.
//! The macro:
//! 1. Emits the recipe fn **verbatim** — so it's compiled and
//!    type-checked against the component's *live* props. If a prop
//!    changes shape and the recipe isn't updated, this is a compile
//!    error. That's the whole point: self-verifying examples.
//! 2. Captures the fn's source as written (see `source_text`), its
//!    `///` docs, and the set of components its `ui!`/`jsx!` body
//!    references (the composes walk), and registers a
//!    `mcp_catalog::RecipeEntry` so the catalog / MCP / docs can surface
//!    it.
//!
//! This whole module is compiled only under the `catalog` feature — the
//! `recipe!` proc-macro emits NOTHING when `catalog` is off (see
//! `lib.rs`), so recipes (and their imports) vanish entirely from
//! production builds at zero cost. The author writes plain
//! `recipe!(...)` with no `#[cfg]` of their own.

use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::parse::{Parse, ParseStream};

struct RecipeInput {
    /// The entity the recipe primarily demonstrates (`recipe!`'s first
    /// arg) — a component, utility, free function, or type. A path so
    /// `recipe!(idea_ui::Select, …)` works too; only the last segment is
    /// recorded as the target name.
    target: syn::Path,
    /// The recipe function — real, compiled code.
    func: syn::ItemFn,
}

impl Parse for RecipeInput {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let target: syn::Path = input.parse()?;
        input.parse::<syn::Token![,]>()?;
        let func: syn::ItemFn = input.parse()?;
        Ok(RecipeInput { target, func })
    }
}

fn recipe_source(func: &syn::ItemFn) -> String {
    use quote::ToTokens;
    let mut body = TokenStream2::new();
    func.vis.to_tokens(&mut body);
    func.sig.to_tokens(&mut body);
    func.block.to_tokens(&mut body);
    let Some(code) = crate::source_text::verbatim(&body) else {
        let file = syn::File { shebang: None, attrs: Vec::new(), items: vec![syn::Item::Fn(func.clone())] };
        return prettyplease::unparse(&file);
    };
    let mut out = String::new();
    for attr in &func.attrs {
        if let syn::Meta::NameValue(nv) = &attr.meta {
            if let (true, syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Str(s), .. })) =
                (attr.path().is_ident("doc"), &nv.value)
            {
                out.push_str(&format!("///{}\n", s.value()));
            }
        }
    }
    out.push_str(&code);
    out.push('\n');
    out
}

pub(crate) fn emit(input: TokenStream2) -> TokenStream2 {
    let RecipeInput { target, func } = match syn::parse2::<RecipeInput>(input) {
        Ok(p) => p,
        Err(e) => return e.to_compile_error(),
    };

    let name_str = func.sig.ident.to_string();
    let target_str = target
        .segments
        .last()
        .map(|s| s.ident.to_string())
        .unwrap_or_default();
    // Prose description (LLM context + docs) — the `///` on the fn.
    let docs = crate::mcp_emit::collect_doc_comments(&func.attrs);

    // The recipe fn's source for display: its `///` docs, then the fn as
    // the author wrote it (`source_text::verbatim` — `prettyplease` can't
    // lay out a `ui!` body, which is most of a recipe). Generated tokens
    // have no source, so a recipe built by another macro is unparsed by
    // `prettyplease` instead. Either way it is the fn as written, without
    // the `#[allow(dead_code)]` added below for compilation.
    let source = recipe_source(&func);

    // Components the recipe's `ui!`/`jsx!` body references — reuse the
    // `#[component]` composes walker. Dedup + sort for a stable list.
    let mut uses: Vec<String> = crate::mcp_emit::collect_composes(&func.block)
        .into_iter()
        .map(|(name, _line)| name)
        .collect();
    uses.sort();
    uses.dedup();
    let use_lits = uses.iter().map(|u| quote! { #u });
    // `previews`: an exported render fn for the docs' live recipe preview.
    let (preview_export, preview_symbol) = crate::preview_emit::recipe_symbol(&func);
    let preview_registration =
        crate::preview_emit::registration("Recipe", &func.sig.ident.to_string(), &preview_symbol);

    quote! {
        // The recipe fn — compiled + type-checked against the live
        // component props. May be uncalled (docs-only), hence the allow.
        #[allow(dead_code)]
        #func
        #preview_export

        ::runtime_core::__mcp::inventory::submit! {
            ::runtime_core::__mcp::RecipeEntry {
                name: #name_str,
                target: #target_str,
                module_path: module_path!(),
                file: file!(),
                line: line!(),
                docs: #docs,
                source: #source,
                uses: &[ #(#use_lits),* ],
            }
        }
        #preview_registration
    }
}
