//! Framework procedural macros.
//!
//! Two macros are exported:
//!
//! - `#[component]` (attribute) — rewrites a function body for reactivity
//!   and generates a sibling `name!` invocation macro for the component.
//!   See [`component_attr`], [`reactivity`], and [`invocation_macro`].
//!
//! - `ui!` (function-like) — JSX-style DSL for composing components.
//!   Parses `Name(prop = value) { children }` and desugars to plain Rust
//!   calls / per-component `name!` invocations. See [`ui`].
//!
//! ## Heuristics, limitations
//!
//! - Reactivity is detected by `.get()` calls; false positives on
//!   `HashMap::get()` waste work but don't break anything.
//! - `text` and `button` are recognized by literal name only; renamed
//!   imports or fully-qualified paths are not detected.
//! - `vec![...]` and `children![...]` are special-cased; other list-shaped
//!   macros are opaque to the reactivity rewriter.

mod component_attr;
// Always compiled (so its unit tests run on a default `cargo test`), but
// its helpers are only *called* under `strict-docs` — suppress dead-code
// when the feature is off.
#[cfg_attr(not(feature = "strict-docs"), allow(dead_code))]
mod doc_check;
// Like `doc_check`: always compiled so its unit tests run, but its helper
// is only called under `strict-naming` — suppress dead-code when off.
#[cfg_attr(not(feature = "strict-naming"), allow(dead_code))]
mod naming_check;
mod invocation_macro;
mod jsx;
mod lazy;
#[cfg(feature = "catalog")]
mod external_emit;
#[cfg(feature = "catalog")]
mod mcp_emit;
#[cfg(feature = "catalog")]
mod schema_emit;
#[cfg(feature = "catalog")]
mod tool_emit;
#[cfg(feature = "catalog")]
mod recipe_emit;
#[cfg(feature = "catalog")]
mod scope_emit;
mod methods_block;
mod path_analysis;
mod primitives;
mod props_attr;
mod reactivity;
mod stylesheet;
mod text_fmt;
mod ui;

use proc_macro::TokenStream;
use quote::quote;
use syn::parse_macro_input;
use syn::ItemFn;

/// `#[derive(IdealystSchema)]` — registers a props struct's per-field
/// information into the MCP catalog. Used alongside `#[component]`
/// on the struct that the component takes as its props parameter.
/// Recognises `#[schema(constraint = "...")]` field attributes for
/// free-form constraint hints (spec §4.3).
///
/// With the `catalog` feature on, registers the struct's per-field schema
/// into the catalog. With `strict-docs` on, additionally requires a doc
/// comment on every named field / enum variant (a missing one is a
/// `compile_error!`). With neither feature this derive expands to
/// nothing.
#[proc_macro_derive(IdealystSchema, attributes(schema))]
pub fn derive_idealyst_schema(input: TokenStream) -> TokenStream {
    let parsed = parse_macro_input!(input as syn::DeriveInput);
    let mut out = proc_macro2::TokenStream::new();
    // `strict-docs`: one `compile_error!` per undocumented prop/variant.
    #[cfg(feature = "strict-docs")]
    out.extend(doc_check::require_schema_docs(&parsed));
    // `mcp`: the inventory registration carrying the field docs to the
    // catalog. `emit` consumes the input, so clone — `strict-docs` may
    // have already borrowed it above.
    #[cfg(feature = "catalog")]
    out.extend(schema_emit::emit(parsed.clone()));
    // Keep `parsed` "used" when neither feature touched it.
    let _ = &parsed;
    out.into()
}

