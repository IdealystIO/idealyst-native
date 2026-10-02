//! `#[component(remote)]`: one component source, compiled three ways.
//!
//! - **Bundle build** (`--cfg idealyst_stream_guest`): the body compiles
//!   (into `__<Name>_remote_body`), and a `#[no_mangle]` mount export
//!   `__idealyst_remote_<Name>` decodes the props the app sent and returns
//!   the encoded tree (`runtime_vocabulary::remote::bundle::__mount`).
//! - **Native app build**: the body is NOT compiled in. The component's fn
//!   sends its props (`RemoteProp::send` — signals cross as handles into the
//!   app's graph, plain values by value) and mounts the component from the
//!   installed bundle (`runtime_vocabulary::remote::host::__mount_remote`).
//! - **Web app build**: the body compiles as for any component — remote is
//!   a native mechanism, and web ships proper bundles.
//! - **Native app build with the vocabulary's `remote-inline` feature**: the
//!   body compiles and runs in-process, as on web — the same source as
//!   native code (to measure a remote component against itself, or debug it
//!   without the bundle). Picked by `__remote_native_body!`, so it follows
//!   the vocabulary's feature and the component's crate declares no cfg.
//!
//! Everything else (`#[component]`'s props struct, `ui!` dispatch, scope
//! wrapping) is the ordinary emission: the rewritten fn goes back through
//! it, so `ui! { Greeting(name = …, count = …) }` reads the same either way.
//!
//! Props are taken as DECLARED: a remote component's parameters are not
//! wrapped `Reactive<T>`. A plain value is fixed at mount; for a live
//! value, declare `ReadSignal<T>` (or `Signal<T>` for two-way). That makes
//! what crosses explicit: a handle into the app's graph, or a copy.

use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::{FnArg, ItemFn, Pat, Type};

/// The bundle build, or a web app build: the body is compiled.
const HAS_BODY: &str = "any(idealyst_stream_guest, target_arch = \"wasm32\")";

