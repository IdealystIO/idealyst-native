//! `#[host_fn]` — an app function remote code can call: in the app the
//! function itself, in a remote bundle a stub that asks the app to run it.
//! One definition, in a crate both builds compile (an app, or a library
//! like idea-ui).
//!
//! - **App build:** the function, unchanged. When the app hosts remote
//!   components, also `<name>::export()`, the `HostFnDef` the app lists in
//!   its allowlist (`remote_host::remote::install_with`).
//! - **Bundle build:** a stub. A sync fn keeps its signature; its body
//!   becomes one wasm import call. An `async fn` returns a
//!   `runtime_vocabulary::remote::bundle::HostFuture<Output>`, which the
//!   framework's `spawn_then(future, then)` drives as it does natively.
//!
//! Arguments and the result cross as `RemoteValue`s — plain data, including
//! values that cross by key (an idea-theme `ToneRef`), which only the app
//! decodes. Each stub links an import named `<module_path>::<fn>#<schema>`,
//! the schema a fingerprint of the signature, checked at load.
//!
//! # Generic host functions
//!
//! The app has a generic function only as the instantiations it compiled,
//! and a bundle may call it with a type the app has never seen. So each
//! type parameter is classified by its bound
//! (`runtime_vocabulary::host_types`):
//!
//! - **`T: Key`** — the app compiles the body ONCE, with `T = KeyBytes`
//!   (the key's order-preserving bytes); the bundle's stub encodes each
//!   key and decodes any it gets back. Any key type works, a bundle-only
//!   one included.
//! - **`T: Opaque`**, or no bound at all — once, with `T = OpaqueBytes`
//!   (the value's codec bytes, carried unread). The bundle's type must be
//!   a `RemoteValue` (`#[derive(Remote)]`).
//! - **`T: Numeric`** — the app compiles the body for all ten number types
//!   and the call carries which (a tag byte per such parameter; at most
//!   two, since each multiplies the copies by ten).
//! - **Any other bound** (`S: Area`) — the type must be one the app knows,
//!   so the app lists the ones bundles may use
//!   (`host_fn_instances!(tools::total_area: Circle, Rect)`), and the call
//!   starts with the type's `RemoteName`. A bundle calling an instance the
//!   app didn't list is stopped, with an error naming it. At most one such
//!   parameter.
//!
//! Native callers in the app call the generic function with their own
//! types, as any Rust function. Erased parameters may appear in an
//! argument or the result as themselves or inside `Vec`, `Option`, tuples
//! and arrays (each such place is walked by the stub); anything else
//! naming one is a compile error saying so. A bound that needs code the
//! bundle has (`F: Fn(..)`) is refused: calling back into the bundle for
//! each element is slower than doing the work in the bundle.
//!
//! The schema hashes the signature with each type parameter written as its
//! position and kind (`$Key0`), so renaming one doesn't refuse old bundles
//! and changing its kind does.
//!
//! Which build is decided by the vocabulary (`__remote_guest_split!`,
//! `__remote_enabled!`), so the defining crate declares no cfg and an app
//! without remote components gets only the function.

use std::collections::HashMap;

use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote, ToTokens};
use syn::visit::Visit;
use syn::visit_mut::VisitMut;
use syn::{FnArg, GenericParam, ItemFn, Pat, ReturnType, Type, TypeParamBound, WherePredicate};

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    /// `KeyBytes` in the app.
    Key,
    /// `OpaqueBytes` in the app.
    Opaque,
    /// One instantiation per number type, picked by a tag.
    Numeric,
    /// One instantiation per type the app lists, picked by name.
    Listed,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Key => "Key",
            Kind::Opaque => "Opaque",
            Kind::Numeric => "Numeric",
            Kind::Listed => "Listed",
        }
    }
}

/// The bound's trait name (its last path segment), or `None` for a
/// lifetime.
fn bound_name(b: &TypeParamBound) -> Option<(String, &syn::TraitBound)> {
    match b {
        TypeParamBound::Trait(t) => Some((t.path.segments.last()?.ident.to_string(), t)),
        _ => None,
    }
}

/// Each type parameter's kind, from its bounds (inline and in the where
/// clause).
fn classify(func: &ItemFn) -> syn::Result<Vec<(syn::Ident, Kind)>> {
    classify_with_bounds(func).map(|(p, _)| p)
}

