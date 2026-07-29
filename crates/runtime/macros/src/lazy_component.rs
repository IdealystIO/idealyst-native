//! Lazy-component glue: the `BuildElement` a `#[component(lazy)]` /
//! `#[lazy]` component gets instead of the eager one.
//!
//! Where an eager component's `build(self)` calls the component fn inline,
//! a lazy component's `build(self)` compiles the fn's body into a separate
//! wasm chunk (via a nested `#[wasm_split]` async fn) and returns an
//! `Element::Lazy` that loads it on first mount. The component's props cross
//! the chunk boundary as the loader's argument, so runtime input Just Works —
//! no separate "typed args" surface.
//!
//! The generated props struct gains two config fields the component fn never
//! sees — `loading` and `error` — typed as [`LazyLoadingUi`] / [`LazyErrorUi`]
//! so the `ui!` call site can pass an ordinary closure
//! (`Profile(id = 5, loading = || ui! { … })`).
//!
//! Retry: `#[component(lazy)]` moves the props into the loader exactly once
//! (no `Clone` bound) — the error UI is message-only. `#[component(lazy,
//! retryable)]` derives `Clone` on the props so the loader can be re-invoked,
//! enabling `LazyError::retry()`.

use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::{Attribute, Ident, Visibility};

/// Everything `emit_inline_lazy_glue` needs to build a lazy component's props
/// struct + `BuildElement`. Fields mirror the eager inline-props glue plus the
/// component's argument names (for the chunk body) and the `retryable` flag.
pub(crate) struct LazyGlue<'a> {
    pub(crate) fn_name: &'a Ident,
    pub(crate) props_ident: &'a Ident,
    pub(crate) vis: &'a Visibility,
    pub(crate) fn_docs: &'a [&'a Attribute],
    pub(crate) struct_doc: &'a str,
    /// Pre-rendered `pub #name: #ty,` field declarations for the component's
    /// own props (the fn parameters).
    pub(crate) field_defs: Vec<TokenStream2>,
    /// Pre-rendered `#name: <default>,` entries for those same fields.
    pub(crate) default_fields: Vec<TokenStream2>,
    /// The component fn's parameter names, in order — read off `props` inside
    /// the chunk body to call the fn.
    pub(crate) arg_names: &'a [&'a Ident],
    pub(crate) retryable: bool,
}

