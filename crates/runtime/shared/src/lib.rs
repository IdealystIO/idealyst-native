//! # runtime-shared — the permanent shared runtime substrate
//!
//! The non-walker half of what was historically `runtime-core`,
//! extracted in the final phase of the idea-lite core migration so the
//! new core (`runtime-world` / `runtime-scene` / `runtime-vocabulary` /
//! `runtime-core`) and the style pipeline (`css`, `runtime-layout`)
//! can share these modules WITHOUT depending on the old walker,
//! `Element` wire tree, or `Backend` mega-trait.
//!
//! `runtime-core` depends on this crate and re-exports everything at
//! its old paths (`runtime_core::style`, `runtime_core::Signal`,
//! `runtime_core::primitives::icon::IconData`, …), so old-core
//! consumers compile unchanged. When the old core is deleted, this
//! crate remains as the substrate under the new core.
//!
//! ## What lives here
//!
//! - the style engine + data model (`style`, `sources`, `premint`,
//!   `container_query`, `styled_text`, `text_defaults`), colors, and
//!   the cached-stylesheet registry (single TLS authority — see the
//!   Android TLS-key-limit note on `cached_stylesheet`),
//! - assets + fonts (`assets`, `typeface!` / `face!` /
//!   `__face_source!`),
//! - the animation system, touch/hover/wheel/file-drop event types and
//!   gesture recognizers,
//! - the LEGACY reactive arena (`Signal` / `Effect` / `Ref` /
//!   `Reactive`). It lives here — not in the old core — because the
//!   shared substrate is *built on it*: the viewport/breakpoint/
//!   safe-area signals, the style engine's reactive sources, the
//!   animation bindings, session timers, and the robot watch registry
//!   all create and own arena slots. Moving the modules without their
//!   arena would have required forking every one of those TLS
//!   authorities (the state-split hazard). The arena is transitional
//!   here: when the old core dies and the last arena consumer migrates
//!   to `runtime-world`, it can be dropped from this crate.
//! - scheduling / time / session, viewport + breakpoint + safe-area
//!   TLS state, logging, debug phase counters,
//! - the host-registry surface (`host`): `Platform`, `ColorScheme`,
//!   `Screenshot`, `open_url` / `set_fullscreen` / `announce` and the
//!   per-thread slots backends install into at mount,
//! - the robot automation registry + bridge, native introspection,
//! - the per-primitive prop/handle STRUCTS (`primitives::*`,
//!   `handles`): handles, Ops traits, payload/data types — NOT the
//!   `Element`/`Bound` builder fns, which stay with the walker.
//!
//! ## What deliberately does NOT live here
//!
//! The old-core walker, `Element`, the `Backend` trait, the
//! `Bound`/builder authoring layer, `external`, and the compile-checked
//! `recipes` (they exercise the old author surface by name). Those stay
//! in `runtime-core` until it is deleted.
//!
//! ## TLS-authority rule
//!
//! Every thread-local in this crate has exactly ONE definition — this
//! crate's. `runtime-core` re-exports, never duplicates: a second copy
//! of (say) the viewport signal or the touch-claim slot would silently
//! split state between the two cores.

/// Panic with an actionable diagnostic in dev builds and a terse stable
/// code in release builds. (Same-shaped sibling of runtime-core's
/// `diag_panic!` — each crate carries its own copy because the macro is
/// crate-internal by design; it expands to a plain `panic!`, so there
/// is no shared-state hazard in the duplication.)
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
pub mod backend;
pub mod breakpoint;
pub mod by_identity;
pub mod container_query;
pub mod collections;
pub mod color;
pub mod host;
pub mod introspect;
#[doc(hidden)]
pub mod batch;
#[doc(hidden)]
pub mod derive;
#[doc(hidden)]
pub mod handles;
#[doc(hidden)]
pub mod identity;
pub mod logging;
/// Premint style-dump registry — only the CLI's ephemeral dump build
/// enables the feature; shipped builds never carry it.
#[cfg(feature = "style-dump")]
pub mod premint;
#[doc(hidden)]
pub mod reactive;
#[doc(hidden)]
pub mod reactive_value;
#[doc(hidden)]
pub mod safe_area;
pub mod num;
pub mod page_meta;
#[doc(hidden)]
pub mod viewport;
pub mod scheduling;
pub mod session;
pub mod time;
#[doc(hidden)]
pub mod sources;
#[doc(hidden)]
pub mod sticky;
pub mod style;
pub mod styled_text;
pub mod text_defaults;
pub mod unsupported;
#[doc(hidden)]
pub mod touch;
pub mod wheel;
pub mod hover;
pub mod file_drop;
pub mod primitives;