/// `#[idealyst_tool]` — register a standalone function as an MCP
/// tool (spec §4.2). The function body is left unchanged; the
/// attribute only emits an `inventory::submit!` of a `ToolEntry`
/// alongside it. When the `catalog` feature is off the attribute is a
/// no-op (function emitted unchanged, no registration).
#[proc_macro_attribute]
pub fn idealyst_tool(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let item_fn = parse_macro_input!(item as ItemFn);
    #[cfg(not(feature = "catalog"))]
    {
        return TokenStream::from(quote! { #item_fn });
    }
    #[cfg(feature = "catalog")]
    {
        let registration = tool_emit::emit(&item_fn);
        TokenStream::from(quote! {
            #item_fn
            #registration
        })
    }
}

/// `recipe!(Component, fn name() -> Element { … })` — declare a
/// compile-checked usage example ("recipe") for `Component`.
///
/// The recipe's function is emitted verbatim, so it's **compiled and
/// type-checked against the component's live props** — if a prop changes
/// and the recipe isn't updated, it fails to compile. The macro also
/// captures the fn's formatted source, its `///` docs, and the
/// components its `ui!`/`jsx!` body uses, registering a
/// `RecipeEntry` for the catalog (so MCP + docs surface working,
/// verified examples).
///
/// Self-gating: with the `catalog` feature OFF this expands to
/// **nothing** — recipes (and the imports inside them) cost zero in
/// production and aren't compiled at all. So write recipes anywhere
/// (their own file, a `*_recipes.rs`, a separate crate) with no `#[cfg]`
/// of your own; they materialize only when the catalog is built.
///
/// Recipes should be self-contained — put the needed `use`s inside the
/// fn body — both so they read as complete, copy-pasteable examples and
/// so nothing dangles when the macro expands to nothing.
#[proc_macro]
pub fn recipe(input: TokenStream) -> TokenStream {
    // Recipes exist only when building the catalog. Off → emit nothing
    // (the body, and any imports it relies on, simply don't compile).
    #[cfg(not(feature = "catalog"))]
    {
        let _ = input;
        TokenStream::new()
    }
    #[cfg(feature = "catalog")]
    {
        recipe_emit::emit(input.into()).into()
    }
}

/// `doc_scope!(Marker = "Title" [, slug = "…"] [, docs = "…"]
/// [, order = N])` — declare a documentation **scope**, a flat label
/// that groups catalog entities by feature area.
///
/// Scopes are flat (no hierarchy); every documentable entity is assigned
/// to the nearest enclosing scope by module proximity — so `#[component]`
/// etc. take **no** scope argument; a component inherits the `doc_scope!`
/// declared in its module (or an ancestor). Identity is the `slug`
/// (default = lowercased marker ident), independent of module location.
/// See `docs/catalog-scopes-spec.md`.
///
/// Self-gating like `recipe!`: with the `catalog` feature OFF this
/// expands to **nothing** (scopes cost zero in production). Write
/// `doc_scope!(...)` anywhere with no `#[cfg]` of your own.
#[proc_macro]
pub fn doc_scope(input: TokenStream) -> TokenStream {
    #[cfg(not(feature = "catalog"))]
    {
        let _ = input;
        TokenStream::new()
    }
    #[cfg(feature = "catalog")]
    {
        scope_emit::emit(input.into()).into()
    }
}

/// `ui! { ... }` — JSX-style DSL for component composition.
///
/// See the [`ui`] module for the grammar.
#[proc_macro]
pub fn ui(input: TokenStream) -> TokenStream {
    // Parse-or-recover rather than `parse_macro_input!`. The latter
    // replaces the whole invocation with a bare `compile_error!` on any
    // parse failure — which is *most* keystrokes mid-edit — leaving
    // rust-analyzer with no typed tokens inside the block, so completion
    // / hover / go-to-def die for the entire `ui! { … }`. `emit_recovery`
    // keeps the diagnostic but also re-surfaces every complete sub-expr
    // in a dead-but-typed position so the IDE stays useful while typing.
    let input: proc_macro2::TokenStream = input.into();
    match syn::parse2::<ui::Ui>(input.clone()) {
        Ok(parsed) => ui::emit(parsed).into(),
        Err(err) => ui::emit_recovery(input, &err).into(),
    }
}

/// `lazy! { … }` — inline code-splitting boundary. The block's UI
/// is hoisted into a `#[wasm_split]` async fn so the build-time
/// wasm-split step can extract it into a separate wasm chunk
/// loaded on demand. Native targets compile the block inline
/// (wasm-split's macro is transparent off-wasm).
///
/// See the [`lazy`] module for details, constraints, and naming.
#[proc_macro]
pub fn lazy(input: TokenStream) -> TokenStream {
    lazy::emit(input)
}

/// `jsx! { ... }` — JSX-flavored variant of `ui!`. Same emission backend,
/// angle-bracket syntax: `<Foo prop="x" expr={e} ref={r}>...</Foo>` or
/// `<Foo />`. See the [`jsx`] module for the full grammar.
#[proc_macro]
pub fn jsx(input: TokenStream) -> TokenStream {
    // See `ui` above for why this is parse-or-recover, not
    // `parse_macro_input!`. The recovery emitter is grammar-agnostic
    // (it walks raw tokens), so `jsx!` reuses `ui::emit_recovery`.
    let input: proc_macro2::TokenStream = input.into();
    match syn::parse2::<jsx::Jsx>(input.clone()) {
        Ok(parsed) => jsx::emit(parsed).into(),
        Err(err) => ui::emit_recovery(input, &err).into(),
    }
}

/// `stylesheet! { ... }` — declaration macro for a typed stylesheet
/// with variants and overrides. See the [`stylesheet`] module for the
/// grammar.
#[proc_macro]
pub fn stylesheet(input: TokenStream) -> TokenStream {
    let parsed = parse_macro_input!(input as stylesheet::StyleSheetDecl);
    stylesheet::emit(parsed).into()
}

/// `text_fmt!("template", args...)` — sugar for constructing a
/// reactive text binding that hands per-fire fan-out to the active
/// backend's binding layer (web backend: JS-side).
///
/// Args wrapped in `bind!(...)` are signals; others are captured
/// by-value at construction time. See the [`text_fmt`] module for
/// the full grammar.
///
/// ```ignore
/// let id: u32 = 42;
/// let global: Signal<u32> = ...;
/// text_fmt!("leaf {}: g={}", id, bind!(global))
/// ```
#[proc_macro]
pub fn text_fmt(input: TokenStream) -> TokenStream {
    let parsed = parse_macro_input!(input as text_fmt::TextFmtInput);
    text_fmt::emit(parsed).into()
}

/// `#[component]` — annotates a component function. Rewrites its body for
/// reactivity (cloning parameter-rooted paths into reactive closures) and
/// emits the dispatch glue `ui!`/`jsx!` target: a `pub type Name =
/// NameProps` tag alias plus an `impl runtime_core::BuildElement for
/// NameProps` (a no-arg component gets an empty marker struct instead).
/// This replaced the old per-component `macro_rules!` — see
/// [`invocation_macro`].
///
/// Optional attribute arguments:
/// - `default(field = expr, …)` — declare per-field defaults the
///   invocation macro fills in when the caller omits them.
/// - `children` — mark this component as a container (informational; the
///   invocation macro is unchanged).
/// `#[props]` — reactive-by-default props struct. Rewrites each scalar-data
/// field `T` → `Reactive<T>` so a `ui!` call site can pass a `Signal`/`rx!`
/// and have it carry through live, while plain values stay zero-overhead
/// `Static` snapshots. Handlers, children, refs, and existing reactive
/// sources are left alone (see [`props_attr`]); per-field `#[prop(static)]`
/// / `#[prop(reactive)]` override the heuristic. Place ABOVE the derives:
///
/// ```ignore
/// #[props]
/// #[derive(IdealystSchema)]
/// pub struct FooProps {
///     content: String,                 // → Reactive<String>
///     #[prop(static)] size: FooSize,   // stays FooSize
///     on_change: Rc<dyn Fn(String)>,   // left alone (handler)
/// }
/// ```
#[proc_macro_attribute]
pub fn props(_attr: TokenStream, item: TokenStream) -> TokenStream {
    props_attr::emit(item.into()).into()
}

#[proc_macro_attribute]
pub fn component(attr: TokenStream, item: TokenStream) -> TokenStream {
    let attr = match component_attr::parse_component_attr(attr.into()) {
        Ok(a) => a,
        Err(e) => return e.to_compile_error().into(),
    };
    let mut item_fn = parse_macro_input!(item as ItemFn);
    // `strict-docs`: require a doc comment on the component fn. Computed
    // from the original attrs before any rewrite; emitted alongside the
    // component so the error points at the fn name. Empty when the
    // feature is off (zero generated tokens).
    #[cfg(feature = "strict-docs")]
    let strict_doc_err = doc_check::require_component_doc(&item_fn);
    #[cfg(not(feature = "strict-docs"))]
    let strict_doc_err = proc_macro2::TokenStream::new();
    // `strict-naming`: require the component fn name be PascalCase — the
    // convention `ui!`/`jsx!` use to route a tag to component dispatch.
    // Computed before any rewrite so the error points at the fn name.
    // Empty when the feature is off (zero generated tokens).
    #[cfg(feature = "strict-naming")]
    let strict_naming_err = naming_check::require_component_pascal_case(&item_fn);
    #[cfg(not(feature = "strict-naming"))]
    let strict_naming_err = proc_macro2::TokenStream::new();
    // Components read as PascalCase at the `ui!` call site. Authors who
    // also name the fn itself PascalCase — the "true `fn` component"
    // style — would otherwise trip Rust's `non_snake_case` lint. Inject
    // `#[allow(non_snake_case)]` on the generated fn so a `#[component]`
    // can be PascalCase without a manual allow. No-op for the
    // conventional snake_case component fn.
    item_fn.attrs.push(syn::parse_quote!(#[allow(non_snake_case)]));
    // Look for a `methods!` block inside the body and lift it out into
    // a generated handle struct + Bindable wiring. The fn's body and
    // return type are rewritten in place when methods! is present.
    let (methods_extra, method_infos) = match methods_block::extract_and_rewrite(&mut item_fn) {
        Ok((extra, infos)) => (extra, infos),
        Err(e) => return e.to_compile_error().into(),
    };
    reactivity::rewrite(&mut item_fn);

    // When the `debug-stats` feature is on (forwarded from
    // `runtime-core/debug-stats`), wrap the rewritten body with
    // component enter/exit recording. The wrap happens at the macro
    // level so it covers every `#[component]` automatically — no
    // per-component decorator needed.
    #[cfg(feature = "debug-stats")]
    wrap_component_body_for_debug(&mut item_fn);

    let invocation = invocation_macro::generate_build_impl(&item_fn, &attr);

    // When the `catalog` feature is on, emit an inventory submission so the
    // component is discoverable through `mcp-catalog`'s catalog. The
    // submission is a sibling of the function so the linker-section
    // magic in `inventory` works as expected. When the feature is off,
    // this expands to an empty token stream — zero overhead.
    #[cfg(feature = "catalog")]
    let mcp_registration = mcp_emit::emit(&item_fn, &method_infos);
    #[cfg(not(feature = "catalog"))]
    let mcp_registration = {
        let _ = &method_infos;
        proc_macro2::TokenStream::new()
    };

    // `#[component(external)]` → an `ExternalEntry` for `idealyst export`.
    // Like `mcp_registration` this is catalog-gated and computed before
    // the hot-reload split below rewrites `item_fn` into a token stream.
    #[cfg(feature = "catalog")]
    let external_registration = match &attr.external {
        Some(spec) => external_emit::emit(&item_fn, spec),
        None => proc_macro2::TokenStream::new(),
    };
    #[cfg(not(feature = "catalog"))]
    let external_registration = {
        let _ = &attr.external;
        proc_macro2::TokenStream::new()
    };

    // When the `hot-reload` feature is on, split the function into
    // an inner `__<Name>_hot_impl` containing the rewritten body and
    // an outer `<Name>` that dispatches through
    // `dev_hot::call`. This puts every component on the
    // jump-table fast path — replacing a component's body at
    // runtime swaps the function pointer the outer fn calls. When
    // the feature is off, `item_fn` is emitted unchanged. The
    // wrapper is the LAST transform so it sees the fully-rewritten
    // body (reactivity, methods!, debug-stats).
    #[cfg(feature = "hot-reload")]
    let item_fn = split_for_hot_reload(item_fn);
    #[cfg(not(feature = "hot-reload"))]
    let item_fn = quote! { #item_fn };

    TokenStream::from(quote! {
        #strict_doc_err
        #strict_naming_err
        #methods_extra
        #item_fn
        #invocation
        #mcp_registration
        #external_registration
    })
}

/// Split a fully-rewritten component fn into an inner impl + outer
/// hot-reload dispatcher. The outer keeps the original name and
/// signature; the inner gets `__<Name>_hot_impl` and the actual
/// body. The outer's body is
///
/// ```ignore
/// ::dev_hot::call(__<Name>_hot_impl, (arg1, arg2, ...))
/// ```
///
/// which dispatches through subsecond's jump table. Generics and
/// where clauses propagate to both; `pub`/`pub(crate)` etc. stays
/// on the outer; the inner is `#[doc(hidden)]` `fn` (not pub) so
/// authors can't accidentally call it.
#[cfg(feature = "hot-reload")]
fn split_for_hot_reload(item_fn: ItemFn) -> proc_macro2::TokenStream {
    use proc_macro2::Span;
    use syn::{parse_quote, FnArg, Ident, ItemFn, Pat, PatIdent};

    let outer_name = item_fn.sig.ident.clone();
    let inner_name = Ident::new(&format!("__{}_hot_impl", outer_name), outer_name.span());

    // Build the inner fn: same signature minus the visibility, body
    // unchanged. Renaming preserves debug names — the inner is what
    // panics / shows in backtraces.
    let mut inner = item_fn.clone();
    inner.vis = syn::Visibility::Inherited;
    inner.sig.ident = inner_name.clone();
    let inner_attrs_doc_hidden: syn::Attribute = parse_quote!(#[doc(hidden)]);
    let inner_attrs_allow_nonsnake: syn::Attribute =
        parse_quote!(#[allow(non_snake_case)]);
    inner.attrs.push(inner_attrs_doc_hidden);
    inner.attrs.push(inner_attrs_allow_nonsnake);

    // Build the outer fn: same signature, body replaced with a
    // tail-call through dev_hot::call. We need to pass the
    // args as a tuple. Walk the fn args, generate fresh idents that
    // match each arg's binding, and pack them.
    //
    // For a `props: &CounterProps` parameter, the outer fn keeps
    // that signature so callers see no change; the body just does
    // `dev_hot::call(__Counter_hot_impl, (props,))`.
    let mut outer = item_fn;
    outer.attrs.retain(|a| !a.path().is_ident("inline")); // don't double-inline
    let args = collect_arg_idents(&outer.sig.inputs);
    let arg_tuple = if args.is_empty() {
        quote::quote! { () }
    } else if args.len() == 1 {
        let a = &args[0];
        quote::quote! { (#a,) }
    } else {
        quote::quote! { (#(#args),*) }
    };
    // Body: forward to the inner impl via dev_hot's wrapper.
    // Reach `dev_hot` through `runtime_core::__hot` so the
    // generated code resolves in every consumer crate without
    // forcing them to take a direct dep on dev-hot.
    //
    // Critical: assign the inner fn item to a `fn(...)` typed local
    // first. A bare named function in Rust is a *zero-sized fn item
    // type* — passing it directly into `dev_hot::call` makes
    // `F` a ZST, which routes through subsecond's trait-object code
    // path (it keys the jump table on `<F as HotFunction>::call_it`,
    // not on the user's function). Our diff generator emits entries
    // for `__*_hot_impl` symbols by name, so we need the dispatch to
    // go through the fn-pointer path. Coercing the fn item to an
    // explicit `fn(...)` pointer here forces `size_of::<F>() ==
    // size_of::<fn()>()` inside `HotFn::try_call`, taking
    // `call_as_ptr` — which uses the function pointer's runtime
    // address as the lookup key. That's the address our diff
    // generator wrote into the table.
    let inner_fn_pointer_types: Vec<&syn::Type> = outer
        .sig
        .inputs
        .iter()
        .filter_map(|arg| match arg {
            syn::FnArg::Typed(pt) => Some(&*pt.ty),
            _ => None,
        })
        .collect();
    let inner_fn_pointer_ret = match &outer.sig.output {
        syn::ReturnType::Default => quote::quote! { () },
        syn::ReturnType::Type(_, t) => quote::quote! { #t },
    };
    outer.block = parse_quote! {
        {
            let __idealyst_hot_inner: fn(#(#inner_fn_pointer_types),*) -> #inner_fn_pointer_ret
                = #inner_name;
            ::runtime_core::__hot::call(__idealyst_hot_inner, #arg_tuple)
        }
    };
    // Avoid spurious lints on the outer's generated body.
    outer
        .attrs
        .push(parse_quote!(#[allow(clippy::needless_pass_by_value)]));

    let _ = Span::call_site();
    let _ = std::marker::PhantomData::<PatIdent>;
    let _ = std::marker::PhantomData::<Pat>;
    let _ = std::marker::PhantomData::<FnArg>;
    let _ = std::marker::PhantomData::<ItemFn>;

    quote::quote! {
        #inner
        #outer
    }
}

/// Extract the binding idents from a fn's parameter list. Patterns
/// other than a plain `name: Type` (e.g. tuple destructuring,
/// `mut name`) are normalized to their binding ident. Components in
/// this framework use simple `props: &SomeProps` shapes so this is
/// always a clean unwrap; we conservatively bail to an empty list if
/// the shape is unexpected, which yields a `()` arg tuple — fine,
/// the inner fn's signature will reject it at compile time and the
/// author gets a normal Rust error pointing at their component.
#[cfg(feature = "hot-reload")]
fn collect_arg_idents(
    inputs: &syn::punctuated::Punctuated<syn::FnArg, syn::Token![,]>,
) -> Vec<syn::Ident> {
    let mut out = Vec::new();
    for arg in inputs.iter() {
        let syn::FnArg::Typed(pat_type) = arg else {
            continue;
        };
        if let syn::Pat::Ident(pi) = pat_type.pat.as_ref() {
            out.push(pi.ident.clone());
        }
    }
    out
}

/// Wrap the component's body with `record_component_enter` /
/// `record_component_exit` calls. The component's name (the literal
/// fn ident) is passed as `&'static str` so it survives into the
/// recorded event without allocation.
#[cfg(feature = "debug-stats")]
fn wrap_component_body_for_debug(item_fn: &mut ItemFn) {
    use proc_macro2::Span;
    use syn::{parse_quote, Block};
    let name_lit = syn::LitStr::new(&item_fn.sig.ident.to_string(), Span::call_site());
    let original: Block = std::mem::replace(
        &mut *item_fn.block,
        Block { brace_token: Default::default(), stmts: Vec::new() },
    );
    *item_fn.block = parse_quote! {
        {
            ::runtime_core::debug::record_component_enter(#name_lit);
            let __idealyst_debug_result = #original;
            ::runtime_core::debug::record_component_exit(#name_lit);
            __idealyst_debug_result
        }
    };
}
