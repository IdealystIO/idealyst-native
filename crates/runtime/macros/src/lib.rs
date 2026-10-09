//! The idealyst proc macros: `#[component]`, `ui!`, `jsx!`,
//! `stylesheet!`, `#[props]`, the catalog macros (`#[derive(IdealystSchema)]`,
//! `#[idealyst_tool]`, `recipe!`, `doc_scope!`), and the rest below.
//!
//! This crate is only the proc-macro boundary. Each macro converts its
//! `proc_macro` token streams to `proc_macro2` and calls the function of
//! the same name in `runtime_macros_expand`, where the expansion — and
//! its documentation — lives. The split exists because a proc-macro
//! crate can't be used as a library, and the catalog scanner has to run
//! this exact expansion outside rustc (see `runtime-macros-expand`).

use proc_macro::TokenStream;
use runtime_macros_expand as expand;

/// `#[derive(Remote)]` — let a value type cross between an app and its
/// remote bundles (an app component's prop set by remote code, a remote
/// component's prop, context), field by field. A field type that can't
/// cross fails by name when a value does, never at compile time; the whole
/// expansion is empty unless the app hosts remote components. See
/// `remote_derive`.
#[proc_macro_derive(Remote)]
pub fn derive_remote(input: TokenStream) -> TokenStream {
    expand::derive_remote(input.into()).into()
}

/// `#[derive(Key)]` — a struct or enum a generic `#[host_fn]` can compare,
/// hash and clone without knowing its type
/// (`runtime_vocabulary::host_types::Key`). Also emits `PartialEq`, `Eq`,
/// `PartialOrd`, `Ord` and `Hash` (fields in order, an enum's variant
/// first), so its order and its key bytes can't disagree; derive `Clone`
/// yourself. See `key_derive`.
#[proc_macro_derive(Key)]
pub fn derive_key(input: TokenStream) -> TokenStream {
    expand::derive_key(input.into()).into()
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
    expand::derive_idealyst_schema(input.into()).into()
}

/// `#[idealyst_tool]` — register a standalone function as an MCP
/// tool (spec §4.2). The function body is left unchanged; the
/// attribute only emits an `inventory::submit!` of a `ToolEntry`
/// alongside it. When the `catalog` feature is off the attribute is a
/// no-op (function emitted unchanged, no registration).
#[proc_macro_attribute]
pub fn idealyst_tool(attr: TokenStream, item: TokenStream) -> TokenStream {
    expand::idealyst_tool(attr.into(), item.into()).into()
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
    expand::recipe(input.into()).into()
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
    expand::doc_scope(input.into()).into()
}

/// `ui! { ... }` — JSX-style DSL for component composition.
///
/// See the `ui` module for the grammar.
#[proc_macro]
pub fn ui(input: TokenStream) -> TokenStream {
    expand::ui(input.into()).into()
}

/// `lazy! { … }` — inline code-splitting boundary. The block's UI
/// is hoisted into a `#[wasm_split]` async fn so the build-time
/// wasm-split step can extract it into a separate wasm chunk
/// loaded on demand. Native targets compile the block inline
/// (wasm-split's macro is transparent off-wasm).
///
/// See the `lazy` module for details, constraints, and naming.
#[deprecated(
    since = "0.5.0",
    note = "use a lazy component instead: `#[component(lazy)] fn Chunk() -> Element { … }` \
            (or the `#[lazy]` shorthand). Same chunking mechanism, but with typed props \
            across the boundary, named chunk files, and the standard `loading`/`error` \
            props instead of builder methods."
)]
#[proc_macro]
pub fn lazy(input: TokenStream) -> TokenStream {
    expand::lazy(input.into()).into()
}

/// `jsx! { ... }` — JSX-flavored variant of `ui!`. Same emission backend,
/// angle-bracket syntax: `<Foo prop="x" expr={e} ref={r}>...</Foo>` or
/// `<Foo />`. See the `jsx` module for the full grammar.
#[proc_macro]
pub fn jsx(input: TokenStream) -> TokenStream {
    expand::jsx(input.into()).into()
}

/// `stylesheet! { ... }` — declaration macro for a typed stylesheet
/// with variants and overrides. See the `stylesheet` module for the
/// grammar.
#[proc_macro]
pub fn stylesheet(input: TokenStream) -> TokenStream {
    expand::stylesheet(input.into()).into()
}

/// `#[host_fn]` — an app function remote code can call: the function in the
/// app, a stub asking the app to run it in a remote bundle. Arguments and
/// result cross as `RemoteValue`s. It may be generic over `Key`, `Opaque`
/// and `Numeric` parameters, and a bundle may call it with types the app
/// never compiled. See `host_fn`.
#[proc_macro_attribute]
pub fn host_fn(attr: TokenStream, item: TokenStream) -> TokenStream {
    expand::host_fn(attr.into(), item.into()).into()
}

/// `#[props]` — reactive-by-default props struct. Rewrites each scalar-data
/// field `T` → `Reactive<T>` so a `ui!` call site can pass a `Signal`/`rx!`
/// and have it carry through live, while plain values stay zero-overhead
/// `Static` snapshots. Handlers, children, refs, and existing reactive
/// sources are left alone (see `props_attr`); per-field `#[prop(static)]`
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
pub fn props(attr: TokenStream, item: TokenStream) -> TokenStream {
    expand::props(attr.into(), item.into()).into()
}

/// `#[component]` — annotates a component function. Rewrites its body for
/// reactivity (cloning parameter-rooted paths into reactive closures) and
/// emits the dispatch glue `ui!`/`jsx!` target: a `pub type Name =
/// NameProps` tag alias plus an `impl runtime_core::BuildElement for
/// NameProps` (a no-arg component gets an empty marker struct instead).
/// This replaced the old per-component `macro_rules!` — see
/// `invocation_macro`.
///
/// Props can be declared two ways:
///
/// **Inline** (preferred) — ordinary fn parameters; the macro generates
/// the props struct, wrapping each data param `T` → `Reactive<T>` with
/// the same rules as `#[props]` (so the body sees `Reactive<String>` for
/// a `label: String` param — call `.get()`). Per-arg `#[prop(...)]`
/// accepts `default = expr`, `static`, `reactive` (plus `optional` /
/// `into` as parity no-ops), and doc comments on a param become the
/// prop's hover docs. See `inline_props`.
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
#[proc_macro_attribute]
pub fn component(attr: TokenStream, item: TokenStream) -> TokenStream {
    expand::component(attr.into(), item.into()).into()
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
    expand::lazy_component(attr.into(), item.into()).into()
}