/// [`classify`], with each parameter's bounds (a listed parameter's
/// instantiation repeats them).
fn classify_with_bounds(func: &ItemFn) -> syn::Result<(Vec<(syn::Ident, Kind)>, HashMap<String, Vec<TypeParamBound>>)> {
    let mut bounds: HashMap<String, Vec<&TypeParamBound>> = HashMap::new();
    let mut order = Vec::new();
    for p in &func.sig.generics.params {
        match p {
            GenericParam::Type(t) => {
                order.push(t.ident.clone());
                bounds.entry(t.ident.to_string()).or_default().extend(t.bounds.iter());
            }
            GenericParam::Lifetime(l) => {
                return Err(syn::Error::new_spanned(l, "#[host_fn] can't take lifetime parameters: arguments cross by value"));
            }
            GenericParam::Const(c) => {
                return Err(syn::Error::new_spanned(c, "#[host_fn] can't take const parameters"));
            }
        }
    }
    if let Some(w) = &func.sig.generics.where_clause {
        for pred in &w.predicates {
            let WherePredicate::Type(pt) = pred else {
                return Err(syn::Error::new_spanned(pred, "#[host_fn]: only bounds on its type parameters are supported"));
            };
            let key = pt.bounded_ty.to_token_stream().to_string();
            match bounds.get_mut(&key) {
                Some(list) => list.extend(pt.bounds.iter()),
                None => {
                    return Err(syn::Error::new_spanned(
                        &pt.bounded_ty,
                        "#[host_fn]: a where-bound must be on one of its type parameters",
                    ))
                }
            }
        }
    }

    const KEY_EXTRAS: &[&str] = &["Key", "Opaque", "Ord", "PartialOrd", "Eq", "PartialEq", "Hash", "Clone", "Debug", "Send", "Sync"];
    const OPAQUE_EXTRAS: &[&str] = &["Opaque", "Clone", "Debug", "Send", "Sync"];
    const NUMERIC_EXTRAS: &[&str] = &[
        "Numeric", "Copy", "Clone", "Debug", "Display", "Default", "PartialEq", "PartialOrd", "Send", "Sync",
        "Add", "Sub", "Mul", "Div", "AddAssign", "SubAssign", "MulAssign", "DivAssign", "Sum", "Product",
    ];

    let mut out = Vec::new();
    let mut numeric = 0;
    for id in order {
        let list = &bounds[&id.to_string()];
        let names: Vec<(String, &syn::TraitBound)> = list.iter().filter_map(|b| bound_name(b)).collect();
        for (n, t) in &names {
            if matches!(n.as_str(), "Fn" | "FnMut" | "FnOnce") {
                return Err(syn::Error::new_spanned(
                    t,
                    "#[host_fn] can't take a closure: the app would call back into the bundle for every use, \
                     which is slower than doing the work in the bundle. Pass the data the closure would compute \
                     (a key, a range, a field list) instead",
                ));
            }
        }
        let has = |n: &str| names.iter().any(|(m, _)| m == n);
        let (kind, allowed) = if has("Numeric") {
            (Kind::Numeric, NUMERIC_EXTRAS)
        } else if has("Key") {
            (Kind::Key, KEY_EXTRAS)
        } else if names.iter().all(|(n, _)| OPAQUE_EXTRAS.contains(&n.as_str())) {
            (Kind::Opaque, OPAQUE_EXTRAS)
        } else {
            // Another trait: the app's types, listed.
            (Kind::Listed, &[][..])
        };
        if let Some((n, t)) = names.iter().find(|(n, _)| kind != Kind::Listed && !allowed.contains(&n.as_str())) {
            let hint = match kind {
                Kind::Key => "a `Key` parameter is compared, hashed and cloned as its bytes",
                Kind::Numeric => "a `Numeric` parameter is one of the number types",
                Kind::Opaque | Kind::Listed => unreachable!("an Opaque parameter has only Opaque's bounds"),
            };
            return Err(syn::Error::new_spanned(
                t,
                format!(
                    "#[host_fn]: `{id}: {n}` isn't supported — {hint}, so the app compiles one copy for every \
                     type a bundle could pass; `{n}` would need the bundle's own code"
                ),
            ));
        }
        if kind == Kind::Numeric {
            numeric += 1;
        }
        out.push((id, kind));
    }
    if out.iter().filter(|(_, k)| *k == Kind::Listed).count() > 1 {
        return Err(syn::Error::new_spanned(
            &func.sig.generics,
            "#[host_fn]: at most one parameter bounded by an app trait (each is listed by the app, one instance per type)",
        ));
    }
    if numeric > 2 {
        return Err(syn::Error::new_spanned(
            &func.sig.generics,
            "#[host_fn]: at most two `Numeric` parameters (each multiplies the app's copies of the body by ten)",
        ));
    }
    let bounds = bounds.into_iter().map(|(k, v)| (k, v.into_iter().cloned().collect())).collect();
    Ok((out, bounds))
}

/// Replace each type parameter with `with(ident, kind)` throughout a type.
struct Subst<'a, F: Fn(&syn::Ident, Kind) -> Option<Type>> {
    params: &'a [(syn::Ident, Kind)],
    with: F,
}

impl<F: Fn(&syn::Ident, Kind) -> Option<Type>> VisitMut for Subst<'_, F> {
    fn visit_type_mut(&mut self, ty: &mut Type) {
        if let Type::Path(p) = ty {
            if p.qself.is_none() && p.path.segments.len() == 1 && p.path.segments[0].arguments.is_empty() {
                let id = &p.path.segments[0].ident;
                if let Some((_, kind)) = self.params.iter().find(|(pid, _)| pid == id) {
                    if let Some(new) = (self.with)(id, *kind) {
                        *ty = new;
                        return;
                    }
                }
            }
        }
        syn::visit_mut::visit_type_mut(self, ty);
    }
}

fn subst(ty: &Type, params: &[(syn::Ident, Kind)], with: impl Fn(&syn::Ident, Kind) -> Option<Type>) -> Type {
    let mut ty = ty.clone();
    Subst { params, with }.visit_type_mut(&mut ty);
    ty
}

/// Does `ty` name any of `idents`?
fn mentions(ty: &Type, idents: &[&syn::Ident]) -> bool {
    struct Find<'a> {
        idents: &'a [&'a syn::Ident],
        found: bool,
    }
    impl<'ast> Visit<'ast> for Find<'_> {
        fn visit_ident(&mut self, i: &'ast syn::Ident) {
            if self.idents.contains(&i) {
                self.found = true;
            }
        }
    }
    let mut f = Find { idents, found: false };
    f.visit_type(ty);
    f.found
}

