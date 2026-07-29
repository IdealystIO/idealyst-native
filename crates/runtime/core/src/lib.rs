//! Framework core: the OLD-core walker half — `Element`, the `Backend`
//! mega-trait, the render walker, and the `Bound`/builder authoring
//! layer — **pending deletion** once the last old-core consumer moves
//! to the new core (runtime-world / runtime-scene / runtime-vocabulary).
//!
//! Everything else that used to live here — the style engine, colors,
//! assets/fonts, animation, touch/hover/wheel/file-drop types, the
//! legacy reactive arena, scheduling/time/session, viewport/breakpoint/
//! safe-area state, logging, debug counters, the robot registry +
//! bridge, introspection, and the per-primitive prop/handle structs —
//! moved to `runtime-shared` (the permanent shared substrate) and is
//! re-exported here at its old paths, so existing consumers compile
//! unchanged. Each thread-local moved WITH its module: runtime-shared
//! owns the single authority, this crate only re-exports (a duplicate
//! TLS would silently split state between the cores).

/// Panic with an actionable diagnostic in dev builds and a terse stable
/// code in release builds.
///
/// Long diagnostic prose ships in wasm rodata — the framework's six
/// biggest messages alone were ~1.5 KB of every release bundle. The
/// prose (and any format-arg machinery it pulls in) is compiled out of
/// release; the short code keeps the failure greppable, and rebuilding
/// in dev (`idealyst dev`) reproduces the full message at the same site.
///
/// `$code` is a stable slug (`"reentrant-signal-read"`), not an error
/// number — grep the codebase for it to find the one call site and its
/// full prose. The slug prefixes the message in BOTH modes so
/// `#[should_panic(expected = "<slug>")]` tests hold under
/// `cargo test --release` too. (runtime-shared carries its own copy —
/// the macro is crate-internal by design.)
macro_rules! diag_panic {
    ($code:literal, $fmt:literal $(, $arg:expr)* $(,)?) => {{
        #[cfg(debug_assertions)]
        { panic!(concat!("idealyst[", $code, "]: ", $fmt) $(, $arg)*) }
        #[cfg(not(debug_assertions))]
        { panic!(concat!("idealyst[", $code, "] (debug build has details)")) }
    }};
}
pub(crate) use diag_panic;

// ---------------------------------------------------------------------------
// The shared substrate — everything non-walker lives in `runtime-shared`
// and is re-exported wholesale at the old paths. Local modules below
// (accessibility, primitives, …) shadow the same-named shared modules
// where this crate keeps an Element/Backend-coupled remainder; those
// wrappers re-export the shared half internally.
// ---------------------------------------------------------------------------
pub use runtime_shared::*;

/// Old-path re-exports of the decl macros that moved to
/// `runtime-shared` with the types they construct (`Typeface`, `Ref`,
/// `Reactive`, `Effect`). `$crate::…` inside their expansions now
/// resolves against runtime-shared — same items, one authority.
pub use runtime_shared::{__face_source, effect, face, node_ref, rx, typeface};

/// Accessibility surface: shared substrate + the walker-only
/// `primitive_kind` remainder (see the module).
pub mod accessibility;
mod backend;
mod builder;
mod element;
mod external;
mod walker;
/// Per-primitive surface: the shared prop/handle types (re-exported
/// from `runtime-shared`) plus this crate's `Element`/`Bound` builder
/// remainders.
pub mod primitives;

// Walker-coupled regression tests relocated from runtime-shared's
// style/reactive test modules during the substrate extraction.
#[cfg(test)]
mod split_walker_tests;

// Lets code inside this crate (macro expansions) refer to itself by
// its external name.
extern crate self as runtime_core;

// NOTE (wave 2b, catalog re-anchor): the core-primitive recipes that
// lived here as `recipe!` fns are now STATIC catalog data in
// `runtime_shared::recipes` (source text via `include_str!`), with a
// dual-core compile gate in `crates/dev/newcore-app`. They had to
// leave this crate: `recipe!` compiles `ui!` bodies, and under a
// graph-wide `runtime-macros/new-core` emission those lower to
// `runtime_vocabulary::glue` paths this crate must not depend on —
// which made `runtime-core/catalog` unbuildable in `--new-core`
// graphs (and the catalog dies with this crate at P7 otherwise).

