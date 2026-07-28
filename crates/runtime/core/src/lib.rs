//! Framework core: primitives, Backend trait, render walker, reactivity.

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
/// `cargo test --release` too.
macro_rules! diag_panic {
    ($code:literal, $fmt:literal $(, $arg:expr)* $(,)?) => {{
        #[cfg(debug_assertions)]
        { panic!(concat!("idealyst[", $code, "]: ", $fmt) $(, $arg)*) }
        #[cfg(not(debug_assertions))]
        { panic!(concat!("idealyst[", $code, "] (debug build has details)")) }
    }};
}
pub(crate) use diag_panic;

pub mod accessibility;
pub mod animation;
pub mod assets;
mod backend;
pub mod breakpoint;
pub mod container_query;
pub mod collections;
pub mod color;
pub mod introspect;
mod batch;
mod builder;
mod derive;
mod external;
mod handles;
mod identity;
pub mod logging;
mod element;
/// Premint style-dump registry — only the CLI's ephemeral dump build
/// enables the feature; shipped builds never carry it.
#[cfg(feature = "style-dump")]
pub mod premint;
mod reactive;
mod reactive_value;
mod safe_area;
pub mod num;
pub mod page_meta;
mod viewport;
pub mod scheduling;
pub mod session;
pub mod time;
mod sources;
// With `style-dynamic` off, the live engine's registration/resolution
// internals become unreachable (the walker arms that called them are
// compiled out) — that's the point, DCE drops them from the binary. The
// items stay compiled so the public API surface is identical in every
// configuration; silence the dead-code lint rather than cfg-ing the API.
#[cfg_attr(not(feature = "style-dynamic"), allow(dead_code))]
mod style;
pub mod styled_text;
pub mod text_defaults;
mod touch;
pub mod wheel;
pub mod hover;
pub mod file_drop;
mod walker;
pub mod primitives;

// Cross-platform per-frame + async-driver primitives. Off by default;
// see the `async-driver` feature in Cargo.toml.
#[cfg(feature = "async-driver")]
pub mod driver;

// Lets code inside this crate (recipes, macro expansions) refer to itself by
// its external name, so captured recipe source shows the paths an app author
// would actually write.
extern crate self as runtime_core;

/// Compile-checked usage recipes (docs / MCP catalog). The `recipe!` macro
/// self-gates on the `catalog` feature, so this is empty in production.
pub mod recipes;

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

