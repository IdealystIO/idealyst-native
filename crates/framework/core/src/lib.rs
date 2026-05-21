//! Framework core: primitives, Backend trait, render walker, reactivity.

pub mod animation;
pub mod assets;
mod backend;
mod batch;
mod builder;
mod derive;
mod external;
mod handles;
mod identity;
mod primitive;
mod reactive;
mod safe_area;
pub mod scheduling;
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

#[cfg(feature = "debug-stats")]
pub mod debug;

#[cfg(feature = "robot")]
pub mod robot;

/// Re-export of `serde_json` for use by the `#[component]` macro's
/// `methods!` auto-registration codegen — proc macros emit absolute
/// paths and we don't want every consuming crate to take a direct
/// dep on `serde_json`.
#[doc(hidden)]
pub use serde_json as __serde_json;

pub use assets::{
    Asset, AssetId, AssetKind, AssetSource, AssetTag, SystemFallback, Typeface, TypefaceFace,
    TypefaceId,
};
pub use backend::{Backend, ColorScheme, VirtualizerCallbacks};
pub use batch::{BackendBatch, BatchOp};
pub use handles::{
    ButtonHandle, ButtonOps, PressableHandle, PressableOps, RefFill, RefOps, StateBits, TextHandle,
    TextOps, ViewHandle, ViewOps,
};
pub use builder::{
    button, pressable, switch, text, view, when, Bindable, Bound, ChildList, IntoDisabledSource,
    IntoPrimitive,
};
pub use derive::{Action, Derived, IntoAction, IntoDerived};
pub use identity::{
    current_identity, hash_key, style_path_hash, use_id, use_id_keyed, with_current_identity,
    Identity,
};
pub use primitive::Primitive;
pub use sources::{
    signal_class, IntoStyleSource, IntoTextSource, JsBindingSpec, SignalClassSpec, StyleSource,
    TextSource,
};
pub use touch::{TouchEvent, TouchHandler, TouchId, TouchPhase, TouchPoint, TouchResponse};
pub use touch::recognizers::{
    long_press, pan, tap, LongPressRecognizer, PanEvent, PanRecognizer, TapRecognizer,
};
pub use walker::{mount, render, Owner};
pub use primitives::navigator::{
    match_pattern, ContentBuilder, DefaultLinkKind, DrawerContentProps, DrawerHandle,
    DrawerNavigator, DrawerNavigatorCallbacks, DrawerSide, DrawerType, HeaderButton, HeaderStyle,
    LayoutPlan, LayoutProps, MountPolicy, MountResult, NavCommand, NavState, Navigator,
    NavigatorCallbacks, NavigatorControl, NavigatorHandle, NavigatorOps, Route, RouteParams,
    Screen, ScreenOptions, TabNavigator, TabNavigatorCallbacks, TabPlacement, TabRegistration,
    TabSpec, TabsHandle,
};
pub use primitives::icon::{icon, FillRule, IconData, IconHandle, IconOps, StrokeAnimation};
pub use primitives::image::{image, image_asset, ImageHandle, ImageOps};
pub use primitives::key::{KeyEvent, KeyOutcome};
pub use primitives::text_input::{text_input, TextInputHandle, TextInputOps};
pub use primitives::text_area::{text_area, TextAreaHandle, TextAreaOps};
pub use primitives::code_block::{code_block, CodeBlockHandle};
pub use primitives::toggle::{toggle, ToggleHandle, ToggleOps};
pub use primitives::web_view::{web_view, WebViewHandle, WebViewOps};
pub use primitives::overlay::{
    anchored_overlay, overlay, AnchoredOverlayBuilder, BackdropMode, OverlayBuilder,
};
pub use primitives::flat_list::{flat_list, fixed_size, FlatListItemSize};
pub use primitives::link::NavKind;
pub use primitives::portal::{
    portal, AnchorTarget, AnchorableHandle, ElementAlign, ElementSide, PortalHandle,
    PortalOps, PortalTarget, ViewportPlacement, ViewportRect,
};
pub use external::{external, ErasedHandler, ExternalHandle, ExternalRegistry};
pub use primitives::presence::{
    presence, PresenceAnim, PresenceHandle, PresenceOps, PresenceState,
};
pub use reactive::{
    arena_stats, batch, inject, inject_or, memo, memo_with, on, on_cleanup, on_defer, provide,
    reducer, register_signal_js_notifier, signal_has_js_notifier, unregister_signal_js_notifier,
    untrack, with_inject, ArenaStats, Effect, Ref, Signal, Trackable,
};
#[cfg(feature = "async-driver")]
pub use resource::{resource, Resource, ResourceCancel, ResourceState};
pub use safe_area::{safe_area_insets, set_safe_area_insets, EdgeInsets, SafeAreaSides};
pub use scheduling::{
    after_animation_frame, after_ms, raf_loop, schedule_microtask, RafLoop, ScheduledTask,
};

