//! `#[component(external)]` catalog emission.
//!
//! When a component is tagged `external` and the `catalog` feature is
//! on, [`emit`] produces the `inventory::submit!` of an
//! `mcp_catalog::ExternalEntry` recording the component name, its props
//! struct's short name (the join key to `PropsSchemaEntry`), and the
//! Web Component tag the export will generate. The prop *fields* live in
//! the props struct's own `#[derive(IdealystSchema)]` registration — we
//! deliberately don't duplicate them here.

use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::ItemFn;

use crate::component_attr::ExternalSpec;

pub(crate) fn emit(item_fn: &ItemFn, spec: &ExternalSpec) -> TokenStream2 {
    let name_str = item_fn.sig.ident.to_string();
    // The props struct is the first value parameter's type — the same
    // `fn Foo(props: &FooProps)` shape every component uses. A zero-prop
    // component yields an empty join key (fine; it has no props to bridge).
    let props_short = first_param_type_short(&item_fn.sig).unwrap_or_default();
    let tag = spec
        .tag
        .clone()
        .unwrap_or_else(|| format!("idl-{}", kebab_case(&name_str)));

    quote! {
        ::runtime_core::__mcp::inventory::submit! {
            ::runtime_core::__mcp::ExternalEntry {
                name: #name_str,
                module_path: module_path!(),
                props_short_name: #props_short,
                tag: #tag,
            }
        }
    }
}

/// Short name (last path segment, refs/generics stripped) of the first
/// value parameter's type. `props: &GreeterProps` → `"GreeterProps"`.
fn first_param_type_short(sig: &syn::Signature) -> Option<String> {
    for arg in &sig.inputs {
        let syn::FnArg::Typed(pat_type) = arg else {
            continue;
        };
        return type_short_name(&pat_type.ty);
    }
    None
}

/// Unwrap `&T` / `&'a T` / `&mut T` to the underlying path's bare ident.
/// Mirrors `mcp_emit::type_short_name`.
fn type_short_name(ty: &syn::Type) -> Option<String> {
    match ty {
        syn::Type::Reference(r) => type_short_name(&r.elem),
        syn::Type::Paren(p) => type_short_name(&p.elem),
        syn::Type::Group(g) => type_short_name(&g.elem),
        syn::Type::Path(p) => p.path.segments.last().map(|s| s.ident.to_string()),
        _ => None,
    }
}

/// PascalCase → kebab-case: `"Greeter"` → `"greeter"`,
/// `"MyButton"` → `"my-button"`, `"HTMLView"` → `"htmlview"` (runs of
/// capitals collapse — good enough for a default tag the author can
/// override).
fn kebab_case(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    let mut prev_lower = false;
    for c in s.chars() {
        if c.is_ascii_uppercase() {
            if prev_lower {
                out.push('-');
            }
            out.push(c.to_ascii_lowercase());
            prev_lower = false;
        } else {
            out.push(c);
            prev_lower = c.is_ascii_lowercase() || c.is_ascii_digit();
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::kebab_case;

    #[test]
    fn kebab_basic() {
        assert_eq!(kebab_case("Greeter"), "greeter");
        assert_eq!(kebab_case("MyButton"), "my-button");
        assert_eq!(kebab_case("DatePicker2"), "date-picker2");
    }
}