// No-op `debug` shim for when THIS crate's `debug-stats` is off but the
// `#[component]` macro still emitted `runtime_core::debug::record_component_*`
// calls. That divergence is real: the macro's emission is gated on
// `runtime-macros/debug-stats`, which the resolver unifies in the *host*
// (proc-macro) graph — so enabling debug-stats on ONE workspace package turns
// the macro on for the whole build, while `runtime-core/debug-stats` (which
// gates the real module above, in the *target* graph) stays off for packages
// that didn't ask for it. Without this shim those expansions fail with
// "cannot find `debug` in `runtime_core`". The two fns are the only debug API
// the macro emits; they inline to nothing, preserving the feature's
// zero-overhead-when-off contract. See [[project_inventory_self_registration]]
// neighbours / the debug-stats notes in Cargo.toml.
#[cfg(not(feature = "debug-stats"))]
pub mod debug {
    /// No-op: component enter instrumentation is compiled out when
    /// `runtime-core/debug-stats` is off. Present so macro-emitted calls
    /// still resolve regardless of cross-crate feature unification.
    #[inline(always)]
    pub fn record_component_enter(_name: &'static str) {}
    /// No-op counterpart to [`record_component_enter`].
    #[inline(always)]
    pub fn record_component_exit(_name: &'static str) {}

    // Reactive-identity + transaction no-ops. Called UNCONDITIONALLY from
    // `reactive.rs` (always compiled), so they must resolve regardless of the
    // feature. Each inlines to nothing when `debug-stats` is off, preserving
    // the zero-overhead contract. Mirrors the real fns in `debug.rs`.
    #[inline(always)]
    pub fn record_signal_created(_id: u32, _location: &'static std::panic::Location<'static>) {}
    #[inline(always)]
    pub fn record_effect_created(
        _id: u32,
        _location: &'static std::panic::Location<'static>,
        _component: Option<&'static str>,
    ) {
    }
    #[inline(always)]
    pub fn label_effect(_id: u32, _label: &str) {}
}

#[cfg(feature = "robot")]
pub mod robot;

/// Stub `robot` surface compiled when the `robot` feature is OFF.
///
/// The `#[component]` macro emits `register_component(...)` + a keepalive
/// `Effect` for every `#[method]`-bearing component UNCONDITIONALLY — gating
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

    /// Stub mirror of the real `ComponentInstanceId` so the macro's
    /// `__component_root(.., registration.id())` call type-checks.
    #[derive(Copy, Clone)]
    pub struct ComponentInstanceId(pub u32);

    /// Inert registration guard — no registry entry exists to remove.
    pub struct ComponentRegistration;

    impl ComponentRegistration {
        /// Stub id — never used (the no-op `__component_root` ignores it).
        pub fn id(&self) -> ComponentInstanceId {
            ComponentInstanceId(0)
        }
    }

    /// No-op when the `robot` feature is off. The real implementation
    /// inserts into the thread-local component registry the bridge reads.
    pub fn register_component(
        _name: &'static str,
        _methods: Vec<Method>,
    ) -> ComponentRegistration {
        ComponentRegistration
    }
}

/// Tag a `#[component]`'s root primitive with its component instance so the
/// robot walker can link element↔component (for the inspector's "select an
/// element → call its methods"). Called UNCONDITIONALLY by the `#[component]`
/// macro for `#[method]`-bearing components — cfg-selected like
/// [`robot::register_component`]: a transparent `Element::Component` wrapper
/// when `robot` is on, an identity no-op (zero overhead, no wrapper) when off.
#[cfg(feature = "robot")]
#[doc(hidden)]
pub fn __component_root(child: Element, instance: robot::ComponentInstanceId) -> Element {
    Element::Component {
        instance,
        child: Box::new(child),
    }
}

#[cfg(not(feature = "robot"))]
#[doc(hidden)]
#[inline(always)]
pub fn __component_root(child: Element, _instance: robot::ComponentInstanceId) -> Element {
    child
}

/// Re-export of `serde_json` for use by the `#[component]` macro's
/// `#[method]` auto-registration codegen — proc macros emit absolute
/// paths and we don't want every consuming crate to take a direct
/// dep on `serde_json`.
#[doc(hidden)]
pub use serde_json as __serde_json;

/// Build-probe glue for the `#[component]` macro's untracked-build-read
/// diagnostic (see `reactive::maybe_warn_untracked_build_read`). Emitted
/// at the top of every component body; no-op in release builds.
#[doc(hidden)]
pub use reactive::{__component_build_probe, ComponentBuildProbe};
#[cfg(debug_assertions)]
#[doc(hidden)]
pub use reactive::__take_untracked_build_read_warnings;

/// Re-export of `wasm-split` (the runtime crate, published as
/// `wasm-splitter` and aliased back via `package =`) so the
/// `#[component(lazy)]` glue (and the deprecated `lazy!` macro) can
/// reach the `#[wasm_split]` attribute without forcing every author
/// crate to add the dep to its own `[dependencies]`. Same pattern as
/// `__serde_json` above.
#[doc(hidden)]
pub use wasm_split as __wasm_split;

/// SipHash-free `HashMap`/`HashSet` for framework-internal collections,
/// re-exported so backends share one hasher choice without their own
/// `rustc-hash` dep line. std's default `RandomState` costs ~11 KB of
/// SipHash machinery in every wasm bundle for DoS resistance these maps
/// don't need: they're keyed by framework-minted ids, node pointers, and
/// author-authored strings — never attacker-controlled input — and on
/// `wasm32-unknown-unknown` the "random" seed has no OS entropy source
/// anyway. Maps in **public API signatures** (route params, debug-stats
/// summaries) deliberately stay `std::collections::HashMap` so author
/// code sees the ordinary type. Same rationale as `reactive.rs`'s
/// long-standing Fx usage.
pub use rustc_hash::{FxHashMap, FxHashSet};

pub use assets::{
    Asset, AssetId, AssetKind, AssetSource, AssetTag, SystemFallback, Typeface, TypefaceFace,
    TypefaceId,
};
pub use backend::{
    announce, color_scheme, open_url, platform, set_fullscreen, Backend, ColorScheme, Platform,
    Screenshot, VirtualizerCallbacks,
};
pub use accessibility::{
    AccessibilityAction, AccessibilityProps, AccessibilityTraits, LiveRegionPriority, Role,
};
pub use page_meta::{set_page_metadata, take_page_metadata, PageMetadata};
pub use batch::{BackendBatch, BatchOp};
pub use handles::{
    ButtonHandle, ButtonOps, LayoutSubscription, PressableHandle, PressableOps, RefFill, RefOps,
    StateBits, TextHandle, TextOps, ViewHandle, ViewOps,
};
pub use builder::{
    button, dynamic, each_keyed, fragment, one_or_view, pressable, styled_text, switch, text, view,
    when, Bindable, Bound, BuildElement, ChildList, IntoDisabledSource, IntoElement, ReactiveCond,
    ReactiveForEach, ReactiveListKeyed, StaticCond, StaticForEach,
};
pub use derive::{Action, Derived, IntoAction, IntoDerived};
pub use identity::{
    current_identity, hash_key, style_path_hash, use_id, use_id_keyed, with_current_identity,
    Identity,
};
pub use element::{EachKey, EachRowBuild, EachSnapshot, Element};
pub use reactive_value::Reactive;
pub use sources::{
    signal_class, IntoStyleSource, IntoTextSource, JsBindingSpec, ReactiveTextSlot,
    SignalClassSpec, StaticTextSlot, StyleSource, TextSlot, TextSlotPart, TextSource,
    __idealyst_text_from_parts,
};
pub use touch::{
    active_touch_claim, pointer_modifiers, set_active_touch_claim, set_pointer_modifiers,
    PointerModifiers, TouchEvent, TouchHandler, TouchId, TouchPhase, TouchPoint, TouchResponse,
};
pub use wheel::{WheelEvent, WheelHandler, WheelKind};
pub use hover::HoverHandler;
pub use file_drop::{DroppedFile, FileDropEvent, FileDropHandler, FileDropPhase};
pub use touch::recognizer::{
    AsyncNotifier, GestureState, Recognizer, RecognizerCtx, RecognizerKind, RecognizerUpdate,
};
pub use touch::recognizers::{
    long_press, pan, pinch, rotate, swipe, tap, LongPress, LongPressRecognizer, Pan, PanEvent,
    PanRecognizer, Pinch, PinchEvent, PinchRecognizer, Rotate, RotateEvent, RotateRecognizer,
    Swipe, SwipeDirection, SwipeDirs, SwipeRecognizer, Tap, TapRecognizer,
};
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
pub use primitives::icon::{FillRule, IconData, IconHandle, IconOps, StrokeAnimation};
#[cfg(feature = "prim-image")]
pub use primitives::image::{image, image_asset, image_from};
pub use primitives::image::{
    ImageErrorHandler, ImageHandle, ImageLoadEvent, ImageLoadHandler, ImageOps, ImageSource,
};
pub use primitives::key::{KeyEvent, KeyOutcome};
#[cfg(feature = "prim-text-input")]
pub use primitives::text_input::text_input;
pub use primitives::text_input::{TextInputHandle, TextInputOps};
#[cfg(feature = "prim-text-input")]
pub use primitives::text_area::text_area;
pub use primitives::text_area::{TextAreaHandle, TextAreaOps};
#[cfg(feature = "prim-toggle")]
pub use primitives::toggle::toggle;
pub use primitives::toggle::{ToggleHandle, ToggleOps};
#[cfg(feature = "prim-portal")]
pub use primitives::overlay::{anchored_overlay, overlay};
pub use primitives::overlay::{AnchoredOverlayBuilder, BackdropMode, OverlayBuilder};
#[cfg(feature = "prim-virtualizer")]
pub use primitives::flat_list::{flat_list, fixed_size, FlatListItemSize};
pub use primitives::scroll_view::{scroll_view, ScrollViewHandle, ScrollViewOps};
#[cfg(feature = "prim-virtualizer")]
pub use primitives::virtualizer::virtualizer;
pub use primitives::virtualizer::{
    Axis, ItemKey, ItemSize, Lanes, VirtualLayout, VirtualizerHandle,
};
pub use primitives::link::{external_link, NavKind};
#[cfg(feature = "prim-portal")]
pub use primitives::portal::portal;
pub use primitives::portal::{
    AnchorTarget, AnchorableHandle, ElementAlign, ElementSide, PortalHandle,
    PortalOps, PortalTarget, ViewportPlacement, ViewportRect,
};
pub use external::{
    defer_external_registration, deserialize_external_payload, drain_external_registrations,
    external, has_pending_external_registrations, register_external_serde,
    serialize_external_payload, ErasedHandler, ExternalHandle, ExternalRegistry, RegisterExternal,
};
#[cfg(feature = "prim-presence")]
pub use primitives::presence::presence;
pub use primitives::presence::{PresenceAnim, PresenceHandle, PresenceOps, PresenceState};
pub use reactive::{
    arena_stats, batch, cycle, inject, inject_or, install_drop_deferral, install_reactive_idle_hook,
    is_reactive_busy, memo, memo_with, on, on_cleanup, on_defer, provide, reducer,
    register_signal_js_notifier, signal_has_js_notifier, unregister_signal_js_notifier, untrack,
    watch, with_inject, ArenaStats, Effect, ReadSignal, Ref, Signal, Subscription, Trackable,
    WriteSignal,
};
/// Internal re-export for the `#[component]` / `#[method]` codegen only —
/// hidden from the authoring surface. See `reactive::__component_keepalive_effect`.
#[doc(hidden)]
pub use reactive::__component_keepalive_effect;

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
pub use container_query::{
    container_axis_name, container_axis_threshold, CONTAINER_MIN_WIDTH_PREFIX,
};
pub use scheduling::{
    after_animation_frame, after_ms, after_ms_detached, after_ms_scoped, drain_buffered_microtasks,
    is_frame_active, raf_loop, raf_loop_scoped, schedule_microtask, set_frame_active, RafLoop,
    ScheduledTask,
};
pub use logging::{install_logger, is_logger_installed, log, LogLevel, Logger, StderrLogger};

pub use style::{
    cached_stylesheet, default_text_font, derived, install_tokens, pregenerate,
    pregenerate_and_seed, reset_for_ssg_render,
    resolve as resolve_style, set_app_background, set_app_key_handler, set_default_text_font,
    set_scrollbar_theme, take_pending_token_updates, update_tokens,
    AlignContent, AlignItems, AlignSelf, Color, Cursor, Derive, DisplayKind, Easing, FlexDirection, FlexWrap,
    FontFamily, FontStyle, FontWeight, Gradient, GradientKind, GradientStop, TrackSize,
    IntoOverrideSource, IntoVariantSource, JustifyContent, Length, ObjectFit, RadialExtent, Overflow,
    PointerEvents, Position, Shadow, StyleApplication, StyleRules, StyleSheet, TextAlign,
    TextTransform, UserSelect,
    TokenEntry, TokenValue, Tokenized, Transform, Transition, VariantAxis, VariantEnum,
    VariantSet, VariantValue,
};

pub use text_defaults::{
    effective_text_color, THEME_TEXT_COLOR_FALLBACK, THEME_TEXT_COLOR_TOKEN,
};

pub use styled_text::{TextRun, TextRunStyle};

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
/// when neither `catalog` nor `strict-docs` is on (they reference no
/// `mcp-catalog` symbols in that path), so a normal production build
/// resolves them, emits nothing, and carries zero catalog data. They
/// emit catalog registrations only under `catalog`, and doc-enforcement
/// `compile_error!`s only under `strict-docs`. The `#[schema(...)]`
/// helper attribute is registered by the derive regardless of feature
/// (inert when off). A bare `pub use` of a macro has no binary cost.
pub use runtime_macros::{idealyst_tool, IdealystSchema};

/// Wraps an expression as a reactive prop value
/// ([`Reactive::Dynamic`](crate::Reactive)). Use it to pass an inline
/// computed value to a component's `Reactive<T>` prop so it stays
/// live: signals the expression reads become dependencies and the
/// component re-renders that prop when they change.
///
/// An explicit, type-driven opt-in (no `.get()` substring scanning).
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
// The `signal` fn and `children!` macro stay here — `#[macro_export]`
// registers them at the crate root anyway, and keeping them next to
// the `pub use` block makes the public surface a single grep target.

/// Creates a signal — the canonical creation form.
///
/// ```ignore
/// let count = signal(0);
/// count.set(1);
/// ```
///
/// A plain function, not a macro: signal creation needs no token
/// manipulation (unlike `effect!`/`rx!`, which synthesize closures), so
/// the fn form is the whole story. Equivalent to [`Signal::new`];
/// scope adoption happens inside the constructor either way.
///
/// To watch a signal's live value in the Idealyst Inspector, mark it
/// explicitly with [`robot::watch_signal`](crate::robot::watch_signal)
/// (the value type must be `Debug`). Automatic watch-on-create was
/// attempted but is impossible to do safely in stable Rust: rendering a
/// value requires `Debug`, and forcing that on every signal breaks
/// signals over non-`Debug` types (closures, handles, `Option<MediaStream>`,
/// …) — and the "use Debug if present" trick is not inference-safe (it
/// fails to compile for inference-deferred sites like `signal(None)`).
#[track_caller]
pub fn signal<T: Clone + 'static>(value: T) -> Signal<T> {
    Signal::new(value)
}

