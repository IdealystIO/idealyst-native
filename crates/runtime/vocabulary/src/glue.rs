//! # `glue` — the `ui!` / `#[component]` / `stylesheet!` emission surface
//!
//! The macros emit absolute `::runtime_core::…` paths and rewrite their
//! heads to `::runtime_vocabulary::glue::…` (see
//! `runtime-macros-expand/src/new_core.rs`), so a component library needs
//! only this crate, not `runtime-core`. This module therefore provides
//! the *names and call shapes* the emission relies on — `view(children)`,
//! `text(content)`, `when(...)`, `ChildList::append_to`, `IntoElement`,
//! `BuildElement`, the `StaticCond`/`ReactiveCond` and
//! `StaticForEach`/`ReactiveForEach` dispatch traits, the f-string slot
//! machinery — over [`runtime_scene::Element`], the
//! [`builders`](crate::builders), and [`runtime_world`] reactivity. The
//! `runtime-core` root is `pub use runtime_vocabulary::glue::*`, so this
//! is also the author-facing surface.
//!
//! ## Const vs Dyn
//!
//! The static/reactive distinction is type-driven: a literal or plain
//! value lowers to [`Value::Const`] (bound once, zero effects); a
//! closure, `Signal`, `ReadSignal`, or `Memo` lowers to [`Value::Dyn`]
//! (one binding effect). Structured generator-backend binding metadata
//! (`Derived { method, inputs, initial }`, switch wire metadata, the
//! virtualizer `for`-sugar) is not emitted: the macro lowers the same
//! author shapes to the equivalent *closure* forms (same observable
//! reactivity). See the crate docs.
//!
//! ## Lowering table
//!
//! | `ui!` construct | emission |
//! |---|---|
//! | `view { … }` | `glue::view(children)` → `ViewBuilder` → `Element::Item(ViewPrim)` |
//! | `text { "lit" }` | `glue::text(Const)` |
//! | `text { move ‖ … }` | `glue::text(Dyn)` → binding effect |
//! | `text { "{sig} x" }` | `glue::TextSlotPart` → `Value<String>` (all-Const folds to Const) |
//! | `text { f(sig) }` | `text(move ‖ format!("{}", f(sig.get())))` |
//! | `button(label=…, on_click=…)` | `glue::button` → `ButtonBuilder` |
//! | `on_click = m(sig) => out` | `move ‖ out.set(m(sig.get()))` |
//! | reactive `if` / `when` | `glue::when` → `runtime_scene::dyn_keyed` (guarded hole) |
//! | bare-path `if cond` | `StaticCond`/`ReactiveCond` dispatch; reactive arm → `dyn_keyed` |
//! | `if is_even(sig)` | `when(move ‖ is_even(sig.get()), …)` |
//! | reactive `match` | `glue::switch` → `dyn_keyed` (PartialEq dedup) |
//! | literal-armed `match m(sig)` | closure `switch` with `.get()`-rewritten scrutinee |
//! | `for x in vec` | `StaticForEach` → `Vec<Element>` |
//! | `for x in sig, key=…` | `ReactiveForEach` → `runtime_scene::keyed` |
//! | `for i in 0..n.get()` | `each_keyed(EachKey, EachRowBuild)` → `runtime_scene::keyed` |
//! | `for i in 0..3` (static range) | `__static_repeat` → `Element::Many(RepeatPrim)` (batched, one FFI) |
//! | `Comp(prop = v)` | `glue::BuildElement` struct literal |
//! | `#[component]` body | wrapped in `component_scope(move ‖ …)` (run-once, untracked, collected `Owned`) |
//! | `Reactive<T>` props | `glue::Reactive` (world-backed; `IntoValue` bridges to builders) |
//! | empty `if` branch | [`empty_absolute_view`] |
//! | `overlay(...) { … }` | [`primitives::overlay`] → `builders::overlay` (PortalPrim composition) |
//! | `anchored_overlay(target=…)` | `builders::anchored_overlay` |
//! | `presence(present=…) { … }` | [`primitives::presence`] → `builders::presence` (Dyn retire hook) |
//! | `graphics(on_ready=…)` | [`primitives::graphics`] → `builders::graphics` |
//! | `flat_list(data=…, key=…, …)` | [`primitives::flat_list`] — type-erasing adapter over `builders::virtualizer` |
//! | `#[component(lazy)]` | [`primitives::lazy`] (`lazy_split` + the `lazy` prim/handler — see `crate::glue_lazy`) |
//! | `test_id = …` | each wrapper's `.test_id(…)` setter, backed by `crate::robot` (`robot` feature) |
//!
//! The virtualizer `for i in count(sig)` sugar is not supported: `ui!`
//! fails with a compile error naming the replacement.

use std::rc::Rc;

// Accessibility prop bag + icon/activity data types: part of the root
// author surface (`runtime_core::Role`, …).
pub use runtime_shared::accessibility::{
    AccessibilityAction, AccessibilityProps, AccessibilityTraits, LiveRegionPriority, Role,
};
pub use runtime_shared::primitives::activity_indicator::ActivityIndicatorSize;
// The `text_input` soft-keyboard hint (`keyboard = KeyboardKind::Email`):
// at the root so authors spell it `runtime_core::KeyboardKind`.
pub use runtime_shared::primitives::text_input::KeyboardKind;
pub use runtime_shared::primitives::icon::{FillRule, IconData, IconHandle, StrokeAnimation};
// `Easing` is re-exported (not just imported): the `stylesheet!`
// emitter spells `::runtime_core::Easing::…` for `transitions { … }`
// blocks, which the retarget maps here.
pub use runtime_shared::Easing;
// `Color` re-exported for the same reason (stylesheet bodies and app
// preludes reference it).
pub use runtime_shared::Color;
pub use runtime_shared::{FileDropHandler, IntoAction, SafeAreaSides, TouchHandler, WheelHandler};
pub use runtime_shared::ScrollAxis;
// The EVENT PAYLOADS those handler aliases carry. Exporting only the
// `*Handler` type aliases left SDKs unable to name their own handler's
// argument — `Rc<dyn Fn(&FileDropEvent) -> bool>` is unspellable without
// `FileDropEvent`.
pub use runtime_shared::{DroppedFile, FileDropEvent, FileDropPhase, WheelEvent, WheelKind};
// The gesture-recognizer contract (`crates/sdk/client/{pan,zoom,dnd}`
// implement `Recognizer` against it). Shared substrate — the touch
// pipeline is backend-fed, not core-fed.
pub use runtime_shared::{
    AsyncNotifier, GestureState, Recognizer, RecognizerCtx, RecognizerKind, RecognizerUpdate,
};
// Touch-claim arbitration: a scroller claims the active gesture so a
// child pressable stops treating it as a tap. Thread-local pair, no
// reactive-kernel involvement.
pub use runtime_shared::{active_touch_claim, set_active_touch_claim};
// `log!` / `LogLevel` and friends — the framework's platform-routed
// logging module.
pub use runtime_shared::logging;
use runtime_scene::{dyn_element, dyn_keyed, Key};
// `fragment(children)` — flat siblings, no node.
pub use runtime_scene::fragment;

/// A structural hole rebuilt on every dependency change (the scene's
/// un-keyed Dyn driver: the closure IS the dependency source).
pub fn dynamic(build: impl Fn() -> Element + 'static) -> Element {
    runtime_scene::dyn_element(build)
}
use runtime_world::{IntoValue, Value};

use crate::builders::{self, TextContent};

// Re-exports: the reactive surface + the scene Element under the names
// the macro (and app preludes) reach for.
pub use runtime_scene::{component_scope, Element};
pub use runtime_world::{
    effect, memo, on_cleanup, on_owned_drop, on_scope_drop, signal, untrack, Effect, Memo,
    ReadSignal, Signal, WriteSignal,
};

// The escape hatch for the `T: PartialEq` bound those signal handles
// carry: a payload with no value equality (a third-party session object,
// an `Arc<dyn Storage>`) wrapped so the guarded `set` can ask the only
// question that makes sense for it — "is this the same instance?".
// Lives in runtime-shared next to the rest of the permanent substrate;
// exported here so authors spell it `runtime_core::ByIdentity`.
pub use runtime_shared::{ByIdentity, ByIdentityArc};

// The `stylesheet!` emission surface. The macro's output is retargeted
// `::runtime_core::…` → `::runtime_vocabulary::glue::…` wholesale, so
// every name the generated sheet fn / builder / variant enums reference
// must resolve HERE. The style data model lives in runtime-shared — these
// are re-exports. The generated builder's conversion impl targets
// `IntoStyleProp`/`StyleProp`.
pub use crate::style_attach::{signal_class, IntoStyleProp, StyleProp};

/// Scope-liveness guard for callbacks handed to native code.
///
/// The framework already guards every callback it passes across the
/// backend seam (see [`crate::callback_guard`]). A third-party scene
/// `Registry` extension wires its OWN native callbacks — a GTK signal, a
/// UIKit target, a JS closure — which the framework never sees and
/// therefore cannot guard. Those have the same hazard: the toolkit can
/// invoke them after the payload's scope is torn down, and the resulting
/// stale-signal panic is raised inside a non-unwinding C trampoline,
/// which aborts the process rather than reporting a panic.
///
/// Acquire one token in the mount handler and wrap each callback with it.
pub use crate::callback_guard::ScopeAlive;

// Slots — a descendant publishing content into a region an ancestor
// renders, keyed by a marker type. Authoring composition over the kernel
// (signal + context + keyed list), the same tier as `reducer` / `watch` /
// `each_keyed`: no primitive, no payload, no backend method. Generic over
// the slot type, so an app that names none codegens none of it.
pub use crate::slots::{fill_slot, fill_slot_at, slot_is_filled, slot_outlet, Slot};
pub use crate::theme;
// Author-defined token sets: `#[derive(Tokens)]` implements these (the
// derive macro itself is re-exported by `runtime-core`, so
// `use runtime_core::Tokens` imports the derive AND the trait's methods).
pub use crate::tokens::{LeafSlot, TokenField, TokenLeaf, TokenNode, TokenPrimitive, TokenTheme, Tokens};
pub use runtime_shared::{
    cached_stylesheet, derived, Breakpoint, IntoOverrideSource, IntoVariantSource, Length,
    NoTokens, StateBits, StyleApplication, StyleRules, StyleSheet, ThemePalette, TokenEntry,
    TokenValue, TokenVocabulary, Tokenized, Transition, VariantEnum, VariantSet,
};

// Component styles: a component's styled slots declared once
// (`component_styles!`), restyled app-wide by type, extended on open
// variant axes, overridden per instance. The registry and composition live
// in runtime-shared beside the sheet model; `SlotStyle` (the style-prop
// side) lives here beside `IntoStyleProp`.
pub use runtime_shared::component_styles;
pub use runtime_shared::component_styles::{
    AxisDef, AxisKey, ComponentStyles, CompoundKey, CompoundLeg, Restyle, SlotDef, SlotOverride, SlotRestyle,
};
pub use crate::slot_style::{AxisValues, SlotStyle};