// Cross-platform per-frame + async-driver primitives. Off by default;
// see the `async-driver` feature in Cargo.toml.
#[cfg(feature = "async-driver")]
pub mod driver;

// `resource()` / `mutation()` / `async_reducer()` / `NetworkState` —
// async data as reactive primitives, gated with the driver they spawn
// through (same gates as the old core).
#[cfg(feature = "async-driver")]
#[doc(hidden)]
pub mod resource;
#[cfg(feature = "async-driver")]
#[doc(hidden)]
pub mod mutation;
#[cfg(feature = "async-driver")]
#[doc(hidden)]
pub mod async_reducer;
#[cfg(feature = "async-driver")]
#[doc(hidden)]
pub mod network_state;

#[cfg(feature = "debug-stats")]
pub mod debug;

// No-op `debug` shim when `debug-stats` is off — the reactive arena
// calls these unconditionally, and runtime-core / macro-emitted code
// resolves them through its re-export. Mirrors the real fns in
// `debug.rs`; each inlines to nothing. (See runtime-core's Cargo.toml
// for the cross-graph feature-unification rationale.)
#[cfg(not(feature = "debug-stats"))]
pub mod debug {
    /// No-op: component enter instrumentation is compiled out when
    /// `debug-stats` is off. Present so macro-emitted calls still
    /// resolve regardless of cross-crate feature unification.
    #[inline(always)]
    pub fn record_component_enter(_name: &'static str) {}
    /// No-op counterpart to [`record_component_enter`].
    #[inline(always)]
    pub fn record_component_exit(_name: &'static str) {}
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

// Core-primitive usage recipes as static catalog data (see the module
// docs for why they are text + `include_str!` here rather than
// compiled `recipe!` fns in runtime-core). The compile gate for the
// snippet sources lives in `crates/dev/newcore-app` (dual-core).
#[cfg(feature = "catalog")]
pub mod recipes;

#[cfg(feature = "robot")]
pub mod robot;

/// Stub `robot` surface compiled when the `robot` feature is OFF.
/// Same contract as the old runtime-core stub (which now re-exports
/// this one): the names macro-emitted registrations reference must
/// exist in every build, as zero-work no-ops.
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

/// Re-export of `serde_json` for macro codegen (the `#[component]`
/// `#[method]` registration paths) — moved here with the substrate;
/// runtime-core re-exports it at the old `runtime_core::__serde_json`
/// path.
#[doc(hidden)]
pub use serde_json as __serde_json;

/// Re-export of `wasm-split` (published as `wasm-splitter`, aliased
/// back via `package =`) so the `#[component(lazy)]` glue can reach the
/// `#[wasm_split]` attribute without every author crate declaring the
/// dep. Moved here with the anchor set; runtime-core re-exports it at
/// the old `runtime_core::__wasm_split` path.
#[doc(hidden)]
pub use wasm_split as __wasm_split;

/// Build-probe glue for the `#[component]` macro's untracked-build-read
/// diagnostic (see `reactive::maybe_warn_untracked_build_read`).
#[doc(hidden)]
pub use reactive::{__component_build_probe, ComponentBuildProbe};
#[cfg(debug_assertions)]
#[doc(hidden)]
pub use reactive::__take_untracked_build_read_warnings;

/// SipHash-free `HashMap`/`HashSet` for framework-internal collections
/// (see runtime-core's original rationale — moved verbatim).
pub use rustc_hash::{FxHashMap, FxHashSet};

pub use assets::{
    Asset, AssetId, AssetKind, AssetSource, AssetTag, SystemFallback, Typeface, TypefaceFace,
    TypefaceId,
};
pub use host::{
    announce, color_scheme, install_announcer, install_current_color_scheme,
    install_current_platform, install_fullscreen_setter, install_url_opener, open_url, platform,
    set_fullscreen, ColorScheme, Platform, Screenshot, VirtualizerCallbacks,
};
pub use accessibility::{
    AccessibilityAction, AccessibilityProps, AccessibilityTraits, LiveRegionPriority, Role,
};
pub use batch::{BackendBatch, BatchOp};
pub use by_identity::{ByIdentity, ByIdentityArc};
pub use handles::{
    ButtonHandle, ButtonOps, LayoutSubscription, PressableHandle, PressableOps, RefFill, RefOps,
    StateBits, TextHandle, TextOps, ViewHandle, ViewOps,
};
pub use derive::{Action, Derived, IntoAction, IntoDerived};
pub use identity::{
    current_identity, hash_key, style_path_hash, use_id, use_id_keyed, with_current_identity,
    Identity,
};
pub use reactive_value::Reactive;
pub use sources::{
    signal_class, IntoStyleSource, IntoTextSource, JsBindingSpec, ReactiveTextSlot,
    SignalClassSpec, StaticTextSlot, StyleSource, TextSlot, TextSlotPart, TextSource,
    __idealyst_text_from_parts,
};
pub use touch::{
    active_touch_claim, pointer_button, pointer_modifiers, set_active_touch_claim,
    set_pointer_button, set_pointer_modifiers, PointerButton, PointerModifiers, TouchEvent,
    TouchHandler, TouchId, TouchPhase, TouchPoint, TouchResponse,
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
pub use primitives::navigator::shared::{
    join_path, match_pattern, match_prefix, peek_initial_path, screen_query, screen_state,
    set_initial_path, take_initial_path, NavCommand, NavState, NavigatorControl, NavigatorHandle,
    NavigatorOps, Route, RouteParams, ScreenStateGuard,
};
pub use primitives::navigator::query::{
    split_query, strip_query, with_query, QueryParams, ScreenState,
};
pub use primitives::icon::{FillRule, IconData, IconHandle, IconOps, StrokeAnimation};
pub use primitives::image::{
    ImageErrorHandler, ImageHandle, ImageLoadEvent, ImageLoadHandler, ImageOps, ImageSource,
};
pub use primitives::key::{KeyEvent, KeyOutcome};
pub use primitives::text_input::{TextInputHandle, TextInputOps};
pub use primitives::text_area::{TextAreaHandle, TextAreaOps};
pub use primitives::toggle::{ToggleHandle, ToggleOps};
pub use primitives::overlay::BackdropMode;
pub use primitives::scroll_view::{ScrollViewHandle, ScrollViewOps};
pub use primitives::virtualizer::{
    Axis, ItemKey, ItemSize, Lanes, VirtualLayout, VirtualizerHandle,
};
pub use primitives::link::NavKind;
pub use primitives::portal::{
    AnchorTarget, AnchorableHandle, ElementAlign, ElementSide, PortalHandle, PortalOps,
    PortalTarget, ViewportPlacement, ViewportRect,
};
pub use primitives::presence::{PresenceAnim, PresenceHandle, PresenceOps, PresenceState};
pub use primitives::activity_indicator::{
    ActivityIndicatorHandle, ActivityIndicatorOps, ActivityIndicatorSize,
};
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

/// Run `f` with the reactive scope-ownership stack emptied — see the
/// original runtime-core docs (moved verbatim): use for global caches
/// whose first access might land in a transient scope.
pub fn unscope<R>(f: impl FnOnce() -> R) -> R {
    reactive::unscope(f)
}

/// Creates a signal — the canonical creation form. (Legacy-arena
/// `Signal`; the new core's `signal()` lives in `runtime-world`.)
#[track_caller]
pub fn signal<T: Clone + 'static>(value: T) -> Signal<T> {
    Signal::new(value)
}

/// Leptos-parity alias: idealyst's unified [`Signal`] is what Leptos
/// calls `RwSignal`. See [`Signal::set`]'s equality-guard divergence
/// note before porting retrigger-style code.
pub type RwSignal<T> = Signal<T>;

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
// NOTE: `after_ms_scoped` / `raf_loop_scoped` are deliberately absent —
// the old-arena scoped helpers are crate-internal (see their docs); the
// author-facing versions are the vocabulary's newcore-anchored shadows.
pub use scheduling::{
    after_animation_frame, after_ms, after_ms_detached, drain_buffered_microtasks,
    is_frame_active, raf_loop, schedule_microtask, set_frame_active, RafLoop, ScheduledTask,
};
pub use logging::{install_logger, is_logger_installed, log, LogLevel, Logger, StderrLogger};

pub use style::{
    cached_stylesheet, default_text_font, derived, install_tokens, pregenerate,
    pregenerate_and_seed, reset_for_ssg_render,
    empty_absolute_sheet, install_minted_classes, minted_class_known, premint_class_name,
    scan_minted_classes,
    resolve as resolve_style, set_app_background,
    set_app_key_handler, take_pending_app_key_handler, EMPTY_ABSOLUTE_CLASS,
    PREMINT_FONT_INHERIT_CLASS,
    set_default_text_font, set_scrollbar_theme, take_pending_token_updates, update_tokens,
    AlignContent, AlignItems, AlignSelf, Color, Cursor, Derive, DisplayKind, Easing, FlexDirection, FlexWrap,
    FontFamily, FontStyle, FontWeight, Gradient, GradientKind, GradientStop, GridPlacement, TrackSize,
    IntoOverrideSource, IntoVariantSource, JustifyContent, Length, ObjectFit, RadialExtent, Overflow,
    OverscrollBehavior,
    PointerEvents, Position, Shadow, StyleApplication, StyleRules, StyleSheet, TextAlign,
    TextTransform, UserSelect,
    NoTokens, ThemePalette, TokenEntry, TokenValue, TokenVocabulary, Tokenized, Transform,
    Transition,
    VariantAxis, VariantEnum, VariantSet, VariantValue,
};

pub use text_defaults::{
    effective_text_color, THEME_TEXT_COLOR_FALLBACK, THEME_TEXT_COLOR_TOKEN,
};

pub use styled_text::{TextRun, TextRunStyle};

pub use page_meta::{set_page_metadata, take_page_metadata, PageMetadata};

/// Wraps an expression as a reactive prop value
/// ([`Reactive::Dynamic`](crate::Reactive)) — see the original
/// runtime-core docs (moved with the `Reactive` type it constructs).
#[macro_export]
macro_rules! rx {
    ($e:expr) => {
        $crate::Reactive::derive(move || $e)
    };
}

/// Creates a **scope-owned** reactive effect (legacy arena) — expands
/// to [`Effect::scoped`]. Moved with the arena; `runtime_core::effect!`
/// re-exports it. See the original runtime-core docs.
#[macro_export]
macro_rules! effect {
    ($body:expr) => {
        // Scope-owned: the active scope adopts the slot and frees it on
        // teardown. Debug-asserts a scope is active (see `Effect::scoped`).
        $crate::Effect::scoped(move || { $body });
    };
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
/// keyword.
#[macro_export]
macro_rules! node_ref {
    () => {
        $crate::Ref::new()
    };
    ($t:ty) => {
        $crate::Ref::<$t>::new()
    };
}
