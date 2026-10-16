//! # runtime-vocabulary — capability traits and the built-in primitive
//! vocabulary
//!
//! - [`caps`] — one Ops trait per primitive family. Every backend
//!   implements [`runtime_scene::Host`] plus the Ops traits directly;
//!   `tests/caps_conformance.rs` drives the whole surface over `host-mock`.
//! - [`prims`] — one payload struct per primitive (props:
//!   `runtime_world::Value<T>` for reactive-capable props, `Rc<dyn Fn>`
//!   for callbacks, plain fields for inert config).
//! - [`builders`] — the `ui!` contract: `view() -> ViewBuilder`, props
//!   as methods, `.child(...)` with closure children as structural
//!   holes, `.build() -> runtime_scene::Element`.
//! - [`handlers`] — generic mount fns bounded on exactly the caps they
//!   use. [`handlers::register_builtins`] installs every builtin
//!   primitive; [`handlers::register_builtins_with`] takes a
//!   [`BuiltinSet`] to install a subset.
//! - [`style_attach`] — the shared style service
//!   ([`style_attach::StyleProp`]: static, cohort, dynamic, signal-class
//!   and preminted sheet paths, plus the [`style_attach::on_teardown`]
//!   probe); [`theme`] owns the per-world token/version/cohort model.
//!   Known style gaps are listed in `style_attach`'s module docs.
//! - [`glue`] — the emission surface `ui!` / `#[component]` /
//!   `stylesheet!` target: the macros emit absolute `::runtime_core::…`
//!   paths and rewrite their heads to `::runtime_vocabulary::glue::…`
//!   (`runtime-macros-expand/src/new_core.rs`).
//!
//! Backend call streams are pinned by `crates/dev/scene-parity`'s
//! full-op golden suite.
//!
//! ## Not supported by the macro lowering
//!
//! - The virtualizer `for i in count_method(sig)` sugar: it needs the
//!   structured generator-backend `Derived<usize>` count binding, which
//!   no current backend consumes. `ui!` fails with a compile error
//!   pointing at `flat_list(...)` / keyed `for`.
//! - Structured generator-backend bindings in general (`Derived`
//!   metadata, switch wire metadata): the same author shapes lower to the
//!   equivalent closure forms — same observable reactivity, no wire
//!   metadata.
//!
//! Other known gaps: styled-text runs (`create_styled_text`) are created
//! once and not re-realized on a theme swap, and native container-query
//! feedback has no container-width signal (see `style_attach`).
//!
//! ## Node associated type
//!
//! Every Ops trait is a subtrait of [`runtime_scene::Host`], so
//! `Self::Node` is Host's associated node type — declared exactly once
//! and shared by every Ops trait. A generic handler bounds on what it
//! needs (`fn mount_text<B: TextOps + StyleOps>(…) -> B::Node`) and the
//! node types unify through the common supertrait. Host additionally
//! requires `Node: Clone + 'static` and `Self: 'static` — structural
//! regions retain node handles across effect fires.
//!
//! ## Cross-trait defaults
//!
//! A default that calls a method of *another* Ops trait is expressed as a
//! supertrait bound, so a backend that overrides nothing still gets it:
//!
//! - `caps::ExternalOps` is the supertrait of every `create_*` whose
//!   default renders the `missing_primitive_placeholder` (image, icon,
//!   text-input, toggle, slider, activity, virtualizer, grid, graphics,
//!   portal, navigator) — a primitive the backend didn't compile in
//!   degrades to a placeholder instead of failing.
//! - `caps::ViewOps` is the supertrait of defaults that lower to
//!   `create_view` (pressable, link, presence placeholder, document
//!   `create_element`).
//! - `caps::ButtonOps: TextOps` because `update_button_label`'s default
//!   lowers to `update_text`.
//! - Defaults that call methods *within the same trait*
//!   (`create_styled_text` → `create_text`, `apply_styled_variants` →
//!   `apply_styled_states` → `apply_style`, `note_virtualizer_binding` →
//!   `note_repeat_binding`, `execute_batch_with_attach` →
//!   `execute_batch` + `Host::insert_many`, `apply_scroll_view_safe_area_inset`
//!   → `apply_safe_area_padding`, `missing_primitive_placeholder` →
//!   `create_external`) need no extra bound.
//!
//! ## Where the caps signatures' types live
//!
//! The prop/data types the caps signatures reference (`StyleRules`,
//! `AccessibilityProps`, `TouchHandler`, the handler aliases, the
//! `primitives::*` payloads, the handle types, …) all live in
//! `runtime-shared`. This crate does not depend on the author-facing
//! `runtime-core` root; `runtime-core` re-exports this crate's `glue`.

