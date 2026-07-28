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
mod inline_props;
mod invocation_macro;
mod jsx;
mod lazy;
mod lazy_component;
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
#[cfg(feature = "new-core")]
mod new_core;
mod path_analysis;
mod primitives;
mod props_attr;
mod reactivity;
mod stylesheet;
mod ui;

use proc_macro::TokenStream;
use quote::quote;
use syn::parse_macro_input;
use syn::ItemFn;

/// Final post-processing for every entry point that emits core paths:
/// under the `new-core` feature the assembled expansion is retargeted at
/// `::runtime_vocabulary::glue` (see [`new_core`]); otherwise it passes
/// through byte-identical.
fn finish(out: proc_macro2::TokenStream) -> TokenStream {
    #[cfg(feature = "new-core")]
    let out = new_core::retarget(out);
    out.into()
}

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
    // `mut` is exercised only by the feature-gated `extend`s below.
    #[allow(unused_mut)]
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
        Ok(parsed) => finish(ui::emit(parsed, &input)),
        Err(err) => finish(ui::emit_recovery(input, &err)),
    }
}

/// `lazy! { … }` — inline code-splitting boundary. The block's UI
/// is hoisted into a `#[wasm_split]` async fn so the build-time
/// wasm-split step can extract it into a separate wasm chunk
/// loaded on demand. Native targets compile the block inline
/// (wasm-split's macro is transparent off-wasm).
///
/// See the [`lazy`] module for details, constraints, and naming.
#[deprecated(
    since = "0.5.0",
    note = "use a lazy component instead: `#[component(lazy)] fn Chunk() -> Element { … }` \
            (or the `#[lazy]` shorthand). Same chunking mechanism, but with typed props \
            across the boundary, named chunk files, and the standard `loading`/`error` \
            props instead of builder methods."
)]
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
        Ok(parsed) => finish(jsx::emit(parsed, &input)),
        Err(err) => finish(ui::emit_recovery(input, &err)),
    }
}

/// `stylesheet! { ... }` — declaration macro for a typed stylesheet
/// with variants and overrides. See the [`stylesheet`] module for the
/// grammar.
#[proc_macro]
pub fn stylesheet(input: TokenStream) -> TokenStream {
    // Hash the raw input BEFORE parsing consumes it — preminted class
    // names derive from this, so identical sheet source ⇒ identical
    // classes (harmless dedup) and any source edit moves every class
    // the sheet mints (a stale cached `.css` can never mis-style a
    // fresh binary). See `stylesheet::content_hash`.
    let content_hash = stylesheet::content_hash(&input.to_string());
    let parsed = parse_macro_input!(input as stylesheet::StyleSheetDecl);
    // Through `finish`: under `new-core` the emission's absolute
    // `::runtime_core::…` paths retarget to `::runtime_vocabulary::glue::…`
    // (which re-exports the whole sheet vocabulary — StyleSheet,
    // cached_stylesheet, VariantSet, IntoVariantSource, … — plus the
    // new-core-only `IntoStyleProp`/`StyleProp` the builder's conversion
    // impl targets). Old builds pass through byte-identical. The
    // premint-dump linkme registration (`cfg(idealyst_premint_dump)`)
    // would retarget to a nonexistent `glue::premint`, but dump builds
    // are CLI-driven old-core builds — the combination cannot occur; if
    // it ever does, the loud unresolved-path error names glue.
    finish(stylesheet::emit(parsed, content_hash))
}

/// `#[component]` — annotates a component function. Rewrites its body for
/// reactivity (cloning parameter-rooted paths into reactive closures) and
/// emits the dispatch glue `ui!`/`jsx!` target: a `pub type Name =
/// NameProps` tag alias plus an `impl runtime_core::BuildElement for
/// NameProps` (a no-arg component gets an empty marker struct instead).
/// This replaced the old per-component `macro_rules!` — see
/// [`invocation_macro`].
///
/// Props can be declared two ways:
///
/// **Inline** (preferred) — ordinary fn parameters; the macro generates
/// the props struct, wrapping each data param `T` → `Reactive<T>` with
/// the same rules as `#[props]` (so the body sees `Reactive<String>` for
/// a `label: String` param — call `.get()`). Per-arg `#[prop(...)]`
/// accepts `default = expr`, `static`, `reactive` (plus `optional` /
/// `into` as parity no-ops), and doc comments on a param become the
/// prop's hover docs. See [`inline_props`].
///
/// ```ignore
/// #[component]
/// fn Badge(label: String, #[prop(default = 3)] count: i32) -> Element {
///     ui! { text(move || format!("{} ({})", label.get(), count.get())) }
/// }
/// ```
///
/// **Explicit struct** — a single `props: &NameProps` / `props: NameProps`
/// parameter referencing a hand-written (usually `#[props]`) struct. Used
/// when the struct needs extra derives (`IdealystSchema`, doc-controls) or
/// a hand-rolled `Default`.
///
/// Optional attribute arguments:
/// - `default(field = expr, …)` — declare per-field defaults the
///   invocation macro fills in when the caller omits them (explicit-struct
///   form only; inline props use `#[prop(default = …)]`).
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
    finish(props_attr::emit(item.into()))
}

