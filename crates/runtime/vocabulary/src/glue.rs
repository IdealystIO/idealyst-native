//! # `glue` — the `ui!` / `#[component]` emission surface for the NEW core (P3a)
//!
//! When `runtime-macros` is built with its `new-core` feature, the macros
//! retarget every absolute `::runtime_shared::…` path in their OUTPUT to
//! `::runtime_vocabulary::glue::…` (see `runtime-macros/src/new_core.rs`).
//! This module therefore mirrors the *names and call shapes* the macro
//! emission relies on — `view(children)`, `text(content)`, `when(...)`,
//! `ChildList::append_to`, `IntoElement`, `BuildElement`, the
//! `StaticCond`/`ReactiveCond` and `StaticForEach`/`ReactiveForEach`
//! dispatch traits, the f-string slot machinery — implemented against the
//! new core: [`runtime_scene::Element`] + the P2b [`builders`](crate::builders)
//! + [`runtime_world`] reactivity.
//!
//! ## Const vs Dyn is preserved
//!
//! The old lowering's static/reactive distinction survives type-driven,
//! exactly as before: a literal or plain value lowers to
//! [`Value::Const`] (bound once, zero effects); a closure, `Signal`,
//! `ReadSignal`, or `Memo` lowers to [`Value::Dyn`] (one binding effect).
//! What is *dropped* is the old core's structured-binding metadata
//! (`Derived { method, inputs, initial }`, `Element::Switch`,
//! the virtualizer `for`-sugar): those carried wire
//! ids for generator backends (Roku) — pure metadata, no runtime
//! behavior on event-driven backends. Under `new-core` the macro lowers
//! the same author shapes to the equivalent *closure* forms (same
//! observable reactivity), and the generator-backend metadata is a
//! documented deferral (see the crate docs' deferred list).
//!
//! ## Lowering table (old emission → new emission)
//!
//! | `ui!` construct | old-core emission | new-core emission |
//! |---|---|---|
//! | `view { … }` | `runtime_shared::view(children)` | `glue::view(children)` → `ViewBuilder` → `Element::Item(ViewPrim)` |
//! | `text { "lit" }` | `text(TextSource::Static)` | `glue::text(Const)` |
//! | `text { move ‖ … }` | `text(closure)` → Effect | `glue::text(Dyn)` → binding effect |
//! | `text { "{sig} x" }` | `TextSlotPart` + old slot traits | `glue::TextSlotPart` → `Value<String>` (all-Const folds to Const) |
//! | `text { f(sig) }` | `TextSource::Bound(Derived{…})` | `text(move ‖ format!("{}", f(sig.get())))` |
//! | `button(label=…, on_click=…)` | `button(label, IntoAction)` | `glue::button` → `ButtonBuilder` |
//! | `on_click = m(sig) => out` | structured `Action{fire,…}` | `move ‖ out.set(m(sig.get()))` |
//! | reactive `if` / `when` | `runtime_shared::when` (Effect + anchor) | `glue::when` → `runtime_scene::dyn_keyed` (guarded hole) |
//! | bare-path `if cond` | `StaticCond`/`ReactiveCond` dispatch | same trait names in glue; reactive arm → `dyn_keyed` |
//! | `if is_even(sig)` | `when(Derived<bool>, …)` | `when(move ‖ is_even(sig.get()), …)` |
//! | reactive `match` | `runtime_shared::switch` | `glue::switch` → `dyn_keyed` (PartialEq dedup) |
//! | literal-armed `match m(sig)` | `Element::Switch{Derived…}` | closure `switch` with `.get()`-rewritten scrutinee |
//! | `for x in vec` | `StaticForEach` → `Vec<Element>` | same, glue-side |
//! | `for x in sig, key=…` | `ReactiveForEach` → `Element::Each` | glue trait → `runtime_scene::keyed` |
//! | `for i in 0..n.get()` | `each_keyed(EachKey, EachRowBuild)` | same names, mapped onto `runtime_scene::keyed` |
//! | `for i in 0..3` (static range) | `Element::Repeat` (batched) | `__static_repeat` → `Element::Many(RepeatPrim)` (same batched one-FFI path) |
//! | `Comp(prop = v)` | `BuildElement` struct literal | same shape against `glue::BuildElement` |
//! | `#[component]` body | probe + reactivity rewrite | + wrapped in `component_scope(move ‖ …)` (run-once, untracked, collected `Owned`) |
//! | `Reactive<T>` props | `runtime_shared::Reactive` | `glue::Reactive` (same API, world-backed; `IntoValue` bridges to builders) |
//! | empty `if` branch | absolute-positioned `view` via StyleSheet | [`empty_absolute_view`] (same rule) |
//! | `overlay(...) { … }` | `primitives::overlay::overlay` chain | [`primitives::overlay`] → `builders::overlay` (PortalPrim composition) |
//! | `anchored_overlay(target=…)` | `…::anchored_overlay(target, kids)` | same names here → `builders::anchored_overlay` |
//! | `presence(present=…) { … }` | `…::presence(child_fn)` chain | [`primitives::presence`] → `builders::presence` (Dyn retire hook) |
//! | `graphics(on_ready=…)` | `…::graphics(on_ready)` chain | [`primitives::graphics`] → `builders::graphics` |
//! | `flat_list(data=…, key=…, …)` | typed `flat_list<T>` over `virtualizer` | [`primitives::flat_list`] — the SAME type-erasing adapter, over `builders::virtualizer` |
//!
//! ## Deferred surface (loud, not silent)
//!
//! Author constructs that reach an unmigrated subsystem fail to compile
//! with a message naming the migration status (emitted by the macro
//! under `new-core`): `web_view` (old-core SDK component, P6), the
//! virtualizer `for i in count(sig)` sugar (generator-backend
//! `Derived<usize>` metadata, post-P7) and legacy-form `#[method]`
//! blocks (Ref/robot machinery, P5). `#[component(lazy)]` is NO
//! LONGER deferred: it lowers through [`primitives::lazy`]
//! (`lazy_split` + the vocabulary `lazy` prim/handler — see
//! `crate::glue_lazy`). `test_id = …` is NO LONGER deferred: it
//! lowers to each wrapper's `.test_id(…)` setter (identical to the old
//! lowering), backed by the vocabulary robot registry (`crate::robot`,
//! `robot` feature) — the P5 identity seam, brought forward. Everything
//! this module *does* export is fully functional on the new core.

use std::rc::Rc;

// Accessibility prop bag + icon/activity data types: shared with the
// old core and part of its ROOT author surface, so `pub` (the P6 alias
// crates import them by the old root paths).
pub use runtime_shared::accessibility::{
    AccessibilityAction, AccessibilityProps, AccessibilityTraits, LiveRegionPriority, Role,
};
pub use runtime_shared::primitives::activity_indicator::ActivityIndicatorSize;
pub use runtime_shared::primitives::icon::{FillRule, IconData, IconHandle, StrokeAnimation};
// `Easing` is re-exported (not just imported): the `stylesheet!`
// emitter spells `::runtime_shared::Easing::…` for `transitions { … }`
// blocks, which the new-core retarget maps here.
pub use runtime_shared::Easing;
// `Color` re-exported for the same reason (stylesheet bodies and app
// preludes reference it; the type is shared with the old core).
pub use runtime_shared::Color;
pub use runtime_shared::{FileDropHandler, IntoAction, SafeAreaSides, TouchHandler, WheelHandler};
// The EVENT PAYLOADS those handler aliases carry. Exporting only the
// `*Handler` type aliases left SDKs unable to name their own handler's
// argument — `Rc<dyn Fn(&FileDropEvent) -> bool>` is unspellable without
// `FileDropEvent`. Pure shared data types; the old root exported them by
// glob.
pub use runtime_shared::{DroppedFile, FileDropEvent, FileDropPhase, WheelEvent, WheelKind};
// The gesture-recognizer contract (`crates/sdk/client/{pan,zoom,dnd}`
// implement `Recognizer` against it). Shared substrate — the touch
// pipeline is backend-fed, not core-fed.
pub use runtime_shared::{
    AsyncNotifier, GestureState, Recognizer, RecognizerCtx, RecognizerKind, RecognizerUpdate,
};
// Touch-claim arbitration: a scroller claims the active gesture so a
// child pressable stops treating it as a tap. Thread-local pair, no core
// involvement.
pub use runtime_shared::{active_touch_claim, set_active_touch_claim};
// `log!` / `LogLevel` and friends — the framework's platform-routed
// logging module.
pub use runtime_shared::logging;
use runtime_scene::{dyn_element, dyn_keyed, Key};
// `fragment(children)` — flat siblings, no node: part of the old root
// author surface (mirrored 1:1 by the scene fragment).
pub use runtime_scene::fragment;

/// Mirror of the old root `dynamic(build)` — a structural hole rebuilt
/// on every dependency change (the scene's un-keyed Dyn driver; same
/// "the closure IS the dependency source" contract).
pub fn dynamic(build: impl Fn() -> Element + 'static) -> Element {
    runtime_scene::dyn_element(build)
}
use runtime_world::{IntoValue, Value};

use crate::builders::{self, TextContent};

// Re-exports: the reactive surface + the scene Element under the names
// the macro (and app preludes) reach for.
pub use runtime_scene::{component_scope, Element};
pub use runtime_world::{
    effect, memo, on_cleanup, signal, untrack, Effect, Memo, ReadSignal, Signal, WriteSignal,
};

// The escape hatch for the `T: PartialEq` bound those signal handles
// carry: a payload with no value equality (a third-party session object,
// an `Arc<dyn Storage>`) wrapped so the guarded `set` can ask the only
// question that makes sense for it — "is this the same instance?".
// Lives in runtime-shared next to the rest of the permanent substrate;
// exported here so authors spell it `runtime_core::ByIdentity`.
pub use runtime_shared::{ByIdentity, ByIdentityArc};

// The `stylesheet!` emission surface (P3c). Under `new-core` the macro's
// output is retargeted `::runtime_shared::…` → `::runtime_vocabulary::glue::…`
// wholesale, so every name the generated sheet fn / builder / variant
// enums reference must resolve HERE. The style data model itself stays
// runtime-core's (sanctioned transitional dependency, crate docs) — these
// are re-exports, not forks. `IntoStyleProp`/`StyleProp` are the one
// NEW pair: the generated builder's conversion impl targets them instead
// of the old `IntoStyleSource`/`StyleSource`.
pub use crate::style_attach::{signal_class, IntoStyleProp, StyleProp};
pub use crate::theme;
pub use runtime_shared::{
    cached_stylesheet, derived, Breakpoint, IntoOverrideSource, IntoVariantSource, Length,
    StateBits, StyleApplication, StyleRules, StyleSheet, TokenEntry, TokenValue, Tokenized,
    Transition, VariantEnum, VariantSet,
};

// Anchor plumbing for `anchored_overlay(target = …)`: `AnchorTarget`
// (a shared runtime-core data type the vocabulary's `PortalPrim` reuses)
// is constructed from a `Ref<ViewHandle>`-style handle slot. `Ref` is
// old-arena machinery; on the new core only the DETACHED sentinel
// (`Ref::default()` — allocates nothing, `get()` → `None`, anchor falls
// back to unresolved positioning) is sanctioned until the P5 identity
// port wires real anchor filling. Re-exported so a new-core app crate
// can spell the same `AnchorTarget::from(Ref::default())` an old-core
// source uses.
pub use runtime_shared::{Ref, ViewHandle};

// The host-scheduling registry (shared infrastructure, not old-core
// reactivity): new-core drivers and test harnesses install/pump through
// the same `runtime_shared::scheduling` registry the vocabulary handlers
// schedule against (presence anims, portal anchor tracking). Re-exported
// so a new-core crate needs no direct runtime-core dependency.
pub use runtime_shared::scheduling;

// Component instrumentation: `#[component]` emits
// `::runtime_core::debug::record_component_enter/exit(..)` under
// `debug-stats`, and on new-core builds `runtime_core` is the facade
// whose root is `glue::*` — so `debug` must be reachable HERE for those
// expansions to resolve. `runtime_shared::debug` carries the no-op shim
// when `debug-stats` is off, so the re-export is unconditional.
pub use runtime_shared::debug;

// MCP catalog anchor (new-core mirror of `runtime_core::__mcp`): the
// `#[component]` / `doc_scope!` registrations emit
// `::runtime_core::__mcp::inventory::submit!`, which the new-core
// retarget pass rewrites to `::runtime_vocabulary::glue::__mcp::…` —
// so the catalog crate must be reachable HERE. Same `mcp-catalog`
// instance as the old anchor and the facade's re-export: every
// registration lands in one process-wide inventory regardless of
// which spelling emitted it. Gated like the old core's (`catalog`
// feature) so production new-core builds carry zero catalog code.
#[cfg(feature = "catalog")]
#[doc(hidden)]
pub use ::mcp_catalog as __mcp;

// Premint style-dump registry anchor. `stylesheet!` emits its
// `cfg(idealyst_premint_dump)` linkme registration as
// `::runtime_core::premint::{linkme, PREMINT_SHEETS, PremintSheet}`, and
// the retarget pass rewrites that head to
// `::runtime_vocabulary::glue::premint::…` — so the registry has to be
// reachable HERE for a dump build to compile. (runtime-macros' module
// docs called this out as the one retarget path with no glue home, on the
// assumption dump builds stayed old-core; with one core they cannot.)
// Same `style-dump` gate as the substrate module: only the CLI's
// ephemeral dump build and `premint-dump`'s own tests turn it on.
#[cfg(feature = "style-dump")]
pub use runtime_shared::premint;

// ============================================================================
// P6 SDK-retarget surface — the rest of the old runtime-core AUTHOR
// surface that aliased SDK crates (`extern crate runtime_core as
// runtime_core;` — idea-theme / idea-ui / idea-ui-nav) reach for
// directly, outside the macro emission. Three kinds of entry:
//
//   1. SHARED data types/modules (style enums, animation, handles,
//      assets, num, breakpoints) — plain re-exports; the types are the
//      same on both cores (sanctioned transitional dep, crate docs).
//   2. Old free fns whose machinery is core-agnostic (`resolve_style`,
//      scheduling entries, `current_breakpoint` value reads — the same
//      value-read the vocabulary's own `merge_active_breakpoints`
//      performs, with the same documented "bucket flip does not
//      re-fire" limitation).
//   3. New-core REIMPLEMENTATIONS where the old fn touched old-core
//      reactive state: the theme-install family (routed through the
//      per-world `ThemeCtx` + a documented seed of the shared
//      `Tokenized::resolve` registry) and `watch`/`Subscription`
//      (world-effect-backed). These carry logic → covered by the
//      vocabulary test suite.
// ============================================================================

// --- 1. Shared data types & modules -----------------------------------------

// Style vocabulary not already re-exported above (the style DATA MODEL
// is runtime-core's on both cores).
pub use runtime_shared::{
    AlignContent, AlignItems, AlignSelf, Cursor, Derive, DisplayKind, FlexDirection, FlexWrap,
    FontFamily, FontStyle, FontWeight, Gradient, GradientKind, GradientStop, JustifyContent,
    ObjectFit, Overflow, PointerEvents, Position, RadialExtent, Shadow, TextAlign, TextTransform,
    TrackSize, Transform, UserSelect, VariantAxis, VariantValue,
};

// Typefaces + assets (fonts, images): shared asset model.
pub use runtime_shared::assets;
pub use runtime_shared::assets::{SystemFallback, Typeface, TypefaceId};

// The animation driver (AnimatedValue + tweens): pure handle + shared
// `scheduling`-registry machinery — no reactive-arena dependency — so it
// runs unchanged on new-core mounts (vocabulary handlers fill the same
// `ViewHandle`s through `make_view_handle`)… EXCEPT `AnimatedValue`'s
// `bind*` family, whose lifetime anchoring is old-core-scope-based. The
// module mirror below re-exports everything shared and shadows
// `AnimatedValue` with a wrapper whose `bind*` anchors to the NEW
// core's component scope.
//
// WHY (the "Switch thumb never travels" bug): old
// `animation/binding.rs` keeps the per-frame subscription alive by
// handing it to old-core `on_cleanup`, which OUTSIDE any old-core
// Scope silently drops its callback immediately — and a new-core build
// never has an old-core Scope active. So on the new core every
// `av.bind(ref, AnimProp::…)` tore its listener down at bind time: the
// tween ticked, nobody wrote `set_animated_f32`, and idea-ui's Switch
// thumb (Collapsible height, Select chevron, Modal fade, …) never
// moved. runtime-core is read-only this phase, so the fix lives here:
// same subscribe machinery, keepalive owned by a world effect collected
// into the surrounding `component_scope` `Owned` (the
// `__component_keepalive_effect` pattern) — dropped exactly at
// unrealize, which restores the old "subscription dies with the
// component" contract, including the recycled-`Ref` protection the old
// anchoring existed for.
pub mod animation {
    use std::ops::Deref;