/// Leptos-parity alias: idealyst's unified [`Signal`] is what Leptos
/// calls `RwSignal`, so ported code written as `RwSignal::new(v)`
/// resolves unchanged. New idealyst code should write `signal(v)`.
///
/// One deliberate semantic divergence: idealyst's [`Signal::set`] is
/// equality-guarded (a same-value write does not notify), while
/// Leptos's `set` never compares. Ported Leptos code that uses a
/// same-value `set` as a retrigger must switch those sites to
/// [`Signal::set_always`] or [`Signal::touch`].
pub type RwSignal<T> = Signal<T>;

// The `signal!` macro is gone: it carried no macro-only capability (it
// expanded verbatim to `Signal::new`), so the plain `signal(value)`
// function above replaced it outright. A `#[deprecated]` alias was tried
// first, but `use runtime_core::signal;` imports every namespace under
// that name — so the deprecation warning fired on the *import line* of
// every file using the fn form. Removal is the only clean end state;
// stragglers get "cannot find macro `signal`" and drop the `!`.

/// Creates a **scope-owned** reactive effect that re-runs whenever a
/// signal it read on its previous run changes. The `move` keyword is
/// always implied (signal handles are `Copy`), and there is no handle to
/// manage — the surrounding component scope owns the effect and frees it
/// on teardown.
///
/// `effect!` is for reactivity **inside the component tree**. It expands
/// to [`Effect::scoped`], which debug-asserts that a reactive scope is
/// active. To react to a signal from *outside* the tree — app init, an
/// async callback, a platform/service install — use [`watch`] and store
/// the returned [`Subscription`]; that is the form whose lifetime you own.
///
/// ```ignore
/// let count = signal(0);
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
        // Scope-owned: the active scope adopts the slot and frees it on
        // teardown. Debug-asserts a scope is active (see `Effect::scoped`).
        $crate::Effect::scoped(move || { $body });
    };
}

// The `memo!` macro is gone — the plain `memo(move || …)` function (in
// `reactive.rs`, re-exported at the root) is the canonical form. The
// macro only saved typing `move ||` around a closure the author writes
// anyway; by the same standard that removed `signal!`, that isn't
// enough token work to justify a second spelling. (`effect!` stays: it
// wraps a bare BLOCK — statements, not a closure — which a fn can't
// accept.) A deprecated alias was ruled out for the same reason as
// `signal!`: `use runtime_core::memo;` imports both namespaces, so the
// warning would fire on the import line of every fn-form user.

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

