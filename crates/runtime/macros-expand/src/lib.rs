//! The expansion behind every `runtime-macros` proc macro, as a plain
//! library.
//!
//! `runtime-macros` is a `proc-macro` crate, and a proc-macro crate can
//! only be loaded by rustc — nothing else can depend on it. Everything
//! these macros do is a pure function from tokens to tokens, though, and
//! a second caller needs that function outside rustc: the catalog scanner
//! (`catalog-scan`), which reads an app's `#[component]`s,
//! `#[derive(IdealystSchema)]`s, `recipe!`s and `doc_scope!`s straight
//! from source by running THIS expansion and reading the
//! `inventory::submit!` literals it emits. Running the real expansion is
//! the point: a scanner with its own copy of the rules (inline-props
//! wrapping, the injected `bind_to` prop, `#[method]` lifting, the remote
//! split) would drift from the macro, and the catalog would describe
//! components that do not exist.
//!
//! So the expansion lives here, in `proc_macro2` terms, and
//! `runtime-macros` is one shim per macro that converts the token stream
//! and calls in. The features are the same set, forwarded.
//!
//! ## `#[component]`
//!
//! Rewrites a function body for reactivity and generates the dispatch
//! glue for the component. See [`component_attr`], [`reactivity`], and
//! [`invocation_macro`].
//!
//! ## `ui!`
//!
//! JSX-style DSL for composing components. Parses
//! `Name(prop = value) { children }` and desugars to plain Rust calls /
//! `BuildElement` struct literals. See [`ui`].
//!
//! ## The split pass
//!
//! `ui!` has ONE lowering — builder calls inline. Before emitting, it
//! runs [`ui_split`], which separates each site into descriptor DATA
//! (literals, enum-like paths, style-token accessors, attribute names,
//! child order) and an ordered list of dynamic **slots**, and hoists
//! every slot into a `let` at the head of its scope in source order.
//!
//! The split is not an implementation detail of the emission: it is the
//! same pass that produces the `runtime_template::Descriptor` a
//! dev-time overlay patches against. The descriptor itself is built
//! from SOURCE at build time, not emitted here — under `ui-overlay` the
//! emission adds only a per-node origin tag (see `ui_overlay`). See
//! `crates/runtime/template`.
//!
//! ## Heuristics, limitations
//!
//! - Reactivity is detected by `.get()` calls; false positives on
//!   `HashMap::get()` waste work but don't break anything.
//! - `text` and `button` are recognized by literal name only; renamed
//!   imports or fully-qualified paths are not detected.
//! - `vec![...]` and `children![...]` are special-cased; other list-shaped
//!   macros are opaque to the reactivity rewriter.

// `ui_overlay` asks `proc_macro::is_available()` whether it is expanding
// inside rustc (it reads the call site's real location only then). The
// `proc_macro` crate is linkable from any crate for exactly this check;
// outside rustc it answers `false`, which is the scanner's case.
extern crate proc_macro;

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
mod inline_props;
mod inspect_emit;
mod invocation_macro;
mod jsx;
mod lazy;
mod lazy_component;
mod component_golden;
#[cfg(feature = "catalog")]
mod external_emit;
/// The `#[component]` hot-reload split (outer dispatcher + inner
/// `__<Name>_hot_impl`). Compiled unconditionally; whether it RUNS is
/// the `hot-reload` feature, threaded as `hot_split` so one test binary
/// can drive both legs.
mod hot_split;
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
// The catalog's token-to-text, rustc's printer inside rustc and an exact
// emulation of it outside (the catalog scanner).
#[cfg_attr(not(feature = "catalog"), allow(dead_code))]
mod token_text;
mod new_core;
mod path_analysis;
// The `ui!` front end — parser, split pass, numbering, recovery — lives
// in `runtime-macros-parse`, a PLAIN library, because the CLI needs the
// same answers at build time and cannot depend on a proc-macro crate.
// These aliases keep the in-crate paths (`crate::ui_split::…`,
// `crate::primitives::…`) that the emission has always used.
use runtime_macros_parse::primitives;
use runtime_macros_parse::recovery;
mod props_attr;
mod reactivity;
mod remote_component;
mod remote_derive;
mod host_fn;
mod key_derive;
mod stylesheet;
mod ui;
mod ui_overlay;


use proc_macro2::TokenStream;
use quote::quote;
use syn::ItemFn;