pub(crate) fn emit_inline_lazy_glue(g: LazyGlue<'_>) -> TokenStream2 {
    let LazyGlue {
        fn_name,
        props_ident,
        vis,
        fn_docs,
        struct_doc,
        field_defs,
        default_fields,
        arg_names,
        retryable,
    } = g;

    // Chunk boundary marker + a readable chunk filename (`…_lazy_Profile.wasm`
    // instead of a content hash).
    let split_name = format_ident!("__idealyst_lazy_{}", fn_name);

    // Derive `Clone` only when retry is opted in — that's the sole reason the
    // loader needs to reproduce the props, and the only thing that imposes a
    // `Clone` bound on the author's prop fields.
    let clone_derive = if retryable {
        quote! { #[derive(::core::clone::Clone)] }
    } else {
        quote! {}
    };

    // The loader closure. `lazy_split` wants `Fn() -> LazyFuture`, so:
    //   - retryable: props are `Clone`; clone per call so retry can re-run.
    //   - one-shot: move props into a take-once cell; a second call (which only
    //     happens if something drives retry without `retryable`) panics loudly.
    // The loader is bound to a `let` before being handed to `lazy_split`, so
    // the closure's return type is pinned to the concrete future — the unsize
    // coercion to `LazyFuture` (`Pin<Box<dyn Future>>`) has to be forced with an
    // explicit annotation at the `Box::pin` site, or inference keeps the
    // concrete type and the `Fn() -> LazyFuture` bound fails.
    let loader_expr = if retryable {
        quote! {
            {
                let __props = self;
                move || -> ::runtime_core::primitives::lazy::LazyFuture {
                    let __p = ::core::clone::Clone::clone(&__props);
                    ::std::boxed::Box::pin(async move {
                        ::core::result::Result::Ok(__lazy_body(__p).await)
                    })
                }
            }
        }
    } else {
        quote! {
            {
                let __cell = ::std::cell::RefCell::new(::core::option::Option::Some(self));
                move || -> ::runtime_core::primitives::lazy::LazyFuture {
                    let __p = __cell
                        .borrow_mut()
                        .take()
                        .expect(
                            "lazy component loader invoked twice without `retryable` — \
                             add `#[component(lazy, retryable)]` to enable retry()",
                        );
                    ::std::boxed::Box::pin(async move {
                        ::core::result::Result::Ok(__lazy_body(__p).await)
                    })
                }
            }
        }
    };

    let loading_doc = "Loading UI shown while this component's chunk loads. \
                       A `Fn() -> impl IntoElement` closure. Default: empty.";
    let error_doc = "Error UI shown if this component's chunk fails to load. \
                     A `Fn(&LazyError) -> impl IntoElement` closure; the error \
                     carries `.message()` and `.retry()`. Default: log + keep loading.";

    // The chunk body fn. OLD core: constructs the component's `Element`
    // inside the loader future (the walker's `ScopedLoad` re-enters the
    // chunk scope per poll so construction-time reactive state has an
    // owner). NEW core: a future poll runs on the executor with NO world
    // entered — constructing there would panic in `component_scope` — so
    // `__lazy_body` returns a **body thunk** (`LazyBodyThunk`) and the
    // vocabulary's mount handler invokes it inside its swap effect
    // (world entered, `component_scope`-collected). Same `#[wasm_split]`
    // module + fn naming either way, so wasm-split-cli's chunk
    // classification is identical across cores. The `::runtime_core::…`
    // paths below are retargeted to `runtime_vocabulary::glue::…` under
    // `new-core` (glue::primitives::lazy defines the thunk types).
    #[cfg(not(feature = "new-core"))]
    let lazy_body_fn = quote! {
        #[::runtime_core::__wasm_split::wasm_split(#split_name)]
        async fn __lazy_body(props: #props_ident) -> ::runtime_core::Element {
            ::runtime_core::IntoElement::into_element(
                #fn_name(#(props.#arg_names),*)
            )
        }
    };
    #[cfg(feature = "new-core")]
    let lazy_body_fn = quote! {
        #[::runtime_core::__wasm_split::wasm_split(#split_name)]
        async fn __lazy_body(
            props: #props_ident,
        ) -> ::runtime_core::primitives::lazy::LazyBodyThunk {
            // Explicitly typed binding: the wasm `#[wasm_split]`
            // expansion re-homes these statements inside a
            // `Box::pin(async move { … })`, where the async block's
            // tail has NO return-type coercion context — an
            // unannotated `Box::new(closure)` stays `Box<{closure}>`
            // and fails the `Box<dyn FnOnce() -> Element>` output.
            let __thunk: ::runtime_core::primitives::lazy::LazyBodyThunk =
                ::std::boxed::Box::new(move || {
                    ::runtime_core::IntoElement::into_element(
                        #fn_name(#(props.#arg_names),*)
                    )
                });
            __thunk
        }
    };

    quote! {
        #[doc = #struct_doc]
        #clone_derive
        #[allow(non_camel_case_types)]
        #vis struct #props_ident {
            #(#field_defs)*
            #[doc = #loading_doc]
            pub loading: ::runtime_core::primitives::lazy::LazyLoadingUi,
            #[doc = #error_doc]
            pub error: ::runtime_core::primitives::lazy::LazyErrorUi,
        }

        #[automatically_derived]
        impl ::core::default::Default for #props_ident {
            fn default() -> Self {
                Self {
                    #(#default_fields)*
                    loading: ::core::default::Default::default(),
                    error: ::core::default::Default::default(),
                }
            }
        }

        #(#fn_docs)*
        #[allow(non_camel_case_types)]
        #vis type #fn_name = #props_ident;

        #[automatically_derived]
        impl ::runtime_core::BuildElement for #props_ident {
            fn build(self) -> ::runtime_core::Element {
                // The `#[wasm_split]` attribute expands to `wasm_split::…`
                // paths; alias runtime-core's re-export so the author crate
                // needs no direct `wasm-split` dep. `#[allow(unused_imports)]`
                // covers the native expansion, which names no `wasm_split`.
                #[allow(unused_imports)]
                use ::runtime_core::__wasm_split as wasm_split;

                // The component's body — hoisted into a chunk. Receives the
                // props (config fields ride along, unused) and calls the fn
                // (old core: builds the Element; new core: returns the body
                // thunk — see `lazy_body_fn` above).
                #lazy_body_fn

                // Pull the loading/error config out (cheap `Rc` clones) before
                // the props move into the loader.
                let __loading = ::core::clone::Clone::clone(&self.loading).__into_handler();
                let __error = ::core::clone::Clone::clone(&self.error).__into_handler();

                let __loader = #loader_expr;
                let mut __b = ::runtime_core::primitives::lazy::lazy_split(__loader);
                if let ::core::option::Option::Some(__l) = __loading {
                    __b = __b.placeholder(move || __l());
                }
                if let ::core::option::Option::Some(__e) = __error {
                    __b = __b.on_error(move |__err| __e(__err));
                }
                ::runtime_core::IntoElement::into_element(__b)
            }
        }
    }
}
