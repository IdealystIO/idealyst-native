use proc_macro::TokenStream;

use digest::Digest;
use quote::{format_ident, quote};
use syn::{FnArg, Ident, ItemFn, ReturnType, Signature, parse_macro_input, parse_quote};

#[proc_macro_attribute]
pub fn wasm_split(args: TokenStream, input: TokenStream) -> TokenStream {
    let module_ident = parse_macro_input!(args as Ident);
    let item_fn = parse_macro_input!(input as ItemFn);

    if item_fn.sig.asyncness.is_none() {
        panic!(
            "wasm_split functions must be async. Use a LazyLoader with synchronous functions instead."
        );
    }

    let LoaderNames {
        split_loader_ident,
        impl_import_ident,
        impl_export_ident,
        load_module_ident,
        ..
    } = LoaderNames::new(item_fn.sig.ident.clone(), module_ident.to_string());

    let mut desugard_async_sig = item_fn.sig.clone();
    desugard_async_sig.asyncness = None;
    desugard_async_sig.output = match &desugard_async_sig.output {
        ReturnType::Default => {
            parse_quote! { -> ::std::pin::Pin<Box<dyn ::std::future::Future<Output = ()>>> }
        }
        ReturnType::Type(_, ty) => {
            parse_quote! { -> ::std::pin::Pin<Box<dyn ::std::future::Future<Output = #ty>>> }
        }
    };

    let import_sig = Signature {
        ident: impl_import_ident.clone(),
        ..desugard_async_sig.clone()
    };

    let export_sig = Signature {
        ident: impl_export_ident.clone(),
        ..desugard_async_sig.clone()
    };

    let default_item = item_fn.clone();

    let mut wrapper_sig = item_fn.sig;
    wrapper_sig.asyncness = Some(Default::default());

    let mut args = Vec::new();
    for (i, param) in wrapper_sig.inputs.iter_mut().enumerate() {
        match param {
            syn::FnArg::Receiver(_) => args.push(format_ident!("self")),
            syn::FnArg::Typed(pat_type) => {
                let param_ident = format_ident!("__wasm_split_arg_{i}");
                args.push(param_ident.clone());
                *pat_type.pat = syn::Pat::Ident(syn::PatIdent {
                    attrs: vec![],
                    by_ref: None,
                    mutability: None,
                    ident: param_ident,
                    subpat: None,
                });
            }
        }
    }

    let attrs = &item_fn.attrs;
    let stmts = &item_fn.block.stmts;

    quote! {
        #[cfg(target_arch = "wasm32")]
        #wrapper_sig {
            #(#attrs)*
            #[allow(improper_ctypes_definitions)]
            #[unsafe(no_mangle)]
            pub unsafe extern "C" #export_sig {
                Box::pin(async move { #(#stmts)* })
            }

            #[link(wasm_import_module = "./__wasm_split.js")]
            unsafe extern "C" {
                #[unsafe(no_mangle)]
                fn #load_module_ident (
                    callback: unsafe extern "C" fn(*const ::std::ffi::c_void, bool),
                    data: *const ::std::ffi::c_void
                );

                #[allow(improper_ctypes)]
                #[unsafe(no_mangle)]
                #import_sig;
            }

            thread_local! {
                static #split_loader_ident: wasm_split::LazySplitLoader = unsafe {
                    wasm_split::LazySplitLoader::new(#load_module_ident)
                };
            }

            // Initiate the download by calling the load_module_ident function which will kick-off the loader
            if !wasm_split::LazySplitLoader::ensure_loaded(&#split_loader_ident).await {
                panic!("Failed to load wasm-split module");
            }

            unsafe { #impl_import_ident( #(#args),* ) }.await
        }

        #[cfg(not(target_arch = "wasm32"))]
        #default_item
    }
    .into()
}

/// Create a lazy loader for a given function. Meant to be used in statics. Designed for libraries to
/// integrate with.
///
/// ```rust, ignore
/// fn SomeFunction(args: Args) -> Ret {}
///
/// static LOADER: wasm_split::LazyLoader<Args, Ret> = lazy_loader!(SomeFunction);
///
/// LOADER.load().await.call(args)
/// ```
#[proc_macro]
pub fn lazy_loader(input: TokenStream) -> TokenStream {
    // We can only accept idents/paths that will be the source function
    let sig = parse_macro_input!(input as Signature);
    let params = sig.inputs.clone();
    let outputs = sig.output.clone();
    let Some(FnArg::Typed(arg)) = params.first().cloned() else {
        panic!(
            "Lazy Loader must define a single input argument to satisfy the LazyLoader signature"
        )
    };
    let arg_ty = arg.ty.clone();
    let LoaderNames {
        name,
        split_loader_ident,
        impl_import_ident,
        impl_export_ident,
        load_module_ident,
        ..
    } = LoaderNames::new(
        sig.ident.clone(),
        sig.abi
            .as_ref()
            .and_then(|abi| abi.name.as_ref().map(|f| f.value()))
            .expect("abi to be module name")
            .to_string(),
    );

    quote! {
        {
            #[cfg(target_arch = "wasm32")]
            {
                #[link(wasm_import_module = "./__wasm_split.js")]
                unsafe extern "C" {
                    // The function we'll use to initiate the download of the module
                    #[unsafe(no_mangle)]
                    fn #load_module_ident(
                        callback: unsafe extern "C" fn(*const ::std::ffi::c_void, bool),
                        data: *const ::std::ffi::c_void,
                    );

                    #[allow(improper_ctypes)]
                    #[unsafe(no_mangle)]
                    fn #impl_import_ident(arg: #arg_ty) #outputs;
                }


                #[allow(improper_ctypes_definitions)]
                #[unsafe(no_mangle)]
                pub unsafe extern "C" fn #impl_export_ident(arg: #arg_ty) #outputs {
                    #name(arg)
                }

                thread_local! {
                    static #split_loader_ident: wasm_split::LazySplitLoader = unsafe {
                        wasm_split::LazySplitLoader::new(#load_module_ident)
                    };
                };

                unsafe {
                    wasm_split::LazyLoader::new(#impl_import_ident, &#split_loader_ident)
                }
            }

            #[cfg(not(target_arch = "wasm32"))]
            {
                wasm_split::LazyLoader::preloaded(#name)
            }
        }
    }
    .into()
}

struct LoaderNames {
    name: Ident,
    split_loader_ident: Ident,
    impl_import_ident: Ident,
    impl_export_ident: Ident,
    load_module_ident: Ident,
}

impl LoaderNames {
    fn new(name: Ident, module: String) -> Self {
        // The disambiguator must be UNIQUE per split function and STABLE
        // across unrelated edits. Upstream hashed `format!("{span:?}")`,
        // and a proc-macro `Span`'s debug form is its absolute byte range
        // in the crate-wide source map — so appending one comment line to
        // any file parsed earlier renamed every split export parsed after
        // it. Each rename is a new `DefPath`, which re-runs the crate-wide
        // reachability set, which turns `is_reachable_non_generic` red for
        // every function, which invalidates nearly every codegen unit:
        // measured on CrewForge (17 lazy areas, wasm32), a one-byte edit
        // re-codegened 238 of 256 CGUs and the rebuilt objects were
        // byte-identical. Native builds never emit this glue, which is why
        // the same edit reused 255 CGUs there.
        //
        // `(module, name, file)` is unique for every real call site — two
        // `#[wasm_split(m)] async fn f` in one file is already a duplicate
        // item — and only changes when the function itself is renamed or
        // moved to another file. Line and column are deliberately left out:
        // an edit above the function in its own file must not rename it.
        // `Span::file` needs proc-macro2's `span-locations` feature; in
        // fallback mode (unit tests) it is a per-parse placeholder, so the
        // byte-shift regression test below parses both copies of the
        // function from one string.
        let file = name.span().file();
        let unique_identifier = base16::encode_lower(
            &sha2::Sha256::digest(format!("{module} {name} {file}"))[..16],
        );

        Self {
            split_loader_ident: format_ident!("__wasm_split_loader_{module}"),
            impl_export_ident: format_ident!(
                "__wasm_split_00___{module}___00_export_{unique_identifier}_{name}"
            ),
            impl_import_ident: format_ident!(
                "__wasm_split_00___{module}___00_import_{unique_identifier}_{name}"
            ),
            load_module_ident: format_ident!(
                "__wasm_split_load_{module}_{unique_identifier}_{name}"
            ),
            name,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::LoaderNames;
    use proc_macro2::TokenStream;
    use std::str::FromStr;

    fn fn_idents(src: &str) -> Vec<syn::Ident> {
        let file: syn::File = syn::parse2(TokenStream::from_str(src).unwrap()).unwrap();
        file.items
            .into_iter()
            .map(|item| match item {
                syn::Item::Fn(f) => f.sig.ident,
                _ => panic!("expected only fn items"),
            })
            .collect()
    }

    /// The bug: upstream derived the export name from the ident's absolute
    /// byte position, so the same function parsed after more source (which
    /// is what an edit to any earlier file does to every later file) got a
    /// different symbol. Two copies of one function in one file, at
    /// different byte offsets, must name the same export. (One string,
    /// because proc-macro2's fallback `Span::file()` is a per-parse
    /// placeholder; in the compiler it is the real path.)
    #[test]
    fn regression_export_ident_stable_across_byte_shift() {
        let idents = fn_idents(
            "async fn __lazy_body() {}\n// an edit above shifts what follows\nasync fn __lazy_body() {}",
        );
        let [first, second]: [syn::Ident; 2] = idents.try_into().unwrap();
        assert_ne!(
            format!("{:?}", first.span()),
            format!("{:?}", second.span()),
            "test setup: the two copies must sit at different byte offsets"
        );
        assert_eq!(first.span().file(), second.span().file());

        let a = LoaderNames::new(first, "__idealyst_lazy_Panel".into());
        let b = LoaderNames::new(second, "__idealyst_lazy_Panel".into());
        assert_eq!(a.impl_export_ident, b.impl_export_ident);
        assert_eq!(a.impl_import_ident, b.impl_import_ident);
        assert_eq!(a.load_module_ident, b.load_module_ident);
    }

    /// Uniqueness still holds where it is needed: a different split module
    /// or a different function name is a different export.
    #[test]
    fn export_ident_varies_by_module_and_name() {
        let [f, g]: [syn::Ident; 2] =
            fn_idents("async fn f() {}\nasync fn g() {}").try_into().unwrap();
        let same = LoaderNames::new(f.clone(), "m".into());
        let other_module = LoaderNames::new(f, "n".into());
        let other_name = LoaderNames::new(g, "m".into());
        assert_ne!(same.impl_export_ident, other_module.impl_export_ident);
        assert_ne!(same.impl_export_ident, other_name.impl_export_ident);
    }
}
