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

mod bind;
mod bind_press;
mod bind_repeat;
mod bind_switch;
mod bind_when;
mod component_attr;
mod invocation_macro;
mod jsx;
mod methods_block;
mod path_analysis;
mod reactivity;
mod stylesheet;
mod ui;

use proc_macro::TokenStream;
use quote::quote;
use syn::parse_macro_input;
use syn::ItemFn;

/// `ui! { ... }` — JSX-style DSL for component composition.
///
/// See the [`ui`] module for the grammar.
#[proc_macro]
pub fn ui(input: TokenStream) -> TokenStream {
    let parsed = parse_macro_input!(input as ui::Ui);
    ui::emit(parsed).into()
}

/// `jsx! { ... }` — JSX-flavored variant of `ui!`. Same emission backend,
/// angle-bracket syntax: `<Foo prop="x" expr={e} ref={r}>...</Foo>` or
/// `<Foo />`. See the [`jsx`] module for the full grammar.
#[proc_macro]
pub fn jsx(input: TokenStream) -> TokenStream {
    let parsed = parse_macro_input!(input as jsx::Jsx);
    jsx::emit(parsed).into()
}

/// `stylesheet! { ... }` — declaration macro for a typed stylesheet
/// with variants and overrides. See the [`stylesheet`] module for the
/// grammar.
#[proc_macro]
pub fn stylesheet(input: TokenStream) -> TokenStream {
    let parsed = parse_macro_input!(input as stylesheet::StyleSheetDecl);
    stylesheet::emit(parsed).into()
}

/// `bind!(fn(signals...))` — produce a reactive `TextSource::Bound`
/// from a call-shaped expression. The expansion carries both a
/// closure (for Effect-driven backends) and the symbolic
/// `signal_ids` + `method` name (for backends that ship bindings
/// declaratively). See [`bind`] for the grammar and constraints.
#[proc_macro]
pub fn bind(input: TokenStream) -> TokenStream {
    let parsed = parse_macro_input!(input as bind::BindInput);
    bind::emit(parsed).into()
}

/// `bind_press!(fn(signals) => output_signal)` — produce a
/// `ButtonAction` for the `on_click` slot of a `Button`. Closure +
/// binding both populated, mirroring `bind!`. See [`bind_press`]
/// for the grammar.
#[proc_macro]
pub fn bind_press(input: TokenStream) -> TokenStream {
    let parsed = parse_macro_input!(input as bind_press::BindPressInput);
    bind_press::emit(parsed).into()
}

/// `bind_when!(fn(signals), then = ..., else_ = ...)` — produce a
/// `Primitive::When` with attached binding metadata so backends
/// that ship structural reactivity declaratively (Roku) can swap
/// pre-built subtrees on signal change. See [`bind_when`] for the
/// full grammar.
#[proc_macro]
pub fn bind_when(input: TokenStream) -> TokenStream {
    let parsed = parse_macro_input!(input as bind_when::BindWhenInput);
    bind_when::emit(parsed).into()
}

/// `bind_switch!(fn(signals), pat => ..., _ => default)` — N-way
/// structural reactivity. See [`bind_switch`] for the grammar.
#[proc_macro]
pub fn bind_switch(input: TokenStream) -> TokenStream {
    let parsed = parse_macro_input!(input as bind_switch::BindSwitchInput);
    bind_switch::emit(parsed).into()
}

/// `bind_repeat!(fn(signals), max = N, row = |i| ...)` — fixed-max
/// reactive list. See [`bind_repeat`] for the grammar.
#[proc_macro]
pub fn bind_repeat(input: TokenStream) -> TokenStream {
    let parsed = parse_macro_input!(input as bind_repeat::BindRepeatInput);
    bind_repeat::emit(parsed).into()
}

/// `#[component]` — annotates a component function. Rewrites its body for
/// reactivity (cloning parameter-rooted paths into reactive closures) and
/// emits a sibling `name!` invocation macro.
///
/// Optional attribute arguments:
/// - `default(field = expr, …)` — declare per-field defaults the
///   invocation macro fills in when the caller omits them.
/// - `children` — mark this component as a container (informational; the
///   invocation macro is unchanged).
#[proc_macro_attribute]
pub fn component(attr: TokenStream, item: TokenStream) -> TokenStream {
    let attr = match component_attr::parse_component_attr(attr.into()) {
        Ok(a) => a,
        Err(e) => return e.to_compile_error().into(),
    };
    let mut item_fn = parse_macro_input!(item as ItemFn);
    // Look for a `methods!` block inside the body and lift it out into
    // a generated handle struct + Bindable wiring. The fn's body and
    // return type are rewritten in place when methods! is present.
    let methods_extra = match methods_block::extract_and_rewrite(&mut item_fn) {
        Ok(extra) => extra,
        Err(e) => return e.to_compile_error().into(),
    };
    reactivity::rewrite(&mut item_fn);

    // When the `debug-stats` feature is on (forwarded from
    // `framework-core/debug-stats`), wrap the rewritten body with
    // component enter/exit recording. The wrap happens at the macro
    // level so it covers every `#[component]` automatically — no
    // per-component decorator needed.
    #[cfg(feature = "debug-stats")]
    wrap_component_body_for_debug(&mut item_fn);

    let invocation = invocation_macro::generate_invocation_macro(&item_fn, &attr);

    // When the `hot-reload` feature is on, split the function into
    // an inner `__<Name>_hot_impl` containing the rewritten body and
    // an outer `<Name>` that dispatches through
    // `framework_hot::call`. This puts every component on the
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
        #methods_extra
        #item_fn
        #invocation
    })
}

/// Split a fully-rewritten component fn into an inner impl + outer
/// hot-reload dispatcher. The outer keeps the original name and
/// signature; the inner gets `__<Name>_hot_impl` and the actual
/// body. The outer's body is
///
/// ```ignore
/// ::framework_hot::call(__<Name>_hot_impl, (arg1, arg2, ...))
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
    // tail-call through framework_hot::call. We need to pass the
    // args as a tuple. Walk the fn args, generate fresh idents that
    // match each arg's binding, and pack them.
    //
    // For a `props: &CounterProps` parameter, the outer fn keeps
    // that signature so callers see no change; the body just does
    // `framework_hot::call(__Counter_hot_impl, (props,))`.
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
    // Body: forward to the inner impl via framework_hot's wrapper.
    // Reach `framework_hot` through `framework_core::__hot` so the
    // generated code resolves in every consumer crate without
    // forcing them to take a direct dep on framework-hot.
    outer.block = parse_quote! {
        {
            ::framework_core::__hot::call(#inner_name, #arg_tuple)
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
            ::framework_core::debug::record_component_enter(#name_lit);
            let __idealyst_debug_result = #original;
            ::framework_core::debug::record_component_exit(#name_lit);
            __idealyst_debug_result
        }
    };
}
