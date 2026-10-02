//! `#[derive(Remote)]`: a value type crosses between an app and its remote
//! bundles, field by field, every way it can — bundle → app as an
//! `ImportArg` (an app component's prop), app → bundle as a `RemoteProp`
//! (a remote component's prop, or context), and as plain data, a
//! `RemoteValue` (a signal's value, a callback's argument).
//!
//! Each field crosses as its own type does, through the vocabulary's
//! probes (`Arg<T>`): a field type that can't cross is a runtime error
//! naming it when a value actually crosses, never a compile error, so a
//! library can derive it on every value type without breaking an app
//! build. The whole expansion goes through `__remote_enabled!`, so it
//! costs nothing in an app that hosts no remote components, and each
//! method through the side macros (`__remote_bundle_code!` /
//! `__remote_app_code!`), which follow the vocabulary's build rather than
//! the deriving crate's.
//!
//! Encoding: a struct's fields in declaration order; an enum's variant
//! index (`u32`), then that variant's fields. Both sides compile the same
//! source, so positions agree.

use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::{Data, DeriveInput, Fields};

/// The bindings a pattern over `fields` introduces, and that pattern.
fn bind(fields: &Fields) -> (Vec<syn::Ident>, Vec<&syn::Type>, TokenStream2) {
    match fields {
        Fields::Named(f) => {
            let names: Vec<syn::Ident> = f.named.iter().map(|f| f.ident.clone().expect("named")).collect();
            let types = f.named.iter().map(|f| &f.ty).collect();
            (names.clone(), types, quote! { { #(#names),* } })
        }
        Fields::Unnamed(f) => {
            let names: Vec<syn::Ident> = (0..f.unnamed.len()).map(|i| format_ident!("__f{}", i)).collect();
            let types = f.unnamed.iter().map(|f| &f.ty).collect();
            (names.clone(), types, quote! { ( #(#names),* ) })
        }
        Fields::Unit => (Vec::new(), Vec::new(), quote! {}),
    }
}

/// `path { a: <recv>, … }` / `path(<recv>, …)` / `path`, one receive per
/// field.
fn construct(path: TokenStream2, fields: &Fields, recv: impl Fn(&syn::Type) -> TokenStream2) -> TokenStream2 {
    match fields {
        Fields::Named(f) => {
            let parts = f.named.iter().map(|f| {
                let (n, r) = (f.ident.as_ref().expect("named"), recv(&f.ty));
                quote! { #n: #r }
            });
            quote! { #path { #(#parts),* } }
        }
        Fields::Unnamed(f) => {
            let parts = f.unnamed.iter().map(|f| recv(&f.ty));
            quote! { #path( #(#parts),* ) }
        }
        Fields::Unit => path,
    }
}

pub(crate) fn derive(input: DeriveInput) -> syn::Result<TokenStream2> {
    if !input.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &input.generics,
            "#[derive(Remote)] types can't be generic yet: each side decodes one concrete type",
        ));
    }
    let name = &input.ident;
    let v = quote!(::runtime_vocabulary::remote);

    // Bundle → app (`ImportArg`): send by value, receive with the import's
    // context. App → bundle (`RemoteProp`): send by reference (the app
    // keeps its value), receive plain.
    let send_field = |f: &syn::Ident, t: &syn::Type| quote! { (&#v::Arg::<#t>::new()).send(#f, __out); };
    let recv_field = |t: &syn::Type| quote! { (&#v::Arg::<#t>::new()).receive(__in, __cx)? };
    let send_prop_field = |f: &syn::Ident, t: &syn::Type| quote! { (&#v::Arg::<#t>::new()).send_prop(#f, __out, __keep); };
    let recv_prop_field = |t: &syn::Type| quote! { (&#v::Arg::<#t>::new()).receive_prop(__in) };
    // Plain data (`RemoteValue`): what a signal holding it, or a callback
    // taking it, needs. Both sides, by reference / fallible.
    let encode_field = |f: &syn::Ident, t: &syn::Type| quote! { (&#v::Arg::<#t>::new()).encode_value(#f, __out); };
    let decode_field = |t: &syn::Type| quote! { (&#v::Arg::<#t>::new()).decode_value(__in)? };

    let (send, recv, send_prop, recv_prop, encode, decode) = match &input.data {
        Data::Struct(s) => {
            let (names, types, pattern) = bind(&s.fields);
            let sends = names.iter().zip(&types).map(|(n, t)| send_field(n, t));
            let prop_sends = names.iter().zip(&types).map(|(n, t)| send_prop_field(n, t));
            let encodes = names.iter().zip(&types).map(|(n, t)| encode_field(n, t));
            let built = construct(quote!(Self), &s.fields, recv_field);
            let built_prop = construct(quote!(Self), &s.fields, recv_prop_field);
            let built_value = construct(quote!(Self), &s.fields, decode_field);
            (
                quote! { let Self #pattern = self; #(#sends)* },
                quote! { ::core::result::Result::Ok(#built) },
                quote! { let Self #pattern = self; #(#prop_sends)* },
                quote! { #built_prop },
                quote! { let Self #pattern = self; #(#encodes)* },
                quote! { ::core::result::Result::Ok(#built_value) },
            )
        }
        Data::Enum(e) => {
            let mut send_arms = Vec::new();
            let mut recv_arms = Vec::new();
            let mut prop_send_arms = Vec::new();
            let mut prop_recv_arms = Vec::new();
            let mut encode_arms = Vec::new();
            let mut decode_arms = Vec::new();
            for (i, variant) in e.variants.iter().enumerate() {
                let i = i as u32;
                let vn = &variant.ident;
                let (names, types, pattern) = bind(&variant.fields);
                let sends = names.iter().zip(&types).map(|(n, t)| send_field(n, t));
                let prop_sends = names.iter().zip(&types).map(|(n, t)| send_prop_field(n, t));
                send_arms.push(quote! { Self::#vn #pattern => { #v::__send_value(&#i, __out); #(#sends)* } });
                prop_send_arms.push(quote! { Self::#vn #pattern => { #v::__send_value(&#i, __out); #(#prop_sends)* } });
                let encodes = names.iter().zip(&types).map(|(n, t)| encode_field(n, t));
                encode_arms.push(quote! { Self::#vn #pattern => { #v::__send_value(&#i, __out); #(#encodes)* } });
                let built_value = construct(quote!(Self::#vn), &variant.fields, decode_field);
                decode_arms.push(quote! { #i => ::core::result::Result::Ok(#built_value), });
                let built = construct(quote!(Self::#vn), &variant.fields, recv_field);
                let built_prop = construct(quote!(Self::#vn), &variant.fields, recv_prop_field);
                recv_arms.push(quote! { #i => ::core::result::Result::Ok(#built), });
                prop_recv_arms.push(quote! { #i => #built_prop, });
            }
            let unknown = quote! {
                ::std::format!("`{}` has no variant {}: the app and the bundle disagree about it", ::core::any::type_name::<Self>(), __n)
            };
            (
                quote! { match self { #(#send_arms)* } },
                quote! {
                    match #v::__try_receive_value::<u32>(__in)? {
                        #(#recv_arms)*
                        __n => ::core::result::Result::Err(#unknown),
                    }
                },
                quote! { match self { #(#prop_send_arms)* } },
                quote! {
                    match #v::__receive_value::<u32>(__in) {
                        #(#prop_recv_arms)*
                        __n => ::core::panic!("{}", #unknown),
                    }
                },
                quote! { match self { #(#encode_arms)* } },
                quote! {
                    match #v::__try_receive_value::<u32>(__in)? {
                        #(#decode_arms)*
                        __n => ::core::result::Result::Err(#unknown),
                    }
                },
            )
        }
        Data::Union(u) => {
            return Err(syn::Error::new_spanned(u.union_token, "#[derive(Remote)] doesn't support unions"));
        }
    };

    Ok(quote! {
        ::runtime_vocabulary::__remote_enabled! {
            impl #v::ImportArg for #name {
                ::runtime_vocabulary::__remote_bundle_code! {
                    fn send(self, __out: &mut ::std::vec::Vec<u8>) {
                        #[allow(unused_imports)]
                        use #v::{ViaImport as _, ViaUnsupported as _};
                        #send
                    }
                }
                ::runtime_vocabulary::__remote_app_code! {
                    fn receive(
                        __in: &mut &[u8],
                        __cx: &#v::host::ImportCx,
                    ) -> ::core::result::Result<Self, ::std::string::String> {
                        #[allow(unused_imports)]
                        use #v::{ViaImport as _, ViaUnsupported as _};
                        let _ = (&__in, __cx);
                        #recv
                    }
                }
            }
            impl #v::RemoteValue for #name {
                fn encode(&self, __out: &mut ::std::vec::Vec<u8>) {
                    #[allow(unused_imports)]
                    use #v::{ViaValue as _, ViaNoValue as _};
                    #encode
                }
                fn decode(__in: &mut &[u8]) -> ::core::result::Result<Self, ::std::string::String> {
                    #[allow(unused_imports)]
                    use #v::{ViaValue as _, ViaNoValue as _};
                    let _ = &__in;
                    #decode
                }
            }
            impl #v::RemoteProp for #name {
                ::runtime_vocabulary::__remote_app_code! {
                    fn send(&self, __out: &mut ::std::vec::Vec<u8>, __keep: &mut #v::host::Keep) {
                        #[allow(unused_imports)]
                        use #v::{ViaRemoteProp as _, ViaNoRemoteProp as _};
                        let _ = &__keep;
                        #send_prop
                    }
                }
                ::runtime_vocabulary::__remote_bundle_code! {
                    fn receive(__in: &mut &[u8]) -> Self {
                        #[allow(unused_imports)]
                        use #v::{ViaRemoteProp as _, ViaNoRemoteProp as _};
                        let _ = &__in;
                        #recv_prop
                    }
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_struct_and_an_enum_expand_through_the_gate() {
        let s: DeriveInput = syn::parse_quote! { struct Opt { id: String, label: Reactive<String> } };
        let out = derive(s).unwrap().to_string();
        assert!(out.contains("__remote_enabled"), "{out}");
        assert!(out.contains("ImportArg for Opt") && out.contains("RemoteProp for Opt"), "{out}");
        let e: DeriveInput = syn::parse_quote! { enum Close { None, Button(Rc<dyn Fn()>), Custom { el: Element } } };
        let out = derive(e).unwrap().to_string();
        assert!(out.contains("Self :: Button (__f0)") && out.contains("Self :: Custom { el }"), "{out}");
    }

    #[test]
    fn generics_are_rejected() {
        let g: DeriveInput = syn::parse_quote! { struct G<T> { t: T } };
        assert!(derive(g).is_err());
    }
}