/// Final post-processing for every entry point that emits core paths:
/// the assembled expansion is retargeted at `::runtime_vocabulary::glue`
/// (see [`new_core`]).
fn finish(out: TokenStream) -> TokenStream {
    new_core::retarget(out)
}

/// The same pass under the name the in-crate unit tests have always
/// called it by.
fn finish2(out: TokenStream) -> TokenStream {
    finish(out)
}

/// Parse `input` as `T`, or hand back the parse error as the expansion —
/// what `syn::parse_macro_input!` does, in `proc_macro2` terms.
macro_rules! parse_or_return {
    ($input:expr => $ty:ty) => {
        match syn::parse2::<$ty>($input) {
            Ok(parsed) => parsed,
            Err(e) => return e.to_compile_error(),
        }
    };
}

// ---------------------------------------------------------------------
// Entry points. Each is the body of the same-named macro in
// `runtime-macros`, which only converts between `proc_macro` and
// `proc_macro2` token streams. The author-facing docs live there.
// ---------------------------------------------------------------------

/// `#[derive(Remote)]`. See `remote_derive`.
pub fn derive_remote(input: TokenStream) -> TokenStream {
    let parsed = parse_or_return!(input => syn::DeriveInput);
    match remote_derive::derive(parsed) {
        Ok(out) => out,
        Err(e) => e.to_compile_error(),
    }
}

/// `#[derive(Key)]`. See `key_derive`.
pub fn derive_key(input: TokenStream) -> TokenStream {
    let parsed = parse_or_return!(input => syn::DeriveInput);
    match key_derive::derive(parsed) {
        Ok(out) => out,
        Err(e) => e.to_compile_error(),
    }
}

/// `#[derive(IdealystSchema)]`: the catalog registration (under
/// `catalog`) and the doc enforcement (under `strict-docs`). Empty with
/// neither feature.
pub fn derive_idealyst_schema(input: TokenStream) -> TokenStream {
    let parsed = parse_or_return!(input => syn::DeriveInput);
    // `mut` is exercised only by the feature-gated `extend`s below.
    #[allow(unused_mut)]
    let mut out = TokenStream::new();
    // `strict-docs`: one `compile_error!` per undocumented prop/variant.
    #[cfg(feature = "strict-docs")]
    out.extend(doc_check::require_schema_docs(&parsed));
    // `catalog`: the inventory registration carrying the field docs to
    // the catalog. `emit` consumes the input, so clone — `strict-docs`
    // may have already borrowed it above.
    #[cfg(feature = "catalog")]
    out.extend(schema_emit::emit(parsed.clone()));
    // Keep `parsed` "used" when neither feature touched it.
    let _ = &parsed;
    out
}

