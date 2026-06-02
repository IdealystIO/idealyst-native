//! Framework core: primitives, Backend trait, render walker, reactivity.

pub mod accessibility;
pub mod animation;
pub mod assets;
mod backend;
pub mod breakpoint;
pub mod color;
mod batch;
mod builder;
mod derive;
mod external;
mod handles;
mod identity;
pub mod logging;
mod element;
mod reactive;
mod reactive_value;
mod safe_area;
pub mod page_meta;
mod viewport;
pub mod scheduling;
pub mod session;
pub mod time;
mod sources;
mod style;
mod touch;
mod walker;
pub mod primitives;

// Cross-platform per-frame + async-driver primitives. Off by default;
// see the `async-driver` feature in Cargo.toml.
#[cfg(feature = "async-driver")]
pub mod driver;

// `resource()` — async data as a reactive primitive. Depends on the
// async-driver feature for `spawn_async`; gated together.
#[cfg(feature = "async-driver")]
mod resource;

// `mutation()` — callback-driven async work as a reactive primitive.
// Sibling to `resource()`; same async-driver gate.
#[cfg(feature = "async-driver")]
mod mutation;

// `async_reducer()` — async dual of the sync `reducer()` in
// `reactive.rs`. Bridges caller-owned `Signal<S>` state to an
// async action via a reducer-shaped apply closure.
#[cfg(feature = "async-driver")]
mod async_reducer;

// `NetworkState` — collapsed enum projection of the state structs
// owned by `resource` and `mutation`. Lives next to those modules
// under the same async-driver gate so it can reference both.
#[cfg(feature = "async-driver")]
mod network_state;

#[cfg(feature = "debug-stats")]
pub mod debug;

#[cfg(feature = "robot")]
pub mod robot;

/// Stub `robot` surface compiled when the `robot` feature is OFF.
///
/// The `#[component]` macro emits `register_component(...)` + a keepalive
/// `Effect` for every `methods! { ... }` block UNCONDITIONALLY — gating
/// it on the *consuming* crate's `robot` feature was a footgun (a
/// scaffolded app, or idea-ui, never sets that feature, so its component
/// methods silently never registered; see
/// `regression_component_methods_register_without_local_feature`). So the
/// names the macro references must exist in every build. When `robot` is
/// off these are zero-work stubs: `register_component` builds nothing and
/// hands back an inert guard, so non-robot builds pay only for
/// constructing the `Method` vec (which the optimizer can largely strip).
#[cfg(not(feature = "robot"))]
pub mod robot {
    use std::rc::Rc;

    /// Stub mirror of the real [`robot::Method`](crate::robot). Same
    /// field shape so the macro's struct literal type-checks.
    pub struct Method {
        pub name: &'static str,
        pub args: &'static [(&'static str, &'static str)],
        pub invoke: Rc<dyn Fn(&serde_json::Value) -> Result<(), String>>,
    }

    /// Inert registration guard — no registry entry exists to remove.
    pub struct ComponentRegistration;

    /// No-op when the `robot` feature is off. The real implementation
    /// inserts into the thread-local component registry the bridge reads.
    pub fn register_component(
        _name: &'static str,
        _methods: Vec<Method>,
    ) -> ComponentRegistration {
        ComponentRegistration
    }
}

/// Re-export of `serde_json` for use by the `#[component]` macro's
/// `methods!` auto-registration codegen — proc macros emit absolute
/// paths and we don't want every consuming crate to take a direct
/// dep on `serde_json`.
#[doc(hidden)]
pub use serde_json as __serde_json;

/// Re-export of `wasm-split` (the runtime crate, published as
/// `wasm-splitter` and aliased back via `package =`) so the `lazy!`
/// macro's expansion can reach the `#[wasm_split]` attribute without
/// forcing every author crate to add the dep to its own
/// `[dependencies]`. Same pattern as `__serde_json` above.
#[doc(hidden)]
pub use wasm_split as __wasm_split;

