//! # `glue` — the `ui!` / `#[component]` emission surface for the NEW core (P3a)
//!
//! When `runtime-macros` is built with its `new-core` feature, the macros
//! retarget every absolute `::runtime_core::…` path in their OUTPUT to
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
//! | `view { … }` | `runtime_core::view(children)` | `glue::view(children)` → `ViewBuilder` → `Element::Item(ViewPrim)` |
//! | `text { "lit" }` | `text(TextSource::Static)` | `glue::text(Const)` |
//! | `text { move ‖ … }` | `text(closure)` → Effect | `glue::text(Dyn)` → binding effect |
//! | `text { "{sig} x" }` | `TextSlotPart` + old slot traits | `glue::TextSlotPart` → `Value<String>` (all-Const folds to Const) |
//! | `text { f(sig) }` | `TextSource::Bound(Derived{…})` | `text(move ‖ format!("{}", f(sig.get())))` |
//! | `button(label=…, on_click=…)` | `button(label, IntoAction)` | `glue::button` → `ButtonBuilder` |
//! | `on_click = m(sig) => out` | structured `Action{fire,…}` | `move ‖ out.set(m(sig.get()))` |
//! | reactive `if` / `when` | `runtime_core::when` (Effect + anchor) | `glue::when` → `runtime_scene::dyn_keyed` (guarded hole) |
//! | bare-path `if cond` | `StaticCond`/`ReactiveCond` dispatch | same trait names in glue; reactive arm → `dyn_keyed` |
//! | `if is_even(sig)` | `when(Derived<bool>, …)` | `when(move ‖ is_even(sig.get()), …)` |
//! | reactive `match` | `runtime_core::switch` | `glue::switch` → `dyn_keyed` (PartialEq dedup) |
//! | literal-armed `match m(sig)` | `Element::Switch{Derived…}` | closure `switch` with `.get()`-rewritten scrutinee |
//! | `for x in vec` | `StaticForEach` → `Vec<Element>` | same, glue-side |
//! | `for x in sig, key=…` | `ReactiveForEach` → `Element::Each` | glue trait → `runtime_scene::keyed` |
//! | `for i in 0..n.get()` | `each_keyed(EachKey, EachRowBuild)` | same names, mapped onto `runtime_scene::keyed` |
//! | `for i in 0..3` (static range) | `Element::Repeat` (batched) | `__static_repeat` → `Element::Many(RepeatPrim)` (same batched one-FFI path) |
//! | `Comp(prop = v)` | `BuildElement` struct literal | same shape against `glue::BuildElement` |
//! | `#[component]` body | probe + reactivity rewrite | + wrapped in `component_scope(move ‖ …)` (run-once, untracked, collected `Owned`) |
//! | `Reactive<T>` props | `runtime_core::Reactive` | `glue::Reactive` (same API, world-backed; `IntoValue` bridges to builders) |
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
//! under `new-core`): in-app `link(route = …)` (the ambient
//! link-activator seam lives in the old routing registry — navigation
//! SDK retarget, P6), `web_view` (old-core SDK component, P6), the
//! virtualizer `for i in count(sig)` sugar (generator-backend
//! `Derived<usize>` metadata, post-P7), `#[component(lazy)]` (no
//! lazy/chunk-mount prim in the vocabulary yet) and `#[method]` blocks
//! (Ref/robot machinery, P5). `test_id = …` is NO LONGER deferred: it
//! lowers to each wrapper's `.test_id(…)` setter (identical to the old
//! lowering), backed by the vocabulary robot registry (`crate::robot`,
//! `robot` feature) — the P5 identity seam, brought forward. Everything
//! this module *does* export is fully functional on the new core.

use std::rc::Rc;

use runtime_core::accessibility::{
    AccessibilityProps, AccessibilityTraits, LiveRegionPriority, Role,
};
use runtime_core::primitives::activity_indicator::ActivityIndicatorSize;
use runtime_core::primitives::icon::{IconData, StrokeAnimation};
// `Easing` is re-exported (not just imported): the `stylesheet!`
// emitter spells `::runtime_core::Easing::…` for `transitions { … }`
// blocks, which the new-core retarget maps here.
pub use runtime_core::Easing;
// `Color` re-exported for the same reason (stylesheet bodies and app
// preludes reference it; the type is shared with the old core).
pub use runtime_core::Color;
use runtime_core::{FileDropHandler, IntoAction, SafeAreaSides, TouchHandler, WheelHandler};
use runtime_scene::{dyn_element, dyn_keyed, fragment, Key};
use runtime_world::{IntoValue, Value};

use crate::builders::{self, TextContent};

// Re-exports: the reactive surface + the scene Element under the names
// the macro (and app preludes) reach for.
pub use runtime_scene::{component_scope, Element};
pub use runtime_world::{
    effect, memo, on_cleanup, signal, untrack, Effect, Memo, ReadSignal, Signal, WriteSignal,
};