// The active-theme stash: a component library's typed theme object,
// installed / swapped / read back generically (see `active_theme`).
pub use crate::active_theme::{
    active_theme, active_theme_untracked, apply_host_surface_tokens, install_theme, install_themes, set_theme,
    stash_active_theme, theme_installed, ThemeTokens,
};

// Anchor plumbing for `anchored_overlay(target = …)`: `AnchorTarget`
// (a runtime-shared type the vocabulary's `PortalPrim` reuses) is
// constructed from a `Ref<ViewHandle>`-style handle slot, which the mount
// handlers fill through the prim's `ref_fill`. `Ref::default()` is the
// detached sentinel (allocates nothing, `get()` → `None`, the anchor falls
// back to unresolved positioning).
pub use runtime_shared::{Ref, ViewHandle};

/// Fill `r` with a mounted element's handle for as long as the element
/// is mounted: the fill is released when the enclosing subtree tears
/// down, so `r.get()` answers `None` once the element is gone and a
/// `#[method]` call through it is skipped instead of reaching freed
/// state. A later fill of the same ref (a remount that landed first) is
/// left in place. Call where effect creation is legal — a mount handler
/// or a component body; every `bind` setter and the `#[method]`
/// component's `bind_to` go through here, and an extension's mount
/// handler that hands an author a handle should too.
pub fn fill_ref_while_mounted<H: 'static>(r: Ref<H>, handle: H) {
    let lease = r.fill_owned(handle);
    crate::style_attach::on_teardown(move || lease.release());
}

// The host-scheduling registry: drivers and test harnesses install/pump
// through the same `runtime_shared::scheduling` registry the vocabulary
// handlers schedule against (presence anims, portal anchor tracking).
pub use runtime_shared::scheduling;

// Component instrumentation: `#[component]` emits
// `::runtime_core::debug::record_component_enter/exit(..)` under
// `debug-stats`, retargeted here — so `debug` must be reachable HERE for
// those expansions to resolve. `runtime_shared::debug` carries the no-op
// shim when `debug-stats` is off, so the re-export is unconditional.
pub use runtime_shared::debug;

// MCP catalog anchor: the `#[component]` / `doc_scope!` registrations
// emit `::runtime_core::__mcp::inventory::submit!`, which the retarget
// pass rewrites to `::runtime_vocabulary::glue::__mcp::…` — so the
// catalog crate must be reachable HERE. One `mcp-catalog` instance: every
// registration lands in one process-wide inventory regardless of which
// spelling emitted it. Gated by `catalog` so production builds carry
// zero catalog code.
#[cfg(feature = "catalog")]
#[doc(hidden)]
pub use ::mcp_catalog as __mcp;

// Hot-reload dispatch anchor, the `__mcp` of the subsecond split. The
// `#[component]` hot-reload emission spells
// `::runtime_core::__hot::call(inner_fn_ptr, args)`, which the retarget
// pass rewrites to `::runtime_vocabulary::glue::__hot::call(..)` — so
// `dev_hot` has to be reachable HERE, not only from the author-facing
// root (a component in a component LIBRARY expands to the same path and
// that crate need not depend on runtime-core).
//
// `dev-hot` is rlib-only on purpose: subsecond's jump table is a
// process-global, so the whole graph must share one statically linked
// copy. Gated by `hot-reload`, which also turns on `dev-hot/hot` — see
// the feature's comment in Cargo.toml for why both live behind one
// switch.
#[cfg(feature = "hot-reload")]
#[doc(hidden)]
pub use ::dev_hot as __hot;

// Premint style-dump registry anchor. `stylesheet!` emits its
// `cfg(idealyst_premint_dump)` linkme registration as
// `::runtime_core::premint::{linkme, PREMINT_SHEETS, PremintSheet}`, and
// the retarget pass rewrites that head to
// `::runtime_vocabulary::glue::premint::…` — so the registry has to be
// reachable HERE for a dump build to compile. Same `style-dump` gate as the substrate module: only the CLI's
// ephemeral dump build and `premint-dump`'s own tests turn it on.
#[cfg(feature = "style-dump")]
pub use runtime_shared::premint;

// ============================================================================
// The rest of the author surface — what component libraries (idea-theme,
// idea-ui, …) reach for directly through `runtime_core`, outside the macro
// emission. Three kinds of entry:
//
//   1. runtime-shared data types/modules (style enums, animation, handles,
//      assets, num, breakpoints) — plain re-exports.
//   2. runtime-shared free fns with no reactive state (`resolve_style`,
//      scheduling entries, `premint_class_name`).
//   3. Glue implementations where the runtime-shared fn of the same name
//      is built on the legacy `runtime_shared::reactive` arena, which no
//      world effect can observe: the theme-install family (routed through
//      the per-world `ThemeCtx` + a seed of the shared `Tokenized::resolve`
//      registry), `current_breakpoint`, the scoped scheduling fns, and
//      `watch`/`Subscription` (world-effect-backed). These carry logic →
//      covered by the vocabulary test suite.
// ============================================================================

// --- 1. Shared data types & modules -----------------------------------------

// Style vocabulary not already re-exported above (the style data model
// lives in runtime-shared). Every type a `StyleRules` field (or
// its payload) holds must appear here or above — an app names style
// types through `runtime_core` only. `runtime-core/tests/style_reexports.rs`
// destructures `StyleRules` exhaustively through that path, so a new
// field whose type is missing here fails to compile there.
pub use runtime_shared::{
    AlignContent, AlignItems, AlignSelf, BorderStyle, Cursor, Derive, DisplayKind, FlexDirection, FlexWrap,
    FontFamily, FontStyle, FontWeight, Gradient, GradientKind, GradientStop, GridPlacement,
    JustifyContent, ObjectFit, Overflow, OverscrollBehavior, PointerEvents, Position,
    RadialExtent, ScrollbarVisibility, Shadow, TextAlign, TextTransform, TrackSize, Transform,
    UserSelect, VariantAxis, VariantValue,
};

// Typefaces + assets (fonts, images): shared asset model.
pub use runtime_shared::assets;
pub use runtime_shared::assets::{SystemFallback, Typeface, TypefaceId};

// The animation driver (AnimatedValue + tweens): pure handle + shared
// `scheduling`-registry machinery, so it runs as-is on vocabulary mounts
// (handlers fill the same `ViewHandle`s through `make_view_handle`)…
// EXCEPT `AnimatedValue`'s `bind*` family. The module below re-exports
// everything shared and shadows `AnimatedValue` with a wrapper whose
// `bind*` anchors to the world's component scope.
//
// WHY (the "Switch thumb never travels" bug): runtime-shared's
// `animation/binding.rs` keeps the per-frame subscription alive by
// handing it to the legacy `runtime_shared::reactive::on_cleanup`, which
// outside a legacy scope drops its callback immediately — and no legacy
// scope is ever active under a world. So every `av.bind(ref, AnimProp::…)`
// tore its listener down at bind time: the tween ticked, nobody wrote
// `set_animated_f32`, and idea-ui's Switch thumb (Collapsible height,
// Select chevron, Modal fade, …) never moved. The wrapper keeps the same
// subscribe machinery, with the keepalive owned by a world effect
// collected into the surrounding `component_scope` `Owned` (the
// `__component_keepalive_effect` pattern) — dropped exactly at unrealize,
// so the subscription dies with the component, which also protects a
// recycled `Ref`.
pub mod animation {
    use std::ops::Deref;

    pub use runtime_shared::animation::*;

    use runtime_shared::{Ref, TextHandle, ViewHandle};

    /// `AnimatedValue` with world-scoped `bind*`: same
    /// construction/animate/get surface (via `Deref`). See the comment
    /// above `mod animation` for why this exists.
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

