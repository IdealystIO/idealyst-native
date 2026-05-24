//! Phase 3b — `#[derive(IdealystSchema)]` emission.
//!
//! For named structs: registers
//! - one `mcp_catalog::PropsSchemaEntry` (legacy/back-compat path
//!   the existing MCP server reads when joining `ParamSpec` to the
//!   prop schema), AND
//! - one `mcp_catalog::TypeEntry { shape: Struct }` (the unified
//!   type catalog).
//!
//! For enums: registers
//! - one `mcp_catalog::TypeEntry { shape: Enum }` containing each
//!   variant's name, docs, and payload (unit / tuple / struct).
//!
//! Recognised `#[schema(...)]` field attributes:
//! - `constraint = "..."` — free-form constraint hint. Surfaces as
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
    let type_docs = collect_field_docs(&input.attrs);

    match &input.data {
        Data::Struct(s) => match &s.fields {
            Fields::Named(named) => emit_named_struct(&struct_name_str, &type_docs, &named.named),
            // Tuple structs and unit structs: emit an empty `TypeEntry`
            // (shape `Struct` with no fields) so consumers know the
            // type exists, but no `PropsSchemaEntry` since prop names
            // don't apply.
            _ => emit_empty_struct(&struct_name_str, &type_docs),
        },
        Data::Enum(e) => emit_enum(&struct_name_str, &type_docs, e),
        // Unions: silently inert. The catalog has no use for them.
        Data::Union(_) => TokenStream2::new(),
    }
}

fn emit_named_struct(
    struct_name_str: &str,
    type_docs: &str,
    fields: &syn::punctuated::Punctuated<syn::Field, syn::Token![,]>,
) -> TokenStream2 {
    let field_specs: Vec<TokenStream2> = fields
        .iter()
        .filter_map(|f| {
            let name_ident = f.ident.as_ref()?;
            let name = name_ident.to_string();
            let ty = &f.ty;
            let type_str = quote! { #ty }.to_string();
            let doc = collect_field_docs(&f.attrs);
            let constraint = collect_constraint(&f.attrs);
            Some(quote! {
                ::runtime_core::__mcp::PropFieldSpec {
                    name: #name,
                    type_str: #type_str,
                    doc: #doc,
                    constraint: #constraint,
                }
            })
        })
        .collect();
    let field_specs_props = field_specs.clone();
    let field_specs_type = field_specs;

    quote! {
        ::runtime_core::__mcp::inventory::submit! {
            ::runtime_core::__mcp::PropsSchemaEntry {
                short_name: #struct_name_str,
                module_path: module_path!(),
                fields: &[ #(#field_specs_props),* ],
            }
        }
        ::runtime_core::__mcp::inventory::submit! {
            ::runtime_core::__mcp::TypeEntry {
                short_name: #struct_name_str,
                module_path: module_path!(),
                docs: #type_docs,
                shape: ::runtime_core::__mcp::TypeShape::Struct {
                    fields: &[ #(#field_specs_type),* ],
                },
            }
        }
    }
}

fn emit_empty_struct(struct_name_str: &str, type_docs: &str) -> TokenStream2 {
    quote! {
        ::runtime_core::__mcp::inventory::submit! {
            ::runtime_core::__mcp::TypeEntry {
                short_name: #struct_name_str,
                module_path: module_path!(),
                docs: #type_docs,
                shape: ::runtime_core::__mcp::TypeShape::Struct {
                    fields: &[],
                },
            }
        }
    }
}

fn emit_enum(enum_name_str: &str, type_docs: &str, data: &syn::DataEnum) -> TokenStream2 {
    let variants: Vec<TokenStream2> = data
        .variants
        .iter()
        .map(|v| {
            let name = v.ident.to_string();
            let docs = collect_field_docs(&v.attrs);
            let payload: Vec<TokenStream2> = match &v.fields {
                Fields::Unit => Vec::new(),
                Fields::Named(named) => named
                    .named
                    .iter()
                    .filter_map(|f| {
                        let name = f.ident.as_ref()?.to_string();
                        let ty = &f.ty;
                        let type_str = quote! { #ty }.to_string();
                        let doc = collect_field_docs(&f.attrs);
                        let constraint = collect_constraint(&f.attrs);
                        Some(quote! {
                            ::runtime_core::__mcp::PropFieldSpec {
                                name: #name,
                                type_str: #type_str,
                                doc: #doc,
                                constraint: #constraint,
                            }
                        })
                    })
                    .collect(),
                Fields::Unnamed(unnamed) => unnamed
                    .unnamed
                    .iter()
                    .map(|f| {
                        let ty = &f.ty;
                        let type_str = quote! { #ty }.to_string();
                        let doc = collect_field_docs(&f.attrs);
                        let constraint = collect_constraint(&f.attrs);
                        // Tuple variants have no field name — emit
                        // empty string so consumers can detect the
                        // positional shape.
                        quote! {
                            ::runtime_core::__mcp::PropFieldSpec {
                                name: "",
                                type_str: #type_str,
                                doc: #doc,
                                constraint: #constraint,
                            }
                        }
                    })
                    .collect(),
            };
            quote! {
                ::runtime_core::__mcp::VariantSpec {
                    name: #name,
                    docs: #docs,
                    payload: &[ #(#payload),* ],
                }
            }
        })
        .collect();

    quote! {
        ::runtime_core::__mcp::inventory::submit! {
            ::runtime_core::__mcp::TypeEntry {
                short_name: #enum_name_str,
                module_path: module_path!(),
                docs: #type_docs,
                shape: ::runtime_core::__mcp::TypeShape::Enum {
                    variants: &[ #(#variants),* ],
                },
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