// `resource()` / `mutation()` (feature `async-driver`).
#[cfg(feature = "async-driver")]
pub mod async_reactive;
pub mod backend;
pub mod builders;
pub mod callback_guard;
pub mod caps;
pub mod glue;
// `glue::primitives::lazy` lives in its own file (re-exported into the
// inline `glue::primitives` module — the canonical path) — the
// `#[component(lazy)]` emission surface.
#[doc(hidden)]
pub mod glue_lazy;
pub mod handlers;
/// The bounds generic `#[host_fn]`s are written against (`Key`, `Opaque`,
/// `Numeric`). In every build: such a function compiles with or without
/// remote components.
pub mod host_types;
pub mod prims;
#[cfg(all(feature = "remote", any(not(target_arch = "wasm32"), idealyst_stream_guest)))]
pub mod remote;

/// `#[component]`'s emission for this build: a remote bundle build (the
/// vocabulary's `idealyst_stream_guest`), where an app component's body is
/// replaced by an import of the app's copy, or any other build. Chosen by
/// the VOCABULARY's flag, so a crate using `#[component]` declares no cfg
/// (an emitted `#[cfg(idealyst_stream_guest)]` warned in every such crate).
#[cfg(idealyst_stream_guest)]
#[macro_export]
#[doc(hidden)]
macro_rules! __remote_guest_split {
    (bundle: { $($b:tt)* } app: { $($a:tt)* }) => { $($b)* };
}
#[cfg(not(idealyst_stream_guest))]
#[macro_export]
#[doc(hidden)]
macro_rules! __remote_guest_split {
    (bundle: { $($b:tt)* } app: { $($a:tt)* }) => { $($a)* };
}

/// A `ui!` call of a component: `BuildElement::build_set` with the props
/// the call site set. In a remote bundle build, the set goes as a type
/// (`__build_site`), so an imported app component can leave a constant
/// record of the call site (`remote::site`) — which props of which app
/// component the bundle uses, kept only if the call site is reachable.
#[cfg(all(feature = "remote", idealyst_stream_guest))]
#[macro_export]
#[doc(hidden)]
macro_rules! __remote_site {
    ($props:expr, [$($s:literal),* $(,)?]) => {{
        struct __Set;
        impl $crate::glue::PropSet for __Set {
            const SET: &'static [&'static str] = &[$($s),*];
            const JOINED: &'static str = ::core::concat!($($s, "\n"),*);
        }
        $crate::glue::BuildElement::__build_site::<__Set>($props)
    }};
}
#[cfg(not(all(feature = "remote", idealyst_stream_guest)))]
#[macro_export]
#[doc(hidden)]
macro_rules! __remote_site {
    ($props:expr, [$($s:literal),* $(,)?]) => {
        $crate::glue::BuildElement::build_set($props, &[$($s),*])
    };
}

/// `#[component(remote)]`'s emission for this build: where the body is
/// compiled (a remote bundle, or a web app — remote is a native mechanism)
/// `body`, else `native` (an app that mounts the component from a bundle).
/// The vocabulary's flags decide, so the component's crate declares no cfg.
#[cfg(any(idealyst_stream_guest, target_arch = "wasm32"))]
#[macro_export]
#[doc(hidden)]
macro_rules! __remote_component {
    (body: { $($b:tt)* } native: { $($n:tt)* }) => { $($b)* };
}
#[cfg(not(any(idealyst_stream_guest, target_arch = "wasm32")))]
#[macro_export]
#[doc(hidden)]
macro_rules! __remote_component {
    (body: { $($b:tt)* } native: { $($n:tt)* }) => { $($n)* };
}

/// A native app mounting a remote component needs the vocabulary's
/// `remote` feature (the real macro lives in `remote`): without it, say so.
#[cfg(not(all(feature = "remote", any(not(target_arch = "wasm32"), idealyst_stream_guest))))]
#[macro_export]
#[doc(hidden)]
macro_rules! __remote_native_body {
    ($($t:tt)*) => {
        // Braced: valid in item and expression position alike.
        ::core::compile_error! {
            "#[component(remote)] in a native app needs runtime-vocabulary's `remote` feature: \
             the app mounts the component from a bundle"
        }
    };
}

/// `#[derive(Remote)]`'s output: kept in a build that hosts or is a remote
/// bundle, dropped (unexpanded) everywhere else — so a library can derive
/// it unconditionally and cost nothing in apps without remote components.
#[cfg(all(feature = "remote", any(not(target_arch = "wasm32"), idealyst_stream_guest)))]
#[macro_export]
#[doc(hidden)]
macro_rules! __remote_enabled {
    ($($t:tt)*) => { $($t)* };
}
#[cfg(not(all(feature = "remote", any(not(target_arch = "wasm32"), idealyst_stream_guest))))]
#[macro_export]
#[doc(hidden)]
macro_rules! __remote_enabled {
    ($($t:tt)*) => {};
}