#[proc_macro_attribute]
pub fn component(attr: TokenStream, item: TokenStream) -> TokenStream {
    let attr = match component_attr::parse_component_attr(attr.into()) {
        Ok(a) => a,
        Err(e) => return e.to_compile_error().into(),
    };
    emit_component(attr, item)
}

/// `#[lazy]` — shorthand for `#[component(lazy)]`. The component's body ships in
/// a separate wasm chunk, loaded on first mount; its props become the args that
/// cross the split. `#[lazy(retryable)]` == `#[component(lazy, retryable)]`;
/// the same `#[component(...)]` argument grammar is accepted, with `lazy`
/// implied. Prefer this for the common "this component is heavy, always
/// chunk it" case; drop to explicit `#[component(lazy, …)]` when you need other
/// component options alongside.
#[proc_macro_attribute]
pub fn lazy_component(attr: TokenStream, item: TokenStream) -> TokenStream {
    let attr = match component_attr::parse_lazy_attr(attr.into()) {
        Ok(a) => a,
        Err(e) => return e.to_compile_error().into(),
    };
    emit_component(attr, item)
}

fn emit_component(attr: component_attr::ComponentAttr, item: TokenStream) -> TokenStream {
    let mut item_fn = parse_macro_input!(item as ItemFn);
    // `new-core` deferrals — loud, named, never silent (repo rule: an
    // unmigrated feature must fail with its migration status).
    #[cfg(feature = "new-core")]
    {
        if attr.lazy {
            return syn::Error::new_spanned(
                &item_fn.sig.ident,
                "#[component(lazy)] / #[lazy] is not yet available on the new core \
                 (idea-lite migration: wasm-split chunking retargets in P3b). Build \
                 without `runtime-macros/new-core` or make the component eager.",
            )
            .to_compile_error()
            .into();
        }
        if methods_block::has_method_fns(&item_fn) {
            return syn::Error::new_spanned(
                &item_fn.sig.ident,
                "#[method] blocks are not yet available on the new core (idea-lite \
                 migration: the Ref/robot handle machinery migrates in P5). Build \
                 without `runtime-macros/new-core` or drop the #[method] fns.",
            )
            .to_compile_error()
            .into();
        }
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
    let inline_glue = match inline_props::try_expand(&mut item_fn, &attr) {
        Ok(g) => g,
        Err(e) => return e.to_compile_error().into(),
    };
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
        .to_compile_error()
        .into();
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
        Err(e) => return e.to_compile_error().into(),
    };
    reactivity::rewrite(&mut item_fn);

    // NEW-core body semantics: a component runs ONCE, untracked, with
    // every signal/effect it creates collected into an `Owned` scope
    // attached to the returned element (idea-lite's `component_scope`;
    // handbook §6/§9). The wrap targets `::runtime_core::component_scope`
    // and the retarget pass maps it to
    // `runtime_vocabulary::glue::component_scope`. Only bodies returning
    // bare `Element` are wrapped — richer return types (`Bindable<H>`,
    // …) can't flow through the `FnOnce() -> Element` collector.
    #[cfg(feature = "new-core")]
    wrap_component_body_new_core(&mut item_fn);

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
    let invocation = match inline_glue {
        Some(glue) => glue,
        None => invocation_macro::generate_build_impl(&item_fn, &attr),
    };

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
    // body (reactivity, #[method] lifting, debug-stats).
    #[cfg(feature = "hot-reload")]
    let item_fn = split_for_hot_reload(item_fn);
    #[cfg(not(feature = "hot-reload"))]
    let item_fn = quote! { #item_fn };

    finish(quote! {
        #strict_doc_err
        #strict_naming_err
        #methods_extra
        #item_fn
        #invocation
        #mcp_registration
        #external_registration
    })
}

/// Wrap a component fn's body in `component_scope(move || { … })` — the
/// new-core run-once/untracked/collected contract. Applies only when the
/// declared return type is bare `Element` (same token-level check as
/// `reactivity::returns_primitive`).
#[cfg(feature = "new-core")]
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
    let block = &item_fn.block;
    item_fn.block = syn::parse_quote!({
        ::runtime_core::component_scope(move || #block)
    });
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
