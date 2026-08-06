//! # runtime-vocabulary — capability traits and the built-in primitive
//! vocabulary
//!
//! The capability-trait split of the deleted `Backend` mega-trait. One
//! Ops trait per primitive family lives in [`caps`]; every backend
//! implements [`runtime_scene::Host`] plus the thirty Ops traits
//! directly. (During the migration a `LegacyBridge<B>` adapter wrapped
//! an old `Backend` impl in this surface to prove the signature freeze
//! compiled; it was deleted with the trait it adapted. Its delegation
//! proof lives on in `tests/caps_conformance.rs`, which drives the same
//! thirty caps + seven `Host` ops over `host-mock`.)
//!
//! ## P2b — the built-in primitive vocabulary
//!
//! - [`prims`] — one payload struct per primitive (props:
//!   `runtime_world::Value<T>` for reactive-capable props, `Rc<dyn Fn>`
//!   for callbacks, plain fields for inert config).
//! - [`builders`] — the `ui!` contract: `view() -> ViewBuilder`, props
//!   as methods, `.child(...)` with closure children as structural
//!   holes, `.build() -> runtime_scene::Element`.
//! - [`handlers`] — generic mount fns bounded on exactly the caps they
//!   use; the old walker's per-primitive build modules, invariant for
//!   invariant. [`handlers::register_builtins`] installs all 13.
//! - [`style_attach`] — the shared style service: [`style_attach::StyleProp`]
//!   with the two P2 paths (static resolved rules, dynamic rules
//!   closure) and the [`style_attach::on_teardown`] probe.
//!
//! Backend-call-stream parity with the old walker is pinned by
//! `crates/dev/scene-parity`'s full-op golden suite.
//!
//! ## P2b scope — the 13 core primitives
//!
//! `view`, `text`, `button`, `pressable`, `image`, `icon`, `toggle`,
//! `slider`, `activity_indicator`, `link`, `scroll_view`, `text_input`,
//! `text_area`.
//!
//! **Explicitly deferred to later phases** (per the migration plan's
//! phasing — documented here so nothing is silently missing):
//!
//! - `virtualizer`/`flat_list`, `graphics`, `portal` (+ the
//!   `overlay`/`anchored_overlay` compositions) and `presence` HAVE
//!   landed (`handlers/{virtualizer,graphics,portal,presence}.rs`) —
//!   the virtualizer ports the closure path only; generator-backend
//!   structured bindings stay deferred with the rest of the wire
//!   metadata (below). Presence is re-expressed on the scene Dyn
//!   driver's retire hook (see `handlers/presence.rs` for the one
//!   sanctioned semantic change: mid-exit re-present rebuilds instead
//!   of reusing the exiting scope). The NAVIGATOR primitives (swap +
//!   stack + outlet) HAVE landed too (`handlers/navigator.rs`):
//!   screens are retained `Realized` subtrees, `SwapNav`/`StackNav`
//!   ride the world context, dispatch commits on the flush; still
//!   deferred from that port (each with its phase, listed in the
//!   handler's module docs): web URL sync + deferred initial mount
//!   (P3), native system-back + native push surfaces (P4/P5), robot
//!   nav registry (P5), stack per-screen header options + `link(route
//!   = …)` integration (P6);
//! - styled-text runs beyond the basic `create_styled_text` path (theme
//!   re-realization, `JsBinding` fan-out);
//! - hydration behavior (P3 web);
//! - safe-area *reactive* inset re-application (P3; P2 applies once at
//!   mount) and the native container-query inline-size feedback loop.
//!
//! The **style engine** (P3c) is ported: [`style_attach::StyleProp`]
//! carries the sheet paths (static + cohort, dynamic, signal-class,
//! preminted) on top of the P2 resolved-rules paths, and [`theme`] owns
//! the per-world token/version/cohort model. Remaining style gaps are
//! listed loudly in `style_attach`'s module docs (native container
//! feedback, native breakpoint re-fire, the JS class-binding fan-out).
//!
//! ## P3a — the macro glue ([`glue`])
//!
//! [`glue`] is the emission surface `ui!` / `#[component]` target when
//! `runtime-macros` is built with its `new-core` feature (the macros
//! retarget `::runtime_shared::…` paths in their OUTPUT to
//! `::runtime_vocabulary::glue::…`). Additional deferrals specific to
//! that path — each fails compilation under `new-core` with a message
//! naming its migration phase, never silently:
//!
//! - the `overlay` / `anchored_overlay` / `presence` / `graphics` /
//!   `flat_list` tags NOW LOWER (glue wrappers in
//!   `glue::primitives::{overlay,presence,graphics,flat_list}` over
//!   [`builders::overlay`], [`builders::anchored_overlay`],
//!   [`builders::presence`], [`builders::graphics`],
//!   [`builders::virtualizer`]); `flat_list` ports the old typed
//!   adapter onto the closure-form virtualizer;
//! - the virtualizer `for i in count_method(sig)` sugar stays deferred
//!   (it lowers to the structured generator-backend `Derived<usize>`
//!   count binding — wire metadata, post-P7 with the rest);
//! - ~~in-app `link(route = …)`~~ — SUPPORTED (P6 nav wave): the
//!   navigators `provide` a [`prims::LinkActivator`] around every
//!   screen/layout build (swap Selects, stack Pushes) and the link
//!   mount resolves it — the old ambient-navigator contract;
//!   `external = …` unchanged;
//! - ~~`test_id = …`~~ — SUPPORTED (P5 identity seam, brought forward):
//!   every prim the old core's `with_test_id` covers carries a
//!   `test_id` slot + builder setter, mount handlers register into the
//!   vocabulary-owned `robot` registry (`src/robot.rs`, behind the
//!   `robot` Cargo feature), and the macro lowers `test_id = …` to the
//!   setter on both cores. The P5 robot REMAINDER is also in: the
//!   `#[method]` component registry + element link (`robot_methods`,
//!   emission surface in `glue`), the signal watch registry
//!   (`robot_watch` / `robot::watch_signal`), and the navigator
//!   registry (`robot::register_navigator`, fed by
//!   `handlers/navigator.rs`) — all served by `robot::bridge`'s
//!   wire-identical `list_components` / `invoke_method` /
//!   `list_watched_signals` / `read_signal` / `list_navigators` /
//!   `get_navigator_state` verbs;
//! - `web_view` (dispatches through the old-core WebView SDK component;
//!   SDK retarget, P6);
//! - ~~`#[component(lazy)]` / `#[lazy]`~~ — SUPPORTED: the `lazy`
//!   chunk boundary is a vocabulary prim ([`prims::lazy`]) mounted by
//!   [`handlers::mount_lazy`] (imperative placeholder→body swap, no
//!   reactive anchor — SSR bytes stay identical to the old walker's),
//!   with the emission surface in `glue::primitives::lazy` (thunk-
//!   flavored loader: construction runs in the swap effect, under the
//!   world). `#[method]`
//!   blocks LOWER (P5 robot remainder) for the inline-props component
//!   shape; only the legacy explicit-props/`Bindable` form stays a
//!   loud macro error (its return type rides the old `Element`);
//! - structured generator-backend bindings (`Derived` metadata,
//!   `Element::Switch` wire metadata): lowered instead to the
//!   equivalent closure forms — same observable reactivity, no wire
//!   metadata (generator backends re-land post-P7). The static-range
//!   `Repeat` batching and the f-string `JsBinding` text fast path are
//!   PORTED (no longer deferred): `Element::Many(RepeatPrim)` +
//!   `handlers::repeat` drive one `execute_batch_with_attach` FFI on
//!   batching backends, and `TextSourceProp::JsBinding` +
//!   `register_reactive_text_binding` + per-signal notifier effects
//!   (`notify_signal_text_js`) deliver JS-side text fan-out.
//!
//! `COVERAGE.md` (crate root) maps every one of the Backend trait's
//! methods to its Ops trait — nothing is silently unaccounted for.
//!
//! ## Node associated type
//!
//! Every Ops trait is a subtrait of [`runtime_scene::Host`], so
//! `Self::Node` is Host's associated node type — declared exactly once,
//! shared by all thirty traits, and structurally identical to the old
//! `Backend::Node`. A generic handler bounds on what it needs
//! (`fn mount_text<B: TextOps + StyleOps>(…) -> B::Node`) and the node
//! types unify through the common supertrait. Host additionally requires
//! `Node: Clone + 'static` and `Self: 'static` — structural regions
//! retain node handles across effect fires.
//!
//! ## Cross-trait defaults
//!
//! Backend defaults that call *other* Backend methods are preserved as
//! supertrait bounds rather than moved to the bridge, so a future direct
//! implementor (P2b backends) keeps today's default behavior:
//!
//! - `caps::ExternalOps` is the supertrait of every `create_*` whose
//!   default renders the `missing_primitive_placeholder` (image, icon,
//!   text-input, toggle, slider, activity, virtualizer, graphics,
//!   portal, navigator) — a missing primitive degrades to an External
//!   placeholder, same as today.
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
//!   `create_external`) keep their bodies in place unchanged.
//!
//! ## Where the frozen signatures' types live
//!
//! The prop/data types the caps signatures reference (`StyleRules`,
//! `AccessibilityProps`, `TouchHandler`, the handler aliases, the
//! `primitives::*` payloads, the handle types, …) all live in
//! `runtime-shared`, the permanent substrate. This crate has no
//! dependency on the author-surface `runtime-core` root at all.

