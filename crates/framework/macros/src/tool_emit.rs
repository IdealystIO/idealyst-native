//! Phase 3c — `#[idealyst_tool]` attribute macro.
//!
//! Like `#[component]` minus the reactivity/composes infrastructure
//! — `#[idealyst_tool]` simply registers the function into the MCP
//! catalog as a `ToolEntry`. The function body is left as-is; the
//! macro is purely additive.
//!
//! When the `mcp` feature is off the attribute expands to the
//! function unchanged. Per spec §4.2.

use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::ItemFn;

pub(crate) fn emit(item_fn: &ItemFn) -> TokenStream2 {
    let name_str = item_fn.sig.ident.to_string();
    let docs = collect_doc_comments(&item_fn.attrs);
    let params = collect_params(&item_fn.sig);
    let return_type = collect_return_type(&item_fn.sig);

    let param_entries = params.iter().map(|(name, ty, short)| {
        quote! {
            ::framework_core::__mcp::ParamSpec {
                name: #name,
                type_str: #ty,
                type_short_name: #short,
            }
        }
    });

    quote! {
        ::framework_core::__mcp::inventory::submit! {
            ::framework_core::__mcp::ToolEntry {
                name: #name_str,
                module_path: module_path!(),
                file: file!(),
                line: line!(),
                docs: #docs,
                params: &[ #(#param_entries),* ],
                return_type: #return_type,
            }
        }
    }
}

/// Same doc-comment harvester as `mcp_emit` — duplicated to keep
/// each emission module standalone.
fn collect_doc_comments(attrs: &[syn::Attribute]) -> String {
    let mut lines: Vec<String> = Vec::new();
    for attr in attrs {
        if !attr.path().is_ident("doc") {
            continue;
        }
        if let syn::Meta::NameValue(nv) = &attr.meta {
            if let syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Str(s), .. }) = &nv.value {
                let raw = s.value();
                lines.push(raw.strip_prefix(' ').unwrap_or(&raw).to_string());
            }
        }
    }
    lines.join("\n")
}

/// Mirror of `mcp_emit::collect_params` (same shape — same join into
/// `PropsSchemaEntry`'s `short_name` field). Kept inline rather than
/// re-exported so the two modules can evolve independently if tools
/// ever grow their own per-parameter sugar.
fn collect_params(sig: &syn::Signature) -> Vec<(String, String, String)> {
    let mut out = Vec::with_capacity(sig.inputs.len());
    for arg in &sig.inputs {
        let syn::FnArg::Typed(pat_type) = arg else {
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

fn type_short_name(ty: &syn::Type) -> Option<String> {
    match ty {
        syn::Type::Reference(r) => type_short_name(&r.elem),
        syn::Type::Paren(p) => type_short_name(&p.elem),
        syn::Type::Group(g) => type_short_name(&g.elem),
        syn::Type::Path(p) => p.path.segments.last().map(|s| s.ident.to_string()),
        _ => None,
    }
}

fn collect_return_type(sig: &syn::Signature) -> String {
    match &sig.output {
        syn::ReturnType::Default => String::new(),
        syn::ReturnType::Type(_, ty) => quote! { #ty }.to_string(),
    }
}