#[cfg(feature = "robot")]
#[doc(hidden)]
pub fn __component_root(child: Element, instance: robot::ComponentInstanceId) -> Element {
    Element::Component {
        instance,
        child: Box::new(child),
    }
}

/// Tag a `#[component]`'s root primitive with its component instance so the
/// robot walker can link element↔component. Called UNCONDITIONALLY by the
/// `#[component]` macro for `#[method]`-bearing components — a transparent
/// `Element::Component` wrapper when `robot` is on, an identity no-op when
/// off. (The `robot` module itself — real and stub — now lives in
/// runtime-shared and arrives via the glob re-export above.)
#[cfg(not(feature = "robot"))]
#[doc(hidden)]
#[inline(always)]
pub fn __component_root(child: Element, _instance: robot::ComponentInstanceId) -> Element {
    child
}

pub use backend::Backend;
pub use builder::{
    button, dynamic, each_keyed, fragment, one_or_view, pressable, styled_text, switch, text, view,
    when, Bindable, Bound, BuildElement, ChildList, IntoDisabledSource, IntoElement, ReactiveCond,
    ReactiveForEach, ReactiveListKeyed, StaticCond, StaticForEach,
};
pub use element::{EachKey, EachRowBuild, EachSnapshot, Element};
pub use walker::{build_detached, mount, render, DetachedScope, Owner};
pub use primitives::navigator::{
    current_screen_state, join_path, match_pattern, match_prefix, peek_initial_path,
    set_initial_path, take_initial_path, MountResult, NavCommand,
    NavState, NavigatorConfig,
    NavigatorControl, NavigatorHandle, NavigatorHandler, NavigatorHost, NavigatorOps,
    NavigatorRegistry, Route, RouteParams, Screen, ScreenStateGuard,
};
#[cfg(feature = "prim-icon")]
pub use primitives::icon::icon;
#[cfg(feature = "prim-image")]
pub use primitives::image::{image, image_asset, image_from};
#[cfg(feature = "prim-text-input")]
pub use primitives::text_input::text_input;
#[cfg(feature = "prim-text-input")]
pub use primitives::text_area::text_area;
#[cfg(feature = "prim-toggle")]
pub use primitives::toggle::toggle;
#[cfg(feature = "prim-portal")]
pub use primitives::overlay::{anchored_overlay, overlay};
pub use primitives::overlay::{AnchoredOverlayBuilder, OverlayBuilder};
#[cfg(feature = "prim-virtualizer")]
pub use primitives::flat_list::{flat_list, fixed_size, FlatListItemSize};
pub use primitives::scroll_view::scroll_view;
#[cfg(feature = "prim-virtualizer")]
pub use primitives::virtualizer::virtualizer;
pub use primitives::link::external_link;
#[cfg(feature = "prim-portal")]
pub use primitives::portal::portal;
pub use external::{
    defer_external_registration, deserialize_external_payload, drain_external_registrations,
    external, has_pending_external_registrations, register_external_serde,
    serialize_external_payload, ErasedHandler, ExternalHandle, ExternalRegistry, RegisterExternal,
};
#[cfg(feature = "prim-presence")]
pub use primitives::presence::presence;

pub use runtime_macros::{
    component, doc_scope, jsx, lazy_component, props, recipe, stylesheet, ui,
};
// `lazy!` is deprecated (use `#[component(lazy)]`); re-exported for
// compatibility while call sites migrate. The `allow` silences the
// deprecation warning on the re-export itself — use sites still warn.
#[allow(deprecated)]
pub use runtime_macros::lazy;

/// `#[idealyst_tool]` and `#[derive(IdealystSchema)]` — the
/// catalog-registration macros. **Always re-exported**, exactly like
/// `#[component]`, so author/SDK/idea-ui code can annotate freely
/// without feature-gating the import: the macros expand to a **no-op**
/// when neither `catalog` nor `strict-docs` is on. See the original
/// rationale in git history.
pub use runtime_macros::{idealyst_tool, IdealystSchema};

// Re-export of `dev_hot` so the `#[component]` macro's
// generated code can reach it via a path that's available to every
// user crate that depends on runtime-core. The macro emits
// `::runtime_core::__hot::call(...)`; users don't have to add
// `dev-hot` to their own `Cargo.toml`. Hidden from rustdoc —
// not part of the author-facing surface.
#[cfg(feature = "hot-reload")]
#[doc(hidden)]
pub use dev_hot as __hot;

