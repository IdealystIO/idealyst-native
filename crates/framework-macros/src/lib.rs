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
    let invocation = invocation_macro::generate_invocation_macro(&item_fn, &attr);
    TokenStream::from(quote! {
        #methods_extra
        #item_fn
        #invocation
    })
}
