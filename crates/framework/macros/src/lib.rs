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
    TokenStream::from(quote! {
        #methods_extra
        #item_fn
        #invocation
    })
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