// Re-export of `mcp_catalog` so the `#[component]` macro can emit
// `::runtime_core::__mcp::inventory::submit!` and have it resolve
// in any crate that depends on `runtime-core` with the `mcp`
// feature on. This is the OLD-core anchor only: under `new-core`
// the retargeted emission resolves `runtime_vocabulary::glue::__mcp`
// and alias-resolved (derive/tool/recipe) emissions resolve the
// facade's `__mcp` — all three re-export the same `mcp-catalog`
// crate instance, so every registration lands in one inventory.
#[cfg(feature = "catalog")]
#[doc(hidden)]
pub use mcp_catalog as __mcp;

/// Builds a `Vec<Element>` from a mixed-shape list of children.
///
/// Each argument must implement [`ChildList`]; the macro flattens
/// `Option<Element>` (skipping `None`) and `Vec<Element>` (extending
/// inline) so call sites can write conditionals naturally.
///
/// ```ignore
/// view(children![
///     text("always"),
///     logged_in.then(|| text("conditional")),
///     items.into_iter().map(|i| text(i)).collect::<Vec<_>>(),
/// ])
/// ```
#[macro_export]
macro_rules! children {
    ($($child:expr),* $(,)?) => {{
        let mut __c: ::std::vec::Vec<$crate::Element> = ::std::vec::Vec::new();
        $( $crate::ChildList::append_to($child, &mut __c); )*
        __c
    }};
}

/// Constructs an `AnimatedValue<T>` — the per-frame motion handle
/// you pass to `subscribe_and_apply(...)` and `.animate(...)`. `T`
/// is inferred from the initial value: `f32` for scalar motion,
/// `(f32, f32, f32, f32)` for color, etc.
///
/// ```ignore
/// let opacity = animated!(0.0_f32);
/// let color = animated!((0.0_f32, 0.0_f32, 0.0_f32, 1.0_f32));
/// ```
#[macro_export]
macro_rules! animated {
    ($value:expr) => {
        $crate::animation::AnimatedValue::new($value)
    };
}

/// Schedules a single `av.animate(animator)` call at `at_ms`
/// milliseconds from now. Returns a `ScheduledTask` that cancels
/// the pending dispatch on drop.
///
/// The macro clones the AnimatedValue handle into the closure, so
/// `$av` is consumed by reference and the original binding stays
/// available for further `animate_at!` calls.
///
/// ```ignore
/// let task = animate_at!(800, opacity, TweenTo::new(1.0, Duration::from_millis(400)).ease_out());
/// // hold `task` somewhere durable (e.g. on_cleanup) to keep the
/// // timer alive.
/// ```
#[macro_export]
macro_rules! animate_at {
    ($at:expr, $av:expr, $animator:expr) => {{
        let __av = ($av).clone();
        $crate::after_ms($at, move || {
            __av.animate($animator);
        })
    }};
}

/// Declarative multi-phase animation timeline. Each `at => { ... }`
/// clause fires one or more `av.animate(animator)` calls at that
/// moment; `AnimatedValue` handles are cloned into per-task
/// closures automatically.
///
/// The scheduled tasks are **anchored to the current reactive
/// scope** — when the surrounding `effect!` re-runs or the `Owner`
/// drops, every pending dispatch is cancelled.
///
/// See the docs on `session::after_ms` for the epoch semantics under
/// hot reload. AV slot must be a bare identifier; for complex sources
/// use [`animate_at!`] directly.
#[macro_export]
macro_rules! timeline {
    ( $( $at:expr => { $( $av:ident : $animator:expr ),* $(,)? } ),* $(,)? ) => {{
        // The offsets are interpreted as ms-since-session-epoch
        // (see [`$crate::session::epoch_micros`]). After a hot-patch
        // rerender the epoch is preserved, so the act timeline doesn't
        // visually replay on every save. Each fired body's task is
        // anchored to the current reactive scope via the underlying
        // `after_ms_scoped`, so scope cleanup cancels pending timers.
        $(
            {
                let __at: u64 = $at as u64;
                $(
                    {
                        let __av = $av.clone();
                        $crate::session::after_ms(__at, move || {
                            __av.animate($animator);
                        });
                    }
                )*
            }
        )*
    }};
}