    pub use runtime_shared::animation::*;

    use runtime_shared::{Ref, TextHandle, ViewHandle};

    /// New-core `AnimatedValue`: same construction/animate/get surface
    /// (via `Deref`), new-core-safe `bind*`. See the module-mirror
    /// comment above for why this exists.
    pub struct AnimatedValue<T: Animatable> {
        inner: runtime_shared::animation::AnimatedValue<T>,
    }

    impl<T: Animatable> Clone for AnimatedValue<T> {
        fn clone(&self) -> Self {
            AnimatedValue { inner: self.inner.clone() }
        }
    }

    impl<T: Animatable> Deref for AnimatedValue<T> {
        type Target = runtime_shared::animation::AnimatedValue<T>;
        fn deref(&self) -> &Self::Target {
            &self.inner
        }
    }

    impl<T: Animatable> AnimatedValue<T> {
        /// Mirror of [`runtime_shared::animation::AnimatedValue::new`].
        pub fn new(initial: T) -> Self {
            AnimatedValue { inner: runtime_shared::animation::AnimatedValue::new(initial) }
        }

        /// Wrap an old-core `AnimatedValue` handle in the new-core-safe
        /// shadow. Glue-internal seam for surfaces that must return the
        /// SHARED handle rather than a fresh one — `glue::session::animated`
        /// hands out the session-registry instance so hot-patch rerenders
        /// keep the AV's current value, exactly like the old core.
        #[doc(hidden)]
        pub fn __from_inner(inner: runtime_shared::animation::AnimatedValue<T>) -> Self {
            AnimatedValue { inner }
        }
    }

    /// Anchor `guard` (subscription + strong AV clone) to the current
    /// component scope: a dependency-free world effect owns it, the
    /// surrounding `component_scope`/realize collector owns the effect,
    /// so the guard drops exactly when the component's subtree
    /// unrealizes. Outside any world, mirror the old contract — "bind
    /// outside a scope is effectively a no-op" — by dropping the guard
    /// immediately instead of panicking in `effect()`.
    fn scope_keepalive(guard: impl Sized + 'static) {
        if !runtime_world::is_entered() {
            drop(guard);
            return;
        }
        let _ = runtime_world::effect(move || {
            // Owns `guard` for the effect's lifetime; reads no signals,
            // so it fires once and never re-runs.
            let _ = &guard;
        });
    }

    /// Re-apply the CURRENT value once the mount has filled the ref —
    /// the old `reapply_after_mount_note` contract (the immediate apply
    /// at bind time no-ops on an unfilled ref, and a static value never
    /// ticks again). Detached 0-delay task: on the new core `Ref` slots
    /// are never freed/recycled (no old-core Scope ever drops them —
    /// the documented `Ref::new` leak rule), so a post-teardown fire
    /// writes through a stale handle at worst, a visual no-op on a
    /// detached node.
    fn reapply_after_mount(apply: impl FnOnce() + 'static) {
        runtime_shared::scheduling::after_ms_detached(0, apply);
    }

    impl AnimatedValue<f32> {
        /// New-core mirror of the old scalar `bind` — see
        /// `runtime_shared::animation::binding` for the author-facing
        /// docs; delivery is identical (`ViewHandle::set_animated_f32`
        /// per fire), only the keepalive differs (module comment).
        pub fn bind(&self, target: Ref<ViewHandle>, prop: AnimProp) {
            let sub: Subscription<f32> = self.inner.subscribe_and_apply(move |v, _vel| {
                let value = *v;
                target.with(|handle| handle.set_animated_f32(prop, value));
            });
            scope_keepalive((sub, self.inner.clone()));
            let av = self.inner.clone();
            reapply_after_mount(move || {
                target.with(|handle| handle.set_animated_f32(prop, av.get()));
            });
        }
    }

    impl AnimatedValue<(f32, f32, f32, f32)> {
        /// New-core mirror of the old `bind_color`.
        pub fn bind_color(&self, target: Ref<ViewHandle>, prop: AnimProp) {
            let sub: Subscription<(f32, f32, f32, f32)> =
                self.inner.subscribe_and_apply(move |v, _vel| {
                    let (r, g, b, a) = *v;
                    target.with(|handle| handle.set_animated_color(prop, [r, g, b, a]));
                });
            scope_keepalive((sub, self.inner.clone()));
            let av = self.inner.clone();
            reapply_after_mount(move || {
                let (r, g, b, a) = av.get();
                target.with(|handle| handle.set_animated_color(prop, [r, g, b, a]));
            });
        }

        /// New-core mirror of the old `bind_gradient_stop`.
        pub fn bind_gradient_stop(&self, target: Ref<ViewHandle>, stop_idx: u8) {
            self.bind_color(target, AnimProp::GradientStopColor(stop_idx));
        }

        /// New-core mirror of the old `bind_text_color` (routes through
        /// `TextOps::set_animated_color` — see the old impl's docs).
        pub fn bind_text_color(&self, target: Ref<TextHandle>, prop: AnimProp) {
            let sub: Subscription<(f32, f32, f32, f32)> =
                self.inner.subscribe_and_apply(move |v, _vel| {
                    let (r, g, b, a) = *v;
                    target.with(|handle| handle.set_animated_color(prop, [r, g, b, a]));
                });
            scope_keepalive((sub, self.inner.clone()));
            let av = self.inner.clone();
            reapply_after_mount(move || {
                let (r, g, b, a) = av.get();
                target.with(|handle| handle.set_animated_color(prop, [r, g, b, a]));
            });
        }
    }
}

/// Session-persistent state (`session::animated` AVs, the session
/// epoch, hot-patch survival) — the registry/epoch machinery is the
/// shared old-core `session` module (pure thread-local state, no
/// reactive-arena dependency), so most names re-export. Two shadows:
///
/// - [`session::animated`] returns the glue [`animation::AnimatedValue`]
///   wrapper (new-core-safe `bind*`) around the SAME session-registry
///   instance, so hot-patch value survival is preserved.
/// - [`session::after_ms`] anchors through
///   [`crate::scoped_scheduling`] instead of the old-core scope (which
///   is inert on a new-core mount — the "welcome acts never fire" bug).
///
/// `session::signal` is deliberately NOT mirrored: it returns an
/// old-core `Signal` no world effect can subscribe to. A new-core
/// session-persistent reactive scalar needs a world-signal port —
/// unresolved use of `glue::session::signal` failing to compile is the
/// loud marker for that seam.
pub mod session {
    pub use runtime_shared::session::{
        clear, clear_prefix, epoch_micros, get_or_init, push_scope, reset_epoch, ScopeGuard,
    };

    pub use crate::scoped_scheduling::session_after_ms as after_ms;

    /// New-core mirror of `runtime_shared::session::animated` — same
    /// key-registry persistence, glue wrapper on the way out so `bind*`
    /// anchors to the new core (see [`super::animation`]).
    pub fn animated<T>(
        key: &'static str,
        initial: T,
    ) -> super::animation::AnimatedValue<T>
    where
        T: runtime_shared::animation::Animatable + 'static,
    {
        super::animation::AnimatedValue::__from_inner(runtime_shared::session::animated(
            key, initial,
        ))
    }
}

// Numeric helpers.
pub use runtime_shared::num;

// Accessibility prop bag (the payload types the a11y setters carry).
pub use runtime_shared::accessibility;

// Breakpoints: the enum/threshold table are shared re-exports, but
// `current_breakpoint` is a new-core REIMPLEMENTATION (§3 below): the
// old fn returns an old-core signal a world effect cannot subscribe to,
// which froze every breakpoint-dependent `when` at its seed (the
// idea-ui-docs "hamburger visible at desktop width" bug). The module
// mirror re-exports the shared items and shadows the signal fn.
pub mod breakpoint {
    pub use runtime_shared::breakpoint::*;
    // Explicit re-export shadows the glob's old-core `current_breakpoint`.
    pub use super::current_breakpoint;
}
pub use runtime_shared::{breakpoints, install_breakpoints, Breakpoints};

/// Reactive current-breakpoint bucket — new-core routing: the ambient
/// world's [`ViewportCtx`](crate::viewport) memo (per-world, re-fires
/// only on bucket flips). Old-core signature parity (`ReadSignal`, read
/// via `.get()`); handler-safe like the theme surface — outside
/// `World::enter` it resolves the last ambient world's ctx (drawer-link
/// handlers call `sidebar_pinned`, which lands here).
pub fn current_breakpoint() -> ReadSignal<runtime_shared::Breakpoint> {
    crate::viewport::viewport_ctx().breakpoint()
}

// Root-level scheduling names (old runtime-core re-exported these at the
// crate root as well as under `scheduling::`). The SCOPED variants are
// NOT the old fns: their old-core scope anchoring is inert on a
// new-core mount (outside an old-core scope the handle drops at
// registration and the timer/loop never fires — the "welcome never
// animates" bug), so they're shadowed by the new-core-anchored
// versions in [`crate::scoped_scheduling`].
pub use runtime_shared::scheduling::{
    after_animation_frame, after_ms, after_ms_detached, raf_loop, schedule_microtask, RafLoop,
    ScheduledTask,
};
pub use crate::scoped_scheduling::{after_ms_scoped, raf_loop_scoped};

// Interaction handles (filled by the mount handlers' `ref_fill` via
// `make_*_handle` — real handles, not sentinels, on the new core).
pub use runtime_shared::{PressableHandle, TextHandle};

// Text/style source data types old component code stores and matches on.
// `StyleSource` is pure data (application + closure variants); the
// vocabulary converts it to `StyleProp` at attach time.
pub use runtime_shared::{IntoTextSource, StyleSource, TextSource};

// Styled-run data types at the root, exactly where the old core
// re-exported them (`runtime_shared::{TextRun, TextRunStyle}`).
pub use runtime_shared::{TextRun, TextRunStyle};

// `flat_list` / `fixed_size` / `FlatListItemSize` at the ROOT, where the
// old core exported them (`runtime-core/src/lib.rs`'s
// `pub use primitives::flat_list::{flat_list, fixed_size, FlatListItemSize}`).
// They also live at `primitives::flat_list::…`, but the documented
// spelling — taught in `websites/docs`'s lists page — is the root one, so
// the root is where they have to resolve.
pub use primitives::flat_list::{fixed_size, flat_list, FlatListItemSize};

/// Module mirror of old `runtime_shared::styled_text` — the data helpers
/// author code imports by path (`styled_text::plain_text_of`). The
/// same-named CONSTRUCTOR fn below lives in the value namespace, the
/// old core's exact shape (its `styled_text` fn + module coexist too).
pub mod styled_text {
    pub use runtime_shared::styled_text::{plain_text_of, TextRun, TextRunStyle};
}

/// Builder-shaped mirror of the old `styled_text(runs)` return
/// (`Bound<TextHandle>`): `.with_style(…)` then [`IntoElement`].
pub struct StyledText(crate::builders::TextBuilder);

/// One text node with inline-styled ranges — the new-core lowering is
/// the vocabulary [`text()`](crate::builders::text) builder's `runs`
/// channel, mounted via the caps `create_styled_text` (exactly the old
/// primitive's backend call).
pub fn styled_text(runs: Vec<TextRun>) -> StyledText {
    StyledText(crate::builders::text().runs(runs))
}

impl StyledText {
    /// Attach the author style — same call shape as `.with_style` on
    /// the old-core `Bound<TextHandle>`.
    pub fn with_style(self, style: impl crate::style_attach::IntoStyleProp) -> Self {
        StyledText(self.0.style(style))
    }
}

impl IntoElement for StyledText {
    fn into_element(self) -> Element {
        self.0.build()
    }
}

// --- 2. Core-agnostic free fns ----------------------------------------------

// The style resolution engine: the SAME entry the vocabulary's
// `style_attach` resolves every sheet application through on the new
// core. Token values inside resolve against the shared
// `Tokenized::resolve` registry, which the theme-install family below
// seeds per install/update.
pub use runtime_shared::resolve_style;

// --- 3. New-core reimplementations ------------------------------------------

/// Install the app's theme tokens. New-core routing: the ambient world's
/// [`ThemeCtx`](crate::theme) records + queues them for the backend
/// (`StyleOps::install_tokens`, delivered by the theme driver / next
/// sheet attach), and the values are ALSO seeded into runtime-core's
/// shared token registry — see [`seed_shared_token_registry`] for why
/// that second write exists and why it is safe.
pub fn install_tokens(tokens: &[runtime_shared::TokenEntry]) {
    theme::install_tokens(tokens);
    seed_shared_token_registry(tokens, false);
}

/// Swap/patch theme tokens. Mirror of [`install_tokens`] — per-world
/// ThemeCtx first (version bump → driver flush → cohort re-apply), then
/// the shared-registry seed so re-resolution reads the new values.
pub fn update_tokens(tokens: &[runtime_shared::TokenEntry]) {
    theme::update_tokens(tokens);
    seed_shared_token_registry(tokens, true);
}

/// See [`crate::theme::set_app_background`].
pub fn set_app_background(color: runtime_shared::Tokenized<runtime_shared::Color>) {
    theme::set_app_background(color);
}

/// See [`crate::theme::set_default_text_font`].
pub fn set_default_text_font(font: Option<FontFamily>) {
    theme::set_default_text_font(font);
}

/// See [`crate::theme::set_scrollbar_theme`].
pub fn set_scrollbar_theme(
    thumb: runtime_shared::Tokenized<runtime_shared::Color>,
    track: runtime_shared::Tokenized<runtime_shared::Color>,
) {
    theme::set_scrollbar_theme(thumb, track);
}

/// Seed runtime-core's token-signal registry with the installed values.
///
/// WHY (P6): `Tokenized::{resolve, value}` — called by author/component
/// code at build time (`resolve_style(&app).color.resolve()` for icon
/// tints, tab indicators, …) and by non-cascading backends at
/// `apply_style` time — reads runtime-core's thread-local token
/// registry, not the per-world ThemeCtx. Without this seed those reads
/// return the compile-time FALLBACK palette (the `theme_token!` light
/// defaults), so a dark-theme install would leave every Rust-side
/// resolved color light. The old-core writer (`install_tokens` /
/// `update_tokens`) is safe to reuse here: token signals live outside
/// any old-core scope by design, no old-core effects exist in a
/// new-core build (so `update`'s subscriber fan-out is a no-op), and
/// the pending-delivery queues those writers fill for the OLD walker's
/// backend flush are drained immediately below so repeated theme swaps
/// can't accumulate them.
fn seed_shared_token_registry(tokens: &[runtime_shared::TokenEntry], update: bool) {
    if update {
        runtime_shared::update_tokens(tokens);
    } else {
        runtime_shared::install_tokens(tokens);
    }
    // Drain the old walker-flush queues nothing consumes on this core.
    let _ = runtime_shared::take_pending_token_updates();
}

/// Caller-owned reactive subscription — the new-core mirror of
/// `runtime_shared::watch`. Runs `f` now and re-runs it when a signal it
/// read changes, until the handle drops. Backed by a world effect
/// collected into a private [`Owned`](runtime_world::Owned) scope (never
/// adopted by the caller's scope — the old contract), so it must be
/// created where effect creation is legal (inside the ambient world:
/// component builds, effects, flushes).
#[must_use = "a Subscription disposes its effect when dropped — store it (or call \
              `.leak()`) to keep the effect running"]
pub struct Subscription {
    owned: Option<runtime_world::Owned>,
}

impl Subscription {
    /// Keep the subscription alive for the world's lifetime, giving up
    /// the handle (mirror of the old `Subscription::leak`). Forgetting
    /// the `Owned` skips its retiring Drop, pinning the effect slot.
    pub fn leak(mut self) {
        if let Some(owned) = self.owned.take() {
            std::mem::forget(owned);
        }
    }
}