        /// Wrap a runtime-shared `AnimatedValue` handle in the
        /// world-scoped wrapper. Glue-internal seam for surfaces that must
        /// return the SHARED handle rather than a fresh one —
        /// `glue::session::animated` hands out the session-registry
        /// instance so hot-patch rerenders keep the AV's current value.
        #[doc(hidden)]
        pub fn __from_inner(inner: runtime_shared::animation::AnimatedValue<T>) -> Self {
            AnimatedValue { inner }
        }
    }

    /// Anchor `guard` (subscription + strong AV clone) to the current
    /// component scope: a dependency-free world effect owns it, the
    /// surrounding `component_scope`/realize collector owns the effect,
    /// so the guard drops exactly when the component's subtree
    /// unrealizes. Outside any world, bind is a no-op: the guard drops
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

    /// Re-apply the CURRENT value once the mount has filled the ref (the
    /// immediate apply at bind time no-ops on an unfilled ref, and a
    /// static value never ticks again). Detached 0-delay task: `Ref`
    /// slots are never freed/recycled under a world (the documented
    /// `Ref::new` leak rule), so a post-teardown fire writes through a
    /// stale handle at worst, a visual no-op on a detached node.
    fn reapply_after_mount(apply: impl FnOnce() + 'static) {
        runtime_shared::scheduling::after_ms_detached(0, apply);
    }

    impl AnimatedValue<f32> {
        /// Scalar `bind` — see `runtime_shared::animation::binding` for
        /// the author-facing docs; delivery is identical
        /// (`ViewHandle::set_animated_f32` per fire), only the keepalive
        /// differs (module comment).
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
        /// `bind_color` with the world-scoped keepalive.
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

        /// `bind_gradient_stop` with the world-scoped keepalive.
        pub fn bind_gradient_stop(&self, target: Ref<ViewHandle>, stop_idx: u8) {
            self.bind_color(target, AnimProp::GradientStopColor(stop_idx));
        }

        /// `bind_text_color` with the world-scoped keepalive (routes
        /// through `TextOps::set_animated_color` — see
        /// `runtime_shared::animation::binding`).
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
/// epoch, hot-patch survival) — the registry/epoch machinery is
/// `runtime_shared::session` (pure thread-local state), so most names
/// re-export. Two shadows:
///
/// - [`session::animated`] returns the glue [`animation::AnimatedValue`]
///   wrapper (world-scoped `bind*`) around the SAME session-registry
///   instance, so hot-patch value survival is preserved.
/// - [`session::after_ms`] anchors through [`crate::scoped_scheduling`]
///   (the shared one anchors to the legacy arena and never fires — the
///   "welcome acts never fire" bug).
///
/// `session::signal` is deliberately NOT re-exported: the shared one
/// returns a legacy-arena `Signal` no world effect can subscribe to, so
/// a session-persistent reactive scalar needs a world-signal
/// implementation first. `glue::session::signal` failing to resolve is
/// the loud marker for that gap.
pub mod session {
    pub use runtime_shared::session::{
        clear, clear_prefix, epoch_micros, get_or_init, push_scope, reset_epoch, ScopeGuard,
    };

    pub use crate::scoped_scheduling::session_after_ms as after_ms;

    /// `runtime_shared::session::animated` — same key-registry
    /// persistence — wrapped so `bind*` anchors to the world (see
    /// [`super::animation`]).
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
// `current_breakpoint` is the glue implementation (§3): the shared fn
// returns a legacy-arena signal a world effect cannot subscribe to,
// which froze every breakpoint-dependent `when` at its seed (the
// idea-ui-docs "hamburger visible at desktop width" bug). The module
// re-exports the shared items and shadows the signal fn.
pub mod breakpoint {
    pub use runtime_shared::breakpoint::*;
    // Explicit re-export shadows the glob's legacy `current_breakpoint`.
    pub use super::current_breakpoint;
}
pub use runtime_shared::{breakpoints, install_breakpoints, Breakpoints};

/// Reactive current-breakpoint bucket: the ambient world's
/// [`ViewportCtx`](crate::viewport) memo (per-world, re-fires only on
/// bucket flips), read via `.get()`. Handler-safe like the theme
/// surface — outside
/// `World::enter` it resolves the last ambient world's ctx (drawer-link
/// handlers call `sidebar_pinned`, which lands here).
pub fn current_breakpoint() -> ReadSignal<runtime_shared::Breakpoint> {
    crate::viewport::viewport_ctx().breakpoint()
}

// Root-level scheduling names (also reachable under `scheduling::`). The
// SCOPED variants are the world-anchored ones in
// [`crate::scoped_scheduling`] — see its module docs for why they can't
// anchor to the legacy arena.
pub use runtime_shared::scheduling::{
    after_animation_frame, after_ms, after_ms_detached, raf_loop, schedule_microtask, RafLoop,
    ScheduledTask,
};
pub use crate::scoped_scheduling::{after_ms_scoped, raf_loop_scoped};

// Interaction handles (filled by the mount handlers' `ref_fill` via
// `make_*_handle`).
pub use runtime_shared::{PressableHandle, TextHandle};
// Every handle type a `bind = r` / `on_handle` setter takes is nameable at
// the root, so `let r: Ref<ButtonHandle> = Ref::new();` needs no deep path.
pub use runtime_shared::ButtonHandle;
pub use runtime_shared::primitives::graphics::GraphicsHandle;
pub use runtime_shared::primitives::virtualizer::VirtualizerHandle;

// Text/style source data types component code stores and matches on.
// `StyleSource` is pure data (application + closure variants); the
// vocabulary converts it to `StyleProp` at attach time.
pub use runtime_shared::{IntoTextSource, StyleSource, TextSource};

// Styled-run data types at the root.
pub use runtime_shared::{TextRun, TextRunStyle};

// `flat_list` / `fixed_size` / `FlatListItemSize` at the ROOT too. They
// also live at `primitives::flat_list::…`, but the documented spelling — taught in `websites/docs`'s lists page — is the root one, so
// the root is where they have to resolve.
pub use primitives::flat_list::{fixed_size, flat_list, FlatListItemSize};

/// The styled-text data helpers author code imports by path
/// (`styled_text::plain_text_of`). The same-named CONSTRUCTOR fn below
/// lives in the value namespace, so the module and fn coexist.
pub mod styled_text {
    pub use runtime_shared::styled_text::{plain_text_of, TextRun, TextRunStyle};
}

/// What [`styled_text`] returns: `.with_style(…)` then [`IntoElement`].
pub struct StyledText(crate::builders::TextBuilder);

/// One text node with inline-styled ranges — the vocabulary
/// [`text()`](crate::builders::text) builder's `runs` channel, mounted
/// via the caps `create_styled_text`.
pub fn styled_text(runs: Vec<TextRun>) -> StyledText {
    StyledText(crate::builders::text().runs(runs))
}

impl StyledText {
    /// Attach the author style.
    pub fn with_style(self, style: impl crate::style_attach::IntoStyleProp) -> Self {
        StyledText(self.0.style(style))
    }
}

impl IntoElement for StyledText {
    fn into_element(self) -> Element {
        self.0.build()
    }
}

// --- 2. Free fns with no reactive state -------------------------------------

// The style resolution engine: the SAME entry the vocabulary's
// `style_attach` resolves every sheet application through. Token values inside resolve against the shared
// `Tokenized::resolve` registry, which the theme-install family below
// seeds per install/update.
pub use runtime_shared::resolve_style;

// The preminted-class naming scheme. Author code never calls this;
// it is spelled here so `idea-theme`'s assembled sheets and the
// `premint-dump` tool reach the same function through the facade —
// the class name is the only thing joining the dump binary to the
// shipped bundle, so both must derive it from one implementation.
pub use runtime_shared::premint_class_name;
pub use runtime_shared::PREMINT_FONT_INHERIT_CLASS;

// --- 3. Glue implementations -------------------------------------------------

/// Install the app's theme tokens. The ambient world's
/// [`ThemeCtx`](crate::theme) records + queues them for the backend
/// (`StyleOps::install_tokens`, delivered by the theme driver / next
/// sheet attach), and the values are ALSO seeded into runtime-shared's
/// token registry — see [`seed_shared_token_registry`] for why
/// that second write exists and why it is safe.
pub fn install_tokens(tokens: &[runtime_shared::TokenEntry]) {
    theme::install_tokens(tokens);
    seed_shared_token_registry(tokens, false);
}

/// Swap/patch theme tokens. Same routing as [`install_tokens`] — per-world
/// ThemeCtx first (version bump → driver flush → cohort re-apply), then
/// the shared-registry seed so re-resolution reads the new values.
pub fn update_tokens(tokens: &[runtime_shared::TokenEntry]) {
    theme::update_tokens(tokens);
    seed_shared_token_registry(tokens, true);
}

/// Read one token's live value — the read half of [`update_tokens`],
/// for tooling that edits a theme (a control seeded with what the app
/// is actually painting, not with the base palette's default). Reads
/// the per-world table, which is authoritative; the shared registry
/// `seed_shared_token_registry` keeps in step is a mirror.
pub fn token_value(name: &str) -> Option<runtime_shared::TokenValue> {
    theme::token_value(name)
}

/// Every token name the ambient world has a value for — including
/// extension tokens no vocabulary can enumerate. See
/// [`theme::token_names`].
pub fn token_names() -> Vec<&'static str> {
    theme::token_names()
}

/// Declare every switchable palette so a STATIC render can serialize
/// them all — see [`crate::theme::install_theme_palettes`]. Unlike
/// [`install_tokens`] this seeds no shared registry: the inactive
/// palettes must not participate in token RESOLUTION, only in the CSS a
/// static document emits for the reader's system preference or stored
/// `[data-theme]` choice.
pub fn install_theme_palettes(palettes: &[runtime_shared::ThemePalette]) {
    theme::install_theme_palettes(palettes);
}

/// See [`crate::theme::set_app_background`].
pub fn set_app_background(color: runtime_shared::Tokenized<runtime_shared::Color>) {
    theme::set_app_background(color);
}

/// See [`crate::theme::set_default_text_font`].
pub fn set_default_text_font(font: Option<FontFamily>) {
    theme::set_default_text_font(font);
}

/// See [`crate::theme::default_text_font`] — the read side of the
/// channel above. Both the live apply path (`fill_default_text_font`)
/// and the preminted one (the dump's `--iy-default-font` custom
/// property) consume it, so a sheet that wants its family to survive a
/// theme swap leaves `font_family` unset and lets this fill it.
pub fn default_text_font() -> Option<FontFamily> {
    theme::default_text_font()
}

/// See [`crate::theme::set_scrollbar_theme`].
pub fn set_scrollbar_theme(
    thumb: runtime_shared::Tokenized<runtime_shared::Color>,
    track: runtime_shared::Tokenized<runtime_shared::Color>,
) {
    theme::set_scrollbar_theme(thumb, track);
}

/// Seed runtime-shared's token-signal registry with the installed values.
///
/// WHY: `Tokenized::{resolve, value}` — called by author/component code
/// at build time (`resolve_style(&app).color.resolve()` for icon tints,
/// tab indicators, …) and by non-cascading backends at `apply_style`
/// time — reads runtime-shared's thread-local token registry, not the
/// per-world ThemeCtx. Without this seed those reads return the
/// compile-time FALLBACK palette (the light defaults), so a dark-theme
/// install would leave every Rust-side resolved color light. The shared
/// writers (`install_tokens` / `update_tokens`) are safe to call: token
/// signals live outside any legacy scope by design, no legacy-arena
/// effects subscribe to them (so `update`'s subscriber fan-out is a
/// no-op), and the pending-delivery queue those writers fill is drained
/// immediately below — nothing consumes it, so repeated theme swaps would
/// otherwise accumulate entries.
fn seed_shared_token_registry(tokens: &[runtime_shared::TokenEntry], update: bool) {
    if update {
        runtime_shared::update_tokens(tokens);
    } else {
        runtime_shared::install_tokens(tokens);
    }
    // Drain the pending-delivery queue; nothing consumes it (see above).
    let _ = runtime_shared::take_pending_token_updates();
}

/// Caller-owned reactive subscription. Runs `f` now and re-runs it when
/// a signal it read changes, until the handle drops. Backed by a world
/// effect collected into a private [`Owned`](runtime_world::Owned) scope
/// (never adopted by the caller's scope), so it must be
/// created where effect creation is legal (inside the ambient world:
/// component builds, effects, flushes).
#[must_use = "a Subscription disposes its effect when dropped — store it (or call \
              `.leak()`) to keep the effect running"]
pub struct Subscription {
    owned: Option<runtime_world::Owned>,
}

impl Subscription {
    /// Keep the subscription alive for the world's lifetime, giving up
    /// the handle. Forgetting
    /// the `Owned` skips its retiring Drop, pinning the effect slot.
    pub fn leak(mut self) {
        if let Some(owned) = self.owned.take() {
            std::mem::forget(owned);
        }
    }
}

/// See [`Subscription`].
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

/// Disable scope OWNERSHIP for `f` (world-lifetime slots for global
/// caches): slots created inside are owned by the world root instead of
/// the enclosing collector. See [`runtime_world::unscoped`].
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
// per-app singletons. Worlds are transient (one per SSR request), so an
// SDK's
// "global" (e.g. idea-theme's active-theme signal) must live with the
// world, exactly like the vocabulary's own ThemeCtx.
//
// A provision is owned by the scope that made it, like a signal: it is
// retracted when that scope drops, so a context value can never outlive
// the handles it carries. `unscope(|| provide(v))` opts a genuinely
// world-lifetime service out of that; a bounded region wraps the
// `provide` in its own `collect_owned` — see `provide`'s docs.
pub use runtime_world::{inject, provide};

// The handler-safety probe (idea-theme's active-theme slot): platform
// event handlers run OUTSIDE `World::enter`, where `inject`/`provide`
// panic — code that must serve both build-time and handler-time callers
// forks on this and uses a build-time-captured handle in the `false`
// branch (capture, don't inject). `world_is_entered` below is the public
// spelling.
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
/// should call this.
pub fn world_is_entered() -> bool {
    runtime_world::is_entered()
}

// TEST-SUPPORT re-export of the kernel `World` for SDK test suites that
// need enter/EXIT control — `__with_fresh_world`
// keeps the world ambient for the whole body, which cannot express "the
// handler runs outside enter" (idea-theme's set-theme-from-handler
// regression). Doc-hidden: not an author surface.
#[doc(hidden)]
pub use runtime_world::World as __World;

