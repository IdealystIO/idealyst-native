//! Phase 3b — `#[derive(IdealystSchema)]` emission.
//!
//! Walks the struct's named fields and registers a
//! `framework_mcp::PropsSchemaEntry` per struct so the MCP runtime
//! can expand a component's `&FooProps` parameter into its constituent
//! fields. Per spec §4.3 the derive is purely opt-in — users only
//! reach for it when they want richer prop info than `ParamSpec`
//! provides on its own.
//!
//! Recognised `#[schema(...)]` field attributes:
//! - `constraint = "..."` — free-form constraint hint (e.g.
//!   `"valid CSS color"`). Surfaces in the catalog as
//!   `PropFieldSpec.constraint`. Empty when absent.
//!
//! Like `mcp_emit`, this is no-op'd at the macro level when the
//! `mcp` feature is off (call site in `lib.rs`).

use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{Data, DeriveInput, Fields};

pub(crate) fn emit(input: DeriveInput) -> TokenStream2 {
    let struct_ident = &input.ident;
    let struct_name_str = struct_ident.to_string();

    // Only named-struct fields make sense as props. Tuple structs and
    // unit structs can't have prop names, so emit nothing — the
    // derive is silently inert on those shapes rather than failing
    // the user's build.
    let fields = match &input.data {
        Data::Struct(s) => match &s.fields {
            Fields::Named(named) => &named.named,
            _ => return TokenStream2::new(),
        },
        _ => return TokenStream2::new(),
    };

    let field_entries = fields.iter().filter_map(|f| {
        let name_ident = f.ident.as_ref()?;
        let name = name_ident.to_string();
        let ty = &f.ty;
        let type_str = quote! { #ty }.to_string();
        let doc = collect_field_docs(&f.attrs);
        let constraint = collect_constraint(&f.attrs);
        Some(quote! {
            ::framework_core::__mcp::PropFieldSpec {
                name: #name,
                type_str: #type_str,
                doc: #doc,
                constraint: #constraint,
            }
        })
    });

    quote! {
        ::framework_core::__mcp::inventory::submit! {
            ::framework_core::__mcp::PropsSchemaEntry {
                short_name: #struct_name_str,
                module_path: module_path!(),
                fields: &[ #(#field_entries),* ],
            }
        }
    }
}

fn collect_field_docs(attrs: &[syn::Attribute]) -> String {
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

/// Parse `#[schema(constraint = "...")]` off a field. Quietly ignores
/// other `#[schema(...)]` arguments — the derive is forward-compatible
/// with future hint kinds the spec might add.
fn collect_constraint(attrs: &[syn::Attribute]) -> String {
    for attr in attrs {
        if !attr.path().is_ident("schema") {
            continue;
        }
        let mut found = String::new();
        let _ = attr.parse_nested_meta(|m| {
            if m.path.is_ident("constraint") {
                let value = m.value()?;
                let s: syn::LitStr = value.parse()?;
                found = s.value();
            }
            Ok(())
        });
        if !found.is_empty() {
            return found;
        }
    }
    String::new()
}