pub use style::{
    derived, install_tokens, pregenerate, resolve as resolve_style,
    take_pending_token_updates, update_tokens, AlignContent, AlignItems, AlignSelf, Color,
    Derive, Easing, FlexDirection, FlexWrap, FontFamily, FontStyle, FontWeight, Gradient,
    GradientKind, GradientStop, IntoOverrideSource, IntoVariantSource, JustifyContent, Length, RadialExtent,
    Overflow, Position, Shadow, StyleApplication, StyleRules, StyleSheet, TextAlign,
    TextTransform, TokenEntry, TokenValue, Tokenized, Transform, Transition, VariantAxis,
    VariantEnum, VariantSet, VariantValue,
};

pub use framework_macros::{
    component, jsx, stylesheet, text_fmt, ui,
};

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

// Re-export of `framework_hot` so the `#[component]` macro's
// generated code can reach it via a path that's available to every
// user crate that depends on framework-core. The macro emits
// `::framework_core::__hot::call(...)`; users don't have to add
// `framework-hot` to their own `Cargo.toml`. Hidden from rustdoc —
// not part of the author-facing surface.
#[cfg(feature = "hot-reload")]
#[doc(hidden)]
pub use framework_hot as __hot;

// This crate root used to be ~3200 lines containing everything below.
// Each major concern now lives in its own private submodule and the
// crate's public surface is preserved via the `pub use` block above:
//
//   sources.rs   — TextSource / IntoTextSource / StyleSource /
//                  IntoStyleSource
//   handles.rs   — StateBits / ButtonHandle / ViewHandle / TextHandle /
//                  RefOps / RefFill
//   primitive.rs — the `Primitive` enum and `impl Primitive`
//   builder.rs   — Bound<H> / Bindable<H> / ChildList /
//                  IntoDisabledSource / IntoPrimitive plus the
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

/// Builds a `Vec<Primitive>` from a mixed-shape list of children.
///
/// Each argument must implement [`ChildList`]; the macro flattens
/// `Option<Primitive>` (skipping `None`) and `Vec<Primitive>` (extending
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
        let mut __c: ::std::vec::Vec<$crate::Primitive> = ::std::vec::Vec::new();
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

/// Builds a `Vec<ScheduledTask>` from a declarative multi-phase
/// animation timeline. Each `at => { ... }` clause fires one or
/// more `av.animate(animator)` calls at that moment; AnimatedValue
/// handles are cloned into per-task closures automatically.
///
/// Hold the returned Vec alive (typically via `on_cleanup(move ||
/// drop(tasks))` inside an `effect!`) for the duration you want
/// the timers to fire — dropping it cancels every pending
/// dispatch.
///
/// ```ignore
/// let tasks = timeline! {
///     400 => {
///         opacity: TweenTo::new(1.0, Duration::from_millis(700)).ease_out(),
///         scale: SpringTo::new(1.0).stiffness(170.0).damping(22.0),
///     },
///     2_400 => {
///         opacity: TweenTo::new(0.0, Duration::from_millis(500)).ease_in_out(),
///     },
/// };
/// on_cleanup(move || drop(tasks));
/// ```
///
/// AV slot must be a bare identifier (`opacity`, `welcome_color`)
/// because the macro clones the handle by writing `$av.clone()`.
/// For more complex sources (`self.av`, `foo.bar.av`), use
/// [`animate_at!`] directly.
#[macro_export]
macro_rules! timeline {
    ( $( $at:expr => { $( $av:ident : $animator:expr ),* $(,)? } ),* $(,)? ) => {{
        let mut __tasks: ::std::vec::Vec<$crate::ScheduledTask> = ::std::vec::Vec::new();
        $(
            {
                let __at = $at;
                $(
                    {
                        let __av = $av.clone();
                        __tasks.push($crate::after_ms(__at, move || {
                            __av.animate($animator);
                        }));
                    }
                )*
            }
        )*
        __tasks
    }};
}