/// TEST-HARNESS seam: run `f` inside a fresh world (created, entered,
/// dropped afterwards) — for SDK unit tests that need reactive state
/// without mounting anything. Returns `f`'s value; staged writes that
/// `f` makes are committed with a final flush before the world drops so
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

runtime_tls::thread_local! {
    /// The stack of live [`__with_fresh_world`] worlds (nested test
    /// helpers), so [`__flush_test_world`] can commit mid-test.
    /// Test-support only — never touched by production mounts, so its
    /// TLS key is never created outside test runs.
    static TEST_WORLDS: std::cell::RefCell<Vec<Rc<runtime_world::World>>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// TEST-HARNESS seam: commit staged writes on the innermost
/// `__with_fresh_world` world, for set-then-assert test bodies. Returns
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
// --- Root-surface data types + handle types ---------------------------------
//
// Primitives' data/handle types at the ROOT as well as under
// `primitives::…`; SDK crates import both spellings. All runtime-shared
// types.

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
// through `InputOps` on every backend) — no reactive-kernel state.
pub use runtime_shared::{
    long_press, pan, pinch, rotate, swipe, tap, tap_with_press, LongPress, LongPressRecognizer,
    Pan, PanEvent,
    PanRecognizer, Pinch, PinchEvent, PinchRecognizer, Rotate, RotateEvent, RotateRecognizer,
    Swipe, SwipeDirection, SwipeDirs, SwipeRecognizer, Tap, TapRecognizer,
};
pub use runtime_shared::{
    pointer_button, pointer_modifiers, PointerButton, PointerModifiers, TouchEvent, TouchId,
    TouchPhase, TouchPoint, TouchResponse,
};

/// The platform's safe-area insets (status bar, home indicator, notch,
/// rounded-corner sides), reactive: rotation or a status bar appearing
/// re-fires readers. To keep content clear of them, give a container
/// `safe_area` sides instead; read this to react in app code. Zero on
/// platforms without a safe area. See `crate::safe_area`.
pub use crate::safe_area::safe_area_insets;

// Ambient host identity + frame gating + clocks + the async driver:
// pure thread-local / registry reads. `platform()` reads the value the
// mounting host installed (native boots install it from the backend's
// `platform()`; web installs it in `start_in`); `is_frame_active()` is the
// embedded-host paint gate (the wgpu hosts flip it while their surface
// is hidden so raf-driven author loops can freeze their clocks);
// `time` / `driver` are the monotonic clock and the spawn/render-loop
// registries.
pub use runtime_shared::{is_frame_active, platform, ColorScheme, Platform};
pub use runtime_shared::time;
// The rest of the ambient-host free-function surface from
// `runtime_shared::host`. All four are thread-local reads/writes against
// installers a host wires at boot.
//
// `announce` is the a11y live-region author surface, `open_url` /
// `set_fullscreen` route through the host-installed setters (no-op when
// the host installed none), and `color_scheme()` reads the installed
// scheme. Pinned by `glue_host_surface.rs`.
pub use runtime_shared::{announce, color_scheme, open_url, set_fullscreen};
/// `runtime_core::host::…` — the module path alongside the root-level
/// re-exports above. Author code
/// and SDKs reach `host::color_scheme()` through this; the installers
/// (`install_announcer`, `install_url_opener`, …) are the boot-side half
/// that backends call.
pub use runtime_shared::host;
// Color parsing/blending helpers (`color::parse_or`, `color::Rgba`) —
// shared substrate used by the canvas SDK's scene lowering.
pub use runtime_shared::color;
// App-level keyboard input (key down + up, regardless of focus): the
// scoped `on_key` / `key_state` author surface, the guard-returning
// `add_key_listener`, and the event types. `set_app_key_handler` is the
// older single-slot key-down hook, now a wrapper over a listener.
pub use crate::key_input::{add_key_listener, key_state, on_key, KeyState};
pub use runtime_shared::key_input::KeyListener;
pub use runtime_shared::primitives::key::{AppKeyEvent, KeyPhase};
pub use runtime_shared::set_app_key_handler;
// App situation signals (focus, visibility, reduce-motion, primary
// pointer, user activation, pointer lock) and app-wide mouse input with
// pointer lock. See `crate::app_input`.
pub use crate::app_input::{
    add_pointer_listener, app_focused, app_visible, exit_pointer_lock, on_pointer, pointer_locked,
    pointer_state, primary_pointer, reduce_motion, request_pointer_lock, set_cursor_hidden,
    user_activated, PointerState,
};
pub use runtime_shared::app_state::PointerKind;
// App-wide "the person is using the app" heartbeat (idle clocks, kiosk
// timeouts, auto-lock). See `crate::user_activity`.
pub use crate::user_activity::{add_user_activity_listener, user_activity};
pub use runtime_shared::user_activity::UserActivityListener;
pub use runtime_shared::pointer_input::{
    AppPointerEvent, MouseButton, PointerListener, PointerOutcome, PointerPhase,
};
// `driver` (spawn_async + render_loop) is gated by `async-driver`; apps
// that reach `runtime_shared::driver` through `runtime_core` enable
// `runtime-core/async-driver` (which forwards here).
#[cfg(feature = "async-driver")]
pub use runtime_shared::driver;
// `spawn_then` — the scope-safe imperative spawn: IO in the future, every
// signal read/write in a sync callback that runs inside a turn or not at
// all. Prefer it over reaching into `driver::spawn_async` directly; see
// `crate::scoped_spawn` for the guarantee.
#[cfg(feature = "async-driver")]
pub use crate::scoped_spawn::{spawn_then, spawn_then_in};
// `resource` / `mutation` — world-backed implementations (the
// runtime-shared fns of the same names are built on the legacy arena);
// see `crate::async_reactive`'s module docs for anchoring and completion
// staging. Gated by `async-driver`. The shared `NetworkState` enum
// re-exports; a `From<&…State>` impl is impossible here (orphan rule) —
// use the handles' `network_state()`.
#[cfg(feature = "async-driver")]
pub use crate::async_reactive::{
    async_reducer, mutation, resource, AsyncReducer, AsyncStatus, Mutation, MutationState,
    Resource, ResourceCancel, ResourceState,
};
#[cfg(feature = "async-driver")]
pub use runtime_shared::NetworkState;

/// The reactive viewport-size signal: the ambient world's
/// [`ViewportCtx`](crate::viewport) signal (per-world; each backend's
/// resize source pushes into it — see the viewport module docs for the
/// per-platform wiring), read via `.get()`. Handler-safe outside
/// `World::enter` via the last-ambient fallback.
pub fn viewport_size() -> Signal<runtime_shared::ViewportSize> {
    crate::viewport::viewport_ctx().size_signal()
}

/// The reactive soft-keyboard inset: how much of the app's bottom edge
/// the on-screen keyboard covers once its current animation settles, and
/// that animation's timing. To keep content clear of the keyboard use
/// [`keyboard_avoiding_view`]; read this to react in app code — hide a tab
/// bar while it is up, or animate in step via `KeyboardInset::transition`.
/// Always `HIDDEN` on platforms without a soft keyboard. See
/// `crate::keyboard`.
pub use crate::keyboard::keyboard_inset;
pub use runtime_shared::{KeyboardAvoid, KeyboardAvoidBehavior, KeyboardInset};

// `IntoStyleSource` — alias of `IntoStyleProp`, "the trait
// `.with_style(…)` accepts": helper fns bounded `impl IntoStyleSource`
// accept exactly what the glue wrappers' `.with_style` takes (resolved
// rules, sheets, sheet-builder outputs, closures). The `StyleSource` DATA
// enum is the runtime-shared type (re-exported above).
pub use crate::style_attach::IntoStyleProp as IntoStyleSource;

// --- Root-surface primitive constructors -------------------------------------
//
// Root spellings of the glue's `primitives::…` wrappers.

pub use primitives::image::{image, image_asset};
pub use primitives::link::{external_link, link};
// The navigation data surface at the root (`use runtime_core::{Route,
// Screen}` in app crates): `Route` is the runtime-shared type; `Screen`
// is the carrier (scene `Element` + opaque options).
pub use primitives::navigator::{Route, RouteParams, Screen};
// Screen state ⇄ query params: `screen_state::<S>()` is what a route
// builder calls to seed its signals, and it must resolve at the same
// root path authors already import `Route` from.
pub use primitives::navigator::{screen_query, screen_state, QueryParams, ScreenState};
pub use primitives::overlay::{anchored_overlay, overlay};
pub use primitives::presence::presence;
pub use primitives::scroll_view::scroll_view;
pub use primitives::text_area::text_area;
pub use primitives::text_input::text_input;
pub use primitives::toggle::toggle;

/// Dispatch an [`ImageSource`] to the url / asset constructor. A `Url`
/// source becomes a closure over its `Reactive<String>`'s `get`, so it
/// binds as a reactive prop.
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

/// Fire `f(new, Some(prev))` on every dependency change AFTER the first
/// run (the first run only records the baseline). A world effect.
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

/// Fire `f(new, prev)` immediately AND on every dependency change. The
/// subscription set is `deps`; the body runs untracked, so reads inside
/// it never widen it. The only difference from [`on_defer`] is that the
/// first run invokes the body instead of only recording the baseline.
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

/// `memo_with(eq, f)`: a cached derivation whose
/// notification is gated by a CALLER-SUPPLIED equality instead of
/// `PartialEq` — tolerance comparisons, "equal enough to skip a
/// repaint", and the like.
///
/// Implemented over the world kernel rather than re-exported: the shared
/// `memo_with` builds a legacy-arena `Signal` + `Effect`, which nothing
/// on a world mount subscribes to.
///
/// **`T: PartialEq` is required**: the world kernel's signal storage is
/// `PartialEq`-bounded end to end. `set_always` makes the caller's `eq`
/// the authoritative gate. For a `T`
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
    // first commit sees a coherent value.
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

/// `reducer(initial, f)`: a `(state, dispatch)` pair where every
/// dispatch folds the current state with an action.
///
/// Implemented over the world kernel for the same reason as
/// [`memo_with`]:
///
/// - `S: PartialEq` is required (world signal storage is
///   `PartialEq`-bounded).
/// - No `cycle(..)` wrap is needed: a dispatch's writes are STAGED and
///   the flush commits them together, so sibling writes coalesce into one
///   fan-out by construction.
///
/// Every dispatch notifies (`set_always`), so a reducer that folds to an
/// equal state still wakes its subscribers.
pub fn reducer<S, A>(initial: S, f: impl Fn(&S, A) -> S + 'static) -> (Signal<S>, impl Fn(A))
where
    S: Clone + PartialEq + 'static,
{
    let state = signal(initial);
    let dispatch = move |action: A| {
        // Fold against the STAGED value so two dispatches in one event
        // turn compose (0→1→2) instead of both reading the committed 0.
        state.update(|current| f(current, action));
        // Preserve "a dispatch always notifies": `update` stages
        // guarded, so a fold back to an equal value would be silent
        // otherwise.
        state.touch();
    };
    (state, dispatch)
}

// Per-position ids for markup (`use_id`, `use_id_keyed`): the scene's
// build frames — unique in a tree, the same on every build of the same
// tree (see `runtime_scene`'s `id_frame`). The structural `Identity`
// hashes are runtime-shared's.
pub use runtime_scene::{use_id, use_id_keyed};
pub use runtime_shared::{current_identity, hash_key, with_current_identity, Identity};

/// The dependency-source trait `on` / `on_defer` are generic over: a
/// tracked read of the current value.
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
// Component-registry emission surface. The `#[component]` macro emits, for
// every `Element`-returning body, the `__inspect` registration bracket
// (below), and for a methods-bearing component `robot::Method` literals +
// `robot::__attach_component_methods(...)` — all retargeted here.
// The registration surface always exists (`crate::robot_methods` is
// stub-shaped when the vocabulary `robot` feature is off), so the
// emission is unconditional. `bind_to` props ride the re-exported
// runtime-shared `Ref` (above): `Ref::new/fill/get` operate on a
// thread-local ref arena — a `Ref` created outside a legacy scope never
// frees its slot, the documented `Ref::new` rule.
// ============================================================================

/// Re-export of `serde_json` for the `#[method]` auto-registration
/// codegen (mirror of `runtime_shared::__serde_json`).
#[doc(hidden)]
pub use runtime_shared::__serde_json;

/// Re-export of the `wasm-split` runtime crate (mirror of
/// `runtime_shared::__wasm_split`) so the `#[component(lazy)]`
/// emission's `#[…__wasm_split::wasm_split(…)]` attribute + its
/// `use …__wasm_split as wasm_split;` alias resolve after the
/// retarget, without author crates depending on `wasm-split` directly.
#[doc(hidden)]
pub use runtime_shared::__wasm_split;

/// The names the macro spells as `::runtime_core::robot::…`, resolved
/// against the vocabulary method registry.
pub mod robot {
    pub use crate::robot_methods::{
        __attach_component_methods, register_component, ComponentInstanceId,
        ComponentRegistration, Method,
    };
}

/// The component-registry emission surface every `#[component]` body
/// expands to (see `robot_methods`' module docs): the registration
/// bracket, and the prop probes `#[component]` / `#[props]` build its
/// props list from. Always present; only robot builds do any work.
#[doc(hidden)]
pub mod __inspect {
    pub use crate::robot_methods::{__inspect_component, ComponentInspect};
    pub use crate::robot_props::{entry, probe, InspectProps, PropEntry, PropProbe, PropsProbe};
}

/// Compatibility shim for `runtime-macros` 1.9 and older, which wrap the
/// root of every `#[method]` component in this call, in every build.
/// Those macros can still resolve against this vocabulary (both sit behind
/// `runtime-core`'s caret requirements), so removing the name broke their
/// expansion. Identity: the element↔component link it used to arm is now
/// made by `__inspect::__inspect_component`, which newer macros emit, so an
/// old-macros robot build only loses that link in the Inspector.
#[doc(hidden)]
pub fn __component_root(
    child: Element,
    _instance: crate::robot_methods::ComponentInstanceId,
) -> Element {
    child
}

/// Keepalive for a hand-registered component's robot registration: an
/// effect whose closure owns the `ComponentRegistration` guard. Created
/// inside a component body — i.e. inside `component_scope`'s collector —
/// so the surrounding `Owned` owns it and the registration deregisters
/// exactly when the subtree unrealizes. The closure reads no signals, so
/// the effect fires once and never re-runs. (`#[component]` does not
/// emit this: its registration rides `__inspect::ComponentInspect`.)
#[doc(hidden)]
pub fn __component_keepalive_effect(f: impl FnMut() + 'static) {
    let mut f = f;
    let _ = runtime_world::effect(move || {
        f();
    });
}

/// A fresh, world-root-owned signal — used by the macro for the
/// *uncontrolled* `text_input` / `toggle` / `slider` defaults
/// (`runtime_world::Signal` has no `new`, and this glue cannot add one to
/// a foreign type).
pub fn fresh_signal<T: PartialEq + 'static>(value: T) -> Signal<T> {
    signal(value)
}

// ============================================================================
// IntoElement + ChildList — the coercion seams every emission site uses.
// ============================================================================

/// Anything the macro can coerce to one [`Element`]: Element itself,
/// every glue wrapper, and zero-arg closures, which become structural
/// holes.
pub trait IntoElement {
    fn into_element(self) -> Element;
}

impl IntoElement for Element {
    fn into_element(self) -> Element {
        self
    }
}

/// A closure child is a structural hole: rebuilt whenever the signals it
/// eagerly reads change (`dyn_element` semantics).
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
/// through this (Element, Vec, Option, closures, glue wrappers).
/// Descriptor DATA types, reached from macro output as
/// `::runtime_core::__template::…` (the `finish()` retarget maps that
/// onto `::runtime_vocabulary::glue::__template::…`). Double underscore
/// for the same reason `__mcp` has one: macro-support surface, not an
/// author API.
///
/// Re-exported UNCONDITIONALLY, because `#[component]` / `#[props]`
/// generate an `__apply_literal` naming `TemplateLiteral` on every props
/// struct. The overlay that consumes it is behind the `ui-overlay`
/// feature; this type is not.
///
/// It is deliberately just the LITERAL type and its fallback trait. The
/// descriptor types proper — `Descriptor`, `Node`, `SiteId` — are build
/// artifacts a tool reads, never something compiled code names, so
/// nothing re-exports them into the author surface.
pub mod __template {
    pub use runtime_template::LiteralValue as TemplateLiteral;

    /// The fallback half of `#[component]`'s generated
    /// `__apply_literal`.
    ///
    /// A blanket impl so a props type WITHOUT the generated inherent
    /// method still compiles at a call site. Rust resolves inherent
    /// methods before trait methods, so a `#[component]` / `#[props]`
    /// struct uses its own and everything else silently answers "not
    /// applied" — which is the contract: such a component is not
    /// patchable, never an error.
    pub trait ApplyLiteralFallback {
        /// Returns whether the literal was applied.
        fn __apply_literal(&mut self, _name: &str, _value: &TemplateLiteral) -> bool {
            false
        }

        /// Returns whether the props type has a `children` field to put
        /// them in. See `__apply_literal` — same mechanism, same
        /// reason.
        fn __apply_children(
            &mut self,
            _children: ::std::vec::Vec<runtime_scene::Element>,
        ) -> bool {
            false
        }

        /// Register a constructor and apply whatever the overlay
        /// staged for the node being built. The generated inherent
        /// version does both; this one is what a props type the macros
        /// never touched gets, and it correctly does nothing — such a
        /// component is not patchable and not insertable, which is the
        /// contract, not an error.
        fn __overlay_bind(&mut self, _tag: &'static str) {}

        /// Build one of these from literal props and children.
        ///
        /// `None` here means "this type cannot be constructed from
        /// data", which is the right answer for a props type with no
        /// generated `BuildElement` — a hand-rolled one, or a struct
        /// that is not props at all. The overlay then refuses to insert
        /// it rather than guessing.
        fn __overlay_ctor(
            _props: &[(&str, &TemplateLiteral)],
            _children: ::std::vec::Vec<runtime_scene::Element>,
        ) -> ::core::option::Option<runtime_scene::Element>
        where
            Self: Sized,
        {
            ::core::option::Option::None
        }
    }

    impl<T> ApplyLiteralFallback for T {}
}

/// The dev-time overlay's runtime entry points, reached from macro
/// output as `::runtime_core::__overlay::…`.
///
/// Only exists under `ui-overlay` — and so does the emission that calls
/// it, so the two are never out of step within one build graph.
#[cfg(feature = "ui-overlay")]
pub use crate::overlay as __overlay;

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
/// a `view` wrapper for a genuinely multi-node list, and — for an empty
/// list — the layout-neutral
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
/// [`fragment`] (flat siblings — the scene's keyed driver accounts a
/// fragment row as its node vector).
fn one_or_fragment(mut nodes: Vec<Element>) -> Element {
    match nodes.len() {
        1 => nodes.pop().expect("len checked"),
        _ => fragment(nodes),
    }
}

/// The static-range `for` lowering (`for i in 0..n { single-node }`).
/// Emitted by `ui!`/`jsx!` only for an ident pattern, exclusive
/// both-bound range, single-node body, and non-reactive bounds. Mounts
/// through the vocabulary's
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
/// `if` contributes no flex slot (toggling an overlay through `if` would
/// otherwise shift its siblings' layout).
///
/// Styled by the NAMED, link-time-registered
/// [`runtime_shared::empty_absolute_sheet`] rather than raw
/// `StyleRules`: this was the last un-preminted style on the website
/// corpus — the one the framework itself emits — and raw rules have
/// nothing to premint by definition.
pub fn empty_absolute_view() -> Element {
    builders::view()
        .style(runtime_shared::StyleApplication::new(
            runtime_shared::empty_absolute_sheet(),
        ))
        .build()
}

// ============================================================================
// Glue wrappers — chainable shims over the builders.
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
/// (registered by the mount handler under the `robot` feature). The
/// `test_id_ignored` arm is for wrappers with no identity slot to forward
/// to — just the virtualizer behind `flat_list`. Don't move a wrapper into
/// it to save a slot: a silently dropped anchor leaves the node
/// unaddressable by `find_element` (why `LinkPrim` has one).
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
            /// Accepted and dropped: this primitive has no identity
            /// slot. See the macro docs above.
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

        /// The authored `.into()` coercion (e.g. a `flat_list` `render`
        /// closure returning `ui! { … }.into()`).
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

/// `view(children)` — the positional constructor the macro emits, over
/// the `ViewBuilder`.
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

    /// Raw touch handler (takes an author closure, not a pre-wrapped
    /// `TouchHandler` Rc).
    pub fn on_touch(
        mut self,
        handler: impl Fn(&runtime_shared::TouchEvent) -> TouchResponse + 'static,
    ) -> Self {
        self.b = self.b.on_touch(Rc::new(handler));
        self
    }

    /// Wheel / trackpad scroll handler.
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

    /// File drag-and-drop handler.
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
// keyboard_avoiding_view
// ---------------------------------------------------------------------------

/// `keyboard_avoiding_view(children)` — a container whose content the
/// backend keeps clear of the soft keyboard, moving in step with the
/// platform's own keyboard animation. Wrap the app root to avoid the
/// keyboard everywhere; wrap a portal's content (a modal, a sheet) to avoid
/// it there — portals mount outside the root. Defaults to
/// `KeyboardAvoidBehavior::Padding`, animated. See `docs/keyboard.md`.
pub fn keyboard_avoiding_view(children: Vec<Element>) -> GlueKeyboardAvoidingView {
    GlueKeyboardAvoidingView {
        b: builders::keyboard_avoiding_view().children(children),
        a11y: AccessibilityProps::default(),
    }
}

pub struct GlueKeyboardAvoidingView {
    b: builders::ViewBuilder,
    a11y: AccessibilityProps,
}

impl GlueKeyboardAvoidingView {
    /// `Padding` (default): the content area ends at the keyboard's top.
    /// `Translate`: the whole view lifts, nothing re-lays out.
    pub fn behavior(mut self, behavior: runtime_shared::KeyboardAvoidBehavior) -> Self {
        self.b = self.b.behavior(behavior);
        self
    }

    /// `false`: jump to the final position instead of moving with the
    /// keyboard's animation.
    pub fn animated(mut self, animated: bool) -> Self {
        self.b = self.b.animated(animated);
        self
    }

    pub fn safe_area(mut self, sides: SafeAreaSides) -> Self {
        self.b = self.b.safe_area(sides);
        self
    }

    /// Fill a `Ref<ViewHandle>` with this view's handle at mount.
    pub fn bind(mut self, r: Ref<ViewHandle>) -> Self {
        self.b = self.b.on_handle(move |h| crate::glue::fill_ref_while_mounted(r, h));
        self
    }
}

glue_wrapper_common!(GlueKeyboardAvoidingView);

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
/// [`IntoAction`] (plain closures included).
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
// `primitives` submodules — the `::runtime_core::primitives::…` paths the
// macro emits for the form-control / media tags (retargeted here).
// ============================================================================

pub mod primitives {
    /// The `#[component(lazy)]` emission surface (`lazy_split`,
    /// `LazyLoadingUi`/`LazyErrorUi`, the thunk-flavored `LazyFuture`)
    /// — implemented in `crate::glue_lazy`, re-exported here so the
    /// retargeted `::runtime_core::primitives::lazy::…` paths resolve.
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
            /// Fires when the image finishes loading.
            pub fn on_load(mut self, f: impl Fn(&ImageLoadEvent) + 'static) -> Self {
                self.b = self.b.on_load(std::rc::Rc::new(f));
                self
            }

            /// Fires when the image fails to load.
            pub fn on_error(mut self, f: impl Fn() + 'static) -> Self {
                self.b = self.b.on_error(std::rc::Rc::new(f));
                self
            }

            /// Reactive alt text.
            pub fn alt_reactive(mut self, f: impl Fn() -> Option<String> + 'static) -> Self {
                self.b = self.b.alt_dyn(f);
                self
            }

            /// Fill `r` with the mounted node's handle.
            pub fn bind(mut self, r: super::super::Ref<ImageHandle>) -> Self {
                self.b = self.b.on_handle(move |h| crate::glue::fill_ref_while_mounted(r, h));
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

        impl GlueToggle {
            /// Inert switch — a plain `bool` or a live source (`move ||
            /// locked.get()`, a `Signal`). While `true` the switch can't
            /// be flipped by pointer or keyboard, is not keyboard-focusable,
            /// never fires `on_change`, and resolves its style's
            /// `state disabled` overlay.
            pub fn disabled(mut self, disabled: impl IntoValue<bool>) -> Self {
                self.b = self.b.disabled(disabled);
                self
            }
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

            /// Inert slider — see
            /// [`GlueToggle::disabled`](super::toggle::GlueToggle::disabled).
            pub fn disabled(mut self, disabled: impl IntoValue<bool>) -> Self {
                self.b = self.b.disabled(disabled);
                self
            }
        }

        glue_wrapper_common!(GlueSlider);
    }

    pub mod text_input {
        use super::super::*;

        pub use runtime_shared::primitives::text_input::{KeyboardKind, TextInputHandle};

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
            /// before the input handles them.
            pub fn on_key_down(mut self, f: impl Fn(&KeyEvent) -> KeyOutcome + 'static) -> Self {
                self.b = self.b.on_key_down(f);
                self
            }

            /// Reactive placeholder.
            pub fn placeholder_reactive(
                mut self,
                placeholder_src: impl Into<super::super::Reactive<Option<String>>>,
            ) -> Self {
                let r = placeholder_src.into();
                self.b = self.b.placeholder_dyn(move || r.get());
                self
            }

            /// Fill `r` with the mounted node's handle.
            pub fn bind(mut self, r: super::super::Ref<TextInputHandle>) -> Self {
                self.b = self.b.on_handle(move |h| crate::glue::fill_ref_while_mounted(r, h));
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

            /// Soft-keyboard hint: which on-screen keyboard a phone or
            /// tablet shows (`KeyboardKind::Email` → the `@`/`.com`
            /// layout, `Phone` → a digit pad, …). A plain kind or a live
            /// source (`Signal`/`Memo`/`rx!`) — a live one re-applies in
            /// place without rebuilding the input. Desktop backends have
            /// no soft keyboard and ignore it.
            pub fn keyboard(
                mut self,
                keyboard: impl Into<super::super::Reactive<KeyboardKind>>,
            ) -> Self {
                self.b = match keyboard.into() {
                    super::super::Reactive::Static(kind) => self.b.keyboard(kind),
                    live => self.b.keyboard_dyn(move || live.get()),
                };
                self
            }

            /// Inert field — a plain `bool` or a live source (`move ||
            /// locked.get()`, a `Signal`). While `true` the field is not
            /// editable, not keyboard-focusable (it drops focus if it
            /// held it), never fires `on_change`, and resolves its
            /// style's `state disabled` overlay.
            pub fn disabled(mut self, disabled: impl IntoValue<bool>) -> Self {
                self.b = self.b.disabled(disabled);
                self
            }

            /// Focus once, right after mount (HTML `autofocus`). Plain
            /// `bool`, mount-time only; the last autofocus field mounted
            /// wins. Works inside a portal / modal: `focus()` waits for
            /// the node to be attached.
            pub fn autofocus(mut self, autofocus: bool) -> Self {
                self.b = self.b.autofocus(autofocus);
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
            /// `true` scrolls left-right, `false` top-to-bottom (the
            /// default). Shorthand for `.axis(ScrollAxis::Horizontal)` /
            /// `.axis(ScrollAxis::Vertical)`.
            pub fn horizontal(mut self, horizontal: bool) -> Self {
                self.b = self.b.horizontal(horizontal);
                self
            }

            /// Which axes scroll — `ScrollAxis::Both` for content that
            /// overflows in both directions. See [`ScrollAxis`].
            pub fn axis(mut self, axis: ScrollAxis) -> Self {
                self.b = self.b.axis(axis);
                self
            }

            pub fn safe_area(mut self, sides: SafeAreaSides) -> Self {
                self.b = self.b.safe_area(sides);
                self
            }

            /// Clamp the scroll to its content (`false`), or ask for
            /// the platform's overscroll spring (`true`). Saying
            /// nothing leaves the platform default.
            pub fn bounces(mut self, bounces: bool) -> Self {
                self.b = self.b.bounces(bounces);
                self
            }

            /// Whether the scroller bounces with NOTHING to scroll.
            /// `false` keeps the spring where content overflows and
            /// drops it where it fits.
            pub fn always_bounce(mut self, always: bool) -> Self {
                self.b = self.b.always_bounce(always);
                self
            }

            pub fn on_scroll(mut self, handler: impl Fn(f32, f32) + 'static) -> Self {
                self.b = self.b.on_scroll(handler);
                self
            }

            /// Fire when the reader arrives at the end of the scroll
            /// axis — once per arrival. See `ScrollViewPrim` for which
            /// backends answer it.
            pub fn on_end_reached(mut self, handler: impl Fn() + 'static) -> Self {
                self.b = self.b.on_end_reached(handler);
                self
            }

            /// How close to the end counts as arriving, in logical px.
            pub fn end_reached_threshold(mut self, px: f32) -> Self {
                self.b = self.b.end_reached_threshold(px);
                self
            }

            /// Fill `r` with the mounted node's handle.
            pub fn bind(mut self, r: super::super::Ref<ScrollViewHandle>) -> Self {
                self.b = self.b.on_handle(move |h| crate::glue::fill_ref_while_mounted(r, h));
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
            /// Reactive indicator size.
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
        /// (`link(route = …, params = …) { … }`). The destination
        /// resolves against the ENCLOSING navigator's ambient
        /// `LinkActivator` at mount — the swap Selects, the stack
        /// Pushes — and a link mounted outside any navigator silently
        /// no-ops on activation. The pre-computed URL feeds the web
        /// `<a href>` / right-click affordances.
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
            /// Reactive URL (href swaps in place on signal change).
            pub fn url(mut self, url: impl IntoValue<String>) -> Self {
                self.b = self.b.url(url);
                self
            }

            pub fn on_activate(mut self, f: impl Fn() + 'static) -> Self {
                self.b = self.b.on_activate(f);
                self
            }
        }

        // `test_id` is FORWARDED, not dropped (`LinkPrim::test_id`): a
        // dropped one would leave automation no way to navigate a
        // route-based app.
        glue_wrapper_common!(GlueLink);
    }

    /// `overlay(children)` / `anchored_overlay(target, children)` — the
    /// portal compositions. NOTE: unlike the other primitives these have
    /// no accessibility setters (the lowering hardcodes a default a11y bag
    /// on the portal), and `with_style` styles the CONTENT WRAPPER
    /// (`content_style` semantics).
    pub mod overlay {
        use super::super::*;
        pub use runtime_shared::primitives::overlay::BackdropMode;
        pub use runtime_shared::primitives::portal::{
            AnchorTarget, ElementAlign, ElementSide, PortalHandle, ViewportPlacement,
        };

        /// Viewport-anchored overlay (modal, drawer, sheet). Defaults:
        /// `Center`, `Dismiss` backdrop, trap ON.
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

            /// No `cycle()` wrap needed: every write stages until the
            /// driver flush by design.
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

            /// Content-wrapper style.
            pub fn with_style(mut self, s: impl IntoStyleProp) -> Self {
                self.b = self.b.style(s);
                self
            }

            /// Receive the mounted node's handle.
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

        /// Element-anchored overlay (popover, tooltip, menu). Defaults:
        /// `Below`/`Start`, offset 0, NO backdrop, trap OFF.
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

            /// The presence predicate: a `Fn() -> bool` closure, a
            /// signal, or a bool (`IntoValue<bool>`).
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

            /// Accepted and ignored: a presence carries no style —
            /// styling belongs on the child view, not on the presence
            /// node. Kept so `presence(style = …)` compiles.
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

            /// Receive the mounted node's handle.
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
            /// No `cycle()` wrap needed — staged writes flush with the
            /// driver.
            pub fn on_resize(mut self, f: impl FnMut(OnResizeEvent) + 'static) -> Self {
                self.b = self.b.on_resize(f);
                self
            }

            pub fn on_lost(mut self, f: impl FnMut() + 'static) -> Self {
                self.b = self.b.on_lost(f);
                self
            }

            /// Receive the mounted node's handle.
            pub fn on_handle(mut self, fill: impl FnOnce(GraphicsHandle) + 'static) -> Self {
                self.b = self.b.on_handle(fill);
                self
            }
        }

        glue_wrapper_common!(GlueGraphics);
    }

    /// `flat_list(data, key, size, render)` — the typed, type-erasing
    /// adapter over the closure-form `builders::virtualizer`.
    /// `FlatListItemSize` / `fixed_size` / `Axis` / `Lanes` are the
    /// runtime-shared types (re-exported).
    pub mod flat_list {
        use super::super::*;
        pub use runtime_shared::primitives::flat_list::{fixed_size, FlatListItemSize};
        pub use runtime_shared::primitives::virtualizer::{
            Axis, ItemKey, ItemSize, Lanes, VirtualizerHandle,
        };

        /// The third generic is unused — the macro emits
        /// `::<_, _, (), _>`.
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
            // call time.
            let item_count = move || data.get().len();

            let key = Rc::new(key);
            let item_key = {
                let key = key.clone();
                move |idx: usize| {
                    let v = data.get();
                    match v.get(idx) {
                        Some(item) => key(idx, item),
                        // Out-of-range sentinel: don't collide with
                        // valid keys on a stale index.
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
                    // Stale index → empty view.
                    None => super::super::view(Vec::new()).into_element(),
                }
            };

            // Item-change detection: each mounted
            // row keeps a clone of the item it rendered from; on a data
            // change the backend asks whether a surviving key's item is
            // still `==` and remounts the row only if not. Uses the
            // `T: PartialEq` bound this fn already carries, so no caller
            // gains a bound. Untracked reads: these run from the backend's
            // diff, never as a dependency of anything.
            let item_diff = runtime_shared::primitives::virtualizer::ItemDiff {
                capture: Rc::new(move |idx| {
                    data.with_untracked(|v| v.get(idx).cloned())
                        .map(|item| Box::new(item) as Box<dyn std::any::Any>)
                }),
                differs: Rc::new(move |snapshot, idx| {
                    let Some(old) = snapshot.downcast_ref::<T>() else {
                        return true;
                    };
                    data.with_untracked(|v| v.get(idx).map_or(true, |new| new != old))
                }),
            };

            GlueFlatList {
                b: builders::virtualizer(item_count, item_key, item_size, render)
                    .item_diff(item_diff),
                a11y: AccessibilityProps::default(),
            }
        }

        pub struct GlueFlatList {
            pub(crate) b: builders::VirtualizerBuilder,
            pub(crate) a11y: AccessibilityProps,
        }

        impl GlueFlatList {
            /// Inset the list's CONTENT by the safe area on `sides`
            /// while the scroller draws through it — the virtualized
            /// counterpart of `scroll_view().safe_area(..)`.
            pub fn safe_area(mut self, sides: runtime_shared::SafeAreaSides) -> Self {
                self.b = self.b.safe_area(sides);
                self
            }

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

            /// Receive the mounted node's handle.
            pub fn on_handle(mut self, fill: impl FnOnce(VirtualizerHandle) + 'static) -> Self {
                self.b = self.b.on_handle(fill);
                self
            }

            /// Observe the list's own scroll offset — same `(x, y)`
            /// contract as `scroll_view`'s `.on_scroll(..)`. See
            /// `builders::VirtualizerBuilder::on_scroll`.
            pub fn on_scroll(mut self, handler: impl Fn(f32, f32) + 'static) -> Self {
                self.b = self.b.on_scroll(handler);
                self
            }

            /// Fetch the next page as the reader nears the last item —
            /// see `builders::VirtualizerBuilder::on_end_reached`.
            pub fn on_end_reached(mut self, handler: impl Fn() + 'static) -> Self {
                self.b = self.b.on_end_reached(handler);
                self
            }

            /// How close to the end counts as arriving, in logical px.
            pub fn end_reached_threshold(mut self, px: f32) -> Self {
                self.b = self.b.end_reached_threshold(px);
                self
            }
        }

        glue_wrapper_common!(GlueFlatList, test_id_ignored);
    }

    /// `virtual_grid` — the two-axis virtualized grid.
    ///
    /// Unlike [`flat_list`](super::flat_list), there is no typed
    /// `Signal<Vec<T>>` wrapper: a grid's data is two-dimensional and
    /// the shapes authors actually have (a `Vec<Vec<T>>`, a
    /// `HashMap<(usize, usize), T>`, a sparse fetch keyed by day) don't
    /// reduce to one container. The closure form takes whatever the app
    /// already has, so the wrapper would only ever be a lossy special
    /// case of it.
    pub mod virtual_grid {
        pub use crate::builders::{virtual_grid, VirtualGridBuilder};
        pub use runtime_shared::primitives::virtual_grid::{
            CellKey, GridMetrics, GridWindow, Scrollport, VirtualGridHandle,
        };
    }

    /// `runtime_shared::primitives::portal`'s data types (the
    /// overlay/anchored_overlay COMPOSITIONS live in
    /// [`overlay`](super::primitives::overlay)).
    pub mod portal {
        pub use runtime_shared::primitives::portal::{
            AnchorTarget, AnchorableHandle, ElementAlign, ElementSide, PortalHandle, PortalTarget,
            ViewportPlacement, ViewportRect,
        };
    }

    /// Keyboard event data (`runtime_shared::primitives::key`).
    pub mod key {
        pub use runtime_shared::primitives::key::{KeyEvent, KeyOutcome};
    }

    /// `runtime_shared::primitives::navigator`'s DATA types (routes +
    /// header slot data an author chrome component renders). The
    /// navigator RUNTIME is the vocabulary's `handlers::navigator`
    /// (`SwapNav`/`StackNav` world contexts). `Screen` is the carrier
    /// (scene `Element` + opaque options — see `prims::Screen`).
    pub mod navigator {
        pub use crate::prims::{Screen, ScreenChrome};
        pub use runtime_shared::primitives::navigator::{
            screen_query, screen_state, split_query, with_query, HeaderButton, QueryParams, Route,
            RouteParams, ScreenState, StackHeaderState,
        };
    }

    /// The `text_area` primitive.
    pub mod text_area {
        use super::super::*;

        pub use runtime_shared::primitives::text_area::TextAreaHandle;

        /// `text_area(value, on_change)` — the positional constructor the
        /// macro emits, over the `TextAreaBuilder`.
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
            /// horizontally — the code-editor shape. Forwards to
            /// `TextAreaBuilder::wrap`.
            ///
            /// The horizontal scrolling is the platform's own default for
            /// an unwrapped text control, not something the framework
            /// pins, so a style may say `Overflow::Hidden` and get a clip
            /// instead — which is what `codeblock`'s `code_editor` does
            /// for the layer it stretches over its highlight layer.
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

            /// Focus once, right after mount — see
            /// [`GlueTextInput::autofocus`](super::text_input::GlueTextInput::autofocus).
            pub fn autofocus(mut self, autofocus: bool) -> Self {
                self.b = self.b.autofocus(autofocus);
                self
            }

            /// Inert text area — see
            /// [`GlueTextInput::disabled`](super::text_input::GlueTextInput::disabled).
            pub fn disabled(mut self, disabled: impl runtime_world::IntoValue<bool>) -> Self {
                self.b = self.b.disabled(disabled);
                self
            }

            /// Fill `r` with the mounted node's handle.
            pub fn bind(mut self, r: super::super::Ref<TextAreaHandle>) -> Self {
                self.b = self.b.on_handle(move |h| crate::glue::fill_ref_while_mounted(r, h));
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
/// GUARDED hole ([`dyn_keyed`]) keyed on the predicate's boolean: a
/// predicate that reads extra signals must not rebuild the branch when
/// only those extras changed.
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

/// A reactive branch over a scrutinee with no `PartialEq`: nothing can say
/// two values are equal, so every re-fire of `scrutinee` (it runs tracked)
/// rebuilds the branch with the new value. `render` runs untracked, as in
/// [`switch`].
fn switch_each<S: 'static>(
    scrutinee: impl Fn() -> S + 'static,
    render: impl Fn(&S) -> Element + 'static,
) -> Element {
    let current: Rc<std::cell::RefCell<Option<S>>> = Rc::new(std::cell::RefCell::new(None));
    let seen = Rc::clone(&current);
    runtime_scene::dyn_guarded(
        move || {
            // Read before borrowing: the scrutinee may reach a nested hole
            // that re-enters this cell.
            let value = scrutinee();
            *current.borrow_mut() = Some(value);
            true
        },
        move || render(seen.borrow().as_ref().expect("scrutinee is read before the build")),
    )
}

/// How `ui!` lowers a reactive `match` / `if let`: as
/// `(&__SwitchOn::new(scrutinee)).__switch(render)`, where method
/// resolution picks the hole by autoref specialization, at compile time:
///
/// - `S: PartialEq` — [`__SwitchKeyed`] is found at the by-value step and
///   lowers to [`switch`], so a re-fire with an equal value keeps the arm
///   mounted;
/// - otherwise [`__SwitchEach`], one autoref step later, rebuilds the arm
///   on every re-fire, so a scrutinee such as `Option<Rc<dyn Fn()>>` lowers
///   instead of failing on `switch`'s bound.
///
/// The scrutinee sits in a `Cell` because both methods take `&self` (that
/// is what lets the two impls sit one autoref apart) and each must move it
/// into the hole.
#[doc(hidden)]
pub struct __SwitchOn<S, F> {
    scrutinee: std::cell::Cell<Option<F>>,
    value: std::marker::PhantomData<fn() -> S>,
}

impl<S, F: Fn() -> S> __SwitchOn<S, F> {
    pub fn new(scrutinee: F) -> Self {
        Self { scrutinee: std::cell::Cell::new(Some(scrutinee)), value: std::marker::PhantomData }
    }

    fn take(&self) -> F {
        self.scrutinee.take().expect("a __SwitchOn lowers once")
    }
}

#[doc(hidden)]
pub trait __SwitchKeyed<S> {
    fn __switch(&self, render: impl Fn(&S) -> Element + 'static) -> Element;
}

impl<S: PartialEq + 'static, F: Fn() -> S + 'static> __SwitchKeyed<S> for __SwitchOn<S, F> {
    fn __switch(&self, render: impl Fn(&S) -> Element + 'static) -> Element {
        switch(self.take(), render)
    }
}

#[doc(hidden)]
pub trait __SwitchEach<S> {
    fn __switch(&self, render: impl Fn(&S) -> Element + 'static) -> Element;
}

impl<S: 'static, F: Fn() -> S + 'static> __SwitchEach<S> for &__SwitchOn<S, F> {
    fn __switch(&self, render: impl Fn(&S) -> Element + 'static) -> Element {
        switch_each(self.take(), render)
    }
}

/// How a reactive `if let` binds: `switch` hands each render a `&S`, so a
/// plain `match` on it binds references into the keyed value, and a
/// `'static` closure in the branch (`button(on_click = move || … id …)`)
/// cannot keep one. `ui!` lowers `if let` as
/// `match (&__IfLetArm(v)).__if_let_value() { … }`, and method resolution
/// picks the binding by autoref specialization, at compile time:
///
/// - `S: Clone` (every `.get()` scrutinee) — [`__IfLetOwn`] is found at the
///   by-value step and yields an owned clone, so bindings are owned, as
///   they are in a plain Rust `if let`;
/// - otherwise [`__IfLetBorrow`], one autoref step later, yields the `&S`,
///   so a non-`Clone` scrutinee still compiles with reference bindings.
///
/// Not a `.clone()` in the expansion: that would make `Clone` a new
/// requirement on every reactive `if let` scrutinee.
#[doc(hidden)]
pub struct __IfLetArm<'a, S>(pub &'a S);

#[doc(hidden)]
pub trait __IfLetOwn<S> {
    fn __if_let_value(&self) -> S;
}

impl<S: Clone> __IfLetOwn<S> for __IfLetArm<'_, S> {
    fn __if_let_value(&self) -> S {
        self.0.clone()
    }
}

#[doc(hidden)]
pub trait __IfLetBorrow<'a, S> {
    fn __if_let_value(&self) -> &'a S;
}

impl<'a, S> __IfLetBorrow<'a, S> for &__IfLetArm<'a, S> {
    fn __if_let_value(&self) -> &'a S {
        self.0
    }
}

// ============================================================================
// StaticCond / ReactiveCond — type-driven `if COND` dispatch for a bare
// path/field condition (`if visible`, `if props.open`).
// ============================================================================

/// The static arm: a plain `bool` runs the taken branch's thunk once,
/// contributing its nodes as FLAT siblings (no wrapper, no reactivity).
///
/// `FnOnce` (not `Fn`): a static branch runs at most once, so its thunk
/// may MOVE captures (`text { some_string }`). An `Fn + 'static` bound
/// makes a condition hoisted to a `let bool` fail with E0507 ("cannot
/// move out of a captured variable in an `Fn` closure"), forcing
/// `.clone()` workarounds.
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
/// only defined on `IntoIterator` types) — a reactive list must be keyed.
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
/// `FnOnce`: the keyed reconciler calls
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

/// One interpolation slot: a `Display`
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
                // Initial computed UNTRACKED at slot construction:
                // building an element must not subscribe the ambient
                // scope.
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

/// Assemble f-string pieces into the most capable source the slots allow:
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
/// (`Dynamic`). The live arm's `get()` inside a binding effect
/// subscribes via world signals.
///
/// `Static`/`Const` and `Dynamic`/`Dyn` are isomorphic with
/// `runtime_world::Value`, and [`IntoValue`] is implemented so a
/// `Reactive` prop forwards straight into any builder prop.
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

    /// Map the value, keeping its liveness: a `Static` maps once, a
    /// `Dynamic` maps on every read (so a `Reactive<bool>` prop becomes an
    /// on/off variant-axis value without losing its signal).
    pub fn map<U: 'static>(self, f: impl Fn(T) -> U + 'static) -> Reactive<U>
    where
        T: 'static,
    {
        match self {
            Reactive::Static(v) => Reactive::Static(f(v)),
            Reactive::Dynamic(g) => Reactive::Dynamic(Rc::new(move || f(g()))),
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

/// A `Reactive` variant value selects a sheet axis directly — the shape a
/// `#[props]`-wrapped axis prop arrives in. `Static` stays a constant source
/// (the static, preminted fast path); `Dynamic` re-applies the style when
/// it changes.
impl<E: runtime_shared::VariantEnum> runtime_shared::IntoVariantSource<E> for Reactive<E> {
    fn into_variant_source(self) -> Box<dyn Fn() -> &'static str> {
        match self {
            Reactive::Static(v) => {
                let key = v.as_variant_str();
                Box::new(move || key)
            }
            Reactive::Dynamic(f) => Box::new(move || f().as_variant_str()),
        }
    }
    fn is_reactive(&self) -> bool {
        !self.is_static()
    }
}

/// Bare value → `Static` (the `ui!` `.into()` coercion; `From<T>` and
/// `From<Signal<T>>` can't overlap — `T = Signal<T>` fails the occurs
/// check).
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

    /// `build`, told which fields the call site set (`ui!` passes their
    /// names; the rest came from `defaults()`). Only a remote bundle's
    /// import of an app component overrides it: it sends just those
    /// fields and the app fills the rest from ITS defaults, so a call
    /// site's defaults never have to cross (see `remote::ImportProps`).
    /// Everywhere else this is `build`.
    #[inline(always)]
    fn build_set(self, set: &'static [&'static str]) -> Element {
        let _ = set;
        self.build()
    }

    /// `build_set`, with the call site's set as a type (a remote bundle's
    /// `ui!`, through `__remote_site!`). An imported app component
    /// overrides it to leave a record of the call site in the bundle,
    /// which the release build reads to list what the bundle requires of
    /// an app (`remote::site`).
    #[doc(hidden)]
    #[inline(always)]
    fn __build_site<S: PropSet>(self) -> Element {
        self.build_set(S::SET)
    }
}