/// See [`Subscription`]. Mirror of `runtime_shared::watch`.
#[must_use = "a Subscription disposes its effect when dropped — store it (or call \
              `.leak()`) to keep the effect running"]
pub fn watch<F: FnMut() + 'static>(mut f: F) -> Subscription {
    let ((), owned) = runtime_world::collect_owned(|| {
        let _ = effect(move || {
            f();
        });
    });
    Subscription { owned: Some(owned) }
}

/// Old-core `unscope` disabled scope OWNERSHIP (thread-lifetime slots
/// for global caches). The world kernel's [`runtime_world::unscoped`]
/// is the same contract — slots created inside are owned by the world
/// root instead of the enclosing collector.
pub fn unscope<R>(f: impl FnOnce() -> R) -> R {
    runtime_world::unscoped(f)
}

// Kernel value/scope types for SDK test-support introspection (prim
// payloads carry `Value<T>` props; classified component subtrees retain
// their `Owned`). Hidden: not author surface.
#[doc(hidden)]
pub use runtime_world::Owned;
#[doc(hidden)]
pub use runtime_world::Value as __Value;

// World-context storage (typed provide/inject): the sanctioned home for
// per-app singletons that the old core kept in thread-locals. Worlds
// are transient on the new core (one per SSR request), so an SDK's
// "global" (e.g. idea-theme's active-theme signal) must live with the
// world, exactly like the vocabulary's own ThemeCtx.
pub use runtime_world::{inject, provide};

// The handler-safety probe for aliased crates' per-core fork modules
// (idea-theme's active-theme slot): platform event handlers run OUTSIDE
// `World::enter`, where `inject`/`provide` panic — code that must serve
// both build-time and handler-time callers forks on this and uses a
// build-time-captured handle in the `false` branch (capture, don't
// inject). Double-underscored: migration-internal, no old-core
// counterpart exists (old-core code never needs the fork — its state is
// thread-local).
#[doc(hidden)]
pub use runtime_world::is_entered as __world_is_entered;

/// Is a [`World`](runtime_world::World) currently ambient on this thread?
///
/// **The handler-safe fork.** Framework code, SDKs and components that
/// touch world-scoped context (`theme_ctx`, `viewport_ctx`, an SDK's own
/// per-world context) must branch on this: `true` ⇒ inject/read the
/// ambient world's context; `false` ⇒ the caller is an EVENT HANDLER (or
/// a timer/async continuation), which runs outside `enter`, so it must
/// use a handle captured at build time. Injecting from a handler panics
/// with "signal()/effect() called outside World::enter" — the shape of
/// the idea-theme theme-swap crash.
///
/// This is the public spelling. `__world_is_entered` remains as a
/// doc-hidden alias for macro emissions that already spell it; new code
/// should call this. It was double-underscored while it was believed to
/// be migration-internal, but SDK authors need it — a fork this
/// load-bearing should not look like a private hook.
pub fn world_is_entered() -> bool {
    runtime_world::is_entered()
}

// TEST-SUPPORT mirror of the kernel `World` for aliased crates' own
// dual-core suites that need enter/EXIT control — `__with_fresh_world`
// keeps the world ambient for the whole body, which cannot express "the
// handler runs outside enter" (idea-theme's set-theme-from-handler
// regression). Doc-hidden: not an author surface.
#[doc(hidden)]
pub use runtime_world::World as __World;

/// TEST-HARNESS seam: run `f` inside a fresh world (created, entered,
/// dropped afterwards). SDK crates' unit tests are same-source across
/// cores; on the old core reactive state is ambient thread-local, on
/// the new core it needs a `World::enter`. Their per-core test-support
/// shim routes here (no old-core counterpart exists — the old leg is
/// the identity fn). Returns `f`'s value; staged writes that `f` makes
/// are committed with a final flush before the world drops so
/// assertions on committed state hold.
#[doc(hidden)]
pub fn __with_fresh_world<R>(f: impl FnOnce() -> R) -> R {
    // Panic-safe pop (a failing `#[should_panic]` test must not leave a
    // dead world on the stack for the next test on the same thread).
    struct PopGuard;
    impl Drop for PopGuard {
        fn drop(&mut self) {
            TEST_WORLDS.with(|w| {
                let _ = w.borrow_mut().pop();
            });
        }
    }
    let world = Rc::new(runtime_world::World::new());
    TEST_WORLDS.with(|w| w.borrow_mut().push(world.clone()));
    let _guard = PopGuard;
    let r = world.enter(f);
    world.flush();
    r
}

thread_local! {
    /// The stack of live [`__with_fresh_world`] worlds (nested test
    /// helpers), so [`__flush_test_world`] can commit mid-test.
    /// Test-support only — never touched by production mounts, so its
    /// TLS key is never created outside test runs.
    static TEST_WORLDS: std::cell::RefCell<Vec<Rc<runtime_world::World>>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// TEST-HARNESS seam: commit staged writes on the innermost
/// `__with_fresh_world` world — the new-core analogue of "old-core
/// writes apply immediately" for set-then-assert test bodies. Returns
/// whether a test world was live.
#[doc(hidden)]
pub fn __flush_test_world() -> bool {
    TEST_WORLDS.with(|w| match w.borrow().last() {
        Some(world) => {
            world.flush();
            true
        }
        None => false,
    })
}
// --- P6 root-surface mirrors (data types + handle types) --------------------
//
// The old runtime-core re-exported its primitives' data/handle types at
// the crate ROOT as well as under `primitives::…`; aliased SDK crates
// import both spellings. All shared types (sanctioned transitional dep).

pub use runtime_shared::{
    ImageErrorHandler, ImageHandle, ImageLoadEvent, ImageLoadHandler, ImageSource,
};
pub use runtime_shared::{KeyEvent, KeyOutcome};
pub use runtime_shared::{ScrollViewHandle, TextAreaHandle, TextInputHandle, ToggleHandle};
pub use runtime_shared::{
    AnchorTarget, AnchorableHandle, BackdropMode, ElementAlign, ElementSide, PortalHandle,
    PortalTarget, ViewportPlacement, ViewportRect,
};
pub use runtime_shared::{PresenceAnim, PresenceHandle, PresenceState};
pub use runtime_shared::{LayoutSubscription, NavKind};
pub use runtime_shared::{EdgeInsets, ViewportSize};

// Touch model + gesture recognizers: pure event-fed state machines over
// the `on_touch` channel (which the vocabulary's view handler installs
// through `InputOps` on every backend) — core-agnostic by construction.
pub use runtime_shared::{
    long_press, pan, pinch, rotate, swipe, tap, LongPress, LongPressRecognizer, Pan, PanEvent,
    PanRecognizer, Pinch, PinchEvent, PinchRecognizer, Rotate, RotateEvent, RotateRecognizer,
    Swipe, SwipeDirection, SwipeDirs, SwipeRecognizer, Tap, TapRecognizer,
};
pub use runtime_shared::{
    pointer_modifiers, PointerModifiers, TouchEvent, TouchId, TouchPhase, TouchPoint,
    TouchResponse,
};

// Safe-area: still the old-core value read (same read the vocabulary's
// own apply paths perform; the reactive re-fire port follows the same
// pattern as the viewport ctx when a native backend needs it).
pub use runtime_shared::safe_area_insets;

// Ambient host identity + frame gating + clocks + the async driver:
// pure thread-local / registry reads shared by both cores. `platform()`
// reads the value the mounting host installed (the new-core native
// boots install it from the backend's `platform()`, same as the old
// mount; web installs it in `start_in`); `is_frame_active()` is the
// embedded-host paint gate (the wgpu hosts flip it while their surface
// is hidden so raf-driven author loops can freeze their clocks);
// `time` / `driver` are the monotonic clock and the spawn/render-loop
// registries — all core-agnostic by construction.
pub use runtime_shared::{is_frame_active, platform, ColorScheme, Platform};
pub use runtime_shared::time;
// The rest of the ambient-host free-function surface the old
// `runtime_core` root re-exported from `runtime_shared::host` (the old
// root's `pub use runtime_shared::*;` picked these up for free; this
// facade enumerates its exports, so they had to be named). All four are
// thread-local reads/writes against installers a host wires at boot —
// no core involvement whatsoever, so the old-core behavior is the
// behavior, byte for byte.
//
// `announce` is the a11y live-region author surface, `open_url` /
// `set_fullscreen` route through the host-installed setters (no-op when
// the host installed none), and `color_scheme()` reads the installed
// scheme. Pinned by `glue_host_surface.rs`.
pub use runtime_shared::{announce, color_scheme, open_url, set_fullscreen};
/// `runtime_core::host::…` — the module path the old root also exposed
/// (via its glob) alongside the root-level re-exports above. Author code
/// and SDKs reach `host::color_scheme()` through this; the installers
/// (`install_announcer`, `install_url_opener`, …) are the boot-side half
/// that backends call.
pub use runtime_shared::host;
// Color parsing/blending helpers (`color::parse_or`, `color::Rgba`) —
// shared substrate used by the canvas SDK's scene lowering. Module
// re-export, matching the old root's glob.
pub use runtime_shared::color;
// The app-level key-down handler installer. The caps plumbing
// (`AppEnvOps::set_app_key_handler`) was always mirrored; the free
// function that author code calls was not.
pub use runtime_shared::set_app_key_handler;
// `driver` (spawn_async + render_loop) is feature-gated in runtime-core
// itself (`async-driver`); this crate's same-named forwarding feature
// keeps the gate. Apps that reach `runtime_shared::driver` through the
// facade enable `runtime-vocabulary/async-driver` (the website's
// embedded-simulator new-core build is the precedent).
#[cfg(feature = "async-driver")]
pub use runtime_shared::driver;
// `resource` / `mutation` — new-core REIMPLEMENTATIONS (old fns are
// built on old-core reactivity; see `crate::async_reactive`'s module
// docs for anchoring, completion staging, and the documented
// divergences). Same `async-driver` gate as the old root. The shared
// `NetworkState` enum re-exports; its old `From<&…State>` impls cannot
// (orphan rule) — use the handles' `network_state()`.
#[cfg(feature = "async-driver")]
pub use crate::async_reactive::{
    async_reducer, mutation, resource, AsyncReducer, AsyncStatus, Mutation, MutationState,
    Resource, ResourceCancel, ResourceState,
};
#[cfg(feature = "async-driver")]
pub use runtime_shared::NetworkState;

/// The reactive viewport-size signal — new-core routing: the ambient
/// world's [`ViewportCtx`](crate::viewport) signal (per-world; the web
/// backend's newcore resize source pushes into it — see the viewport
/// module docs for the per-platform wiring status). Old-core signature
/// parity: returns a `Signal<ViewportSize>` read via `.get()`.
/// Handler-safe outside `World::enter` via the last-ambient fallback.
pub fn viewport_size() -> Signal<runtime_shared::ViewportSize> {
    crate::viewport::viewport_ctx().size_signal()
}

// `IntoStyleSource` — on the old core, "the trait `.with_style(…)`
// accepts". The new-core counterpart of that ROLE is `IntoStyleProp`,
// so the old name aliases it here: helper fns bounded
// `impl IntoStyleSource` keep accepting exactly what the glue wrappers'
// `.with_style` takes (resolved rules, sheets, sheet-builder outputs,
// closures). The `StyleSource` DATA enum stays the shared runtime-core
// type (re-exported above) — only the conversion-trait role moves.
pub use crate::style_attach::IntoStyleProp as IntoStyleSource;

// --- P6 root-surface mirrors (primitive constructors) -----------------------
//
// Root spellings of the glue's `primitives::…` wrappers (the old core
// exported both), plus wrappers the P3 set didn't need yet.

pub use primitives::image::{image, image_asset};
pub use primitives::link::{external_link, link};
// Old-core root spellings of the navigation data surface (`use
// runtime_shared::{Route, Screen}` in app crates): `Route` is the shared
// runtime-core type; `Screen` is the new-core carrier (scene `Element`
// + opaque options) at the same name.
pub use primitives::navigator::{Route, RouteParams, Screen};
pub use primitives::overlay::{anchored_overlay, overlay};
pub use primitives::presence::presence;
pub use primitives::scroll_view::scroll_view;
pub use primitives::text_area::text_area;
pub use primitives::text_input::text_input;
pub use primitives::toggle::toggle;

/// Mirror of the old root `image_from`: dispatch an [`ImageSource`] to
/// the url / asset constructor. `ImageSource` is shared DATA — its
/// `Url` variant carries the OLD core's `Reactive<String>`, which is
/// inert data here (`Static` clones; a `Dynamic` closure could only
/// have been built against old-core signals, which a new-core crate
/// cannot construct — the caller-side conversion fails to compile
/// instead of dangling).
pub fn image_from(src: impl Into<ImageSource>) -> primitives::image::GlueImage {
    match src.into() {
        ImageSource::Url(r) => primitives::image::image(move || r.get()),
        ImageSource::Asset(a) => primitives::image::image_asset(a),
        // `ImageSource` is `#[non_exhaustive]`-shaped upstream; new
        // variants must be wired here deliberately, not silently
        // rendered as an empty URL.
        _ => panic!("image_from: unsupported ImageSource variant (added upstream; wire it here)"),
    }
}

/// Mirror of old `on_defer(deps, f)`: fire `f(new, Some(prev))` on every
/// dependency change AFTER the first run (the first run only records the
/// baseline). Same skip-first contract, expressed as a world effect.
pub fn on_defer<D, F>(deps: D, mut f: F) -> Effect
where
    D: Trackable + 'static,
    F: FnMut(&D::Value, Option<&D::Value>) + 'static,
{
    let prev: Rc<std::cell::RefCell<Option<D::Value>>> = Rc::new(std::cell::RefCell::new(None));
    effect(move || {
        let new = deps.track();
        let prev_value = prev.borrow().clone();
        if prev_value.is_some() {
            untrack(|| f(&new, prev_value.as_ref()));
        }
        *prev.borrow_mut() = Some(new);
    })
}

/// Mirror of old `on(deps, f)`: fire `f(new, prev)` immediately AND on
/// every dependency change. The subscription set is `deps`; the body runs
/// untracked, so reads inside it never widen it. Same contract as the old
/// root's `on` — the only difference from [`on_defer`] is that the first
/// run invokes the body instead of only recording the baseline.
pub fn on<D, F>(deps: D, mut f: F) -> Effect
where
    D: Trackable + 'static,
    F: FnMut(&D::Value, Option<&D::Value>) + 'static,
{
    let prev: Rc<std::cell::RefCell<Option<D::Value>>> = Rc::new(std::cell::RefCell::new(None));
    effect(move || {
        let new = deps.track();
        let prev_value = prev.borrow().clone();
        untrack(|| f(&new, prev_value.as_ref()));
        *prev.borrow_mut() = Some(new);
    })
}

/// Mirror of old `memo_with(eq, f)`: a cached derivation whose
/// notification is gated by a CALLER-SUPPLIED equality instead of
/// `PartialEq` — tolerance comparisons, "equal enough to skip a
/// repaint", and the like.
///
/// Implemented over the world kernel rather than re-exported: the shared
/// `memo_with` builds an old-arena `Signal` + `Effect`, which nothing on
/// a world mount subscribes to (the same silent inertness as the old
/// `resource`/`AnimatedValue::bind` — see `crate::async_reactive`).
///
/// **One documented narrowing.** The old signature accepted `T` with no
/// `PartialEq` at all (its stated second use case). The world kernel's
/// signal storage is `PartialEq`-bounded end to end, so this mirror
/// requires `T: PartialEq` and uses `set_always` to make the caller's
/// `eq` the authoritative gate — the custom-comparison case is fully
/// preserved, the no-`PartialEq` case is not expressible. For a `T`
/// without `PartialEq` (e.g. one holding a trait object), wrap it in a
/// newtype whose `PartialEq` impl encodes the comparison and memo on
/// that.
pub fn memo_with<T, F, E>(eq: E, f: F) -> ReadSignal<T>
where
    T: Clone + PartialEq + 'static,
    F: Fn() -> T + 'static,
    E: Fn(&T, &T) -> bool + 'static,
{
    // Seed untracked so a read between construction and the effect's
    // first commit sees a coherent value (old-core behavior).
    let initial = untrack(&f);
    let out = signal(initial.clone());
    let write = out.write_only();
    // The effect compares against its OWN last emission rather than
    // reading `out` (which would subscribe it to its own output).
    let last = Rc::new(std::cell::RefCell::new(initial));
    // The returned `Effect` is a Copy handle with no Drop — the effect
    // itself lives in the arena, owned by the ambient collector, exactly
    // like `runtime_world::memo`'s.
    let _ = effect(move || {
        let next = f();
        let changed = !eq(&last.borrow(), &next);
        if changed {
            *last.borrow_mut() = next.clone();
            // `set_always`: the caller's `eq` already decided, so the
            // kernel's own guarded `set` must not second-guess it (they
            // can disagree — that is the whole point of `memo_with`).
            write.set_always(next);
        }
    });
    out.read_only()
}

/// Mirror of old `reducer(initial, f)`: a `(state, dispatch)` pair where
/// every dispatch folds the current state with an action.
///
/// Implemented over the world kernel for the same reason as
/// [`memo_with`]. Two documented differences, both forced by the kernel:
///
/// - `S: PartialEq` is required (world signal storage is
///   `PartialEq`-bounded); the old signature asked only for `Clone`.
/// - There is no `cycle(..)` wrap. On this kernel a dispatch's writes
///   are STAGED and the flush commits them together, so sibling writes
///   coalesce into one fan-out by construction — `cycle` had no
///   counterpart to port. See the migration guide's breaking-changes
///   table.
///
/// The historical "every dispatch notifies" contract is preserved with
/// `set_always`, so a reducer that folds to an equal state still wakes
/// its subscribers.
pub fn reducer<S, A>(initial: S, f: impl Fn(&S, A) -> S + 'static) -> (Signal<S>, impl Fn(A))
where
    S: Clone + PartialEq + 'static,
{
    let state = signal(initial);
    let dispatch = move |action: A| {
        // Fold against the STAGED value so two dispatches in one event
        // turn compose (0→1→2) instead of both reading the committed 0.
        // The old core read `untrack(|| state.get())`, which was the
        // same intent against a synchronous arena.
        state.update(|current| f(current, action));
        // Preserve "a dispatch always notifies": `update` stages
        // guarded, so a fold back to an equal value would be silent
        // otherwise.
        state.touch();
    };
    (state, dispatch)
}

// Stable per-position identifiers. Pure thread-local reads over
// `runtime_shared::identity` — no core involvement, and the deletion
// baseline lists `identity` as an explicit SURVIVOR (§4.2), so dropping
// the accessors from the author surface was an oversight, not a
// decision. `runtime_core::use_id()` is documented on the docs site.
//
// ⚠️ Contract caveat, honestly stated: `use_id()` derives from the
// AMBIENT identity, which the old walker set per emission
// (`walker.rs::build`). The surviving renderer does not set it yet, so
// every call currently answers from `Identity::UNIDENTIFIED` — stable
// and non-panicking, but NOT per-position-unique. `with_current_identity`
// still works for callers that establish it themselves. Tracked in the
// deletion baseline; pinned by `glue_reactive_surface.rs`.
pub use runtime_shared::{
    current_identity, hash_key, use_id, use_id_keyed, with_current_identity, Identity,
};

/// Mirror of the old `Trackable` (the dependency-source trait `on_defer`
/// is generic over): a tracked read of the current value.
pub trait Trackable {
    type Value: Clone;
    fn track(&self) -> Self::Value;
}

impl<T: PartialEq + Clone + 'static> Trackable for Signal<T> {
    type Value = T;
    fn track(&self) -> T {
        self.get()
    }
}

impl<T: PartialEq + Clone + 'static> Trackable for ReadSignal<T> {
    type Value = T;
    fn track(&self) -> T {
        self.get()
    }
}