// What `#[component]` / `#[props]` emit for remote components, as no-ops in
// every build that neither hosts nor is a remote bundle (the real ones live
// in `remote`). Each pair's cfgs are exact complements.
#[cfg(not(all(feature = "remote", any(not(target_arch = "wasm32"), idealyst_stream_guest))))]
#[macro_export]
#[doc(hidden)]
macro_rules! __remote_nav_handle {
    ($($t:tt)*) => {};
}
#[cfg(not(all(feature = "remote", any(not(target_arch = "wasm32"), idealyst_stream_guest))))]
#[macro_export]
#[doc(hidden)]
macro_rules! __remote_props {
    ($($t:tt)*) => {};
}
#[cfg(not(all(feature = "remote", any(not(target_arch = "wasm32"), idealyst_stream_guest))))]
#[macro_export]
#[doc(hidden)]
macro_rules! __remote_keyed {
    ($($t:tt)*) => {};
}
#[cfg(not(all(feature = "remote", not(target_arch = "wasm32"), not(idealyst_stream_guest))))]
#[macro_export]
#[doc(hidden)]
macro_rules! __remote_key {
    ($($t:tt)*) => {};
}
#[cfg(not(all(feature = "remote", not(target_arch = "wasm32"), not(idealyst_stream_guest))))]
#[macro_export]
#[doc(hidden)]
macro_rules! __remote_app_component {
    ($($t:tt)*) => {};
}
#[cfg(not(all(feature = "remote", not(target_arch = "wasm32"), not(idealyst_stream_guest))))]
#[macro_export]
#[doc(hidden)]
macro_rules! __remote_mount_entry {
    ($($t:tt)*) => {};
}
#[cfg(not(all(feature = "remote", any(not(target_arch = "wasm32"), idealyst_stream_guest), any(idealyst_stream_guest, feature = "remote-loopback"))))]
#[macro_export]
#[doc(hidden)]
macro_rules! __remote_import {
    // Only ever expanded in a remote bundle build (`__remote_guest_split!`'s
    // bundle branch), so reaching this twin means that build lacks the
    // vocabulary's `remote` feature: say so at compile time, rather than
    // compile a bundle whose every app component traps.
    ($($t:tt)*) => {
        ::core::compile_error! {
            "a remote bundle build needs runtime-vocabulary's `remote` feature: \
             it imports the app's components"
        }
    };
}
#[cfg(feature = "robot")]
pub mod robot;
#[cfg(feature = "robot")]
pub(crate) mod robot_highlight;
// Always compiled (inert without `robot`): extension handlers register
// their nodes through it unconditionally.
pub mod robot_ext;
// Always compiled (inert without `robot`): `#[robot_fn]` emits
// `robot_fn::…` and SDKs register their test controls unconditionally.
pub mod robot_fn;
// Always compiled (stub-shaped without `robot`): every `#[component]`
// emits `glue::__inspect::…` (and a `#[method]` component `glue::robot::…`)
// unconditionally (see robot_methods' module docs).
#[doc(hidden)]
pub mod robot_methods;
// Always compiled for the same reason: `#[component]` / `#[props]` emit
// the prop probes unconditionally (called only in robot builds).
#[doc(hidden)]
pub mod robot_props;
#[cfg(feature = "robot")]
#[doc(hidden)]
pub mod robot_watch;
// `spawn_then` — detached IO, result applied inside a turn (feature
// `async-driver`, same gate as the `spawn_async` it builds on).
#[cfg(feature = "async-driver")]
pub mod scoped_spawn;
pub mod scoped_scheduling;
pub mod slots;
// The dev-time overlay (`ui-overlay`): the per-node origin tag a patch
// addresses nodes by, and the split-pass version marker. No descriptor
// is compiled in — it is a build artifact. See src/overlay/mod.rs.
#[cfg(feature = "ui-overlay")]
pub mod overlay;
pub mod style_attach;
pub mod keyboard;
pub mod safe_area;
pub mod key_input;
pub mod app_input;
pub mod user_activity;
pub mod theme;
pub mod viewport;

pub use builders::{
    activity_indicator, anchored_overlay, button, graphics, icon, image, link, navigator_outlet,
    keyboard_avoiding_view, overlay, portal, presence, pressable, scroll_view, slider,
    stack_navigator, swap_navigator,
    text, text_area, text_input, toggle, view, virtualizer, SceneChild, TextContent,
};
pub use caps::AllCaps;
pub use handlers::{
    register_builtins, register_builtins_with, AllBuiltins, BuiltinSet, CoreOnly,
};