/// The props a `ui!` call site sets, as a type: what a remote bundle's
/// call site passes to [`BuildElement::__build_site`], so the record it
/// leaves can be a constant.
#[doc(hidden)]
pub trait PropSet {
    const SET: &'static [&'static str];
    /// `SET`, each name followed by `\n`.
    const JOINED: &'static str;
}

/// The struct-update base for a SIGNAL-typed prop with no declared
/// default. `runtime_world`'s handles are foreign, so `impl Default for
/// Signal` can't be written here; instead the base mints a fresh
/// default-valued signal in the ambient scope. Omitting a required
/// signal prop therefore reads this fresh signal rather than panicking.
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

/// RAII guard returned by [`__component_build_probe`]. Component bodies
/// run untracked by construction (`component_scope`), so the probe
/// carries nothing in an ordinary build.
///
/// Under `hot-reload` it becomes load-bearing: it is what opens and
/// closes a component's frame on the kernel's hot-state path stack, and
/// that stack is the only identity a preserved signal has across a
/// re-run. RAII rather than a matched pair of calls because a component
/// body can return early or panic, and a leaked frame would mis-key
/// every signal built after it.
pub struct BuildProbeGuard {
    /// Whether this guard actually pushed a frame. `false` when no
    /// build window is open, which is every non-dev build and every
    /// construction outside a mount.
    #[cfg(feature = "hot-reload")]
    pushed: bool,
}