impl<T: PartialEq + Clone + 'static> Trackable for Memo<T> {
    type Value = T;
    fn track(&self) -> T {
        self.get()
    }
}

impl<T: Clone + 'static> Trackable for Reactive<T> {
    type Value = T;
    fn track(&self) -> T {
        self.get()
    }
}

// ============================================================================
// `#[method]` emission surface (P5). The `#[component]` macro emits, for a
// methods-bearing component: `::runtime_shared::robot::Method` literals +
// `::runtime_shared::robot::register_component(...)`, a
// `::runtime_shared::__component_keepalive_effect(...)`, and a
// `::runtime_shared::__component_root(...)` tail wrap — all retargeted here.
// The registration surface always exists (`crate::robot_methods` is
// stub-shaped when the vocabulary `robot` feature is off, mirroring the
// old core's non-robot stub module), so the emission is unconditional on
// both cores. `bind_to` props ride the re-exported old-core `Ref`
// (above): `Ref::new/fill/get` operate purely on runtime-core's
// thread-local ref arena, independent of which core mounted — a `Ref`
// created outside an old-core Scope simply never frees its slot, the
// documented `Ref::new` rule.
// ============================================================================

/// Re-export of `serde_json` for the `#[method]` auto-registration
/// codegen (mirror of `runtime_shared::__serde_json`).
#[doc(hidden)]
pub use runtime_shared::__serde_json;

/// Re-export of the `wasm-split` runtime crate (mirror of
/// `runtime_shared::__wasm_split`) so the `#[component(lazy)]` new-core
/// emission's `#[…__wasm_split::wasm_split(…)]` attribute + its
/// `use …__wasm_split as wasm_split;` alias resolve after the
/// retarget, without author crates depending on `wasm-split` directly.
#[doc(hidden)]
pub use runtime_shared::__wasm_split;

/// The names the macro spells as `::runtime_shared::robot::…`, resolved
/// against the vocabulary method registry.
pub mod robot {
    pub use crate::robot_methods::{
        register_component, ComponentInstanceId, ComponentRegistration, Method,
    };
}

#[doc(hidden)]
pub use crate::robot_methods::__component_root;

/// Keepalive for a `#[method]` component's robot registration: an
/// effect whose closure owns the `ComponentRegistration` guard. Created
/// inside the component body — i.e. inside `component_scope`'s
/// collector — so the surrounding `Owned` owns it and the registration
/// deregisters exactly when the component's subtree unrealizes (the
/// new-core analogue of the old scope-adopted keepalive `Effect`). The
/// closure reads no signals, so the effect fires once and never
/// re-runs.
#[doc(hidden)]
pub fn __component_keepalive_effect(f: impl FnMut() + 'static) {
    let mut f = f;
    let _ = runtime_world::effect(move || {
        f();
    });
}

/// A fresh, world-root-owned signal — used by the macro for the
/// *uncontrolled* `text_input` / `toggle` / `slider` defaults (the old
/// lowering's `Signal::new(...)`; `runtime_world::Signal` has no `new`,
/// and this glue cannot add one to a foreign type).
pub fn fresh_signal<T: PartialEq + 'static>(value: T) -> Signal<T> {
    signal(value)
}

// ============================================================================
// IntoElement + ChildList — the coercion seams every emission site uses.
// ============================================================================

/// Anything the macro can coerce to one [`Element`]. Mirrors
/// `runtime_shared::IntoElement` (Element itself, every glue wrapper, and
/// zero-arg closures, which become structural holes).
pub trait IntoElement {
    fn into_element(self) -> Element;
}

impl IntoElement for Element {
    fn into_element(self) -> Element {
        self
    }
}

/// A closure child is a structural hole: rebuilt whenever the signals it
/// eagerly reads change (`dyn_element` semantics, same as the old core's
/// closure `IntoElement`).
impl<F, E> IntoElement for F
where
    F: Fn() -> E + 'static,
    E: IntoElement,
{
    fn into_element(self) -> Element {
        dyn_element(move || self().into_element())
    }
}

/// The children-flattening seam: `ui!` children blocks append every node
/// through this. Mirrors `runtime_shared::ChildList` (Element, Vec,
/// Option, closures, glue wrappers).
pub trait ChildList {
    fn append_to(self, out: &mut Vec<Element>);
}

impl ChildList for Element {
    fn append_to(self, out: &mut Vec<Element>) {
        out.push(self);
    }
}

impl ChildList for Vec<Element> {
    fn append_to(mut self, out: &mut Vec<Element>) {
        out.append(&mut self);
    }
}

impl<T: IntoElement> ChildList for Option<T> {
    fn append_to(self, out: &mut Vec<Element>) {
        if let Some(v) = self {
            out.push(v.into_element());
        }
    }
}

impl<F, E> ChildList for F
where
    F: Fn() -> E + 'static,
    E: IntoElement,
{
    fn append_to(self, out: &mut Vec<Element>) {
        out.push(self.into_element());
    }
}

/// Collapse a flat node list to ONE element: the sole element verbatim,
/// a `view` wrapper for a genuinely multi-node list (matching the old
/// `one_or_view`), and — for an empty list — the layout-neutral
/// absolutely-positioned empty view (the overlay-`if`-toggle rule: an
/// absent branch must not occupy a flex slot).
pub fn one_or_view(mut nodes: Vec<Element>) -> Element {
    match nodes.len() {
        0 => empty_absolute_view(),
        1 => nodes.pop().expect("len checked"),
        _ => view(nodes).into_element(),
    }
}

/// One element per keyed ROW: sole node verbatim, multi-node rows are a
/// [`fragment`] (flat siblings — the old `each` accounted rows as node
/// *vectors*, and the scene's keyed driver does the same for fragments).
fn one_or_fragment(mut nodes: Vec<Element>) -> Element {
    match nodes.len() {
        1 => nodes.pop().expect("len checked"),
        _ => fragment(nodes),
    }
}

/// The static-range `for` lowering (`for i in 0..n { single-node }`) —
/// the new-core carrier of the old `Element::Repeat`. Emitted by
/// `ui!`/`jsx!` under `new-core` with EXACTLY the old macro's
/// conditions (ident pattern, exclusive both-bound range, single-node
/// body, non-reactive bounds), so both cores make the same batching
/// decision for the same author shape. Mounts through the vocabulary's
/// `repeat` multi-node handler: one `execute_batch_with_attach` FFI on
/// batching backends (web), per-row mounts + one `insert_many`
/// elsewhere.
pub fn __static_repeat(
    count: usize,
    row_builder: impl Fn(usize) -> Element + 'static,
) -> Vec<Element> {
    vec![runtime_scene::many(crate::prims::PrimCell::new(
        crate::prims::RepeatPrim {
            count,
            row_builder: Box::new(row_builder),
        },
    ))]
}

/// The layout-neutral empty branch: `position: absolute`, so a false
/// `if` contributes no flex slot (port of the old
/// `empty_view_primitive` emission — see the overlay-if-toggle memory).
pub fn empty_absolute_view() -> Element {
    let rules = runtime_shared::StyleRules {
        position: Some(runtime_shared::Position::Absolute),
        ..Default::default()
    };
    builders::view().style(rules).build()
}

// ============================================================================
// Glue wrappers — chainable shims over the P2b builders.
//
// The macro chains `.with_style(…)` / `.disabled(…)` / a11y setters onto
// the primitive expression AFTER construction, and trailing author
// chains (`.on_touch(…)`) attach the same way — so each primitive call
// returns a wrapper carrying its builder, finished by `IntoElement`.
// ============================================================================

/// Adds the shared post-fix surface (`with_style` + the a11y setters the
/// `ui!`/`jsx!` attr list recognizes) plus `IntoElement`/`ChildList` to a
/// glue wrapper. Every wrapper stores `a11y: AccessibilityProps` and
/// applies it at build through its builder's `.a11y(…)`.
///
/// `test_id`: the default arm forwards to the builder's identity slot
/// (the P5 seam — registered by the mount handler under the `robot`
/// feature). The `test_id_ignored` arm is for wrappers whose OLD-core
/// element carries no test_id field (`Link`, the virtualizer behind
/// `flat_list`): `Bound::test_id` compiled there but `with_test_id`
/// silently no-opped, and same-source parity means the glue accepts and
/// drops identically rather than breaking those call sites.
macro_rules! glue_wrapper_common {
    ($wrapper:ident) => {
        impl $wrapper {
            /// Robot/automation anchor (`test_id = …`) — stored on the
            /// prim's identity slot; the mount handler registers it in
            /// the robot registry (`robot` feature; inert otherwise).
            pub fn test_id(mut self, id: &'static str) -> Self {
                self.b = self.b.test_id(id);
                self
            }
        }
        glue_wrapper_common!(@common $wrapper);
    };
    ($wrapper:ident, test_id_ignored) => {
        impl $wrapper {
            /// Accepted-and-dropped, mirroring the old core exactly:
            /// this primitive's `Element` variant carries no `test_id`
            /// field there (`with_test_id` no-ops), so the id is
            /// discarded on both cores. See the macro docs above.
            pub fn test_id(self, _id: &'static str) -> Self {
                self
            }
        }
        glue_wrapper_common!(@common $wrapper);
    };
    (@common $wrapper:ident) => {
        impl $wrapper {
            /// `style = …` — static `StyleRules` (applied once) or a
            /// rules closure (re-applied on dependency change). See
            /// [`IntoStyleProp`].
            pub fn with_style(mut self, style: impl IntoStyleProp) -> Self {
                self.b = self.b.style(style);
                self
            }

            pub fn accessibility(mut self, a11y: AccessibilityProps) -> Self {
                self.a11y = a11y;
                self
            }

            pub fn a11y_label(mut self, label: impl Into<String>) -> Self {
                self.a11y.label = Some(label.into());
                self
            }

            pub fn a11y_hint(mut self, hint: impl Into<String>) -> Self {
                self.a11y.hint = Some(hint.into());
                self
            }

            pub fn a11y_role(mut self, role: Role) -> Self {
                self.a11y.role = Some(role);
                self
            }

            pub fn a11y_hidden(mut self, hidden: bool) -> Self {
                self.a11y.hidden = hidden;
                self
            }

            pub fn a11y_traits(mut self, traits: AccessibilityTraits) -> Self {
                self.a11y.traits = traits;
                self
            }

            pub fn live_region(mut self, priority: LiveRegionPriority) -> Self {
                self.a11y.live_region = Some(priority);
                self
            }
        }

        impl IntoElement for $wrapper {
            fn into_element(self) -> Element {
                self.b.a11y(self.a11y).build()
            }
        }

        impl ChildList for $wrapper {
            fn append_to(self, out: &mut Vec<Element>) {
                out.push(self.into_element());
            }
        }

        /// Mirror of the old core's `From<Bound<H>> for Element`: the
        /// authored `.into()` coercion (e.g. a `flat_list` `render`
        /// closure returning `ui! { … }.into()`) keeps compiling
        /// source-identically on the new core.
        impl From<$wrapper> for Element {
            fn from(w: $wrapper) -> Element {
                w.into_element()
            }
        }
    };
}

// ---------------------------------------------------------------------------
// view
// ---------------------------------------------------------------------------

/// `view(children)` — the old positional constructor's shape over the
/// P2b `ViewBuilder`.
pub fn view(children: Vec<Element>) -> GlueView {
    GlueView {
        b: builders::view().children(children),
        a11y: AccessibilityProps::default(),
    }
}

pub struct GlueView {
    b: builders::ViewBuilder,
    a11y: AccessibilityProps,
}

impl GlueView {
    pub fn safe_area(mut self, sides: SafeAreaSides) -> Self {
        self.b = self.b.safe_area(sides);
        self
    }

    /// Mirror of `Bound::on_touch` (author closures, not pre-wrapped
    /// `TouchHandler` Rcs — the old builder wrapped internally too).
    pub fn on_touch(
        mut self,
        handler: impl Fn(&runtime_shared::TouchEvent) -> TouchResponse + 'static,
    ) -> Self {
        self.b = self.b.on_touch(Rc::new(handler));
        self
    }

    /// Mirror of `Bound::on_wheel`.
    pub fn on_wheel(
        mut self,
        handler: impl Fn(&runtime_shared::WheelEvent) -> TouchResponse + 'static,
    ) -> Self {
        self.b = self.b.on_wheel(Rc::new(handler));
        self
    }

    pub fn on_hover(mut self, handler: impl Fn(bool) + 'static) -> Self {
        self.b = self.b.on_hover(handler);
        self
    }