pub use assets::{
    Asset, AssetId, AssetKind, AssetSource, AssetTag, SystemFallback, Typeface, TypefaceFace,
    TypefaceId,
};
pub use backend::{open_url, platform, Backend, ColorScheme, Platform, VirtualizerCallbacks};
pub use page_meta::{set_page_metadata, take_page_metadata, PageMetadata};
pub use batch::{BackendBatch, BatchOp};
pub use handles::{
    ButtonHandle, ButtonOps, LayoutSubscription, PressableHandle, PressableOps, RefFill, RefOps,
    StateBits, TextHandle, TextOps, ViewHandle, ViewOps,
};
pub use builder::{
    button, each_keyed, pressable, switch, text, view, when, Bindable, Bound, BuildElement,
    ChildList, IntoDisabledSource, IntoElement, ReactiveForEach, ReactiveListKeyed, StaticForEach,
};
pub use derive::{Action, Derived, IntoAction, IntoDerived};
pub use identity::{
    current_identity, hash_key, style_path_hash, use_id, use_id_keyed, with_current_identity,
    Identity,
};
pub use element::{EachKey, EachRowBuild, EachSnapshot, Element};
pub use reactive_value::Reactive;
pub use sources::{
    signal_class, IntoStyleSource, IntoTextSource, JsBindingSpec, SignalClassSpec, StyleSource,
    TextSource,
};
pub use touch::{TouchEvent, TouchHandler, TouchId, TouchPhase, TouchPoint, TouchResponse};
pub use touch::recognizers::{
    long_press, pan, tap, LongPressRecognizer, PanEvent, PanRecognizer, TapRecognizer,
};
pub use walker::{build_detached, mount, render, DetachedScope, Owner};
pub use primitives::navigator::{
    current_screen_state, join_path, match_pattern, match_prefix, peek_initial_path,
    set_initial_path, take_initial_path, MountResult, NavCommand,
    NavState, NavigatorConfig,
    NavigatorControl, NavigatorHandle, NavigatorHandler, NavigatorHost, NavigatorOps,
    NavigatorRegistry, Route, RouteParams, Screen, ScreenStateGuard,
};
pub use primitives::icon::{icon, FillRule, IconData, IconHandle, IconOps, StrokeAnimation};
pub use primitives::image::{image, image_asset, ImageHandle, ImageOps};
pub use primitives::key::{KeyEvent, KeyOutcome};
pub use primitives::text_input::{text_input, TextInputHandle, TextInputOps};
pub use primitives::text_area::{text_area, TextAreaHandle, TextAreaOps};
pub use primitives::toggle::{toggle, ToggleHandle, ToggleOps};
pub use primitives::overlay::{
    anchored_overlay, overlay, AnchoredOverlayBuilder, BackdropMode, OverlayBuilder,
};
pub use primitives::flat_list::{flat_list, fixed_size, FlatListItemSize};
pub use primitives::link::{external_link, NavKind};
pub use primitives::portal::{
    portal, AnchorTarget, AnchorableHandle, ElementAlign, ElementSide, PortalHandle,
    PortalOps, PortalTarget, ViewportPlacement, ViewportRect,
};
pub use external::{
    deserialize_external_payload, external, register_external_serde, serialize_external_payload,
    ErasedHandler, ExternalHandle, ExternalRegistry, RegisterExternal,
};
pub use primitives::presence::{
    presence, PresenceAnim, PresenceHandle, PresenceOps, PresenceState,
};
pub use reactive::{
    arena_stats, batch, inject, inject_or, install_drop_deferral, is_reactive_busy, memo,
    memo_with, on, on_cleanup, on_defer, provide, reducer, register_signal_js_notifier,
    signal_has_js_notifier, unregister_signal_js_notifier, untrack, with_inject, ArenaStats,
    Effect, Ref, Signal, Trackable,
};

/// Run `f` with the reactive scope-ownership stack emptied: signals and
/// memos created inside are **not** adopted by the surrounding render
/// scope — they live for the thread's lifetime (the same contract the
/// token registry relies on).
///
/// Use for **global caches**: a thread-lifetime registry or a lazily
/// cached memo (`thread_local OnceCell<Signal<_>>`) whose first access
/// might land in a *transient* scope (e.g. an SSR deferred chrome build,
/// or any backend that builds subtrees in short-lived scopes). Without
/// this, the cached signal id dangles when that first-touch scope drops
/// and its arena slot is recycled — a later read then type-mismatches.
///
/// Sibling to [`untrack`] (which disables dependency *tracking*); this
/// disables scope *ownership*.
pub fn unscope<R>(f: impl FnOnce() -> R) -> R {
    reactive::unscope(f)
}
#[cfg(feature = "async-driver")]
pub use resource::{resource, Resource, ResourceCancel, ResourceState};
#[cfg(feature = "async-driver")]
pub use mutation::{mutation, Mutation, MutationState};
#[cfg(feature = "async-driver")]
pub use async_reducer::{async_reducer, AsyncReducer, AsyncStatus};
#[cfg(feature = "async-driver")]
pub use network_state::NetworkState;
pub use safe_area::{safe_area_insets, set_safe_area_insets, EdgeInsets, SafeAreaSides};
pub use viewport::{set_viewport_size, viewport_size, ViewportSize};
pub use breakpoint::{
    breakpoints, current_breakpoint, install_breakpoints, Breakpoint, Breakpoints,
};
pub use scheduling::{
    after_animation_frame, after_ms, after_ms_scoped, is_frame_active, raf_loop,
    raf_loop_scoped, schedule_microtask, set_frame_active, RafLoop, ScheduledTask,
};
pub use logging::{install_logger, is_logger_installed, log, LogLevel, Logger, StderrLogger};