#[cfg(feature = "hot-reload")]
impl Drop for BuildProbeGuard {
    fn drop(&mut self) {
        if self.pushed {
            runtime_world::hot_state::pop_frame();
        }
    }
}

/// The bracket `#[component]` wraps every body in. A no-op in ordinary
/// builds; under `hot-reload` it opens the component's frame on the
/// hot-state path stack (see [`BuildProbeGuard`]).
pub fn __component_build_probe(_name: &'static str) -> BuildProbeGuard {
    #[cfg(feature = "hot-reload")]
    {
        BuildProbeGuard { pushed: runtime_world::hot_state::push_frame(_name) }
    }
    #[cfg(not(feature = "hot-reload"))]
    {
        BuildProbeGuard {}
    }
}

// ============================================================================
// Prelude — the author surface for an app crate.
// ============================================================================

/// Everything an app written with `ui!` + `#[component]` needs in
/// scope.
pub mod prelude {
    pub use super::{
        component_scope, effect, memo, on_cleanup, on_scope_drop, signal, untrack, BuildElement,
        ChildList, Element, IntoElement, Memo, Reactive, ReadSignal, Signal, WriteSignal,
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

    /// Signal slots assemble to the pre-decomposed JS binding: ids in
    /// slot order, N+1
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
    /// the whole text down the effect path (Value::Dyn).
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
// `pressable(children, on_click)` + the `.bind(Ref<…>)` family.
//
// `pressable` has no `ui!` tag lowering (authors call the fn). `.bind`
// fills a `Ref` slot with the mount-time handle: the vocabulary handlers
// deliver real handles through each prim's `ref_fill` (`make_view_handle`
// & friends), so a bound Ref drives the imperative surfaces
// (AnimatedValue, anchor targets, focus).
// ============================================================================

/// `pressable(children, on_click)` — positional constructor over the
/// `PressableBuilder`.
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

    /// Pointer-enter/leave notifications (`true` on enter), as on a view.
    pub fn on_hover(mut self, handler: impl Fn(bool) + 'static) -> Self {
        self.b = self.b.on_hover(handler);
        self
    }

    /// Fill `r` with the mounted node's handle.
    pub fn bind(mut self, r: Ref<PressableHandle>) -> Self {
        self.b = self.b.on_handle(move |h| crate::glue::fill_ref_while_mounted(r, h));
        self
    }
}

glue_wrapper_common!(GluePressable);

impl GlueView {
    /// Fill `r` with the mounted node's handle.
    pub fn bind(mut self, r: Ref<ViewHandle>) -> Self {
        self.b = self.b.on_handle(move |h| crate::glue::fill_ref_while_mounted(r, h));
        self
    }
}

impl GlueText {
    /// Fill `r` with the mounted node's handle.
    pub fn bind(mut self, r: Ref<TextHandle>) -> Self {
        self.b = self.b.on_handle(move |h| crate::glue::fill_ref_while_mounted(r, h));
        self
    }
}

impl GlueButton {
    /// Fill `r` with the mounted node's handle.
    pub fn bind(mut self, r: Ref<runtime_shared::ButtonHandle>) -> Self {
        self.b = self.b.on_handle(move |h| crate::glue::fill_ref_while_mounted(r, h));
        self
    }
}

impl GlueIcon {
    /// Pin to a `size × size`
    /// point square via a minted, deduped square-sizing sheet (an icon
    /// has no intrinsic content size; without this it collapses to 0×0
    /// under flex). Width/height + `flex_shrink: 0`, cached per rounded px so every
    /// icon at a size shares one sheet/class.
    pub fn size(self, size: f32) -> Self {
        use std::cell::RefCell;
        use std::collections::HashMap;
        runtime_tls::thread_local! {
            static ICON_SIZE_SHEETS: RefCell<HashMap<u32, Rc<StyleSheet>>> =
                RefCell::new(HashMap::new());
        }
        let key = (size * 100.0).round() as u32;
        let sheet = ICON_SIZE_SHEETS.with(|m| {
            if let Some(s) = m.borrow().get(&key) {
                return s.clone();
            }
            // Identity carries the size, so each distinct px gets its own
            // build-time class. The dump MOUNTS the app, so it mints exactly
            // the sizes the app actually renders — the same trick
            // `icon_button.rs`'s per-size sheet plays. Without an identity
            // this sheet has no preminted CSS, and every sized icon on the
            // page fell through to the live style engine and kept it linked
            // (two of the eleven fall-throughs on the catalog's landing
            // route were this one sheet at 18px and 19px).
            let sheet = StyleSheet::r#static(StyleRules {
                width: Some(Tokenized::Literal(Length::Px(size))),
                height: Some(Tokenized::Literal(Length::Px(size))),
                flex_shrink: Some(Tokenized::Literal(0.0)),
                ..Default::default()
            })
            .premint_as(&format!("runtime.v1.icon.size.{key}"));
            m.borrow_mut().insert(key, sheet.clone());
            sheet
        });
        self.with_style(sheet)
    }

    /// Live vector data.
    pub fn data(mut self, f: impl Fn() -> IconData + 'static) -> Self {
        self.b = self.b.data_dyn(f);
        self
    }

    /// Fill `r` with the mounted node's handle.
    pub fn bind(mut self, r: Ref<runtime_shared::IconHandle>) -> Self {
        self.b = self.b.on_handle(move |h| crate::glue::fill_ref_while_mounted(r, h));
        self
    }
}