    /// Mirror of `Bound::on_file_drop`.
    pub fn on_file_drop(
        mut self,
        handler: impl Fn(&runtime_shared::FileDropEvent) -> TouchResponse + 'static,
    ) -> Self {
        self.b = self.b.on_file_drop(Rc::new(handler));
        self
    }

    pub fn preserves_focus(mut self, preserve: bool) -> Self {
        self.b = self.b.preserves_focus(preserve);
        self
    }

    pub fn container(mut self) -> Self {
        self.b = self.b.container();
        self
    }
}

glue_wrapper_common!(GlueView);

// ---------------------------------------------------------------------------
// text
// ---------------------------------------------------------------------------

/// `text(content)` — content coerces via [`TextContent`]: `&str`/`String`
/// / scalars → `Value::Const`; closures / `Signal` / `ReadSignal` /
/// `Memo` / a `Dynamic` [`Reactive`] → `Value::Dyn`.
pub fn text(content: impl TextContent) -> GlueText {
    GlueText {
        b: builders::text().content(content),
        a11y: AccessibilityProps::default(),
    }
}

pub struct GlueText {
    b: builders::TextBuilder,
    a11y: AccessibilityProps,
}

glue_wrapper_common!(GlueText);

// ---------------------------------------------------------------------------
// button
// ---------------------------------------------------------------------------

/// `button(label, on_click)` — label via [`TextContent`], the action via
/// [`IntoAction`] (plain closures included, same as the old core).
pub fn button(label: impl TextContent, on_click: impl IntoAction) -> GlueButton {
    GlueButton {
        b: builders::button().label(label).on_press(on_click),
        a11y: AccessibilityProps::default(),
    }
}

pub struct GlueButton {
    b: builders::ButtonBuilder,
    a11y: AccessibilityProps,
}

impl GlueButton {
    pub fn leading_icon(mut self, icon: IconData) -> Self {
        self.b = self.b.leading_icon(icon);
        self
    }

    pub fn trailing_icon(mut self, icon: IconData) -> Self {
        self.b = self.b.trailing_icon(icon);
        self
    }

    pub fn disabled(mut self, disabled: impl IntoValue<bool>) -> Self {
        self.b = self.b.disabled(disabled);
        self
    }
}

glue_wrapper_common!(GlueButton);

// ---------------------------------------------------------------------------
// icon
// ---------------------------------------------------------------------------

/// `icon(data)` — static [`IconData`]. Optional `.color(…)` /
/// `.stroke(…)` accept static values or closures (`IntoValue`); the
/// closure form updates in place.
pub fn icon(data: IconData) -> GlueIcon {
    GlueIcon {
        b: builders::icon().data(data),
        a11y: AccessibilityProps::default(),
    }
}

pub struct GlueIcon {
    b: builders::IconBuilder,
    a11y: AccessibilityProps,
}

impl GlueIcon {
    pub fn color(mut self, color: impl IntoValue<Color>) -> Self {
        self.b = match color.into_value() {
            Value::Const(c) => self.b.color(c),
            Value::Dyn(f) => self.b.color_dyn(move || f()),
        };
        self
    }

    pub fn stroke(mut self, progress: impl IntoValue<f32>) -> Self {
        self.b = match progress.into_value() {
            Value::Const(p) => self.b.stroke(p),
            Value::Dyn(f) => self.b.stroke_dyn(move || f()),
        };
        self
    }

    /// `draw_in = (duration_ms, easing)` sugar.
    pub fn draw_in(mut self, duration_ms: u32, easing: Easing) -> Self {
        self.b = self.b.draw_in(StrokeAnimation::new(duration_ms, easing));
        self
    }

    /// `animate = StrokeAnimation { … }` — the full-struct form.
    pub fn animate(mut self, anim: StrokeAnimation) -> Self {
        self.b = self.b.draw_in(anim);
        self
    }
}

glue_wrapper_common!(GlueIcon);

// ============================================================================
// `primitives` submodules — mirror the `runtime_shared::primitives::…` paths
// the macro emits for the form-control / media tags.
// ============================================================================

pub mod primitives {
    /// The `#[component(lazy)]` emission surface (`lazy_split`,
    /// `LazyLoadingUi`/`LazyErrorUi`, the thunk-flavored `LazyFuture`)
    /// — implemented in `crate::glue_lazy`, re-exported here so the
    /// retargeted `::runtime_shared::primitives::lazy::…` paths resolve.
    pub use crate::glue_lazy as lazy;

    pub mod image {
        use super::super::*;
        use runtime_shared::assets::{kinds, Asset};

        /// `image(src)` — static or reactive source.
        pub fn image(src: impl IntoValue<String>) -> GlueImage {
            GlueImage {
                b: builders::image().src(src),
                a11y: AccessibilityProps::default(),
            }
        }

        /// `image_asset(asset)` — declarative asset reference.
        pub fn image_asset(asset: Asset<kinds::Image>) -> GlueImage {
            GlueImage {
                b: builders::image().asset(asset),
                a11y: AccessibilityProps::default(),
            }
        }

        pub struct GlueImage {
            pub(crate) b: builders::ImageBuilder,
            pub(crate) a11y: AccessibilityProps,
        }

        impl GlueImage {
            /// Concrete `String` — see `GlueTextInput::placeholder`.
            /// Mirror of `Bound::<ImageHandle>::on_load`.
            pub fn on_load(mut self, f: impl Fn(&ImageLoadEvent) + 'static) -> Self {
                self.b = self.b.on_load(std::rc::Rc::new(f));
                self
            }

            /// Mirror of `Bound::<ImageHandle>::on_error`.
            pub fn on_error(mut self, f: impl Fn() + 'static) -> Self {
                self.b = self.b.on_error(std::rc::Rc::new(f));
                self
            }

            /// Mirror of `Bound::<ImageHandle>::alt_reactive`.
            pub fn alt_reactive(mut self, f: impl Fn() -> Option<String> + 'static) -> Self {
                self.b = self.b.alt_dyn(f);
                self
            }

            /// Mirror of `Bound::<ImageHandle>::bind`.
            pub fn bind(mut self, r: super::super::Ref<ImageHandle>) -> Self {
                self.b = self.b.on_handle(move |h| r.fill(h));
                self
            }

            pub fn alt(mut self, alt: String) -> Self {
                self.b = self.b.alt(alt);
                self
            }
        }

        glue_wrapper_common!(GlueImage);
    }

    pub mod toggle {
        use super::super::*;

        /// `toggle(value, on_change)` — controlled: `value` is the
        /// source of truth, `on_change` reports native flips.
        pub fn toggle(value: impl IntoValue<bool>, on_change: impl Fn(bool) + 'static) -> GlueToggle {
            GlueToggle {
                b: builders::toggle().value(value).on_change(on_change),
                a11y: AccessibilityProps::default(),
            }
        }

        pub struct GlueToggle {
            pub(crate) b: builders::ToggleBuilder,
            pub(crate) a11y: AccessibilityProps,
        }

        glue_wrapper_common!(GlueToggle);
    }

    pub mod slider {
        use super::super::*;

        /// `slider(value, on_change)` (controlled).
        pub fn slider(value: impl IntoValue<f32>, on_change: impl Fn(f32) + 'static) -> GlueSlider {
            GlueSlider {
                b: builders::slider().value(value).on_change(on_change),
                a11y: AccessibilityProps::default(),
            }
        }

        pub struct GlueSlider {
            pub(crate) b: builders::SliderBuilder,
            pub(crate) a11y: AccessibilityProps,
        }

        impl GlueSlider {
            pub fn range(mut self, min: f32, max: f32) -> Self {
                self.b = self.b.range(min, max);
                self
            }

            pub fn step(mut self, step: f32) -> Self {
                self.b = self.b.step(step);
                self
            }
        }

        glue_wrapper_common!(GlueSlider);
    }

    pub mod text_input {
        use super::super::*;

        pub use runtime_shared::primitives::text_input::TextInputHandle;

        /// `text_input(value, on_change)` (controlled single-line).
        pub fn text_input(
            value: impl IntoValue<String>,
            on_change: impl Fn(String) + 'static,
        ) -> GlueTextInput {
            GlueTextInput {
                b: builders::text_input().value(value).on_change(on_change),
                a11y: AccessibilityProps::default(),
            }
        }

        pub struct GlueTextInput {
            pub(crate) b: builders::TextInputBuilder,
            pub(crate) a11y: AccessibilityProps,
        }

        impl GlueTextInput {
            /// Concrete `String` (not `impl Into<String>`): `ui!` wraps
            /// literal prop values in `.into()`, and an `impl` param
            /// would leave that coercion's target ambiguous.
            /// Focus-state notifications (`Fn(bool)`, true on focus).
            pub fn on_focus(mut self, f: impl Fn(bool) + 'static) -> Self {
                self.b = self.b.on_focus(f);
                self
            }

            /// Key interception — return [`KeyOutcome`] to swallow keys
            /// (old `Bound::<TextInputHandle>::on_key_down`).
            pub fn on_key_down(mut self, f: impl Fn(&KeyEvent) -> KeyOutcome + 'static) -> Self {
                self.b = self.b.on_key_down(f);
                self
            }

            /// Mirror of `Bound::<TextInputHandle>::placeholder_reactive`.
            pub fn placeholder_reactive(
                mut self,
                placeholder_src: impl Into<super::super::Reactive<Option<String>>>,
            ) -> Self {
                let r = placeholder_src.into();
                self.b = self.b.placeholder_dyn(move || r.get());
                self
            }

            /// Mirror of `Bound::<TextInputHandle>::bind`.
            pub fn bind(mut self, r: super::super::Ref<TextInputHandle>) -> Self {
                self.b = self.b.on_handle(move |h| r.fill(h));
                self
            }

            pub fn placeholder(mut self, text: String) -> Self {
                self.b = self.b.placeholder(text);
                self
            }

            pub fn secure(mut self, secure: impl IntoValue<bool>) -> Self {
                self.b = self.b.secure(secure);
                self
            }
        }

        glue_wrapper_common!(GlueTextInput);
    }

    pub mod scroll_view {
        use super::super::*;

        /// `scroll_view(children)`.
        pub fn scroll_view(children: Vec<Element>) -> GlueScrollView {
            GlueScrollView {
                b: builders::scroll_view().children(children),
                a11y: AccessibilityProps::default(),
            }
        }

        pub struct GlueScrollView {
            pub(crate) b: builders::ScrollViewBuilder,
            pub(crate) a11y: AccessibilityProps,
        }

        impl GlueScrollView {
            pub fn horizontal(mut self, horizontal: bool) -> Self {
                self.b = self.b.horizontal(horizontal);
                self
            }

            pub fn safe_area(mut self, sides: SafeAreaSides) -> Self {
                self.b = self.b.safe_area(sides);
                self
            }

            pub fn on_scroll(mut self, handler: impl Fn(f32, f32) + 'static) -> Self {
                self.b = self.b.on_scroll(handler);
                self
            }

            /// Mirror of `Bound::<ScrollViewHandle>::bind`.
            pub fn bind(mut self, r: super::super::Ref<ScrollViewHandle>) -> Self {
                self.b = self.b.on_handle(move |h| r.fill(h));
                self
            }
        }

        glue_wrapper_common!(GlueScrollView);
    }

    pub mod activity_indicator {
        use super::super::*;

        pub use runtime_shared::primitives::activity_indicator::ActivityIndicatorSize;

        /// `activity_indicator()`.
        pub fn activity_indicator() -> GlueActivityIndicator {
            GlueActivityIndicator {
                b: builders::activity_indicator(),
                a11y: AccessibilityProps::default(),
            }
        }

        pub struct GlueActivityIndicator {
            pub(crate) b: builders::ActivityIndicatorBuilder,
            pub(crate) a11y: AccessibilityProps,
        }

        impl GlueActivityIndicator {
            /// Mirror of `Bound::<ActivityIndicatorHandle>::size_reactive`.
            pub fn size_reactive(mut self, f: impl Fn() -> ActivityIndicatorSize + 'static) -> Self {
                self.b = self.b.size_dyn(f);
                self
            }

            pub fn size(mut self, size: ActivityIndicatorSize) -> Self {
                self.b = self.b.size(size);
                self
            }

            pub fn color(mut self, color: Color) -> Self {
                self.b = self.b.color(color);
                self
            }
        }

        glue_wrapper_common!(GlueActivityIndicator);
    }

    pub mod link {
        use super::super::*;

        /// `external_link(url, children)` — an off-app link
        /// (`Link(external = "…") { … }`).
        pub fn external_link(url: impl IntoValue<String>, children: Vec<Element>) -> GlueLink {
            GlueLink {
                b: builders::link().url(url).external(true).children(children),
                a11y: AccessibilityProps::default(),
            }
        }

        /// `link(route, params, children)` — in-app navigation
        /// (`link(route = …, params = …) { … }`), the old-core
        /// constructor's exact three-positional shape (P6 un-deferral).
        /// The destination resolves against the ENCLOSING navigator's
        /// ambient `LinkActivator` at mount — the swap Selects, the
        /// stack Pushes — and a link mounted outside any navigator
        /// silently no-ops on activation (old contract). The
        /// pre-computed URL feeds the web `<a href>` / right-click
        /// affordances exactly as before.
        pub fn link<P>(
            route: &runtime_shared::primitives::navigator::Route<P>,
            params: P,
            children: Vec<Element>,
        ) -> GlueLink
        where
            P: runtime_shared::primitives::navigator::RouteParams + Clone + 'static,
        {
            GlueLink {
                b: builders::link().route(route, params).children(children),
                a11y: AccessibilityProps::default(),
            }
        }

        pub struct GlueLink {
            pub(crate) b: builders::LinkBuilder,
            pub(crate) a11y: AccessibilityProps,
        }

        impl GlueLink {
            /// Mirror of the old reactive `.url()` setter (href swaps
            /// in place on signal change).
            pub fn url(mut self, url: impl IntoValue<String>) -> Self {
                self.b = self.b.url(url);
                self
            }

            pub fn on_activate(mut self, f: impl Fn() + 'static) -> Self {
                self.b = self.b.on_activate(f);
                self
            }
        }

        glue_wrapper_common!(GlueLink, test_id_ignored);
    }

    /// `overlay(children)` / `anchored_overlay(target, children)` — the
    /// portal compositions. NOTE: unlike the other primitives, the OLD
    /// core's overlay builders are NOT `Bound<H>` — they're plain
    /// composition builders with no accessibility setters (the lowering
    /// hardcodes a default a11y bag on the portal). These wrappers
    /// mirror that surface exactly: no `a11y_*` methods, `with_style`
    /// styles the CONTENT WRAPPER (the old `OverlayBuilder::with_style`
    /// → `content_style` semantics).
    pub mod overlay {
        use super::super::*;
        pub use runtime_shared::primitives::overlay::BackdropMode;
        pub use runtime_shared::primitives::portal::{
            AnchorTarget, ElementAlign, ElementSide, PortalHandle, ViewportPlacement,
        };

        /// Viewport-anchored overlay (modal, drawer, sheet). Defaults
        /// mirror the old core: `Center`, `Dismiss` backdrop, trap ON.
        pub fn overlay(children: Vec<Element>) -> GlueOverlay {
            GlueOverlay { b: builders::overlay().children(children) }
        }

        pub struct GlueOverlay {
            pub(crate) b: builders::OverlayBuilder,
        }

        impl GlueOverlay {
            pub fn placement(mut self, p: ViewportPlacement) -> Self {
                self.b = self.b.placement(p);
                self
            }

            pub fn backdrop(mut self, m: BackdropMode) -> Self {
                self.b = self.b.backdrop(m);
                self
            }

            pub fn backdrop_style(mut self, s: impl IntoStyleProp) -> Self {
                self.b = self.b.backdrop_style(s);
                self
            }

            /// No `cycle()` wrap (old core wrapped for born-batched
            /// semantics): on the staged-commit kernel every write
            /// stages until the driver flush by design.
            pub fn on_dismiss(mut self, f: impl Fn() + 'static) -> Self {
                self.b = self.b.on_dismiss(f);
                self
            }

            pub fn trap_focus(mut self, t: bool) -> Self {
                self.b = self.b.trap_focus(t);
                self
            }

            pub fn click_through(mut self, t: bool) -> Self {
                self.b = self.b.click_through(t);
                self
            }