pub use style::{
    derived, install_tokens, pregenerate, reset_for_ssg_render, resolve as resolve_style,
    set_app_background, set_scrollbar_theme, take_pending_token_updates, update_tokens,
    AlignContent, AlignItems, AlignSelf, Color, Derive, Easing, FlexDirection, FlexWrap,
    FontFamily, FontStyle, FontWeight, Gradient, GradientKind, GradientStop,
    IntoOverrideSource, IntoVariantSource, JustifyContent, Length, RadialExtent, Overflow,
    Position, Shadow, StyleApplication, StyleRules, StyleSheet, TextAlign, TextTransform,
    TokenEntry, TokenValue, Tokenized, Transform, Transition, VariantAxis, VariantEnum,
    VariantSet, VariantValue,
};

pub use runtime_macros::{
    component, jsx, lazy, recipe, stylesheet, text_fmt, ui,
};

/// `#[idealyst_tool]` and `#[derive(IdealystSchema)]` — the
/// catalog-registration macros. **Always re-exported**, exactly like
/// `#[component]`, so author/SDK/idea-ui code can annotate freely
/// without feature-gating the import: the macros expand to a **no-op**
/// when neither `catalog` nor `strict-docs` is on (they reference no
/// `mcp-catalog` symbols in that path), so a normal production build
/// resolves them, emits nothing, and carries zero catalog data. They
/// emit catalog registrations only under `catalog`, and doc-enforcement
/// `compile_error!`s only under `strict-docs`. The `#[schema(...)]`
/// helper attribute is registered by the derive regardless of feature
/// (inert when off). A bare `pub use` of a macro has no binary cost.
pub use runtime_macros::{idealyst_tool, IdealystSchema};

/// Sentinel macro: marks a `text_fmt!` argument as a reactive
/// signal (rather than a captured value). Has no behavior on its
/// own — `text_fmt!` recognizes the `bind!(...)` token pattern
/// inside its argument list at macro-expansion time and treats the
/// inner expression as a `Signal<T>`. Calling `bind!` outside
/// `text_fmt!` errors at compile time.
///
/// ```ignore
/// // `id` captured, `global` subscribed:
/// text_fmt!("leaf {}: g={}", id, bind!(global))
/// ```
#[macro_export]
macro_rules! bind {
    ($e:expr) => {
        ::std::compile_error!("`bind!` is a sentinel for `text_fmt!` args only — \
                               using it outside `text_fmt!(...)` has no effect")
    };
}

/// Wraps an expression as a reactive prop value
/// ([`Reactive::Dynamic`](crate::Reactive)). Use it to pass an inline
/// computed value to a component's `Reactive<T>` prop so it stays
/// live: signals the expression reads become dependencies and the
/// component re-renders that prop when they change.
///
/// It's the reactive-prop analog of [`bind!`] for `text_fmt!` — an
/// explicit, type-driven opt-in (no `.get()` substring scanning).
/// Bare signals don't need it (`content = my_signal` is already
/// reactive via `IntoProp`); reach for `rx!` when the value is a
/// computed expression over one or more signals.
///
/// ```ignore
/// // computed text that re-renders when `count` changes:
/// Typography(content = rx!(format!("clicked {}×", count.get())))
/// ```
#[macro_export]
macro_rules! rx {
    ($e:expr) => {
        $crate::Reactive::derive(move || $e)
    };
}

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
// feature on — idea-ui, the welcome example, user apps. Without
// this re-export, every consumer crate would need to declare a
// direct dep on `mcp-catalog`. Hidden from rustdoc; not part of
// the author-facing surface. See `docs/mcp-catalog-spec.md`.
#[cfg(feature = "catalog")]
#[doc(hidden)]
pub use mcp_catalog as __mcp;

// This crate root used to be ~3200 lines containing everything below.
// Each major concern now lives in its own private submodule and the
// crate's public surface is preserved via the `pub use` block above:
//
//   sources.rs   — TextSource / IntoTextSource / StyleSource /
//                  IntoStyleSource
//   handles.rs   — StateBits / ButtonHandle / ViewHandle / TextHandle /
//                  RefOps / RefFill
//   primitive.rs — the `Element` enum and `impl Element`
//   builder.rs   — Bound<H> / Bindable<H> / ChildList /
//                  IntoDisabledSource / IntoElement plus the
//                  `view`/`text`/`button`/`when`/`switch` constructors
//   walker.rs    — `render`, `Owner`, the `build` walker, per-primitive
//                  builders, style attachment + theme cohort, and the
//                  reactive-branching builders (when/switch/presence).
//
// The `signal!` and `children!` macros stay here — `#[macro_export]`
// registers them at the crate root anyway, and keeping them next to
// the `pub use` block makes the public surface a single grep target.

