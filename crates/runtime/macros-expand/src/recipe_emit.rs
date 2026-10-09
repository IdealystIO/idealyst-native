//! `recipe!(Component, fn name() -> Element { … })` emission.
//!
//! A **recipe** is a compile-checked usage example for a component.
//! The macro:
//! 1. Emits the recipe fn **verbatim** — so it's compiled and
//!    type-checked against the component's *live* props. If a prop
//!    changes shape and the recipe isn't updated, this is a compile
//!    error. That's the whole point: self-verifying examples.
//! 2. Captures the fn's formatted source (via `prettyplease`), its
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

    // Formatted, exact source of the recipe fn for display/LLM. We
    // unparse the fn as a one-item file so `prettyplease` produces clean,
    // copy-pasteable Rust. Computed from the fn as written (the
    // `#[allow(dead_code)]` we add below for compilation is NOT shown).
    let file = syn::File {
        shebang: None,
        attrs: Vec::new(),
        items: vec![syn::Item::Fn(func.clone())],
    };
    let source = prettyplease::unparse(&file);

    // Components the recipe's `ui!`/`jsx!` body references — reuse the
    // `#[component]` composes walker. Dedup + sort for a stable list.
    let mut uses: Vec<String> = crate::mcp_emit::collect_composes(&func.block)
        .into_iter()
        .map(|(name, _line)| name)
        .collect();
    uses.sort();
    uses.dedup();
    let use_lits = uses.iter().map(|u| quote! { #u });

    quote! {
        // The recipe fn — compiled + type-checked against the live
        // component props. May be uncalled (docs-only), hence the allow.
        #[allow(dead_code)]
        #func

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
    }
}