            /// Content-wrapper style (old `OverlayBuilder::with_style`).
            pub fn with_style(mut self, s: impl IntoStyleProp) -> Self {
                self.b = self.b.style(s);
                self
            }

            /// P2 form of the old `.bind(ref)`.
            pub fn on_handle(mut self, fill: impl FnOnce(PortalHandle) + 'static) -> Self {
                self.b = self.b.on_handle(fill);
                self
            }
        }

        impl IntoElement for GlueOverlay {
            fn into_element(self) -> Element {
                self.b.build()
            }
        }

        impl ChildList for GlueOverlay {
            fn append_to(self, out: &mut Vec<Element>) {
                out.push(self.into_element());
            }
        }

        impl From<GlueOverlay> for Element {
            fn from(w: GlueOverlay) -> Element {
                w.into_element()
            }
        }

        /// Element-anchored overlay (popover, tooltip, menu). Defaults
        /// mirror the old core: `Below`/`Start`, offset 0, NO backdrop,
        /// trap OFF.
        pub fn anchored_overlay(target: AnchorTarget, children: Vec<Element>) -> GlueAnchoredOverlay {
            GlueAnchoredOverlay { b: builders::anchored_overlay(target).children(children) }
        }

        pub struct GlueAnchoredOverlay {
            pub(crate) b: builders::AnchoredOverlayBuilder,
        }

        impl GlueAnchoredOverlay {
            pub fn side(mut self, s: ElementSide) -> Self {
                self.b = self.b.side(s);
                self
            }

            pub fn align(mut self, a: ElementAlign) -> Self {
                self.b = self.b.align(a);
                self
            }

            pub fn offset(mut self, o: f32) -> Self {
                self.b = self.b.offset(o);
                self
            }

            pub fn backdrop(mut self, m: BackdropMode) -> Self {
                self.b = self.b.backdrop(m);
                self
            }

            pub fn backdrop_style(mut self, s: impl IntoStyleProp) -> Self {
                self.b = self.b.backdrop_style(s);
                self
            }

            pub fn on_dismiss(mut self, f: impl Fn() + 'static) -> Self {
                self.b = self.b.on_dismiss(f);
                self
            }

            pub fn trap_focus(mut self, t: bool) -> Self {
                self.b = self.b.trap_focus(t);
                self
            }

            /// Content-wrapper style.
            pub fn with_style(mut self, s: impl IntoStyleProp) -> Self {
                self.b = self.b.style(s);
                self
            }

            pub fn on_handle(mut self, fill: impl FnOnce(PortalHandle) + 'static) -> Self {
                self.b = self.b.on_handle(fill);
                self
            }
        }

        impl IntoElement for GlueAnchoredOverlay {
            fn into_element(self) -> Element {
                self.b.build()
            }
        }

        impl ChildList for GlueAnchoredOverlay {
            fn append_to(self, out: &mut Vec<Element>) {
                out.push(self.into_element());
            }
        }

        impl From<GlueAnchoredOverlay> for Element {
            fn from(w: GlueAnchoredOverlay) -> Element {
                w.into_element()
            }
        }
    }

    /// `presence(child_fn)` — the animated-lifecycle wrapper.
    pub mod presence {
        use super::super::*;
        pub use runtime_shared::primitives::presence::{PresenceAnim, PresenceHandle, PresenceState};

        /// `child` runs once per real mount (present flips true after a
        /// completed unmount). The macro passes `move || <child expr>`.
        pub fn presence<E: IntoElement>(child: impl Fn() -> E + 'static) -> GluePresence {
            GluePresence {
                b: builders::presence(move || child().into_element()),
                a11y: AccessibilityProps::default(),
            }
        }

        pub struct GluePresence {
            pub(crate) b: builders::PresenceBuilder,
            pub(crate) a11y: AccessibilityProps,
        }

        impl GluePresence {
            /// Robot/automation anchor (`test_id = …`) — forwarded to
            /// the presence prim's identity slot (registered on the
            /// placeholder by the mount handler under `robot`).
            pub fn test_id(mut self, id: &'static str) -> Self {
                self.b = self.b.test_id(id);
                self
            }

            /// The presence predicate. The old surface took a bare
            /// `Fn() -> bool` closure; `IntoValue<bool>` accepts the
            /// same closures (plus signals/bools — a superset, same
            /// observable semantics for the closure form).
            pub fn present(mut self, present: impl IntoValue<bool>) -> Self {
                self.b = self.b.present(present);
                self
            }

            pub fn enter(mut self, anim: PresenceAnim) -> Self {
                self.b = self.b.enter(anim);
                self
            }

            pub fn exit(mut self, anim: PresenceAnim) -> Self {
                self.b = self.b.exit(anim);
                self
            }

            /// Accepted-and-ignored, mirroring the old core exactly:
            /// `Element::Presence` carries no style — "styling belongs
            /// on the child View, not on the Presence node" (see
            /// `Element::with_style`'s Presence arm). Kept so
            /// `presence(style = …)` compiles identically on both cores.
            pub fn with_style(self, _style: impl IntoStyleProp) -> Self {
                self
            }

            pub fn accessibility(mut self, a11y: AccessibilityProps) -> Self {
                self.a11y = a11y;
                self
            }

            pub fn a11y_label(mut self, label: impl Into<String>) -> Self {
                self.a11y.label = Some(label.into());
                self
            }

            pub fn a11y_hint(mut self, hint: impl Into<String>) -> Self {
                self.a11y.hint = Some(hint.into());
                self
            }

            pub fn a11y_role(mut self, role: Role) -> Self {
                self.a11y.role = Some(role);
                self
            }

            pub fn a11y_hidden(mut self, hidden: bool) -> Self {
                self.a11y.hidden = hidden;
                self
            }

            pub fn a11y_traits(mut self, traits: AccessibilityTraits) -> Self {
                self.a11y.traits = traits;
                self
            }

            pub fn live_region(mut self, priority: LiveRegionPriority) -> Self {
                self.a11y.live_region = Some(priority);
                self
            }

            /// P2 form of the old `.bind(ref)`.
            pub fn on_handle(mut self, fill: impl FnOnce(PresenceHandle) + 'static) -> Self {
                self.b = self.b.on_handle(fill);
                self
            }
        }

        impl IntoElement for GluePresence {
            fn into_element(self) -> Element {
                self.b.a11y(self.a11y).build()
            }
        }

        impl ChildList for GluePresence {
            fn append_to(self, out: &mut Vec<Element>) {
                out.push(self.into_element());
            }
        }

        impl From<GluePresence> for Element {
            fn from(w: GluePresence) -> Element {
                w.into_element()
            }
        }
    }

    /// `graphics(on_ready)` — the author-driven GPU surface.
    pub mod graphics {
        use super::super::*;
        pub use runtime_shared::primitives::graphics::{GraphicsHandle, OnReadyEvent, OnResizeEvent};

        pub fn graphics(on_ready: impl FnMut(OnReadyEvent) + 'static) -> GlueGraphics {
            GlueGraphics {
                b: builders::graphics(on_ready),
                a11y: AccessibilityProps::default(),
            }
        }

        pub struct GlueGraphics {
            pub(crate) b: builders::GraphicsBuilder,
            pub(crate) a11y: AccessibilityProps,
        }

        impl GlueGraphics {
            /// No `cycle()` wrap (old core wrapped for born-batched
            /// semantics) — staged-commit writes flush with the driver.
            pub fn on_resize(mut self, f: impl FnMut(OnResizeEvent) + 'static) -> Self {
                self.b = self.b.on_resize(f);
                self
            }

            pub fn on_lost(mut self, f: impl FnMut() + 'static) -> Self {
                self.b = self.b.on_lost(f);
                self
            }

            /// P2 form of the old `.bind(ref)`.
            pub fn on_handle(mut self, fill: impl FnOnce(GraphicsHandle) + 'static) -> Self {
                self.b = self.b.on_handle(fill);
                self
            }
        }

        glue_wrapper_common!(GlueGraphics);
    }

    /// `flat_list(data, key, size, render)` — the typed wrapper over the
    /// closure-form virtualizer. This is a PORT of the old
    /// `primitives/flat_list.rs` type-erasure (the old constructor was
    /// itself just an adapter onto `virtualizer(...)`), targeting the
    /// P3-set `builders::virtualizer`. `FlatListItemSize` / `fixed_size`
    /// / `Axis` / `Lanes` are the SAME types as the old core (re-exported),
    /// so author sources compile identically on both cores.
    pub mod flat_list {
        use super::super::*;
        pub use runtime_shared::primitives::flat_list::{fixed_size, FlatListItemSize};
        pub use runtime_shared::primitives::virtualizer::{
            Axis, ItemKey, ItemSize, Lanes, VirtualizerHandle,
        };

        /// Same signature as the old `flat_list<T, K, S, R>` (the third
        /// generic is unused there too — the macro emits `::<_, _, (), _>`).
        /// Extra `T: PartialEq` bound: `runtime_world::Signal<Vec<T>>`
        /// only exists for `PartialEq` payloads (guarded staging), so the
        /// bound is already implied by the `data` argument existing.
        pub fn flat_list<T, K, S, R>(
            data: Signal<Vec<T>>,
            key: K,
            item_size: FlatListItemSize<T>,
            render_item: R,
        ) -> GlueFlatList
        where
            T: Clone + PartialEq + 'static,
            K: Fn(usize, &T) -> ItemKey + 'static,
            S: 'static,
            R: Fn(usize, &T) -> Element + 'static,
        {
            let _ = std::marker::PhantomData::<S>;

            // World signal handles are Copy — each erased closure gets
            // its own copy of `data` and reads the current snapshot at
            // call time (identical to the old adapter).
            let item_count = move || data.get().len();

            let key = Rc::new(key);
            let item_key = {
                let key = key.clone();
                move |idx: usize| {
                    let v = data.get();
                    match v.get(idx) {
                        Some(item) => key(idx, item),
                        // Out-of-range sentinel (old adapter's rule):
                        // don't collide with valid keys on a stale index.
                        None => u64::MAX - idx as u64,
                    }
                }
            };

            let item_size: ItemSize = match item_size {
                FlatListItemSize::Known(f) => ItemSize::Known(Rc::new(move |idx| {
                    let v = data.get();
                    v.get(idx).map(|item| f(idx, item)).unwrap_or(0.0)
                })),
                FlatListItemSize::Measured(f) => ItemSize::Measured(Rc::new(move |idx| {
                    let v = data.get();
                    v.get(idx).map(|item| f(idx, item)).unwrap_or(0.0)
                })),
            };

            let render = move |idx: usize| -> Element {
                let v = data.get();
                match v.get(idx) {
                    Some(item) => render_item(idx, item),
                    // Stale index → empty view (old adapter's rule).
                    None => super::super::view(Vec::new()).into_element(),
                }
            };

            GlueFlatList {
                b: builders::virtualizer(item_count, item_key, item_size, render),
                a11y: AccessibilityProps::default(),
            }
        }

        pub struct GlueFlatList {
            pub(crate) b: builders::VirtualizerBuilder,
            pub(crate) a11y: AccessibilityProps,
        }

        impl GlueFlatList {
            pub fn overscan(mut self, factor: f32) -> Self {
                self.b = self.b.overscan(factor);
                self
            }

            pub fn axis(mut self, axis: Axis) -> Self {
                self.b = self.b.axis(axis);
                self
            }

            pub fn lanes(mut self, lanes: Lanes) -> Self {
                self.b = self.b.lanes(lanes);
                self
            }

            pub fn spacing(mut self, main: f32, cross: f32) -> Self {
                self.b = self.b.spacing(main, cross);
                self
            }

            pub fn gap(mut self, gap: f32) -> Self {
                self.b = self.b.gap(gap);
                self
            }

            /// P2 form of the old `.bind(ref)`.
            pub fn on_handle(mut self, fill: impl FnOnce(VirtualizerHandle) + 'static) -> Self {
                self.b = self.b.on_handle(fill);
                self
            }
        }

        glue_wrapper_common!(GlueFlatList, test_id_ignored);
    }

    /// Mirror of `runtime_shared::primitives::portal` (data types; the
    /// overlay/anchored_overlay COMPOSITIONS live in
    /// [`overlay`](super::primitives::overlay), same as the old split).
    pub mod portal {
        pub use runtime_shared::primitives::portal::{
            AnchorTarget, AnchorableHandle, ElementAlign, ElementSide, PortalHandle, PortalTarget,
            ViewportPlacement, ViewportRect,
        };
    }

    /// Mirror of `runtime_shared::primitives::key` (keyboard event data).
    pub mod key {
        pub use runtime_shared::primitives::key::{KeyEvent, KeyOutcome};
    }

    /// Mirror of `runtime_shared::primitives::navigator`'s DATA types
    /// (routes + header slot data an author chrome component renders).
    /// The navigator RUNTIME on the new core is the vocabulary's
    /// `handlers::navigator` (`SwapNav`/`StackNav` world contexts) —
    /// old `SwapContext`/`ScreenNav` carry old-arena signals and are
    /// deliberately NOT mirrored (the P6 nav SDKs define their own).
    /// `Screen` is the NEW-core carrier (scene `Element` + opaque
    /// options — see `prims::Screen`), spelled at the old type's path
    /// so SDK preludes map it same-source.
    pub mod navigator {
        pub use crate::prims::{Screen, ScreenChrome};
        pub use runtime_shared::primitives::navigator::{
            HeaderButton, Route, RouteParams, StackHeaderState,
        };
    }

    /// Mirror of `runtime_shared::primitives::text_area`.
    pub mod text_area {
        use super::super::*;

        pub use runtime_shared::primitives::text_area::TextAreaHandle;

        /// `text_area(value, on_change)` — the old positional constructor
        /// over the P2b `TextAreaBuilder`.
        pub fn text_area(
            value: impl runtime_world::IntoValue<String>,
            on_change: impl Fn(String) + 'static,
        ) -> GlueTextArea {
            GlueTextArea {
                b: builders::text_area().value(value).on_change(on_change),
                a11y: AccessibilityProps::default(),
            }
        }

        pub struct GlueTextArea {
            pub(crate) b: builders::TextAreaBuilder,
            pub(crate) a11y: AccessibilityProps,
        }

        impl GlueTextArea {
            pub fn placeholder(mut self, text: String) -> Self {
                self.b = self.b.placeholder(text);
                self
            }

            pub fn min_rows(mut self, rows: u32) -> Self {
                self.b = self.b.min_rows(rows);
                self
            }

            pub fn max_rows(mut self, rows: u32) -> Self {
                self.b = self.b.max_rows(rows);
                self
            }

            /// Toggle soft-wrapping. `true` (the default) wraps long lines at
            /// the box edge; `false` keeps them unwrapped and scrolls
            /// horizontally — the code-editor shape. Mirrors
            /// `TextAreaBuilder::wrap`, which this wrapper otherwise hid.
            pub fn wrap(mut self, wrap: bool) -> Self {
                self.b = self.b.wrap(wrap);
                self
            }

            /// Convenience for the code-editor shape: unwrapped lines that
            /// scroll horizontally. Equivalent to `.wrap(false)`. A code editor
            /// is fixed-height, so pair it with a pinned height or a sized
            /// parent at the call site.
            pub fn code_mode(self) -> Self {
                self.wrap(false)
            }

            /// Mirror of `Bound::<TextAreaHandle>::bind`.
            pub fn bind(mut self, r: super::super::Ref<TextAreaHandle>) -> Self {
                self.b = self.b.on_handle(move |h| r.fill(h));
                self
            }
        }

        glue_wrapper_common!(GlueTextArea);
    }
}

// ============================================================================
// when / switch — the reactive-branch lowerings.
// ============================================================================

/// Reactive `if`: `when(cond, then, otherwise)` lowers to the scene's
/// GUARDED hole ([`dyn_keyed`]) keyed on the predicate's boolean — the
/// old walker's `last_active` dedup: a predicate that reads extra
/// signals must not rebuild the branch when only those extras changed.
pub fn when<E1, E2>(
    cond: impl Fn() -> bool + 'static,
    then: impl Fn() -> E1 + 'static,
    otherwise: impl Fn() -> E2 + 'static,
) -> Element
where
    E1: IntoElement,
    E2: IntoElement,
{
    dyn_keyed(cond, move |active: &bool| {
        if *active {
            then().into_element()
        } else {
            otherwise().into_element()
        }
    })
}