// `resource()` / `mutation()` new-core mirrors (feature `async-driver`,
// matching the old root's gate on the same names).
#[cfg(feature = "async-driver")]
pub mod async_reactive;
pub mod backend;
pub mod builders;
pub mod callback_guard;
pub mod caps;
pub mod glue;
// `glue::primitives::lazy` lives in its own file (re-exported into the
// inline `glue::primitives` module — the canonical path) — the
// `#[component(lazy)]` new-core emission surface.
#[doc(hidden)]
pub mod glue_lazy;
pub mod handlers;
pub mod prims;
#[cfg(feature = "robot")]
pub mod robot;
// Always compiled (stub-shaped without `robot`): the `#[component]`
// macro's `#[method]` emission references `glue::robot::…` +
// `glue::__component_root` unconditionally — old-core stub-module
// parity (see robot_methods' module docs).
#[doc(hidden)]
pub mod robot_methods;
#[cfg(feature = "robot")]
#[doc(hidden)]
pub mod robot_watch;
pub mod scoped_scheduling;
pub mod style_attach;
pub mod theme;
pub mod viewport;

pub use builders::{
    activity_indicator, anchored_overlay, button, graphics, icon, image, link, navigator_outlet,
    overlay, portal, presence, pressable, scroll_view, slider, stack_navigator, swap_navigator,
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

/// New-core mirror of `runtime_shared::rx!` — wraps an expression as a
/// reactive prop value ([`glue::Reactive::derive`]). Defined here (not in
/// `runtime-core`) so the `$crate::…` expansion resolves against the
/// vocabulary, keeping the facade paper-thin; the facade re-exports it
/// under the old `runtime_shared::rx` name for aliased SDK crates.
#[macro_export]
macro_rules! rx {
    ($e:expr) => {
        $crate::glue::Reactive::derive(move || $e)
    };
}

/// New-core mirror of `runtime_shared::effect!` — a scope-owned reactive
/// effect over a bare BLOCK. The old expansion is
/// `Effect::scoped(move || { $body })` (adopted by the active scope);
/// here the world effect is collected by the ambient collector (the
/// `component_scope` `Owned`), which is the same ownership contract.
#[macro_export]
macro_rules! effect {
    ($body:expr) => {
        let _ = $crate::glue::effect(move || {
            $body;
        });
    };
}

/// New-core mirror of `runtime_core::animated!` — constructs an
/// [`glue::animation::AnimatedValue`], the per-frame motion handle
/// `subscribe_and_apply(..)` / `.animate(..)` / [`timeline!`] drive.
///
/// The old macro expands to `$crate::animation::AnimatedValue::new($v)`
/// with `$crate` = runtime-core, i.e. the SHARED `AnimatedValue`
/// whose inherent `bind*` methods anchor via the old-core `on_cleanup`
/// — inert (silently dropped) on a new-core mount. This mirror lands on
/// the glue wrapper instead, whose `bind*` keeps the subscription alive
/// through a world effect (see [`glue::animation`]). Defined here rather
/// than in `runtime-core` for the same reason as [`rx!`]/[`effect!`]:
/// the `$crate::…` expansion has to resolve against the vocabulary; the
/// facade re-exports it under the old `runtime_core::animated` name so
/// aliased crates keep compiling `animated!(0.0_f32)` unchanged.
#[macro_export]
macro_rules! animated {
    ($value:expr) => {
        $crate::glue::animation::AnimatedValue::new($value)
    };
}

/// New-core mirror of `runtime_shared::timeline!` — same grammar, same
/// session-epoch-relative act schedule, but routed through
/// [`glue::session::after_ms`] so the tasks anchor to the NEW core's
/// scope (the old macro's `$crate::session::after_ms` resolves against
/// the real runtime-core, whose scope anchoring is old-core-only and
/// inert on a new-core mount — see [`scoped_scheduling`]). Defined here
/// (not in `runtime-core`) for the same reason as [`rx!`]/[`effect!`]:
/// the `$crate::…` expansion must resolve against the vocabulary; the
/// facade re-exports it under the old `runtime_shared::timeline` name.
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