/// Rewrite `item_fn` for remote emission. Returns the rewritten component fn
/// (to go through the ordinary `#[component]` emission) and the extra items
/// (the body fn and the mount export) to emit beside it.
pub(crate) fn prepare(mut item_fn: ItemFn) -> syn::Result<(ItemFn, TokenStream2)> {
    if !item_fn.sig.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &item_fn.sig.generics,
            "#[component(remote)] components can't be generic: the bundle exports one \
             monomorphic mount per component",
        ));
    }
    if crate::methods_block::has_method_fns(&item_fn) {
        return Err(syn::Error::new_spanned(
            &item_fn.sig.ident,
            "#[method] fns can't cross to a remote component yet: their handle would have to \
             call into the bundle",
        ));
    }
    let name = item_fn.sig.ident.clone();
    let name_str = name.to_string();
    let body_fn = format_ident!("__{}_remote_body", name);
    let export_fn = format_ident!("__idealyst_remote_{}", name);
    let has_body: TokenStream2 = HAS_BODY.parse().expect("cfg predicate");

    let mut params: Vec<(syn::Ident, Type)> = Vec::new();
    for input in item_fn.sig.inputs.iter_mut() {
        let FnArg::Typed(pt) = input else {
            return Err(syn::Error::new_spanned(input, "components are free functions"));
        };
        let Pat::Ident(pi) = &*pt.pat else {
            return Err(syn::Error::new_spanned(&pt.pat, "remote component props must be plain identifiers"));
        };
        if pt.attrs.iter().any(|a| a.path().is_ident("prop")) {
            return Err(syn::Error::new_spanned(
                &pt.pat,
                "#[prop(...)] isn't supported on a remote component's props yet: they are taken \
                 as declared (declare `ReadSignal<T>` for a live value)",
            ));
        }
        // Taken as declared: never wrapped `Reactive<T>` (see module docs).
        pt.attrs.push(syn::parse_quote!(#[prop(static)]));
        params.push((pi.ident.clone(), (*pt.ty).clone()));
    }
    let names: Vec<&syn::Ident> = params.iter().map(|(n, _)| n).collect();
    let types: Vec<&Type> = params.iter().map(|(_, t)| t).collect();

    // The body, as its own fn, compiled only where it runs. Same parameter
    // list (without the `#[prop]` / doc attrs), same reactivity rewrite the
    // component fn would have had.
    let mut body = item_fn.clone();
    body.sig.ident = body_fn.clone();
    body.vis = syn::Visibility::Inherited;
    body.attrs.retain(|a| !a.path().is_ident("doc"));
    for input in body.sig.inputs.iter_mut() {
        if let FnArg::Typed(pt) = input {
            pt.attrs.clear();
        }
    }
    crate::reactivity::rewrite(&mut body);

    // A native app build mounts from the bundle, unless the vocabulary's
    // `remote-inline` feature runs the body in-process instead
    // (`__remote_native_body!` picks; see the module docs).
    item_fn.block = Box::new(syn::parse_quote! {{
        #[cfg(#has_body)]
        let __remote_tree = #body_fn(#(#names),*);
        #[cfg(not(#has_body))]
        let __remote_tree = ::runtime_vocabulary::__remote_native_body! {
            inline: { #body_fn(#(#names),*) }
            mount: {
                ::runtime_vocabulary::remote::host::__mount_remote(
                    #name_str,
                    move |__out: &mut ::std::vec::Vec<u8>, __keep: &mut ::runtime_vocabulary::remote::host::Keep| {
                        #(::runtime_vocabulary::remote::RemoteProp::send(&#names, __out, __keep);)*
                    },
                )
            }
        };
        __remote_tree
    }});

    let extra = quote! {
        #[cfg(#has_body)]
        #[allow(non_snake_case)]
        #body

        #[cfg(not(#has_body))]
        ::runtime_vocabulary::__remote_native_body! {
            inline: {
                #[allow(non_snake_case)]
                #body
            }
            mount: {}
        }

        /// The bundle's mount export for this remote component.
        #[cfg(idealyst_stream_guest)]
        #[no_mangle]
        #[allow(non_snake_case)]
        pub extern "C" fn #export_fn(_ptr: u32, len: u32) -> i64 {
            ::runtime_vocabulary::remote::bundle::__mount(len, |__in: &mut &[u8]| {
                #(let #names = <#types as ::runtime_vocabulary::remote::RemoteProp>::receive(__in);)*
                #name(#(#names),*)
            })
        }
    };
    Ok((item_fn, extra))
}

/// What [`import_split`] adds to a component's emission.
#[derive(Default)]
pub(crate) struct Import {
    /// The app-side registration (`__remote_app_component!`), a sibling item.
    pub(crate) registration: TokenStream2,
    /// For the explicit-props form: the import key, so its `BuildElement`
    /// can send the props by value in a bundle build.
    pub(crate) explicit_name: Option<TokenStream2>,
}

/// A component NOT marked `remote` lives in the app binary. In a bundle
/// build (`--cfg idealyst_stream_guest`) its body is not compiled: the fn
/// sends its props and imports the app's copy by name
/// (`runtime_vocabulary::__remote_import!`). In a native app it registers
/// itself (`__remote_app_component!`). Both expand to nothing unless the
/// build hosts or is a remote bundle, so apps without remote components get
/// no code.
///
/// Components whose shape can't be imported (generic, `lazy`, `#[method]`,
/// a non-`Element` return, a parameter list that is neither inline props
/// nor one props struct) are left as they are — compiled into a bundle that
/// uses them.
pub(crate) fn import_split(
    item_fn: &mut ItemFn,
    attr: &crate::component_attr::ComponentAttr,
    inline: bool,
    has_methods: bool,
) -> Import {
    let Some(key) = import_key(item_fn, attr, has_methods) else {
        return Import::default();
    };
    let name = item_fn.sig.ident.clone();
    let name_str = name.to_string();

    let (stub, registration, explicit) = if inline {
        let props = format_ident!("{}Props", name);
        let fields: Vec<syn::Ident> = item_fn
            .sig
            .inputs
            .iter()
            .filter_map(|a| match a {
                FnArg::Typed(pt) => match &*pt.pat {
                    Pat::Ident(pi) => Some(pi.ident.clone()),
                    _ => None,
                },
                FnArg::Receiver(_) => None,
            })
            .collect();
        if fields.len() != item_fn.sig.inputs.len() {
            return Import::default();
        }
        (
            quote! { ::runtime_vocabulary::__remote_import!(#key, #props, #props { #(#fields),* }) },
            quote! {
                ::runtime_vocabulary::__remote_app_component!(
                    #key, #props, <#props as ::runtime_core::BuildElement>::defaults(), |__p| #name(#(__p.#fields),*)
                );
            },
            false,
        )
    } else if item_fn.sig.inputs.is_empty() {
        (
            quote! { ::runtime_vocabulary::__remote_import!(#key, (), ()) },
            quote! { ::runtime_vocabulary::__remote_app_component!(#key, (), (), |__p| { let () = __p; #name() }); },
            false,
        )
    } else {
        let Some((param, path, by_ref)) = single_props_param(&item_fn.sig) else {
            return Import::default();
        };
        let stub = if by_ref {
            quote! {{
                let _ = #param;
                ::core::panic!(
                    "`{}` is an app component: a remote component uses it through `ui!`, not by calling it",
                    #name_str
                )
            }}
        } else {
            quote! { ::runtime_vocabulary::__remote_import!(#key, #path, #param) }
        };
        let amp = if by_ref { quote!(&) } else { quote!() };
        (
            stub,
            quote! {
                ::runtime_vocabulary::__remote_app_component!(
                    #key, #path, <#path as ::runtime_core::BuildElement>::defaults(), |__p| #name(#amp __p)
                );
            },
            true,
        )
    };

    // Swap the body by build kind, chosen by the vocabulary's flag
    // (`__remote_guest_split!`), so the component's crate declares no cfg
    // of its own. The body goes into the macro call as tokens: syn doesn't
    // re-parse it (a macro call stays opaque to syn), and the re-parse is
    // the cost that matters for expansion time (see
    // `wrap_component_body_new_core`).
    let body = std::mem::replace(&mut *item_fn.block, syn::Block { brace_token: Default::default(), stmts: Vec::new() });
    *item_fn.block = syn::parse_quote!({
        let __remote_tree: ::runtime_core::Element = ::runtime_vocabulary::__remote_guest_split! {
            bundle: { #stub }
            app: { #body }
        };
        __remote_tree
    });
    Import { registration, explicit_name: explicit.then_some(key) }
}

/// The name a bundle imports this app component by (`module_path::Name`),
/// or `None` when its shape can't be imported (see [`import_split`]).
/// Decided once, before the inline-props glue is generated, so the glue's
/// `BuildElement::build_set` and the import agree.
pub(crate) fn import_key(
    item_fn: &ItemFn,
    attr: &crate::component_attr::ComponentAttr,
    has_methods: bool,
) -> Option<TokenStream2> {
    if attr.lazy || attr.no_import || has_methods || !item_fn.sig.generics.params.is_empty() || !returns_element(item_fn) {
        return None;
    }
    let name_str = item_fn.sig.ident.to_string();
    Some(quote! { ::core::concat!(::core::module_path!(), "::", #name_str) })
}

/// The `BuildElement::build_set` a bundle build gives an imported app
/// component: send only the fields the call site set (the app fills the
/// rest from its own defaults).
pub(crate) fn build_set_override(key: &TokenStream2, props: &TokenStream2) -> TokenStream2 {
    quote! {
        ::runtime_vocabulary::__remote_guest_split! {
            bundle: {
                fn build_set(self, __set: &'static [&'static str]) -> ::runtime_core::Element {
                    ::runtime_vocabulary::__remote_import!(#key, #props, self, ::core::option::Option::Some(__set))
                }
            }
            app: {}
        }
    }
}

/// `fn Foo(props: FooProps)` / `fn Foo(props: &FooProps)`: the param, the
/// props type, and whether it is taken by reference.
fn single_props_param(sig: &syn::Signature) -> Option<(syn::Ident, TokenStream2, bool)> {
    if sig.inputs.len() != 1 {
        return None;
    }
    let FnArg::Typed(pt) = &sig.inputs[0] else { return None };
    let Pat::Ident(pi) = &*pt.pat else { return None };
    match &*pt.ty {
        Type::Reference(r) => match &*r.elem {
            Type::Path(p) => {
                let path = &p.path;
                Some((pi.ident.clone(), quote!(#path), true))
            }
            _ => None,
        },
        Type::Path(p) => {
            let path = &p.path;
            Some((pi.ident.clone(), quote!(#path), false))
        }
        _ => None,
    }
}

fn returns_element(item_fn: &ItemFn) -> bool {
    let syn::ReturnType::Type(_, ty) = &item_fn.sig.output else { return false };
    let normalized: String = quote!(#ty).to_string().chars().filter(|c| !c.is_whitespace()).collect();
    matches!(normalized.as_str(), "Element" | "runtime_core::Element" | "::runtime_core::Element")
}

/// The mount export's name for component `name` — what the host's loader
/// calls. Kept in one place for both sides.
#[cfg(test)]
pub(crate) fn export_name(name: &str) -> String {
    format!("__idealyst_remote_{name}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emits_a_body_fn_a_stub_and_an_export() {
        let f: ItemFn = syn::parse_quote! {
            /// Docs.
            fn Greeting(name: String, count: ReadSignal<i64>) -> Element {
                ui! { text { "{name}" } }
            }
        };
        let (component, extra) = prepare(f).unwrap();
        let component = quote!(#component).to_string();
        let extra = extra.to_string();
        assert!(component.contains("__Greeting_remote_body (name , count)"), "{component}");
        assert!(component.contains("__mount_remote"), "{component}");
        assert!(component.contains("prop (static)"), "params are taken as declared: {component}");
        assert!(extra.contains(&export_name("Greeting")), "{extra}");
        assert!(extra.contains("fn __Greeting_remote_body"), "{extra}");
        assert!(extra.contains("RemoteProp > :: receive"), "{extra}");
        // The native build's choice between running the body in-process and
        // mounting it is the vocabulary's (`remote-inline`), in both places.
        assert!(component.contains("__remote_native_body"), "{component}");
        assert!(extra.contains("__remote_native_body"), "{extra}");
    }

    #[test]
    fn rejects_generic_and_prop_attributed_components() {
        let generic: ItemFn = syn::parse_quote! { fn G<T>(x: T) -> Element { todo!() } };
        assert!(prepare(generic).is_err());
        let attributed: ItemFn = syn::parse_quote! { fn P(#[prop(default = 1)] x: i32) -> Element { todo!() } };
        assert!(prepare(attributed).is_err());
    }
}