/// `#[idealyst_tool]`: the fn unchanged, plus its `ToolEntry` under
/// `catalog`.
pub fn idealyst_tool(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let item_fn = parse_or_return!(item => ItemFn);
    #[cfg(not(feature = "catalog"))]
    {
        quote! { #item_fn }
    }
    #[cfg(feature = "catalog")]
    {
        let registration = tool_emit::emit(&item_fn);
        quote! {
            #item_fn
            #registration
        }
    }
}

/// `recipe!`: nothing without `catalog` (the recipe is not compiled at
/// all); the fn plus its `RecipeEntry` with it.
pub fn recipe(input: TokenStream) -> TokenStream {
    #[cfg(not(feature = "catalog"))]
    {
        let _ = input;
        TokenStream::new()
    }
    #[cfg(feature = "catalog")]
    {
        recipe_emit::emit(input)
    }
}

/// `doc_scope!`: nothing without `catalog`; its `ScopeEntry` with it.
pub fn doc_scope(input: TokenStream) -> TokenStream {
    #[cfg(not(feature = "catalog"))]
    {
        let _ = input;
        TokenStream::new()
    }
    #[cfg(feature = "catalog")]
    {
        scope_emit::emit(input)
    }
}

/// `ui!`. Parse-or-recover rather than parse-or-error: a bare
/// `compile_error!` on any parse failure — which is *most* keystrokes
/// mid-edit — leaves rust-analyzer with no typed tokens inside the block,
/// so completion / hover / go-to-def die for the entire `ui! { … }`.
/// `emit_recovery` keeps the diagnostic but also re-surfaces every
/// complete sub-expr in a dead-but-typed position.
pub fn ui(input: TokenStream) -> TokenStream {
    match syn::parse2::<runtime_macros_parse::Ui>(input.clone()) {
        Ok(parsed) => finish(ui::emit(parsed, &input)),
        Err(err) => finish(recovery::emit_recovery(input, &err)),
    }
}

/// `lazy!` (deprecated). Through `finish`: the emission's absolute
/// `::runtime_core::…` paths retarget to the `glue` mirrors
/// (`glue::primitives::lazy`, `glue::__wasm_split`).
pub fn lazy(input: TokenStream) -> TokenStream {
    finish(lazy::emit(input))
}

/// `jsx!`. Parse-or-recover for the same reason as [`ui`]; the recovery
/// emitter is grammar-agnostic (it walks raw tokens).
pub fn jsx(input: TokenStream) -> TokenStream {
    match syn::parse2::<jsx::Jsx>(input.clone()) {
        Ok(parsed) => finish(jsx::emit(parsed, &input)),
        Err(err) => finish(recovery::emit_recovery(input, &err)),
    }
}

/// `stylesheet!`.
pub fn stylesheet(input: TokenStream) -> TokenStream {
    // Hash the raw input BEFORE parsing consumes it — preminted class
    // names derive from this, so identical sheet source ⇒ identical
    // classes (harmless dedup) and any source edit moves every class
    // the sheet mints (a stale cached `.css` can never mis-style a
    // fresh binary). See `stylesheet::content_hash`.
    let content_hash = stylesheet::content_hash(&input.to_string());
    let parsed = parse_or_return!(input => stylesheet::StyleSheetDecl);
    // Through `finish`: the emission's absolute `::runtime_core::…` paths
    // retarget to `::runtime_vocabulary::glue::…` (which re-exports the
    // whole sheet vocabulary — StyleSheet, cached_stylesheet, VariantSet,
    // IntoVariantSource, … — plus `IntoStyleProp`/`StyleProp`, which the
    // builder's conversion impl targets).
    //
    // That includes the premint-dump linkme registration
    // (`cfg(idealyst_premint_dump)`), whose `::runtime_core::premint::…`
    // becomes `::runtime_vocabulary::glue::premint::…`. With one core it
    // always occurs, so `glue::premint` is a real re-export behind the
    // vocabulary's `style-dump` feature (which the facade forwards);
    // without it `idealyst build --web --premint` does not compile.
    finish(stylesheet::emit(parsed, content_hash))
}

/// `#[host_fn]`. See `host_fn`.
pub fn host_fn(attr: TokenStream, item: TokenStream) -> TokenStream {
    if !attr.is_empty() {
        return syn::Error::new(proc_macro2::Span::call_site(), "#[host_fn] takes no arguments").to_compile_error();
    }
    let func = parse_or_return!(item => ItemFn);
    match host_fn::expand(func) {
        Ok(out) => out,
        Err(e) => e.to_compile_error(),
    }
}

/// `#[props]`. See [`props_attr`].
pub fn props(_attr: TokenStream, item: TokenStream) -> TokenStream {
    finish(props_attr::emit(item))
}

/// `#[component]`. See [`emit_component_tokens`].
pub fn component(attr: TokenStream, item: TokenStream) -> TokenStream {
    let attr = match component_attr::parse_component_attr(attr) {
        Ok(a) => a,
        Err(e) => return e.to_compile_error(),
    };
    emit_component_tokens(attr, item, HOT_RELOAD_SPLIT)
}

/// `#[lazy]` — `#[component(lazy)]` with the same argument grammar.
pub fn lazy_component(attr: TokenStream, item: TokenStream) -> TokenStream {
    let attr = match component_attr::parse_lazy_attr(attr) {
        Ok(a) => a,
        Err(e) => return e.to_compile_error(),
    };
    emit_component_tokens(attr, item, HOT_RELOAD_SPLIT)
}

/// Whether `#[component]` splits each component into an outer dispatcher
/// plus an inner `__<Name>_hot_impl`. Mirrors the `hot-reload` cargo
/// feature; a `const` rather than a bare `cfg!` so the emission core can
/// be driven BOTH ways from one test binary — which is how
/// `component_golden` proves the feature-off output is byte-identical
/// to the frozen goldens.
const HOT_RELOAD_SPLIT: bool = cfg!(feature = "hot-reload");

/// The `#[component]` emission core, in `proc_macro2` terms so unit tests
/// can call it. `hot_split` is the `hot-reload` gate, threaded as a
/// parameter instead of read from `cfg!` at the use site.
pub(crate) fn emit_component_tokens(
    attr: component_attr::ComponentAttr,
    item: proc_macro2::TokenStream,
    hot_split: bool,
) -> proc_macro2::TokenStream {
    let item_fn = match syn::parse2::<ItemFn>(item) {
        Ok(f) => f,
        Err(e) => return e.to_compile_error(),
    };
    emit_component_fn(attr, item_fn, hot_split, None)
}

/// [`emit_component_tokens`], parsed. `authored` is the body as the author
/// wrote it, for the catalog's walks (`composes`, `animations`): the remote
/// split hands its rewritten fn back through here, and its body is then a
/// build-kind macro the walks can't see into.
fn emit_component_fn(
    attr: component_attr::ComponentAttr,
    mut item_fn: ItemFn,
    hot_split: bool,
    authored: Option<syn::Block>,
) -> proc_macro2::TokenStream {
    #[cfg_attr(not(feature = "catalog"), allow(unused_variables))]
    let authored = authored.unwrap_or_else(|| (*item_fn.block).clone());
    // `#[component(remote)]`: split the body off by build kind, then run the
    // rewritten fn through this same emission as an ordinary component.
    if attr.remote {
        let (component, extra) = match remote_component::prepare(item_fn) {
            Ok(r) => r,
            Err(e) => return e.to_compile_error(),
        };
        let attr = component_attr::ComponentAttr { remote: false, no_import: true, ..attr };
        let emitted = emit_component_fn(attr, component, hot_split, Some(authored));
        return quote::quote! { #emitted #extra };
    }
    // Unmigrated-shape rejection — loud, named, never silent (repo
    // rule: an unmigrated feature must fail with its migration status).
    //
    // `#[method]` lowers for the inline-props component shape: the
    // retarget maps `::runtime_core::robot::…` onto its
    // `runtime_vocabulary::glue` mirror. Only the LEGACY explicit-props
    // form stays rejected: its handle escaped through a `Bindable<H>`
    // return over the deleted `Element` — un-portable by type. Generic
    // components can't take the injected `bind_to` prop either
    // (monomorphic props glue), so they get the same pointer.
    if methods_block::has_method_fns(&item_fn)
        && (inline_props::is_legacy_props_sig(&item_fn.sig)
            || !item_fn.sig.generics.params.is_empty())
    {
        return syn::Error::new_spanned(
            &item_fn.sig.ident,
            "#[method] fns require the inline-props component shape (props \
             as fn parameters; zero parameters is fine) so the handle binds \
             through the auto-injected `bind_to` prop. The legacy \
             explicit-props form returns `Bindable<H>` over the deleted \
             pre-v2 `Element` and cannot lower; generic components can't \
             take the injected prop either.",
        )
        .to_compile_error();
    }
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
    // `#[method]` fns present? Inject the `bind_to: Option<Ref<{Handle}>>`
    // prop BEFORE inline-props expansion so it becomes a real field on the
    // generated props struct — this is what lets the ordinary tag form
    // (`ui! { Counter(bind_to = h) }`) bind a component's method handle.
    // The legacy explicit-props form can't take an injected field (the
    // struct is author-written); it keeps the `Bindable`-return binding.
    let mut bind_to_injected = false;
    if methods_block::has_method_fns(&item_fn)
        && !inline_props::is_legacy_props_sig(&item_fn.sig)
        && item_fn.sig.generics.params.is_empty()
    {
        let already_declared = item_fn.sig.inputs.iter().any(|a| {
            matches!(a, syn::FnArg::Typed(pt)
                if matches!(&*pt.pat, syn::Pat::Ident(pi) if pi.ident == "bind_to"))
        });
        if already_declared {
            return syn::Error::new_spanned(
                &item_fn.sig.ident,
                "components with `#[method]` fns receive an auto-injected `bind_to` \
                 prop for their handle; rename your own `bind_to` parameter",
            )
            .to_compile_error()
            .into();
        }
        let handle = methods_block::derive_handle_name(&item_fn.sig.ident);
        let doc = format!(
            "Fills with this component's [`{handle}`] at build — bind the imperative \
             methods: `ui! {{ Tag(bind_to = my_ref) }}`, then invoke via \
             `my_ref.get()` (not `.with()` — methods write signals)."
        );
        let param: syn::FnArg = syn::parse_quote! {
            #[doc = #doc]
            bind_to: ::core::option::Option<::runtime_core::Ref<#handle>>
        };
        item_fn.sig.inputs.push(param);
        bind_to_injected = true;
    }

    // Inline-props shape (Leptos-style fn parameters): generates the props
    // struct + dispatch glue and rewrites the signature in place (param
    // types wrapped `Reactive<T>`, `#[prop]`/doc attrs stripped). Must run
    // BEFORE the body rewrites so `reactivity::rewrite` sees the final
    // parameter list, and before re-emission so rustc never sees the param
    // attrs. `None` → classic explicit-props path, unchanged.
    // Whether a bundle imports this component from the app, decided once:
    // the inline glue's `build_set` and `import_split` below must agree.
    let import_key = remote_component::import_key(
        &item_fn,
        &attr,
        bind_to_injected || methods_block::has_method_fns(&item_fn),
    );
    let inline_glue = match inline_props::try_expand(&mut item_fn, &attr, import_key.as_ref()) {
        Ok(g) => g,
        Err(e) => return e.to_compile_error(),
    };
    // The explicit-props form reads its props list through the struct's
    // `#[props]`-generated `InspectProps`; the inline form probes params.
    let legacy_props = inline_glue.is_none() && inline_props::is_legacy_props_sig(&item_fn.sig);
    // Lazy mode threads the component's props across the chunk boundary and
    // generates the `loading`/`error` config fields — both of which need the
    // macro-generated inline-props struct. A no-arg or legacy explicit-props
    // component doesn't have one, so route the author to a shape that does.
    if attr.lazy && inline_glue.is_none() {
        return syn::Error::new_spanned(
            &item_fn.sig.ident,
            "#[component(lazy)] / #[lazy] currently requires inline props \
             (declare the props as fn parameters: `#[lazy] fn Foo(id: u32) -> Element`; \
             zero parameters is fine). Generic components can't be lazy (the generated \
             props struct is monomorphic); for a component you need both eager and \
             lazy, wrap the eager one with `lazy_component!(LazyFoo = Foo)`.",
        )
        .to_compile_error();
    }
    // Components read as PascalCase at the `ui!` call site. Authors who
    // also name the fn itself PascalCase — the "true `fn` component"
    // style — would otherwise trip Rust's `non_snake_case` lint. Inject
    // `#[allow(non_snake_case)]` on the generated fn so a `#[component]`
    // can be PascalCase without a manual allow. No-op for the
    // conventional snake_case component fn.
    item_fn.attrs.push(syn::parse_quote!(#[allow(non_snake_case)]));
    // Look for `#[method]` fns inside the body and lift it out into
    // a generated handle struct + Bindable wiring. The fn's body and
    // return type are rewritten in place when #[method] fns are present.
    let (methods_extra, method_infos) = match methods_block::extract_and_rewrite(&mut item_fn, bind_to_injected) {
        Ok((extra, infos)) => (extra, infos),
        Err(e) => return e.to_compile_error(),
    };
    reactivity::rewrite(&mut item_fn);

    // Every component not marked `remote` lives in the app binary: in a
    // bundle build its body is replaced by an import of the app's copy, and
    // in a native app it registers itself for bundles to import (both
    // no-ops unless the build hosts or is a remote bundle).
    let import = remote_component::import_split(
        &mut item_fn,
        &attr,
        inline_glue.is_some(),
        bind_to_injected || !method_infos.is_empty(),
    );

    // NEW-core body semantics: a component runs ONCE, untracked, with
    // every signal/effect it creates collected into an `Owned` scope
    // attached to the returned element (idea-lite's `component_scope`;
    // handbook §6/§9). The wrap targets `::runtime_core::component_scope`
    // and the retarget pass maps it to
    // `runtime_vocabulary::glue::component_scope`. Only bodies returning
    // bare `Element` are wrapped — richer return types (`Bindable<H>`,
    // …) can't flow through the `FnOnce() -> Element` collector.
    wrap_component_body_new_core(&mut item_fn);

    // Register every instance with the robot component registry (props,
    // source location, element link) — the inspector's component tree.
    // Inert outside robot builds; see `inspect_emit`.
    inspect_emit::wrap_body(&mut item_fn, legacy_props, bind_to_injected);

    // Bracket the body with a build probe so the runtime knows "a
    // component body is executing" — that's what powers the dev-build
    // untracked-build-read diagnostic (the hoisted-snapshot trap:
    // `let ok = x.get()…;` at body level looks reactive but froze).
    // The probe fn is `#[inline]` and compiles to nothing in release
    // builds; RAII pop covers early returns.
    {
        let name_lit = item_fn.sig.ident.to_string();
        let probe: syn::Stmt = syn::parse_quote! {
            let __idealyst_build_probe =
                ::runtime_core::__component_build_probe(#name_lit);
        };
        item_fn.block.stmts.insert(0, probe);
    }

    // When the `debug-stats` feature is on (forwarded from
    // `runtime-core/debug-stats`), wrap the rewritten body with
    // component enter/exit recording. The wrap happens at the macro
    // level so it covers every `#[component]` automatically — no
    // per-component decorator needed.
    #[cfg(feature = "debug-stats")]
    wrap_component_body_for_debug(&mut item_fn);

    // Inline mode brings its own dispatch glue (struct + Default +
    // BuildElement); the legacy path derives it from the props-struct sig.
    let import_registration = import.registration.clone();
    let invocation = match inline_glue {
        Some(glue) => glue,
        None => invocation_macro::generate_build_impl(&item_fn, &attr, import.explicit_name.as_ref()),
    };

    // When the `catalog` feature is on, emit an inventory submission so the
    // component is discoverable through `mcp-catalog`'s catalog. The
    // submission is a sibling of the function so the linker-section
    // magic in `inventory` works as expected. When the feature is off,
    // this expands to an empty token stream — zero overhead.
    // The walks read `authored`: `import_split` above has replaced this
    // body with a build-kind macro they can't see into.
    #[cfg(feature = "catalog")]
    let mcp_registration = mcp_emit::emit(&item_fn, &authored, &method_infos);
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
    // `runtime_core::__hot::call` (→ `dev_hot::call`). This puts every
    // component on the jump-table fast path — replacing a component's
    // body at runtime swaps the function pointer the outer fn calls.
    // When the feature is off, `item_fn` is emitted unchanged. The
    // wrapper is the LAST transform so it sees the fully-rewritten
    // body (reactivity, #[method] lifting, debug-stats).
    //
    // `hot_split` rather than a `cfg!` read here: the same emission
    // core has to be drivable both ways inside one test binary (see
    // `hot_reload_split::tests`).
    let item_fn = if hot_split {
        match hot_split::split(item_fn) {
            Ok(split) => split,
            // A shape the fn-pointer dispatch cannot express stays
            // whole — hot-patchable components around it still are.
            // `Refusal` is a value, not a diagnostic: refusing loudly
            // would turn a dev-only accelerator into a compile error
            // on code that builds fine in production.
            Err((refusal, item_fn)) => {
                let _ = refusal;
                quote! { #item_fn }
            }
        }
    } else {
        quote! { #item_fn }
    };

    finish2(quote! {
        #strict_doc_err
        #strict_naming_err
        #methods_extra
        #item_fn
        #invocation
        #mcp_registration
        #external_registration
        #import_registration
    })
}

/// Wrap a component fn's body in `component_scope(move || { … })` — the
/// run-once/untracked/collected contract. Applies only when the declared
/// return type is bare `Element` (same token-level check as
/// `reactivity::returns_primitive`).
fn wrap_component_body_new_core(item_fn: &mut ItemFn) {
    let ty = match &item_fn.sig.output {
        syn::ReturnType::Type(_, ty) => ty,
        syn::ReturnType::Default => return,
    };
    let normalized: String = quote::quote!(#ty)
        .to_string()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    if !matches!(
        normalized.as_str(),
        "Element" | "runtime_core::Element" | "::runtime_core::Element"
    ) {
        return;
    }
    // Build the wrapper from a tiny template and MOVE the already-parsed
    // body into its closure. The obvious `parse_quote!({ … #block })`
    // prints the whole body back into tokens and parses it a second time:
    // measured on CrewForge (213 components), that re-parse was 29% of all
    // `#[component]` expansion time, which runs on every hot-patch replay.
    // The AST produced here is the one the template would have parsed to.
    let body = std::mem::replace(
        &mut *item_fn.block,
        syn::Block { brace_token: Default::default(), stmts: Vec::new() },
    );
    let mut wrapper: syn::Block = syn::parse_quote!({
        ::runtime_core::component_scope(move || {})
    });
    let Some(syn::Stmt::Expr(syn::Expr::Call(call), None)) = wrapper.stmts.first_mut() else {
        unreachable!("the template is one call expression");
    };
    let Some(syn::Expr::Closure(closure)) = call.args.first_mut() else {
        unreachable!("the template's argument is a closure");
    };
    *closure.body = syn::Expr::Block(syn::ExprBlock { attrs: Vec::new(), label: None, block: body });
    *item_fn.block = wrapper;
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
