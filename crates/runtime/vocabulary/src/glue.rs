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
//! `Element::Repeat`, the virtualizer `for`-sugar): those carried wire
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
//! | `for i in 0..3` (static range) | `Element::Repeat` (batched) | type-driven static path (rows built once; batching hint dropped) |
//! | `Comp(prop = v)` | `BuildElement` struct literal | same shape against `glue::BuildElement` |
//! | `#[component]` body | probe + reactivity rewrite | + wrapped in `component_scope(move ‖ …)` (run-once, untracked, collected `Owned`) |
//! | `Reactive<T>` props | `runtime_core::Reactive` | `glue::Reactive` (same API, world-backed; `IntoValue` bridges to builders) |
//! | empty `if` branch | absolute-positioned `view` via StyleSheet | [`empty_absolute_view`] (same rule) |
//!
//! ## Deferred surface (loud, not silent)
//!
//! Author constructs that reach an unmigrated subsystem fail to compile
//! with a message naming the migration status (emitted by the macro
//! under `new-core`): `overlay` / `anchored_overlay` / `presence` /
//! `graphics` / `flat_list` / in-app `link(route = …)` / `test_id = …` /
//! `#[component(lazy)]` / `#[method]` blocks. Everything this module
//! *does* export is fully functional on the new core.

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
macro_rules! glue_wrapper_common {
    ($wrapper:ident) => {
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

        glue_wrapper_common!(GlueLink);
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
    /// An interpolation slot, already coerced static-or-live by TYPE.
    Slot(Value<String>),
}

/// The static slot arm: any `Display` value formats once (`Const`).
pub trait StaticTextSlot {
    fn __idealyst_text_slot(
        self,
        fmt: impl Fn(&dyn std::fmt::Display) -> String + 'static,
    ) -> Value<String>;
}

impl<T: std::fmt::Display> StaticTextSlot for T {
    fn __idealyst_text_slot(
        self,
        fmt: impl Fn(&dyn std::fmt::Display) -> String + 'static,
    ) -> Value<String> {
        Value::Const(fmt(&self))
    }
}

/// The reactive slot arm: a `Signal` / `ReadSignal` / `Memo` /
/// `Dynamic` [`Reactive`] slot re-formats on change (`Dyn`). Method
/// resolution picks this over [`StaticTextSlot`] because the handles
/// don't implement `Display`.
pub trait ReactiveTextSlot {
    fn __idealyst_text_slot(
        self,
        fmt: impl Fn(&dyn std::fmt::Display) -> String + 'static,
    ) -> Value<String>;
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
            ) -> Value<String> {
                Value::Dyn(Box::new(move || fmt(&self.get())))
            }
        }
    )+};
}

impl_reactive_text_slot!(Signal, ReadSignal, Memo);

impl<T: std::fmt::Display + Clone + 'static> ReactiveTextSlot for Reactive<T> {
    fn __idealyst_text_slot(
        self,
        fmt: impl Fn(&dyn std::fmt::Display) -> String + 'static,
    ) -> Value<String> {
        match self {
            Reactive::Static(v) => Value::Const(fmt(&v)),
            Reactive::Dynamic(f) => Value::Dyn(Box::new(move || fmt(&f()))),
        }
    }
}

/// Assemble the parts: all-`Const` → one `Const` concatenation (a
/// static literal stays zero-effect); any `Dyn` slot → one `Dyn`
/// closure re-concatenating on each fire.
pub fn __idealyst_text_from_parts(parts: Vec<TextSlotPart>) -> Value<String> {
    let any_dyn = parts
        .iter()
        .any(|p| matches!(p, TextSlotPart::Slot(Value::Dyn(_))));
    if !any_dyn {
        let mut s = String::new();
        for p in parts {
            match p {
                TextSlotPart::Lit(l) => s.push_str(l),
                TextSlotPart::Slot(Value::Const(v)) => s.push_str(&v),
                TextSlotPart::Slot(Value::Dyn(_)) => unreachable!("any_dyn checked"),
            }
        }
        return Value::Const(s);
    }
    Value::Dyn(Box::new(move || {
        let mut s = String::new();
        for p in &parts {
            match p {
                TextSlotPart::Lit(l) => s.push_str(l),
                TextSlotPart::Slot(Value::Const(v)) => s.push_str(v),
                TextSlotPart::Slot(Value::Dyn(f)) => s.push_str(&f()),
            }
        }
        s
    }))
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

    /// Const-vs-Dyn is the load-bearing invariant of the retargeted
    /// lowering: literals must stay Const (zero effects), signal-reading
    /// shapes must be Dyn.
    #[test]
    fn fstring_parts_const_when_all_static() {
        let v = __idealyst_text_from_parts(vec![
            TextSlotPart::Lit("a"),
            TextSlotPart::Slot(Value::Const("b".into())),
        ]);
        assert!(matches!(v, Value::Const(ref s) if s == "ab"));
    }

    #[test]
    fn fstring_parts_dyn_when_any_slot_is_live() {
        let world = World::new();
        world.enter(|| {
            let n = signal(1i32);
            let v = __idealyst_text_from_parts(vec![
                TextSlotPart::Lit("n="),
                TextSlotPart::Slot(n.__idealyst_text_slot(|d| format!("{d}"))),
            ]);
            match v {
                Value::Dyn(f) => assert_eq!(f(), "n=1"),
                Value::Const(_) => panic!("live slot must produce Dyn"),
            }
        });
    }

    #[test]
    fn static_slot_formats_once_via_display() {
        let v = 7i32.__idealyst_text_slot(|d| format!("[{d}]"));
        assert!(matches!(v, Value::Const(ref s) if s == "[7]"));
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