// The `stylesheet!` emission surface (P3c). Under `new-core` the macro's
// output is retargeted `::runtime_core::…` → `::runtime_vocabulary::glue::…`
// wholesale, so every name the generated sheet fn / builder / variant
// enums reference must resolve HERE. The style data model itself stays
// runtime-core's (sanctioned transitional dependency, crate docs) — these
// are re-exports, not forks. `IntoStyleProp`/`StyleProp` are the one
// NEW pair: the generated builder's conversion impl targets them instead
// of the old `IntoStyleSource`/`StyleSource`.
pub use crate::style_attach::{signal_class, IntoStyleProp, StyleProp};
pub use crate::theme;
pub use runtime_core::{
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
pub use runtime_core::{Ref, ViewHandle};

// The host-scheduling registry (shared infrastructure, not old-core
// reactivity): new-core drivers and test harnesses install/pump through
// the same `runtime_core::scheduling` registry the vocabulary handlers
// schedule against (presence anims, portal anchor tracking). Re-exported
// so a new-core crate needs no direct runtime-core dependency.
pub use runtime_core::scheduling;

// ============================================================================
// `#[method]` emission surface (P5). The `#[component]` macro emits, for a
// methods-bearing component: `::runtime_core::robot::Method` literals +
// `::runtime_core::robot::register_component(...)`, a
// `::runtime_core::__component_keepalive_effect(...)`, and a
// `::runtime_core::__component_root(...)` tail wrap — all retargeted here.
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
/// codegen (mirror of `runtime_core::__serde_json`).
#[doc(hidden)]
pub use runtime_core::__serde_json;

/// The names the macro spells as `::runtime_core::robot::…`, resolved
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
/// `runtime_core::IntoElement` (Element itself, every glue wrapper, and
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
/// through this. Mirrors `runtime_core::ChildList` (Element, Vec,
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
    let rules = runtime_core::StyleRules {
        position: Some(runtime_core::Position::Absolute),
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

    pub fn on_touch(mut self, handler: TouchHandler) -> Self {
        self.b = self.b.on_touch(handler);
        self
    }

    pub fn on_wheel(mut self, handler: WheelHandler) -> Self {
        self.b = self.b.on_wheel(handler);
        self
    }

    pub fn on_hover(mut self, handler: impl Fn(bool) + 'static) -> Self {
        self.b = self.b.on_hover(handler);
        self
    }

    pub fn on_file_drop(mut self, handler: FileDropHandler) -> Self {
        self.b = self.b.on_file_drop(handler);
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
// `primitives` submodules — mirror the `runtime_core::primitives::…` paths
// the macro emits for the form-control / media tags.
// ============================================================================

pub mod primitives {
    pub mod image {
        use super::super::*;
        use runtime_core::assets::{kinds, Asset};

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
        }

        glue_wrapper_common!(GlueScrollView);
    }

    pub mod activity_indicator {
        use super::super::*;

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
        /// (`Link(external = "…") { … }`). In-app `Link(route = …)` is a
        /// navigator concern and fails at the macro level under
        /// `new-core` (navigation migrates at P6).
        pub fn external_link(url: impl IntoValue<String>, children: Vec<Element>) -> GlueLink {
            GlueLink {
                b: builders::link().url(url).external(true).children(children),
                a11y: AccessibilityProps::default(),
            }
        }

        pub struct GlueLink {
            pub(crate) b: builders::LinkBuilder,
            pub(crate) a11y: AccessibilityProps,
        }

        impl GlueLink {
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
        pub use runtime_core::primitives::overlay::BackdropMode;
        pub use runtime_core::primitives::portal::{
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
        pub use runtime_core::primitives::presence::{PresenceAnim, PresenceHandle, PresenceState};

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
        pub use runtime_core::primitives::graphics::{GraphicsHandle, OnReadyEvent, OnResizeEvent};

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
        pub use runtime_core::primitives::flat_list::{fixed_size, FlatListItemSize};
        pub use runtime_core::primitives::virtualizer::{
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
pub trait StaticCond {
    fn __idealyst_if(
        self,
        then: impl Fn() -> Vec<Element> + 'static,
        otherwise: impl Fn() -> Vec<Element> + 'static,
    ) -> Vec<Element>;
}

impl StaticCond for bool {
    fn __idealyst_if(
        self,
        then: impl Fn() -> Vec<Element> + 'static,
        otherwise: impl Fn() -> Vec<Element> + 'static,
    ) -> Vec<Element> {
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
pub type EachRowBuild = Box<dyn Fn() -> Vec<Element>>;

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
/// `runtime_core::TextSlot` (sources.rs), tier for tier: a `Display`
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
            AssembledText::JsBinding(b) => crate::prims::TextSourceProp::JsBinding(b),
        }
    }
}

/// Assemble f-string pieces into the most capable source the slots allow
/// — port of `runtime_core::__idealyst_text_from_parts`, tier for tier:
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
/// (`runtime_core::reactive_value`) so component bodies compile
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
    pub use runtime_core::StyleRules;
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