/// The one generic argument of `Vec<X>` / `Option<X>` (`name` given).
fn single_arg<'a>(ty: &'a Type, name: &str) -> Option<&'a Type> {
    let Type::Path(p) = ty else { return None };
    let seg = p.path.segments.last()?;
    if seg.ident != name {
        return None;
    }
    let syn::PathArguments::AngleBracketed(a) = &seg.arguments else { return None };
    match a.args.first()? {
        syn::GenericArgument::Type(t) if a.args.len() == 1 => Some(t),
        _ => None,
    }
}

/// The bundle stub's encoding of a value of `ty` (`expr` is a reference to
/// it) — byte for byte what the app's `RemoteValue` of the substituted
/// type reads: erased parameters framed, everything else as itself.
struct Walk<'a> {
    erased: &'a [(syn::Ident, Kind)],
    v: TokenStream2,
    depth: usize,
}

impl Walk<'_> {
    fn erased_kind(&self, ty: &Type) -> Option<Kind> {
        if let Type::Path(p) = ty {
            if p.qself.is_none() && p.path.segments.len() == 1 && p.path.segments[0].arguments.is_empty() {
                let id = &p.path.segments[0].ident;
                return self.erased.iter().find(|(e, _)| e == id).map(|(_, k)| *k);
            }
        }
        None
    }

    fn has_erased(&self, ty: &Type) -> bool {
        let ids: Vec<&syn::Ident> = self.erased.iter().map(|(i, _)| i).collect();
        mentions(ty, &ids)
    }

    fn encode(&mut self, ty: &Type, expr: TokenStream2) -> syn::Result<TokenStream2> {
        let v = self.v.clone();
        if let Some(kind) = self.erased_kind(ty) {
            return Ok(match kind {
                Kind::Key => quote! { #v::host_fn::encode_key(#expr, __out); },
                _ => quote! { #v::host_fn::encode_opaque(#expr, __out); },
            });
        }
        if !self.has_erased(ty) {
            return Ok(quote! { #v::RemoteValue::encode(#expr, __out); });
        }
        // A list of keys: one run when they have a fixed width.
        if let Some(inner) = single_arg(ty, "Vec") {
            if self.erased_kind(inner) == Some(Kind::Key) {
                return Ok(quote! { #v::host_fn::encode_keys::<#inner>(#expr, __out); });
            }
        }
        self.depth += 1;
        let x = format_ident!("__x{}", self.depth);
        let out = if let Some(inner) = single_arg(ty, "Vec") {
            let body = self.encode(inner, quote!(#x))?;
            quote! {
                #v::__send_value(&(::std::vec::Vec::len(#expr) as u64), __out);
                for #x in ::core::iter::IntoIterator::into_iter(#expr) { #body }
            }
        } else if let Some(inner) = single_arg(ty, "Option") {
            let body = self.encode(inner, quote!(#x))?;
            quote! {
                #v::__send_value(&::core::option::Option::is_some(#expr), __out);
                if let ::core::option::Option::Some(#x) = ::core::option::Option::as_ref(#expr) { #body }
            }
        } else if let Type::Tuple(t) = ty {
            let names: Vec<_> = (0..t.elems.len()).map(|i| format_ident!("__t{}_{}", self.depth, i)).collect();
            let mut parts = Vec::new();
            for (ty, n) in t.elems.iter().zip(&names) {
                parts.push(self.encode(ty, quote!(#n))?);
            }
            quote! { { let ( #(#names,)* ) = #expr; #(#parts)* } }
        } else if let Type::Array(a) = ty {
            let body = self.encode(&a.elem, quote!(#x))?;
            quote! { for #x in ::core::iter::IntoIterator::into_iter(#expr) { #body } }
        } else if let Type::Paren(p) = ty {
            self.encode(&p.elem, expr)?
        } else {
            return Err(unsupported_shape(ty));
        };
        Ok(out)
    }

    /// An expression decoding a value of `ty` from `__input`, in a scope
    /// returning `Result<_, String>`.
    fn decode(&mut self, ty: &Type) -> syn::Result<TokenStream2> {
        let v = self.v.clone();
        if let Some(kind) = self.erased_kind(ty) {
            return Ok(match kind {
                Kind::Key => quote! { #v::host_fn::decode_key::<#ty>(__input)? },
                _ => quote! { #v::host_fn::decode_opaque::<#ty>(__input)? },
            });
        }
        if !self.has_erased(ty) {
            return Ok(quote! { <#ty as #v::RemoteValue>::decode(__input)? });
        }
        if let Some(inner) = single_arg(ty, "Vec") {
            if self.erased_kind(inner) == Some(Kind::Key) {
                return Ok(quote! { #v::host_fn::decode_keys::<#inner>(__input)? });
            }
        }
        self.depth += 1;
        let out = if let Some(inner) = single_arg(ty, "Vec") {
            let body = self.decode(inner)?;
            quote! {{
                let __n = #v::__try_receive_value::<u64>(__input)?;
                let mut __v = ::std::vec::Vec::new();
                for _ in 0..__n { __v.push(#body); }
                __v
            }}
        } else if let Some(inner) = single_arg(ty, "Option") {
            let body = self.decode(inner)?;
            quote! {
                if #v::__try_receive_value::<bool>(__input)? { ::core::option::Option::Some(#body) } else { ::core::option::Option::None }
            }
        } else if let Type::Tuple(t) = ty {
            let mut parts = Vec::new();
            for ty in &t.elems {
                parts.push(self.decode(ty)?);
            }
            quote! { ( #(#parts,)* ) }
        } else if let Type::Array(a) = ty {
            let (elem, len) = (&a.elem, &a.len);
            let body = self.decode(elem)?;
            quote! {{
                let mut __v: ::std::vec::Vec<#elem> = ::std::vec::Vec::new();
                for _ in 0..(#len) { __v.push(#body); }
                <[#elem; #len] as ::core::convert::TryFrom<::std::vec::Vec<#elem>>>::try_from(__v)
                    .map_err(|_| ::std::string::String::from("array length"))?
            }}
        } else if let Type::Paren(p) = ty {
            self.decode(&p.elem)?
        } else {
            return Err(unsupported_shape(ty));
        };
        Ok(out)
    }
}

fn unsupported_shape(ty: &Type) -> syn::Error {
    syn::Error::new_spanned(
        ty,
        "#[host_fn]: a generic parameter can cross as itself or inside `Vec`, `Option`, tuples and arrays; \
         use one of those here (a map crosses as `Vec<(K, V)>`)",
    )
}

pub(crate) fn expand(func: ItemFn) -> syn::Result<TokenStream2> {
    let sig = &func.sig;
    let name = &sig.ident;
    let vis = &func.vis;
    let is_async = sig.asyncness.is_some();
    let (params, bounds) = classify_with_bounds(&func)?;
    let mut arg_names = Vec::new();
    let mut arg_types: Vec<&Type> = Vec::new();
    for arg in &sig.inputs {
        match arg {
            FnArg::Receiver(r) => return Err(syn::Error::new_spanned(r, "#[host_fn] must be a free function")),
            FnArg::Typed(pt) => {
                if let Type::ImplTrait(_) = &*pt.ty {
                    return Err(syn::Error::new_spanned(&pt.ty, "#[host_fn] arguments can't be `impl Trait`: name a type parameter"));
                }
                match &*pt.pat {
                    Pat::Ident(pi) => {
                        arg_names.push(pi.ident.clone());
                        arg_types.push(&pt.ty);
                    }
                    other => return Err(syn::Error::new_spanned(other, "#[host_fn] arguments must be plain identifiers")),
                }
            }
        }
    }
    let ret_ty: Type = match &sig.output {
        ReturnType::Default => syn::parse_quote!(()),
        ReturnType::Type(_, ty) => (**ty).clone(),
    };
    let v = quote!(::runtime_vocabulary::remote);
    let ht = quote!(::runtime_vocabulary::host_types);

    // The type spellings, hashed with FNV-1a: app and bundle may be built
    // by different toolchains, and std's `DefaultHasher` promises no
    // stability across Rust releases. Each part is length-prefixed so
    // `(ab, c)` and `(a, bc)` differ. A type parameter is spelled by its
    // position and kind; a non-generic signature hashes exactly as before
    // generics existed.
    let schema: u64 = {
        let positional = |ty: &Type| {
            let t = subst(ty, &params, |id, kind| {
                let i = params.iter().position(|(p, _)| p == id).expect("a parameter");
                let marker = format_ident!("__{}{}", kind.name(), i);
                Some(syn::parse_quote!(#marker))
            });
            quote!(#t).to_string()
        };
        let mut parts: Vec<String> = arg_types.iter().map(|ty| positional(ty)).collect();
        parts.push(match &sig.output {
            ReturnType::Default => "()".to_string(),
            ReturnType::Type(_, ty) => positional(ty),
        });
        parts.push(if is_async { "async" } else { "sync" }.to_string());
        if !params.is_empty() {
            parts.push(params.iter().map(|(_, k)| k.name()).collect::<Vec<_>>().join(","));
        }
        fnv1a(&parts)
    };
    let schema_hex = format!("{schema:016x}");

    // The signature's structural shape (`remote::shape`), which the
    // release check compares where the schema can't see: a `#[derive(Remote)]`
    // argument that gained a field keeps its spelling. A generic signature's
    // parameters are checked by kind, in the schema, so it has none.
    let shape_expr = if params.is_empty() {
        let head = if is_async { "async fn(" } else { "fn(" };
        let args = arg_types.iter().enumerate().map(|(i, ty)| {
            let sep = if i == 0 { quote!() } else { quote! { __s.push(','); } };
            quote! { #sep __s.push_str(&::runtime_vocabulary::__shape_of!(#ty)); }
        });
        quote! {{
            let mut __s = ::std::string::String::from(#head);
            #(#args)*
            __s.push_str(")->");
            __s.push_str(&::runtime_vocabulary::__shape_of!(#ret_ty));
            __s
        }}
    } else {
        quote!(::std::string::String::from(::runtime_vocabulary::remote::shape::UNKNOWN))
    };
    // The bundle's record of this stub (`remote::site`): kept while the
    // stub is reachable, so the release build lists the host functions the
    // bundle calls with their shapes.
    let stub_site = quote! {
        {
            extern "C" fn __shape() -> u64 {
                #v::site::leak(#shape_expr)
            }
            #v::site::mark(&const {
                #v::site::Site::new(
                    #v::site::HOST_FN,
                    concat!(module_path!(), "::", stringify!(#name), "#", #schema_hex),
                    "",
                    __shape,
                )
            });
        }
    };

    // --- App side ------------------------------------------------------------
    // The arguments and result with each erased parameter replaced by its
    // stand-in; numeric parameters stay generic (the inner call's own).
    let app_subst = |ty: &Type| {
        subst(ty, &params, |_, kind| match kind {
            Kind::Key => Some(syn::parse_quote!(#ht::KeyBytes)),
            Kind::Opaque => Some(syn::parse_quote!(#ht::OpaqueBytes)),
            Kind::Numeric | Kind::Listed => None,
        })
    };
    let app_arg_types: Vec<Type> = arg_types.iter().map(|t| app_subst(t)).collect();
    let numeric: Vec<&syn::Ident> = params.iter().filter(|(_, k)| *k == Kind::Numeric).map(|(i, _)| i).collect();
    let turbofish: TokenStream2 = if params.is_empty() {
        quote!()
    } else {
        let args = params.iter().map(|(id, kind)| match kind {
            Kind::Key => quote!(#ht::KeyBytes),
            Kind::Opaque => quote!(#ht::OpaqueBytes),
            Kind::Numeric | Kind::Listed => quote!(#id),
        });
        quote!(::<#(#args),*>)
    };
    // A listed parameter keeps its own bounds (the body needs them) and
    // crosses as a value.
    let listed: Vec<&syn::Ident> = params.iter().filter(|(_, k)| *k == Kind::Listed).map(|(i, _)| i).collect();
    let listed_decl: Vec<TokenStream2> = listed
        .iter()
        .map(|l| {
            let b = &bounds[&l.to_string()];
            quote!(#l: #(#b +)* #v::RemoteValue)
        })
        .collect();
    let listed_generics = if listed.is_empty() { quote!() } else { quote!(<#(#listed_decl),*>) };
    let inner_generics = if numeric.is_empty() && listed.is_empty() {
        quote!()
    } else {
        quote!(<#(#listed_decl,)* #(#numeric: #ht::Numeric + #v::RemoteValue),*>)
    };
    let listed_args = quote!(#(#listed,)*);

    let call_ident = format_ident!("__host_fn_call_{}", name);
    let inner_ident = format_ident!("__host_fn_inner_{}", name);
    let export_ident = format_ident!("__host_fn_export_{}", name);
    // A bundle's arguments are untrusted: one that does not decode is an
    // `Err`, which stops that bundle (never a panic in the app).
    let decode_args = quote! {
        #(
            let #arg_names: #app_arg_types = <#app_arg_types as #v::RemoteValue>::decode(&mut __input).map_err(|e| {
                ::std::format!(
                    "host_fn `{}`: argument `{}` does not decode ({})",
                    ::core::stringify!(#name), ::core::stringify!(#arg_names), e
                )
            })?;
        )*
    };
    let encode_reply = |value: TokenStream2| {
        quote! {{
            let mut __out = ::std::vec::Vec::new();
            #v::RemoteValue::encode(&#value, &mut __out);
            __out
        }}
    };

    // The tags a numeric call starts with, one per numeric parameter, and
    // the dispatch from them to the inner call's instantiation.
    let numeric_types: Vec<TokenStream2> =
        ["u8", "i8", "u16", "i16", "u32", "i32", "u64", "i64", "f32", "f64"].iter().map(|t| t.parse().unwrap()).collect();
    let dispatch = |call: &dyn Fn(&[&TokenStream2]) -> TokenStream2| -> TokenStream2 {
        fn level(
            depth: usize,
            count: usize,
            chosen: &mut Vec<usize>,
            types: &[TokenStream2],
            ht: &TokenStream2,
            call: &dyn Fn(&[&TokenStream2]) -> TokenStream2,
        ) -> TokenStream2 {
            if depth == count {
                let picked: Vec<&TokenStream2> = chosen.iter().map(|i| &types[*i]).collect();
                return call(&picked);
            }
            let mut arms = Vec::new();
            for i in 0..types.len() {
                chosen.push(i);
                let body = level(depth + 1, count, chosen, types, ht, call);
                chosen.pop();
                let t = &types[i];
                arms.push(quote! { __t if __t == <#t as #ht::Numeric>::TAG => #body, });
            }
            quote! {
                match __tags[#depth] {
                    #(#arms)*
                    __t => ::core::result::Result::Err(::std::format!("host_fn: no number type has tag {}", __t)),
                }
            }
        }
        level(0, numeric.len(), &mut Vec::new(), &numeric_types, &ht, call)
    };
    let n_tags = numeric.len();
    let take_tags = quote! {
        if __args.len() < #n_tags {
            return ::core::result::Result::Err(::std::string::String::from("host_fn: the call is missing its number types"));
        }
        let (__tags, __rest) = __args.split_at(#n_tags);
    };

    let (call_fn, kind) = if is_async {
        let reply = encode_reply(quote!(#name #turbofish (#(#arg_names),*).await));
        let inner = quote! {
            fn #inner_ident #inner_generics (__args: &[u8]) -> ::core::result::Result<
                ::std::pin::Pin<::std::boxed::Box<dyn ::std::future::Future<Output = ::std::vec::Vec<u8>>>>,
                ::std::string::String,
            > {
                let mut __input: &[u8] = __args;
                #decode_args
                ::core::result::Result::Ok(::std::boxed::Box::pin(async move { #reply }))
            }
        };
        let body = if numeric.is_empty() {
            quote! { #inner_ident::<#listed_args>(&__args) }
        } else {
            let d = dispatch(&|ts| quote! { #inner_ident::<#listed_args #(#ts),*>(__rest) });
            quote! { #take_tags #d }
        };
        (
            quote! {
                #inner
                fn #call_ident #listed_generics (__args: ::std::vec::Vec<u8>) -> ::core::result::Result<
                    ::std::pin::Pin<::std::boxed::Box<dyn ::std::future::Future<Output = ::std::vec::Vec<u8>>>>,
                    ::std::string::String,
                > {
                    #body
                }
            },
            quote!(#v::host_fn::HostFnKind::Async(#call_ident::<#listed_args>)),
        )
    } else {
        let reply = encode_reply(quote!(#name #turbofish (#(#arg_names),*)));
        let inner = quote! {
            fn #inner_ident #inner_generics (__args: &[u8]) -> ::core::result::Result<::std::vec::Vec<u8>, ::std::string::String> {
                let mut __input: &[u8] = __args;
                #decode_args
                ::core::result::Result::Ok(#reply)
            }
        };
        let body = if numeric.is_empty() {
            quote! { #inner_ident::<#listed_args>(__args) }
        } else {
            let d = dispatch(&|ts| quote! { #inner_ident::<#listed_args #(#ts),*>(__rest) });
            quote! { #take_tags #d }
        };
        (
            quote! {
                #inner
                fn #call_ident #listed_generics (__args: &[u8]) -> ::core::result::Result<::std::vec::Vec<u8>, ::std::string::String> {
                    #body
                }
            },
            quote!(#v::host_fn::HostFnKind::Sync(#call_ident::<#listed_args>)),
        )
    };

    // --- Bundle side -----------------------------------------------------------
    // Only `Key` and `Opaque` stand in for the bundle's type; a numeric or
    // listed one is a type the app has, crossing as its value.
    let erased: Vec<(syn::Ident, Kind)> =
        params.iter().filter(|(_, k)| matches!(k, Kind::Key | Kind::Opaque)).cloned().collect();
    let mut walk = Walk { erased: &erased, v: v.clone(), depth: 0 };
    let mut encode_each = Vec::new();
    for (n, ty) in arg_names.iter().zip(&arg_types) {
        encode_each.push(walk.encode(ty, quote!(&#n))?);
    }
    let decode_ret = walk.decode(&ret_ty)?;
    let tags = numeric.iter().map(|t| quote! { __out.push(<#t as #ht::Numeric>::TAG); });
    let names = listed.iter().map(|l| quote! { #v::__send_value(&<#l as #v::host_fn::RemoteName>::NAME, __out); });
    let encode_args = quote! {
        let mut __args = ::std::vec::Vec::new();
        {
            let __out = &mut __args;
            #(#names)*
            #(#tags)*
            #(#encode_each)*
        }
    };
    // What a bundle's value of an erased or numeric parameter needs to
    // cross: its own codec (an `Opaque` value is carried as its encoding;
    // a number is one).
    let mut stub_generics = sig.generics.clone();
    for (id, kind) in &params {
        if *kind != Kind::Key {
            stub_generics.make_where_clause().predicates.push(syn::parse_quote!(#id: #v::RemoteValue));
        }
        if *kind == Kind::Listed {
            stub_generics.make_where_clause().predicates.push(syn::parse_quote!(#id: #v::host_fn::RemoteName));
        }
    }
    let (stub_impl_g, _, stub_where) = stub_generics.split_for_impl();
    let param_idents: Vec<&syn::Ident> = params.iter().map(|(i, _)| i).collect();
    let param_turbofish = if params.is_empty() { quote!() } else { quote!(::<#(#param_idents),*>) };

    let attrs = &func.attrs;
    let block = &func.block;
    let inputs = &sig.inputs;
    let stub = if is_async {
        quote! {
            #(#attrs)*
            #vis fn #name #stub_impl_g (#inputs) -> #v::bundle::HostFuture<#ret_ty> #stub_where {
                #[link(wasm_import_module = "idealyst_host_fn")]
                unsafe extern "C" {
                    #[link_name = concat!(module_path!(), "::", stringify!(#name), "#", #schema_hex)]
                    fn __import(args_ptr: *const u8, args_len: u32, then: u32);
                }
                #stub_site
                #encode_args
                fn __decode #stub_impl_g (__bytes: &[u8]) -> ::core::option::Option<#ret_ty> #stub_where {
                    let mut __bytes = __bytes;
                    let __input = &mut __bytes;
                    (|| -> ::core::result::Result<#ret_ty, ::std::string::String> {
                        ::core::result::Result::Ok(#decode_ret)
                    })().ok()
                }
                // SAFETY: the app copies `args_len` bytes at `args_ptr`
                // during the call.
                #v::bundle::HostFuture::start(stringify!(#name), __args, __decode #param_turbofish, |ptr, len, then| unsafe {
                    __import(ptr, len, then)
                })
            }
        }
    } else {
        quote! {
            #(#attrs)*
            #vis fn #name #stub_impl_g (#inputs) -> #ret_ty #stub_where {
                #[link(wasm_import_module = "idealyst_host_fn")]
                unsafe extern "C" {
                    #[link_name = concat!(module_path!(), "::", stringify!(#name), "#", #schema_hex)]
                    fn __import(args_ptr: *const u8, args_len: u32) -> i64;
                }
                #stub_site
                #encode_args
                // SAFETY: as above; the reply is left in the bundle's
                // argument buffer.
                let __reply = #v::bundle::host_fn_sync(&__args, |ptr, len| unsafe { __import(ptr, len) });
                let mut __bytes: &[u8] = &__reply;
                let __input = &mut __bytes;
                (|| -> ::core::result::Result<#ret_ty, ::std::string::String> {
                    ::core::result::Result::Ok(#decode_ret)
                })().unwrap_or_else(|e| panic!(
                    "host_fn `{}`: the reply does not decode ({}) — the load-time schema check should have refused this bundle",
                    stringify!(#name), e
                ))
            }
        }
    };

    let export_items = if listed.is_empty() {
        quote! {
            #[doc(hidden)]
            #[allow(non_snake_case)]
            fn #export_ident() -> #v::HostFnDef {
                #v::HostFnDef {
                    path: concat!(module_path!(), "::", stringify!(#name)),
                    schema: #schema,
                    kind: #kind,
                    shape: {
                        fn __shape() -> ::std::string::String {
                            #shape_expr
                        }
                        __shape
                    },
                }
            }

            /// The allowlist record for this host function: list it
            /// in `install_with` to let remote code call it.
            #[allow(non_snake_case)]
            #vis mod #name {
                pub fn export() -> ::runtime_vocabulary::remote::HostFnDef {
                    super::#export_ident()
                }
            }
        }
    } else {
        // The app lists the types (`host_fn_instances!`): one instance
        // each, and the record dispatching to them by name. Declared beside
        // the function, where its bounds' paths resolve, and re-exported
        // under its name.
        let instance_ident = format_ident!("__host_fn_instance_{}", name);
        let listed_ident = format_ident!("__host_fn_export_listed_{}", name);
        let reexport_vis = match vis {
            syn::Visibility::Inherited => quote!(pub(super)),
            other => quote!(#other),
        };
        quote! {
            #[doc(hidden)]
            #[allow(non_snake_case)]
            #vis fn #instance_ident #listed_generics () -> #v::host_fn::HostFnKind {
                #kind
            }

            #[doc(hidden)]
            #[allow(non_snake_case)]
            #vis fn #listed_ident(lookup: fn(&str) -> ::core::option::Option<#v::host_fn::HostFnKind>) -> #v::HostFnDef {
                #v::HostFnDef {
                    path: concat!(module_path!(), "::", stringify!(#name)),
                    schema: #schema,
                    kind: #v::host_fn::HostFnKind::Listed { asynchronous: #is_async, lookup },
                    shape: {
                        fn __shape() -> ::std::string::String {
                            #shape_expr
                        }
                        __shape
                    },
                }
            }

            /// The allowlist record for this host function, per type:
            /// `host_fn_instances!(path::to::this_fn: TypeA, TypeB)`.
            #[allow(non_snake_case)]
            #vis mod #name {
                #[doc(hidden)]
                #reexport_vis use super::#instance_ident as instance;
                #[doc(hidden)]
                #reexport_vis use super::#listed_ident as export_listed;
            }
        }
    };

    Ok(quote! {
        ::runtime_vocabulary::__remote_guest_split! {
            bundle: { #stub }
            app: {
                #(#attrs)*
                #vis #sig #block

                // The allowlist record, only where remote components are
                // hosted (an app without them gets just the function).
                ::runtime_vocabulary::__remote_enabled! {
                    #[doc(hidden)]
                    #[allow(non_snake_case)]
                    #call_fn

                    #export_items
                }
            }
        }
    })
}

/// 64-bit FNV-1a over `parts`, each prefixed with its length.
fn fnv1a(parts: &[String]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |bytes: &[u8]| {
        for b in bytes {
            h ^= *b as u64;
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    };
    for p in parts {
        eat(&(p.len() as u64).to_le_bytes());
        eat(p.as_bytes());
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The schema is a wire contract between separately built binaries:
    /// pinned, so a change to how it is computed is a deliberate one.
    #[test]
    fn the_schema_hash_is_pinned() {
        assert_eq!(fnv1a(&["u32".into(), "bool".into(), "sync".into()]), 0x4668_ca41_4944_a897);
        assert_ne!(fnv1a(&["ab".into(), "c".into()]), fnv1a(&["a".into(), "bc".into()]));
    }

    fn schema_of(f: ItemFn) -> String {
        let out = expand(f).unwrap().to_string();
        let at = out.find("schema :").expect("a schema");
        out[at..].split(',').next().unwrap().to_string()
    }

    /// Generics didn't change a non-generic function's fingerprint (old
    /// bundles keep loading).
    #[test]
    fn a_non_generic_schema_is_unchanged() {
        let f: ItemFn = syn::parse_quote! { pub fn f(a: u32, b: bool) -> u32 { 1 } };
        let want = fnv1a(&["u32".into(), "bool".into(), "u32".into(), "sync".into()]);
        assert!(schema_of(f).contains(&format!("{want}u64")));
    }

    /// A type parameter is spelled by position and kind: renaming it keeps
    /// the fingerprint, changing its kind doesn't.
    #[test]
    fn a_generic_schema_follows_kinds_not_names() {
        let a = schema_of(syn::parse_quote! { fn order<K: Key>(keys: Vec<K>) -> Vec<u32> { todo!() } });
        let b = schema_of(syn::parse_quote! { fn order<T: Key>(keys: Vec<T>) -> Vec<u32> { todo!() } });
        let c = schema_of(syn::parse_quote! { fn order<T: Opaque>(keys: Vec<T>) -> Vec<u32> { todo!() } });
        let d = schema_of(syn::parse_quote! { fn order<T: Numeric>(keys: Vec<T>) -> Vec<u32> { todo!() } });
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_ne!(a, d);
        assert_ne!(c, d);
    }

    #[test]
    fn a_host_fn_splits_by_build_through_the_vocabulary() {
        let f: ItemFn = syn::parse_quote! { pub fn set_scheme(dark: bool) -> u32 { 1 } };
        let out = expand(f).unwrap().to_string();
        assert!(out.contains("__remote_guest_split"), "{out}");
        assert!(out.contains("__remote_enabled"), "{out}");
        assert!(!out.contains("cfg ("), "no raw cfg in the defining crate: {out}");
        assert!(out.contains("RemoteValue"), "{out}");
    }

    #[test]
    fn parameters_are_classified_by_their_bounds() {
        let f: ItemFn = syn::parse_quote! {
            fn f<K: Key + Clone, V, N: Numeric, W, S: Area + Clone>(a: Vec<(K, V)>, n: Vec<N>, w: W, s: S) -> u32 where W: Opaque + Send { 0 }
        };
        let kinds: Vec<Kind> = classify(&f).unwrap().into_iter().map(|(_, k)| k).collect();
        assert_eq!(kinds, [Kind::Key, Kind::Opaque, Kind::Numeric, Kind::Opaque, Kind::Listed]);
    }

    /// A parameter bounded by an app trait gets an instance per listed
    /// type, keeping its bounds, and a record dispatching by name.
    #[test]
    fn an_app_trait_parameter_is_listed_by_the_app() {
        let f: ItemFn = syn::parse_quote! { pub fn total_area<S: Area>(shapes: Vec<S>) -> f64 { 0.0 } };
        let out = expand(f).unwrap().to_string();
        assert!(out.contains("fn __host_fn_instance_total_area < S : Area + :: runtime_vocabulary :: remote :: RemoteValue >"), "{out}");
        assert!(out.contains("HostFnKind :: Listed { asynchronous : false , lookup }"), "{out}");
        assert!(out.contains("use super :: __host_fn_instance_total_area as instance"), "{out}");
        assert!(out.contains("RemoteName > :: NAME"), "the bundle names its type: {out}");
        // Regression: the stub framed a listed value as an `Opaque` (a
        // length before each), which the app, decoding the type itself,
        // misread: every shape's fields came out as garbage.
        assert!(!out.contains("encode_opaque"), "a listed value crosses as itself: {out}");
    }

    /// In the app, `Key` becomes `KeyBytes`, `Opaque` `OpaqueBytes`, and a
    /// numeric parameter is dispatched by tag over all ten types.
    #[test]
    fn the_app_instantiates_stand_ins_and_dispatches_numbers() {
        let f: ItemFn = syn::parse_quote! { fn group<K: Key, V: Opaque, N: Numeric>(rows: Vec<(K, V)>, w: Vec<N>) -> Vec<(K, Vec<V>)> { todo!() } };
        let out = expand(f).unwrap().to_string();
        assert!(out.contains("group :: < :: runtime_vocabulary :: host_types :: KeyBytes , :: runtime_vocabulary :: host_types :: OpaqueBytes , N >"), "{out}");
        for t in ["u8", "i8", "u16", "i16", "u32", "i32", "u64", "i64", "f32", "f64"] {
            assert!(out.contains(&format!("__host_fn_inner_group :: < {t} >")), "{t}: {out}");
        }
    }

    fn err(f: ItemFn) -> String {
        expand(f).err().expect("refused").to_string()
    }

    #[test]
    fn closures_and_other_bounds_are_refused_with_a_reason() {
        assert!(err(syn::parse_quote! { fn f<T, F: Fn(&T) -> bool>(v: Vec<T>, keep: F) {} }).contains("closure"));
        assert!(err(syn::parse_quote! { fn f<A: Display, B: Area>(a: A, b: B) {} }).contains("at most one parameter bounded by an app trait"));
        assert!(err(syn::parse_quote! { fn f<K: Key + Default>(v: K) {} }).contains("`K: Default` isn't supported"));
        assert!(err(syn::parse_quote! { fn f<'a>(v: &'a str) {} }).contains("lifetime"));
        assert!(err(syn::parse_quote! { fn f<const N: usize>(v: [u8; N]) {} }).contains("const"));
        assert!(err(syn::parse_quote! { fn f(v: impl Key) {} }).contains("impl Trait"));
        assert!(err(syn::parse_quote! { fn f<A: Numeric, B: Numeric, C: Numeric>(a: A, b: B, c: C) {} }).contains("at most two"));
    }

    /// An erased parameter inside a type the stub can't walk is refused,
    /// naming the alternative.
    #[test]
    fn an_erased_parameter_in_an_unwalkable_shape_is_refused() {
        let e = err(syn::parse_quote! { fn f<K: Key>(m: HashMap<K, u32>) {} });
        assert!(e.contains("Vec<(K, V)>"), "{e}");
        // Numbers cross as themselves anywhere: no walk needed.
        assert!(expand(syn::parse_quote! { fn f<N: Numeric>(m: Option<N>) -> (N, u8) { todo!() } }).is_ok());
    }
}