/// Reactive `match`: keyed on the scrutinee VALUE (`PartialEq` dedup —
/// a re-fire producing an equal scrutinee keeps the mounted arm).
pub fn switch<S: PartialEq + 'static>(
    scrutinee: impl Fn() -> S + 'static,
    render: impl Fn(&S) -> Element + 'static,
) -> Element {
    dyn_keyed(scrutinee, render)
}

// ============================================================================
// StaticCond / ReactiveCond — type-driven `if COND` dispatch for a bare
// path/field condition (`if visible`, `if props.open`).
// ============================================================================

/// The static arm: a plain `bool` runs the taken branch's thunk once,
/// contributing its nodes as FLAT siblings (no wrapper, no reactivity).
///
/// `FnOnce` (not `Fn`), matching old-core `builder.rs::StaticCond`
/// exactly: a static branch runs at most once, so its thunk may MOVE
/// captures (`text { some_string }`). The original `Fn + 'static`
/// bounds here were a retarget bug — same-source components that
/// hoisted a condition to a `let bool` (the documented 0.4.0 form)
/// compiled old-core but hit E0507 ("cannot move out of a captured
/// variable in an `Fn` closure") under new-core, forcing `.clone()`
/// workarounds (website architecture.rs, tutorial chart.rs class).
pub trait StaticCond {
    fn __idealyst_if<T, E>(self, then: T, otherwise: E) -> Vec<Element>
    where
        T: FnOnce() -> Vec<Element>,
        E: FnOnce() -> Vec<Element>;
}

impl StaticCond for bool {
    fn __idealyst_if<T, E>(self, then: T, otherwise: E) -> Vec<Element>
    where
        T: FnOnce() -> Vec<Element>,
        E: FnOnce() -> Vec<Element>,
    {
        if self {
            then()
        } else {
            otherwise()
        }
    }
}

/// The reactive arm: a `Signal<bool>` / `ReadSignal<bool>` /
/// `Memo<bool>` / `Dynamic` [`Reactive<bool>`] condition becomes ONE
/// guarded hole ([`when`]); branch thunks collapse via [`one_or_view`].
pub trait ReactiveCond {
    fn __idealyst_if(
        self,
        then: impl Fn() -> Vec<Element> + 'static,
        otherwise: impl Fn() -> Vec<Element> + 'static,
    ) -> Vec<Element>;
}

macro_rules! impl_reactive_cond {
    ($($ty:ty),+ $(,)?) => {$(
        impl ReactiveCond for $ty {
            fn __idealyst_if(
                self,
                then: impl Fn() -> Vec<Element> + 'static,
                otherwise: impl Fn() -> Vec<Element> + 'static,
            ) -> Vec<Element> {
                vec![when(move || self.get(), move || one_or_view(then()), move || one_or_view(otherwise()))]
            }
        }
    )+};
}

impl_reactive_cond!(Signal<bool>, ReadSignal<bool>, Memo<bool>);

impl ReactiveCond for Reactive<bool> {
    fn __idealyst_if(
        self,
        then: impl Fn() -> Vec<Element> + 'static,
        otherwise: impl Fn() -> Vec<Element> + 'static,
    ) -> Vec<Element> {
        match self {
            Reactive::Static(b) => b.__idealyst_if(then, otherwise),
            Reactive::Dynamic(f) => {
                vec![when(move || f(), move || one_or_view(then()), move || one_or_view(otherwise()))]
            }
        }
    }
}

// ============================================================================
// StaticForEach / ReactiveForEach — type-driven `for` dispatch.
// ============================================================================

/// Static loops: any `IntoIterator` builds its rows once, flat. The
/// keyed method exists so `for x in vec, key = …` compiles (the key is
/// evaluated but unused — static rows never reconcile).
pub trait StaticForEach: IntoIterator + Sized {
    fn __idealyst_for_each(self, row: impl Fn(Self::Item) -> Vec<Element>) -> Vec<Element> {
        let mut out = Vec::new();
        for item in self {
            out.extend(row(item));
        }
        out
    }

    fn __idealyst_for_each_keyed<K: Into<Key>>(
        self,
        key: impl Fn(Self::Item) -> K,
        row: impl Fn(Self::Item) -> Vec<Element>,
    ) -> Vec<Element>
    where
        Self::Item: Clone,
    {
        let mut out = Vec::new();
        for item in self {
            let _ = key(item.clone());
            out.extend(row(item));
        }
        out
    }
}

impl<I: IntoIterator> StaticForEach for I {}

/// Reactive keyed loops: a `Signal` (or `ReadSignal`/`Memo`) of a
/// cloneable collection becomes ONE [`runtime_scene::keyed`] element —
/// rows are kept/created/dropped by key identity, so row-local state
/// survives edits elsewhere in the list.
///
/// There is deliberately NO keyless method here: a keyless
/// `for x in signal { … }` fails to compile (`__idealyst_for_each` is
/// only defined on `IntoIterator` types), which is the migration of the
/// old `ReactiveListKeyed` diagnostic — a reactive list must be keyed.
pub trait ReactiveForEach<T> {
    fn __idealyst_for_each_keyed<K: Into<Key>>(
        self,
        key: impl Fn(T) -> K + 'static,
        row: impl Fn(T) -> Vec<Element> + 'static,
    ) -> Vec<Element>;
}

macro_rules! impl_reactive_for_each {
    ($($handle:ident),+ $(,)?) => {$(
        impl<C, T> ReactiveForEach<T> for $handle<C>
        where
            C: IntoIterator<Item = T> + Clone + PartialEq + 'static,
            T: Clone + 'static,
        {
            fn __idealyst_for_each_keyed<K: Into<Key>>(
                self,
                key: impl Fn(T) -> K + 'static,
                row: impl Fn(T) -> Vec<Element> + 'static,
            ) -> Vec<Element> {
                vec![runtime_scene::keyed(
                    move || self.get().into_iter().collect::<Vec<T>>(),
                    move |item: &T| key(item.clone()),
                    move |item: T| one_or_fragment(row(item)),
                )]
            }
        }
    )+};
}

impl_reactive_for_each!(Signal, ReadSignal, Memo);

// ============================================================================
// each_keyed — the reactive-RANGE loop lowering (`for i in 0..n.get()`).
// ============================================================================

/// A row's identity in an [`each_keyed`] list.
pub struct EachKey(pub Key);

impl EachKey {
    pub fn new(key: impl Into<Key>) -> Self {
        EachKey(key.into())
    }
}

/// Deferred row constructor: builds the row's (possibly multi-) node list.
/// `FnOnce`, mirroring the old core exactly — the keyed reconciler calls
/// each build at most once (kept keys never re-run their build), and row
/// closures legitimately move captures into their nodes.
pub type EachRowBuild = Box<dyn FnOnce() -> Vec<Element>>;

/// A keyed reactive list from a tracked `(key, build)` producer — the
/// lowering target for reactive ranges. Maps onto [`runtime_scene::keyed`]:
/// kept keys reuse their live subtree (`build` is NOT re-run), so
/// growing/shrinking the range keeps surviving rows' state.
pub fn each_keyed(items: impl Fn() -> Vec<(EachKey, EachRowBuild)> + 'static) -> Element {
    runtime_scene::keyed(
        items,
        |(key, _): &(EachKey, EachRowBuild)| key.0.clone(),
        |(_, build): (EachKey, EachRowBuild)| one_or_fragment(build()),
    )
}

// ============================================================================
// Text f-string slots (`text { "{count} items" }`).
// ============================================================================

/// One piece of an interpolated text literal.
pub enum TextSlotPart {
    /// A literal fragment.
    Lit(&'static str),
    /// An interpolation slot, already classified by TYPE.
    Slot(TextSlot),
}

/// One interpolation slot — the new-core mirror of
/// `runtime_shared::TextSlot` (sources.rs), tier for tier: a `Display`
/// value bakes in statically; a signal-backed slot carries its
/// [`Signal::raw_id`] so JS-binding backends fan out without entering
/// Rust per fire; an opaque computed slot (a `Reactive::Dynamic` prop —
/// no signal id) forces the whole text down the effect path.
pub enum TextSlot {
    /// Value formatted once at build time.
    Static(String),
    /// Signal-backed slot (id + initial + untracked/tracked readers).
    Live {
        id: u64,
        initial: String,
        /// UNTRACKED read+format — the backend registration contract
        /// (reactivity flows through the notifier effect, not this).
        stringify: Rc<dyn Fn() -> String>,
        /// TRACKED read+format — the per-signal notifier effect's body
        /// AND the Effect-fallback's per-slot reader.
        read: Rc<dyn Fn() -> String>,
    },
    /// Reactive but with no signal id: whole text → effect path. TRACKED.
    Computed(Rc<dyn Fn() -> String>),
}

/// The static slot arm: any `Display` value formats once.
pub trait StaticTextSlot {
    fn __idealyst_text_slot(
        self,
        fmt: impl Fn(&dyn std::fmt::Display) -> String + 'static,
    ) -> TextSlot;
}

impl<T: std::fmt::Display> StaticTextSlot for T {
    fn __idealyst_text_slot(
        self,
        fmt: impl Fn(&dyn std::fmt::Display) -> String + 'static,
    ) -> TextSlot {
        TextSlot::Static(fmt(&self))
    }
}

/// The reactive slot arm: a `Signal` / `ReadSignal` / `Memo` slot is
/// LIVE (id-bearing — JS fast path eligible); a `Dynamic` [`Reactive`]
/// is opaque-computed. Method resolution picks this over
/// [`StaticTextSlot`] because the handles don't implement `Display`.
pub trait ReactiveTextSlot {
    fn __idealyst_text_slot(
        self,
        fmt: impl Fn(&dyn std::fmt::Display) -> String + 'static,
    ) -> TextSlot;
}

macro_rules! impl_reactive_text_slot {
    ($($handle:ident),+ $(,)?) => {$(
        impl<T> ReactiveTextSlot for $handle<T>
        where
            T: std::fmt::Display + Clone + PartialEq + 'static,
        {
            fn __idealyst_text_slot(
                self,
                fmt: impl Fn(&dyn std::fmt::Display) -> String + 'static,
            ) -> TextSlot {
                let fmt = Rc::new(fmt);
                let fmt_read = fmt.clone();
                // Initial computed UNTRACKED at slot construction (the
                // old core's contract: building an element must not
                // subscribe the ambient scope).
                let initial = untrack(|| fmt(&self.get()));
                TextSlot::Live {
                    id: self.raw_id(),
                    initial,
                    stringify: Rc::new({
                        let fmt = fmt.clone();
                        move || untrack(|| fmt(&self.get()))
                    }),
                    read: Rc::new(move || fmt_read(&self.get())),
                }
            }
        }
    )+};
}

impl_reactive_text_slot!(Signal, ReadSignal, Memo);

impl<T: std::fmt::Display + Clone + 'static> ReactiveTextSlot for Reactive<T> {
    fn __idealyst_text_slot(
        self,
        fmt: impl Fn(&dyn std::fmt::Display) -> String + 'static,
    ) -> TextSlot {
        match self {
            Reactive::Static(v) => TextSlot::Static(fmt(&v)),
            Reactive::Dynamic(f) => TextSlot::Computed(Rc::new(move || fmt(&f()))),
        }
    }
}

/// The assembled f-string — what [`__idealyst_text_from_parts`] hands to
/// `text(...)` through the [`TextContent`] seam. The `JsBinding` arm is
/// the pre-decomposed fast path; consumers that only take a plain value
/// (button labels) degrade it to `Value::Dyn(compute_fallback)` via
/// `into_content` — reactivity identical, delivery via effect.
pub enum AssembledText {
    Value(Value<String>),
    JsBinding(crate::prims::JsTextBinding),
}

impl TextContent for AssembledText {
    fn into_content(self) -> Value<String> {
        match self {
            AssembledText::Value(v) => v,
            AssembledText::JsBinding(b) => {
                let compute = b.compute_fallback;
                Value::Dyn(Box::new(move || compute()))
            }
        }
    }

    fn into_content_prop(self) -> crate::prims::TextSourceProp {
        match self {
            AssembledText::Value(v) => crate::prims::TextSourceProp::Value(v),
            AssembledText::JsBinding(b) => crate::prims::TextSourceProp::JsBinding(Box::new(b)),
        }
    }
}

/// Assemble f-string pieces into the most capable source the slots allow
/// — port of `runtime_shared::__idealyst_text_from_parts`, tier for tier:
/// all-static → one `Const` concatenation; any opaque computed slot →
/// one `Dyn` closure (effect path); live-signal slots only → the
/// pre-decomposed [`JsTextBinding`](crate::prims::JsTextBinding) (JS
/// fast path on capable backends, `compute_fallback` effect elsewhere).
pub fn __idealyst_text_from_parts(parts: Vec<TextSlotPart>) -> AssembledText {
    let any_computed = parts
        .iter()
        .any(|p| matches!(p, TextSlotPart::Slot(TextSlot::Computed(_))));
    let any_live = parts
        .iter()
        .any(|p| matches!(p, TextSlotPart::Slot(TextSlot::Live { .. })));

    if !any_computed && !any_live {
        let mut s = String::new();
        for p in &parts {
            match p {
                TextSlotPart::Lit(l) => s.push_str(l),
                TextSlotPart::Slot(TextSlot::Static(v)) => s.push_str(v),
                _ => unreachable!("no live/computed slots checked"),
            }
        }
        return AssembledText::Value(Value::Const(s));
    }

    if any_computed {
        // Opaque slot present — effect path for the whole text (tracked
        // reads inside the closure subscribe it to every live input).
        let readers: Vec<Rc<dyn Fn() -> String>> = parts
            .into_iter()
            .map(|p| -> Rc<dyn Fn() -> String> {
                match p {
                    TextSlotPart::Lit(l) => Rc::new(move || l.to_string()),
                    TextSlotPart::Slot(TextSlot::Static(v)) => Rc::new(move || v.clone()),
                    TextSlotPart::Slot(TextSlot::Live { read, .. }) => read,
                    TextSlotPart::Slot(TextSlot::Computed(read)) => read,
                }
            })
            .collect();
        return AssembledText::Value(Value::Dyn(Box::new(move || {
            let mut s = String::new();
            for r in &readers {
                s.push_str(&r());
            }
            s
        })));
    }

    // Live slots only: fold static text into the N+1 template parts
    // around each signal slot (the JsBindingSpec field contract).
    let mut template_parts: Vec<String> = vec![String::new()];
    let mut signal_ids: Vec<u64> = Vec::new();
    let mut initial_values: Vec<String> = Vec::new();
    let mut stringifiers: Vec<Rc<dyn Fn() -> String>> = Vec::new();
    let mut tracked_reads: Vec<Rc<dyn Fn() -> String>> = Vec::new();
    for p in parts {
        match p {
            TextSlotPart::Lit(l) => template_parts.last_mut().unwrap().push_str(l),
            TextSlotPart::Slot(TextSlot::Static(v)) => {
                template_parts.last_mut().unwrap().push_str(&v)
            }
            TextSlotPart::Slot(TextSlot::Live { initial, stringify, read, id }) => {
                signal_ids.push(id);
                initial_values.push(initial);
                stringifiers.push(stringify);
                tracked_reads.push(read);
                template_parts.push(String::new());
            }
            TextSlotPart::Slot(TextSlot::Computed(_)) => unreachable!("any_computed checked"),
        }
    }
    let fallback_parts = template_parts.clone();
    let fallback_reads = tracked_reads.clone();
    AssembledText::JsBinding(crate::prims::JsTextBinding {
        signal_ids,
        template_parts,
        initial_values,
        compute_fallback: Rc::new(move || {
            let mut s = String::new();
            for (i, part) in fallback_parts.iter().enumerate() {
                s.push_str(part);
                if let Some(r) = fallback_reads.get(i) {
                    s.push_str(&r());
                }
            }
            s
        }),
        stringifiers,
        tracked_reads,
    })
}

// ============================================================================
// Reactive<T> — the props model (`#[component]` wraps data props in it).
// ============================================================================