/// Re-export so the `builtin_set!` / `builtin_set_keep!` expansions can
/// spell `Registry` without the caller having to depend on `runtime-scene`
/// directly or import it under that name.
#[doc(hidden)]
pub use runtime_scene as __scene;
pub use style_attach::{
    attach_style, on_teardown, signal_class, IntoStyleProp, StyleProp, StyleServices,
};

/// Register one theme token into the MCP catalog.
///
/// A design system calls this from the same declaration that generates
/// the token's accessor, so tooling gets the vocabulary as data — the
/// editor completion behind `t.spacing.│`, and `list_tokens` over MCP.
///
/// ```ignore
/// register_style_token!("spacing-md", "spacing.md", "spacing", "Length", "IdeaThemeRef", {
///     format!("{}px", SpacingTokens.md().value())
/// });
/// ```
///
/// The `default_value` block is a closure body evaluated by the catalog
/// consumer, not at registration — point it at the accessor so the
/// displayed default IS the accessor's fallback.
///
/// **Why the gate lives here, not at the call site.** A `#[cfg(feature =
/// "catalog")]` written in the *calling* crate sees only THAT crate's
/// features, so a design system would have to declare and propagate its
/// own `catalog` feature — and the catalog wrapper only turns on
/// `runtime-core/catalog`, so those registrations would silently never
/// appear. Gating here follows the same feature the `__mcp` re-export
/// does, which is what `recipe!` effectively does from inside the proc
/// macro.
#[cfg(feature = "catalog")]
#[macro_export]
macro_rules! register_style_token {
    ($name:expr, $path:expr, $ns:expr, $ty:expr, $vocab:expr, $default:block) => {
        $crate::glue::__mcp::inventory::submit! {
            $crate::glue::__mcp::StyleTokenEntry {
                name: $name,
                path: $path,
                namespace: $ns,
                value_type: $ty,
                default_value: $crate::glue::__mcp::TokenDefault::Resolver(|| $default),
                vocabulary: $vocab,
            }
        }
    };
}

/// No-op form when the catalog is off — see the gated definition above.
/// Expands to nothing so a design system's token declarations cost
/// exactly zero in shipped apps.
#[cfg(not(feature = "catalog"))]
#[macro_export]
macro_rules! register_style_token {
    ($name:expr, $path:expr, $ns:expr, $ty:expr, $vocab:expr, $default:block) => {};
}

/// Wraps an expression as a reactive prop value
/// ([`glue::Reactive::derive`]). Defined here (not in `runtime-core`) so
/// the `$crate::…` expansion resolves against the vocabulary; the
/// `runtime-core` root re-exports it as `runtime_core::rx!`.
#[macro_export]
macro_rules! rx {
    ($e:expr) => {
        $crate::glue::Reactive::derive(move || $e)
    };
}

/// A scope-owned reactive effect over a bare BLOCK: the world effect is
/// collected by the ambient collector (the `component_scope` `Owned`) and
/// dropped with it. Takes a block — `effect!({ … })`; a closure argument
/// (`effect!(move || …)`) compiles but only builds the closure, so the
/// effect body never runs.
#[macro_export]
macro_rules! effect {
    ($body:expr) => {
        let _ = $crate::glue::effect(move || {
            $body;
        });
    };
}

/// Constructs a [`glue::animation::AnimatedValue`], the per-frame motion
/// handle `subscribe_and_apply(..)` / `.animate(..)` / [`timeline!`] drive.
///
/// Lands on the glue wrapper, not `runtime_shared::animation::AnimatedValue`:
/// the wrapper's `bind*` keeps the subscription alive through a world
/// effect owned by the component scope (see [`glue::animation`]). Defined
/// here for the same reason as [`rx!`]/[`effect!`]; re-exported as
/// `runtime_core::animated!`.
#[macro_export]
macro_rules! animated {
    ($value:expr) => {
        $crate::glue::animation::AnimatedValue::new($value)
    };
}

/// Schedules animator acts at session-epoch-relative times:
/// `timeline! { 0 => { av: anim }, 300 => { … } }`. Each act goes through
/// [`glue::session::after_ms`], so pending acts are cancelled when the
/// enclosing component scope drops (see [`scoped_scheduling`]). Defined
/// here for the same reason as [`rx!`]/[`effect!`]; re-exported as
/// `runtime_core::timeline!`.
#[macro_export]
macro_rules! timeline {
    ( $( $at:expr => { $( $av:ident : $animator:expr ),* $(,)? } ),* $(,)? ) => {{
        $(
            {
                let __at: u64 = $at as u64;
                $(
                    {
                        let __av = $av.clone();
                        $crate::glue::session::after_ms(__at, move || {
                            __av.animate($animator);
                        });
                    }
                )*
            }
        )*
    }};
}