/// Shorthand for `Signal::new(value)`. Equivalent in every way; just less
/// typing at the call site.
///
/// ```ignore
/// let count = signal!(0);
/// // same as: let count = Signal::new(0);
/// ```
#[macro_export]
macro_rules! signal {
    ($value:expr) => {
        $crate::Signal::new($value)
    };
}

/// Creates a reactive [`Effect`] that re-runs whenever a signal it
/// reads on its previous run changes. Equivalent to writing
/// `let _e = Effect::new(move || { ... })` but auto-binds the handle
/// to the surrounding block and skips the `move` keyword (always
/// implied — signal handles are `Copy`).
///
/// Inside a render scope the active `Scope` adopts the effect's
/// arena slot, so the macro's hidden binding's `Drop` is a no-op and
/// the effect lives until the scope ends. Outside any scope (tests,
/// top-level binaries), the binding keeps the effect alive until the
/// end of the enclosing block — call `Effect::new` directly and
/// capture the handle if you need longer lifetime.
///
/// ```ignore
/// let count = signal!(0);
/// effect!({
///     log("count is {}", count.get());
/// });
/// count.set(1); // re-runs the effect
/// ```
///
/// Pairs with [`on_cleanup`] for release semantics: the registered
/// callback fires before the effect's next re-run *and* on disposal.
///
/// ```ignore
/// effect!({
///     let task = after_ms(500, || tick());
///     on_cleanup(move || drop(task));
///     deps.get();
/// });
/// ```
#[macro_export]
macro_rules! effect {
    ($body:expr) => {
        // Hygienic binding: lives to the end of the enclosing block.
        // Inside an active scope this is a no-op handle; outside one
        // the binding is the slot's RAII guard.
        let _effect = $crate::Effect::new(move || { $body });
    };
}

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

/// Constructs a `Ref<H>` — the typed handle a backend mount-time
/// callback fills, that user code reads via `.with(|h| ...)`.
///
/// Two shapes:
///
/// ```ignore
/// let view_ref = node_ref!(ViewHandle);   // explicit handle type
/// let view_ref: Ref<ViewHandle> = node_ref!();  // let-binding type drives inference
/// ```
///
/// Spelled `node_ref!` (not `ref!`) because `ref` is a strict Rust
/// keyword; the macro is for the handle-on-a-mounted-backend-node
/// idiom either way.
#[macro_export]
macro_rules! node_ref {
    () => {
        $crate::Ref::new()
    };
    ($t:ty) => {
        $crate::Ref::<$t>::new()
    };
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
/// drops, every pending dispatch is cancelled. No explicit
/// `on_cleanup(move || drop(tasks))` boilerplate; the macro
/// expands to one internally.
///
/// ```ignore
/// effect!({
///     timeline! {
///         400 => {
///             opacity: TweenTo::new(1.0, Duration::from_millis(700)).ease_out(),
///             scale: SpringTo::new(1.0).stiffness(170.0).damping(22.0),
///         },
///         2_400 => {
///             opacity: TweenTo::new(0.0, Duration::from_millis(500)).ease_in_out(),
///         },
///     };
/// });
/// ```
///
/// AV slot must be a bare identifier (`opacity`, `welcome_color`)
/// because the macro clones the handle by writing `$av.clone()`.
/// For more complex sources (`self.av`, `foo.bar.av`), use
/// [`animate_at!`] directly. To keep the tasks alive past the
/// surrounding scope (rare), build the `Vec<ScheduledTask>` by
/// hand from `animate_at!` calls.
///
/// Outside any reactive scope the auto-anchor is a no-op and the
/// tasks fire freely — same posture as [`crate::on_cleanup`].
#[macro_export]
macro_rules! timeline {
    ( $( $at:expr => { $( $av:ident : $animator:expr ),* $(,)? } ),* $(,)? ) => {{
        // The offsets are interpreted as ms-since-session-epoch
        // (see [`$crate::session::epoch_micros`]). On first mount
        // the epoch is "now" so this matches the historical
        // behavior of scheduling at `delay = at` from `now`. After
        // a hot-patch rerender, the epoch is preserved — already-
        // elapsed acts fire immediately, and if the AVs they touch
        // were declared with [`$crate::session::animated`] (and so
        // retained their current value), the resulting tweens are
        // `current == target` no-ops. Net effect: the act timeline
        // doesn't visually replay on every save.
        //
        // Each fired body's task is anchored to the current
        // reactive scope via the underlying `after_ms_scoped`, so
        // scope cleanup cancels any timer that hasn't fired yet.
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