/// A prop value: a fixed snapshot (`Static`) or a live computation
/// (`Dynamic`). API-identical to the old core's `Reactive<T>`
/// (`runtime_shared::reactive_value`) so component bodies compile
/// unchanged; the live arm reads through the NEW kernel (its `get()`
/// inside a binding effect subscribes via world signals).
///
/// This is the transitional `Value<T>`-with-`From`-coercions form the
/// migration plan's §7 calls for: `Static`/`Const` and `Dynamic`/`Dyn`
/// are isomorphic, and [`IntoValue`] is implemented so a `Reactive` prop
/// forwards straight into any builder prop.
pub enum Reactive<T> {
    /// A one-time value. No subscription, no reactivity.
    Static(T),
    /// A live computation; reading it inside a binding effect subscribes
    /// to the signals the closure touches.
    Dynamic(Rc<dyn Fn() -> T>),
}

impl<T> Reactive<T> {
    /// Build a `Dynamic` from a closure.
    pub fn derive<F: Fn() -> T + 'static>(f: F) -> Self {
        Reactive::Dynamic(Rc::new(f))
    }

    /// True for the `Static` arm — lets a component keep a zero-effect
    /// fast path when no reactive prop was passed.
    pub fn is_static(&self) -> bool {
        matches!(self, Reactive::Static(_))
    }
}

impl<T: Clone> Reactive<T> {
    /// Read the current value. On `Dynamic`, runs the closure — tracked
    /// when called inside a running effect.
    pub fn get(&self) -> T {
        match self {
            Reactive::Static(v) => v.clone(),
            Reactive::Dynamic(f) => f(),
        }
    }

    /// Read without subscribing (snapshot intent declared).
    pub fn get_untracked(&self) -> T {
        match self {
            Reactive::Static(v) => v.clone(),
            Reactive::Dynamic(f) => untrack(|| f()),
        }
    }

    /// Convert into a closure for APIs that take `Fn() -> T`.
    pub fn into_closure(self) -> Rc<dyn Fn() -> T>
    where
        T: 'static,
    {
        match self {
            Reactive::Static(v) => Rc::new(move || v.clone()),
            Reactive::Dynamic(f) => f,
        }
    }

    /// Drive a sink: `Static` applies once (no effect); `Dynamic`
    /// installs a binding effect owned by the ambient collector (the
    /// enclosing component scope / realized subtree).
    pub fn bind(self, mut sink: impl FnMut(T) + 'static)
    where
        T: 'static,
    {
        match self {
            Reactive::Static(v) => sink(v),
            Reactive::Dynamic(f) => {
                let _ = effect(move || sink(f()));
            }
        }
    }
}

impl<T: Clone> Clone for Reactive<T> {
    fn clone(&self) -> Self {
        match self {
            Reactive::Static(v) => Reactive::Static(v.clone()),
            Reactive::Dynamic(f) => Reactive::Dynamic(f.clone()),
        }
    }
}

impl<T: Default> Default for Reactive<T> {
    fn default() -> Self {
        Reactive::Static(T::default())
    }
}

impl<T: std::fmt::Debug> std::fmt::Debug for Reactive<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Reactive::Static(v) => f.debug_tuple("Reactive::Static").field(v).finish(),
            Reactive::Dynamic(_) => f.write_str("Reactive::Dynamic(<closure>)"),
        }
    }
}

/// Bare value → `Static` (the `ui!` `.into()` coercion; same coherence
/// argument as the old core: `From<T>` and `From<Signal<T>>` can't
/// overlap — `T = Signal<T>` fails the occurs check).
impl<T> From<T> for Reactive<T> {
    fn from(v: T) -> Self {
        Reactive::Static(v)
    }
}

impl From<&str> for Reactive<String> {
    fn from(s: &str) -> Self {
        Reactive::Static(s.to_string())
    }
}

impl<T: Clone + PartialEq + 'static> From<Signal<T>> for Reactive<T> {
    fn from(sig: Signal<T>) -> Self {
        Reactive::Dynamic(Rc::new(move || sig.get()))
    }
}

impl<T: Clone + PartialEq + 'static> From<ReadSignal<T>> for Reactive<T> {
    fn from(sig: ReadSignal<T>) -> Self {
        Reactive::Dynamic(Rc::new(move || sig.get()))
    }
}

impl<T: Clone + PartialEq + 'static> From<Memo<T>> for Reactive<T> {
    fn from(m: Memo<T>) -> Self {
        Reactive::Dynamic(Rc::new(move || m.get()))
    }
}

/// Un-`Some`d shorthand for optional-text props.
impl From<String> for Reactive<Option<String>> {
    fn from(s: String) -> Self {
        Reactive::Static(Some(s))
    }
}

impl From<&str> for Reactive<Option<String>> {
    fn from(s: &str) -> Self {
        Reactive::Static(Some(s.to_string()))
    }
}

/// A `Reactive` prop forwards into any builder prop slot.
impl<T: Clone + 'static> IntoValue<T> for Reactive<T> {
    fn into_value(self) -> Value<T> {
        match self {
            Reactive::Static(v) => Value::Const(v),
            Reactive::Dynamic(f) => Value::Dyn(Box::new(move || f())),
        }
    }
}

/// `text(content)` accepts a `Reactive<String>` prop directly — the
/// seam that keeps `Typography(content = …)`-style components reactive.
impl TextContent for Reactive<String> {
    fn into_content(self) -> Value<String> {
        self.into_value()
    }
}

// ============================================================================
// BuildElement — struct-literal component dispatch.
// ============================================================================

/// The component-dispatch contract `ui!` struct literals compile
/// against: `BuildElement::build(Foo { …, ..Foo::defaults() })`.
/// `#[component]` generates the impl (calling the component fn
/// field-by-field); `defaults()` supplies the struct-update base.
pub trait BuildElement: Default {
    fn build(self) -> Element;

    fn defaults() -> Self {
        Self::default()
    }
}

/// The struct-update base for a SIGNAL-typed prop with no declared
/// default. `runtime_world`'s handles are foreign, so the old core's
/// `impl Default for Signal` (detached sentinel) can't be reproduced;
/// instead the base mints a fresh default-valued signal in the ambient
/// scope. Divergence, documented: omitting a required signal prop reads
/// this fresh signal instead of panicking like the old sentinel did.
pub trait DefaultSignalProp {
    fn make() -> Self;
}

impl<T: PartialEq + Default + 'static> DefaultSignalProp for Signal<T> {
    fn make() -> Self {
        signal(T::default())
    }
}

impl<T: PartialEq + Default + 'static> DefaultSignalProp for ReadSignal<T> {
    fn make() -> Self {
        signal(T::default()).read_only()
    }
}

impl<T: PartialEq + Default + 'static> DefaultSignalProp for WriteSignal<T> {
    fn make() -> Self {
        signal(T::default()).write_only()
    }
}

/// `#[component]`'s inline-props glue emits this for signal-typed
/// fields' `Default` (see `inline_props.rs`).
pub fn __default_signal_prop<S: DefaultSignalProp>() -> S {
    S::make()
}

/// RAII guard returned by [`__component_build_probe`]. The old core's
/// probe powers a dev-build untracked-read diagnostic tied to the OLD
/// arena's tracking; the new kernel runs component bodies untracked by
/// construction (`component_scope`), so the probe is a no-op here.
pub struct BuildProbeGuard;

/// No-op stand-in for the old core's `__component_build_probe` (the
/// `#[component]` body brackets itself with it).
pub fn __component_build_probe(_name: &'static str) -> BuildProbeGuard {
    BuildProbeGuard
}

// ============================================================================
// Prelude — the author surface for a new-core app crate.
// ============================================================================

/// Everything an app written with `ui!` + `#[component]` needs in scope
/// on the new core. Mirrors the old core's prelude for the migrated
/// subset.
pub mod prelude {
    pub use super::{
        component_scope, effect, memo, on_cleanup, signal, untrack, BuildElement, ChildList,
        Element, IntoElement, Memo, Reactive, ReadSignal, Signal, WriteSignal,
    };
    pub use runtime_shared::StyleRules;
    pub use runtime_world::{IntoValue, Value};
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime_world::World;

    /// Const-vs-live is the load-bearing invariant of the retargeted
    /// lowering: literals must stay Const (zero effects), signal-reading
    /// shapes must stay reactive.
    #[test]
    fn fstring_parts_const_when_all_static() {
        let v = __idealyst_text_from_parts(vec![
            TextSlotPart::Lit("a"),
            TextSlotPart::Slot(TextSlot::Static("b".into())),
        ]);
        assert!(
            matches!(v, AssembledText::Value(Value::Const(ref s)) if s == "ab"),
            "all-static parts fold to one Const"
        );
    }

    /// Signal slots assemble to the pre-decomposed JS binding (the old
    /// core's `TextSource::JsBinding` tier): ids in slot order, N+1
    /// template parts with captured statics folded in, and a
    /// compute_fallback that renders the whole text.
    #[test]
    fn fstring_parts_jsbinding_when_slots_are_live() {
        let world = World::new();
        world.enter(|| {
            let n = signal(1i32);
            let v = __idealyst_text_from_parts(vec![
                TextSlotPart::Lit("n="),
                TextSlotPart::Slot(n.__idealyst_text_slot(|d| format!("{d}"))),
                TextSlotPart::Lit("!"),
            ]);
            match v {
                AssembledText::JsBinding(b) => {
                    assert_eq!(b.signal_ids, vec![n.raw_id()]);
                    assert_eq!(b.template_parts, vec!["n=".to_string(), "!".to_string()]);
                    assert_eq!(b.initial_values, vec!["1".to_string()]);
                    assert_eq!((b.compute_fallback)(), "n=1!");
                    assert_eq!((b.stringifiers[0])(), "1");
                    n.set(5);
                    // Committed via flush in real flows; direct closure
                    // reads observe staged state per kernel semantics —
                    // just assert the readers stay callable.
                    let _ = (b.tracked_reads[0])();
                }
                AssembledText::Value(_) => panic!("live slots must produce the JsBinding tier"),
            }
        });
    }

    /// An opaque computed slot (Reactive::Dynamic — no signal id) forces
    /// the whole text down the effect path (Value::Dyn), old tiering.
    #[test]
    fn fstring_parts_dyn_when_any_slot_is_computed() {
        let world = World::new();
        world.enter(|| {
            let r: Reactive<i32> = Reactive::derive(|| 4);
            let v = __idealyst_text_from_parts(vec![
                TextSlotPart::Lit("n="),
                TextSlotPart::Slot(r.__idealyst_text_slot(|d| format!("{d}"))),
            ]);
            match v {
                AssembledText::Value(Value::Dyn(f)) => assert_eq!(f(), "n=4"),
                _ => panic!("computed slot must force the Dyn effect tier"),
            }
        });
    }

    #[test]
    fn static_slot_formats_once_via_display() {
        let v = 7i32.__idealyst_text_slot(|d| format!("[{d}]"));
        assert!(matches!(v, TextSlot::Static(ref s) if s == "[7]"));
    }

    #[test]
    fn reactive_prop_coercions_match_old_semantics() {
        let world = World::new();
        world.enter(|| {
            let r: Reactive<String> = "hi".into();
            assert!(r.is_static());
            assert_eq!(r.get(), "hi");

            let sig = signal(3i32);
            let live: Reactive<i32> = sig.into();
            assert!(!live.is_static());
            assert_eq!(live.get(), 3);

            let opt: Reactive<Option<String>> = "x".into();
            assert_eq!(opt.get(), Some("x".to_string()));
        });
    }

    #[test]
    fn static_cond_dispatch_runs_taken_branch_flat() {
        let out = true.__idealyst_if(
            || vec![text("a").into_element(), text("b").into_element()],
            || vec![],
        );
        assert_eq!(out.len(), 2, "static branch splats flat, no wrapper");
    }

    #[test]
    fn static_for_each_builds_rows_flat() {
        let rows = vec![1, 2, 3].__idealyst_for_each(|n| vec![text(format!("{n}")).into_element()]);
        assert_eq!(rows.len(), 3);
    }

    #[test]
    fn one_or_view_empty_is_layout_neutral() {
        // The empty branch must be an Item (a real view), not a bare
        // fragment — it is a swappable placeholder that occupies no slot.
        match one_or_view(vec![]) {
            Element::Item { .. } => {}
            _ => panic!("empty branch must be an absolutely-positioned view Item"),
        }
    }
}

// ============================================================================
// P6: `pressable(children, on_click)` + the `.bind(Ref<…>)` family.
//
// `pressable` never had a `ui!` tag lowering (authors call the fn), so
// it joins the glue in the SDK-retarget wave. `.bind` mirrors the old
// `Bound::bind` — fill a `Ref` slot with the mount-time handle. `Ref`
// lives in runtime-core's thread-local ref arena (core-independent);
// the vocabulary handlers deliver REAL handles through each prim's
// `ref_fill` (`make_view_handle` & friends), so a bound Ref drives the
// same imperative surfaces (AnimatedValue, anchor targets, focus) as on
// the old core.
// ============================================================================

/// `pressable(children, on_click)` — the old positional constructor over
/// the P2b `PressableBuilder`.
pub fn pressable<F: Fn() + 'static>(children: Vec<Element>, on_click: F) -> GluePressable {
    GluePressable {
        b: builders::pressable(on_click).children(children),
        a11y: AccessibilityProps::default(),
    }
}

pub struct GluePressable {
    b: builders::PressableBuilder,
    a11y: AccessibilityProps,
}

impl GluePressable {
    pub fn disabled(mut self, disabled: impl IntoValue<bool>) -> Self {
        self.b = self.b.disabled(disabled);
        self
    }

    pub fn preserves_focus(mut self, preserve: bool) -> Self {
        self.b = self.b.preserves_focus(preserve);
        self
    }

    /// Mirror of `Bound::<PressableHandle>::bind`.
    pub fn bind(mut self, r: Ref<PressableHandle>) -> Self {
        self.b = self.b.on_handle(move |h| r.fill(h));
        self
    }
}

glue_wrapper_common!(GluePressable);

impl GlueView {
    /// Mirror of `Bound::<ViewHandle>::bind`.
    pub fn bind(mut self, r: Ref<ViewHandle>) -> Self {
        self.b = self.b.on_handle(move |h| r.fill(h));
        self
    }
}

impl GlueText {
    /// Mirror of `Bound::<TextHandle>::bind`.
    pub fn bind(mut self, r: Ref<TextHandle>) -> Self {
        self.b = self.b.on_handle(move |h| r.fill(h));
        self
    }
}

impl GlueButton {
    /// Mirror of `Bound::<ButtonHandle>::bind`.
    pub fn bind(mut self, r: Ref<runtime_shared::ButtonHandle>) -> Self {
        self.b = self.b.on_handle(move |h| r.fill(h));
        self
    }
}

impl GlueIcon {
    /// Mirror of `Bound::<IconHandle>::size` — pin to a `size × size`
    /// point square via a minted, deduped square-sizing sheet (an icon
    /// has no intrinsic content size; without this it collapses to 0×0
    /// under flex). Same rule set as the old `icon_size_sheet`
    /// (width/height + `flex_shrink: 0`), cached per rounded px so every
    /// icon at a size shares one sheet/class.
    pub fn size(self, size: f32) -> Self {
        use std::cell::RefCell;
        use std::collections::HashMap;
        thread_local! {
            static ICON_SIZE_SHEETS: RefCell<HashMap<u32, Rc<StyleSheet>>> =
                RefCell::new(HashMap::new());
        }
        let key = (size * 100.0).round() as u32;
        let sheet = ICON_SIZE_SHEETS.with(|m| {
            if let Some(s) = m.borrow().get(&key) {
                return s.clone();
            }
            let sheet = Rc::new(StyleSheet::r#static(StyleRules {
                width: Some(Tokenized::Literal(Length::Px(size))),
                height: Some(Tokenized::Literal(Length::Px(size))),
                flex_shrink: Some(Tokenized::Literal(0.0)),
                ..Default::default()
            }));
            m.borrow_mut().insert(key, sheet.clone());
            sheet
        });
        self.with_style(sheet)
    }

    /// Mirror of `Bound::<IconHandle>::data` — live vector data.
    pub fn data(mut self, f: impl Fn() -> IconData + 'static) -> Self {
        self.b = self.b.data_dyn(f);
        self
    }

    /// Mirror of `Bound::<IconHandle>::bind`.
    pub fn bind(mut self, r: Ref<runtime_shared::IconHandle>) -> Self {
        self.b = self.b.on_handle(move |h| r.fill(h));
        self
    }
}
