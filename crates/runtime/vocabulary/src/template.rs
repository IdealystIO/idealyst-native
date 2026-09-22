//! The template lowering's **builder**: `(descriptor, slots) -> Element`.
//!
//! `ui_lowered!(template { … })` emits, per `ui!` site, a `static`
//! [`runtime_template::Descriptor`] (data) plus an ordered array of
//! [`SlotValue`]s (the site's dynamic expressions, already evaluated).
//! [`build`] turns the pair into the same `Element` the direct lowering
//! would have produced — `crates/dev/ui-lowering-parity` is the gate on
//! "the same".
//!
//! # Why the builder lives here
//!
//! It needs the builtin glue wrappers (`glue::view`, `glue::text`, …),
//! the coercion traits (`TextContent`, `IntoStyleProp`, `IntoAction`)
//! and `BuildElement`. Those are the vocabulary. `runtime-template`
//! stays free of them so a descriptor remains a portable artifact.
//!
//! # Why slots are values, not thunks
//!
//! A slot arrives already evaluated. The site's emission binds every
//! `Prelude` slot to a local in source order and then constructs the
//! array, so the array's construction order *is* source order. The
//! direct lowering hoists the same slots into the same order (see
//! `runtime_macros::ui_split`), which is what makes the two lowerings'
//! observable evaluation order identical rather than merely similar.
//!
//! The exceptions are the slots that MUST stay lazy because the runtime
//! re-invokes them: a reactive `if`'s condition and its two branch
//! thunks ([`SlotValue::Cond`] / [`SlotValue::Branch`]). Those are
//! closures in both lowerings, and constructing a closure has no
//! observable effect, so they are order-free.
//!
//! # Escapes
//!
//! [`Node::Escape`] is the completeness escape hatch: the site built the
//! subtree with the direct lowering and left the finished `Element`(s)
//! in a slot. Every `ui!` construct is expressible that way, so the
//! template lowering is complete from day one, and the
//! descriptor-native set (the `Node::Prim` / `Node::Component` /
//! `Node::Dyn` kinds) grows without ever being a correctness
//! precondition. What an escape costs is the thing descriptors are for:
//! its literals are compiled in, so a static edit inside one needs a
//! rebuild.
//!
//! # Failure mode
//!
//! A descriptor whose prop shape disagrees with the slot the site
//! supplied is a MACRO bug, not an author error, and it cannot be
//! recovered from — the builder has a `StyleProp` where it needs a
//! `String`. So it panics, loudly, naming the site, the node, the prop
//! and both shapes. Same policy as an unregistered scene payload.

use std::any::Any;
use std::rc::Rc;

use runtime_scene::Element;
use runtime_shared::primitives::graphics::{OnReadyEvent, OnResizeEvent};
use runtime_shared::primitives::icon::IconData;
use runtime_shared::primitives::portal::AnchorTarget;
use runtime_shared::{Action, Color, Easing, IntoAction};
use runtime_template::{
    CompiledIn, Descriptor, LiteralValue, Node, PrimKind, PropEntry, PropValue, TemplateSource,
};
use runtime_world::{IntoValue, Value};

use crate::builders::TextContent;
use crate::glue::{self, ChildList, IntoElement};
use crate::prims::TextSourceProp;
use crate::style_attach::{IntoStyleProp, StyleProp};

// ===========================================================================
// Component constructors
// ===========================================================================

/// A site-supplied constructor for one `#[component]` node.
///
/// The builder cannot name a component's props type, so the emission
/// provides this: it mints the props struct's defaults, applies the
/// descriptor's literals through the generated
/// `__apply_literal(&mut Props, name, value) -> bool`, assigns the
/// DYNAMIC props it captured, moves the built children in, and calls
/// `BuildElement::build`.
///
/// `FnOnce`, and that is what lets a dynamic prop work at all: the thunk
/// captures the prop's already-evaluated value by move, so its type
/// never has to leave the call site. Taken from the slot array once and
/// called once — a nested template rebuilds its whole slot array per
/// invocation, so nothing needs to survive a second call.
pub type ComponentCtor = Box<dyn FnOnce(&[PropEntry], Vec<Element>) -> Element>;

// ===========================================================================
// SlotValue
// ===========================================================================

/// One evaluated dynamic expression, typed for the position it feeds.
///
/// The variant is chosen by the EMISSION, from the (node kind, prop
/// name) pair — the same table the builder matches on below. That is
/// what gives the author's expression an expected type at the call site:
/// `SlotValue::press(move |…| …)` types the closure the way
/// `glue::button`'s parameter would.
pub enum SlotValue {
    /// Already taken. A descriptor references each slot exactly once;
    /// meeting this means the descriptor double-referenced one.
    Taken,
    Str(String),
    Bool(Value<bool>),
    Text(TextSourceProp),
    Style(StyleProp),
    Press(Action),
    /// One finished subtree (a single-slot escape).
    Element(Element),
    /// A finished child list (a children-slot escape).
    Elements(Vec<Element>),
    Ctor(ComponentCtor),
    Cond(Rc<dyn Fn() -> bool>),
    Branch(Rc<dyn Fn() -> Element>),
    /// Which arm of a [`Node::Select`] is taken. Compiled, because the
    /// dispatch is patterns.
    Selector(Box<dyn FnOnce() -> usize>),
    /// One arm of a [`Node::Select`]. `FnOnce` because exactly one arm
    /// ever runs and it runs once.
    Arm(Box<dyn FnOnce() -> Vec<Element>>),
    /// A value of whatever type the (kind, prop) pair demands, erased.
    ///
    /// The concrete variants above cover the props that appear on almost
    /// every node — style, text, press, bool, string — so the hot
    /// primitives (`view`/`text`/`button`) allocate nothing extra. The
    /// long tail (a `ViewportPlacement`, an `ActivityIndicatorSize`, an
    /// `Rc<dyn Fn(f32)>`, …) would otherwise need one enum variant and
    /// one `shape()` arm each, twenty-plus of them, every one a place
    /// for the emitter's table and this one to drift apart. Erasing them
    /// costs a `Box` per such prop — rare per tree — and buys a single
    /// seam, [`SlotValue::take_as`].
    ///
    /// The type is still fixed AT THE CALL SITE by the typed constructor
    /// the emitter picked, so an un-annotated closure still infers; the
    /// erasure is only between the constructor and the builder arm.
    Any(Box<dyn Any>),
}

impl SlotValue {
    fn shape(&self) -> &'static str {
        match self {
            SlotValue::Taken => "taken",
            SlotValue::Str(_) => "str",
            SlotValue::Bool(_) => "bool",
            SlotValue::Text(_) => "text",
            SlotValue::Style(_) => "style",
            SlotValue::Press(_) => "press",
            SlotValue::Element(_) => "element",
            SlotValue::Elements(_) => "elements",
            SlotValue::Ctor(_) => "ctor",
            SlotValue::Cond(_) => "cond",
            SlotValue::Branch(_) => "branch",
            SlotValue::Selector(_) => "selector",
            SlotValue::Arm(_) => "arm",
            SlotValue::Any(_) => "any",
        }
    }

    /// Unwrap an [`SlotValue::Any`] as `T`.
    ///
    /// A mismatch is an emitter bug — the constructor and the builder
    /// arm disagree about the (kind, prop) pair's type — and there is no
    /// recovery, so it panics with both type names. Every pair is
    /// exercised by `ui-lowering-parity`.
    fn take_as<T: 'static>(self, what: &str) -> T {
        match self {
            SlotValue::Any(boxed) => match boxed.downcast::<T>() {
                Ok(v) => *v,
                Err(_) => panic!(
                    "template slot for `{what}` is not a {}: the emitter's slot \
                     constructor and the builder's arm disagree",
                    std::any::type_name::<T>()
                ),
            },
            other => panic!(
                "template slot for `{what}` must be an erased value, got `{}`",
                other.shape()
            ),
        }
    }
}

/// Slot constructors. Each one exists to give the author's expression an
/// expected type at the call site — which is also why an un-annotated
/// closure works here (`SlotValue::press(|| …)`) where a bare
/// `let x = |e| …;` would not.
impl SlotValue {
    pub fn string(v: impl Into<String>) -> SlotValue {
        SlotValue::Str(v.into())
    }

    pub fn boolean(v: impl IntoValue<bool>) -> SlotValue {
        SlotValue::Bool(v.into_value())
    }

    pub fn text(v: impl TextContent) -> SlotValue {
        SlotValue::Text(v.into_content_prop())
    }

    pub fn style(v: impl IntoStyleProp) -> SlotValue {
        SlotValue::Style(v.into_style_prop())
    }

    pub fn press(v: impl IntoAction) -> SlotValue {
        SlotValue::Press(v.into_action())
    }

    pub fn element(v: impl IntoElement) -> SlotValue {
        SlotValue::Element(v.into_element())
    }

    pub fn children(v: impl ChildList) -> SlotValue {
        let mut out = Vec::new();
        v.append_to(&mut out);
        SlotValue::Elements(out)
    }

    pub fn ctor(f: impl FnOnce(&[PropEntry], Vec<Element>) -> Element + 'static) -> SlotValue {
        SlotValue::Ctor(Box::new(f))
    }

    /// A static branch's arm chooser.
    pub fn selector(f: impl FnOnce() -> usize + 'static) -> SlotValue {
        SlotValue::Selector(Box::new(f))
    }

    /// One arm of a static branch, producing that arm's children.
    pub fn arm(f: impl FnOnce() -> Vec<Element> + 'static) -> SlotValue {
        SlotValue::Arm(Box::new(f))
    }

    pub fn cond(f: impl Fn() -> bool + 'static) -> SlotValue {
        SlotValue::Cond(Rc::new(f))
    }

    pub fn branch<E: IntoElement>(f: impl Fn() -> E + 'static) -> SlotValue {
        SlotValue::Branch(Rc::new(move || f().into_element()))
    }

    // --- the erased long tail -------------------------------------------
    //
    // One constructor per (kind, prop) TYPE, not per prop. Each exists to
    // give the author's expression an expected type at the call site;
    // `take_as` in the matching builder arm is the other end. The
    // emitter's `prim_prop_slot_ctor` table names these.

    /// A reactive `String` — `text_input`'s value, `image`'s `src`,
    /// `link`'s url.
    pub fn string_value(v: impl IntoValue<String>) -> SlotValue {
        SlotValue::Any(Box::new(v.into_value()))
    }

    /// A reactive `bool` — `presence`'s `present`, `text_input`'s
    /// `secure`.
    pub fn bool_value(v: impl IntoValue<bool>) -> SlotValue {
        SlotValue::Any(Box::new(v.into_value()))
    }

    /// A reactive `f32` — `icon`'s `stroke`.
    pub fn f32_value(v: impl IntoValue<f32>) -> SlotValue {
        SlotValue::Any(Box::new(v.into_value()))
    }

    /// A reactive `Color` — `icon`'s `color`.
    pub fn color_value(v: impl IntoValue<Color>) -> SlotValue {
        SlotValue::Any(Box::new(v.into_value()))
    }

    /// A plain `f32` — `slider`'s range/step, `anchored_overlay`'s
    /// `offset`, `scroll_view`'s `end_reached_threshold`.
    pub fn f32(v: f32) -> SlotValue {
        SlotValue::Any(Box::new(v))
    }

    /// A plain `bool` — `scroll_view`'s flags, `trap_focus`, `a11y_hidden`.
    pub fn plain_bool(v: bool) -> SlotValue {
        SlotValue::Any(Box::new(v))
    }

    /// A value the builder hands to a setter unchanged: an enum, a
    /// handle, an animation config. `T` is pinned by the setter's
    /// parameter type at the call site.
    pub fn plain<T: 'static>(v: T) -> SlotValue {
        SlotValue::Any(Box::new(v))
    }

    /// `text_input`'s `on_change`.
    pub fn on_string(f: impl Fn(String) + 'static) -> SlotValue {
        SlotValue::Any(Box::new(Rc::new(f) as Rc<dyn Fn(String)>))
    }

    /// `toggle`'s `on_change`, `text_input`'s `on_focus`.
    pub fn on_bool(f: impl Fn(bool) + 'static) -> SlotValue {
        SlotValue::Any(Box::new(Rc::new(f) as Rc<dyn Fn(bool)>))
    }

    /// `slider`'s `on_change`.
    pub fn on_f32(f: impl Fn(f32) + 'static) -> SlotValue {
        SlotValue::Any(Box::new(Rc::new(f) as Rc<dyn Fn(f32)>))
    }

    /// `on_scroll(offset_x, offset_y)`.
    pub fn on_scroll(f: impl Fn(f32, f32) + 'static) -> SlotValue {
        SlotValue::Any(Box::new(Rc::new(f) as Rc<dyn Fn(f32, f32)>))
    }

    /// A no-argument callback — `on_dismiss`, `on_end_reached`,
    /// `on_activate`, `on_error`.
    pub fn on_void(f: impl Fn() + 'static) -> SlotValue {
        SlotValue::Any(Box::new(Rc::new(f) as Rc<dyn Fn()>))
    }

    /// `graphics`' `on_ready`. `FnMut`, because the platform hands the
    /// surface back more than once.
    pub fn on_ready(f: impl FnMut(OnReadyEvent) + 'static) -> SlotValue {
        SlotValue::Any(Box::new(Box::new(f) as Box<dyn FnMut(OnReadyEvent)>))
    }

    /// `graphics`' `on_resize`.
    pub fn on_resize(f: impl FnMut(OnResizeEvent) + 'static) -> SlotValue {
        SlotValue::Any(Box::new(Box::new(f) as Box<dyn FnMut(OnResizeEvent)>))
    }

    /// `graphics`' `on_lost`.
    pub fn on_lost(f: impl FnMut() + 'static) -> SlotValue {
        SlotValue::Any(Box::new(Box::new(f) as Box<dyn FnMut()>))
    }

    /// `icon`'s `draw_in` — a `(duration_ms, easing)` tuple the builder
    /// spreads across the two-argument setter. Bound ONCE (the direct
    /// emitter used to splice it twice; see `emit_icon`).
    pub fn draw_in(v: (u32, Easing)) -> SlotValue {
        SlotValue::Any(Box::new(v))
    }
}

// ===========================================================================
// Entry points
// ===========================================================================

/// Build the site's `Element` from its compiled-in descriptor.
pub fn build(descriptor: &Descriptor, slots: &mut [SlotValue]) -> Element {
    build_from(&CompiledIn, descriptor, slots)
}

/// Build a scope that must yield a FLAT child list — an `if`/`match`
/// branch in children position, a `for` row builder.
///
/// Separate from [`build`] because the two differ in exactly the way
/// the direct lowering's `Ctx::Child` and `Ctx::Single` differ: a
/// children slot takes 0/1/N siblings, a single slot takes one
/// `Element` and wraps a multi-node body in a `view`.
pub fn build_list(descriptor: &Descriptor, slots: &mut [SlotValue]) -> Vec<Element> {
    let desc = CompiledIn.resolve(&descriptor.site, descriptor);
    debug_check(desc);
    let mut roots: Vec<Element> = Vec::with_capacity(desc.roots.len());
    for &root in desc.roots.iter() {
        append_node(desc, root, slots, &mut roots);
    }
    roots
}

/// Build through an explicit [`TemplateSource`]. `CompiledIn` is the
/// only implementation today; the parameter is the seam a
/// descriptor-from-elsewhere path would occupy.
pub fn build_from(
    source: &impl TemplateSource,
    descriptor: &Descriptor,
    slots: &mut [SlotValue],
) -> Element {
    let desc = source.resolve(&descriptor.site, descriptor);
    debug_check(desc);
    let mut roots: Vec<Element> = Vec::with_capacity(desc.roots.len());
    for &root in desc.roots.iter() {
        append_node(desc, root, slots, &mut roots);
    }
    // Mirror the direct lowering's top level exactly: a sole element is
    // itself, anything else (including nothing) is wrapped in a `view`.
    if roots.len() == 1 {
        roots.pop().expect("one root")
    } else {
        glue::view(roots).into_element()
    }
}

/// Check an emitted descriptor's internal consistency in DEBUG builds.
///
/// Every emitted descriptor passes through here, so the whole
/// `ui-lowering-parity` suite — and every debug-built app under the
/// template lowering — exercises the validator against real emission
/// output instead of only hand-written fixtures. Compiled out in
/// release.
#[inline]
fn debug_check(desc: &Descriptor) {
    #[cfg(debug_assertions)]
    if let Err(e) = runtime_template::check_well_formed(desc) {
        panic!("{}: malformed descriptor: {e}", desc.site);
    }
    #[cfg(not(debug_assertions))]
    let _ = desc;
}

// ===========================================================================
// Nodes
// ===========================================================================

fn append_node(desc: &Descriptor, index: u32, slots: &mut [SlotValue], out: &mut Vec<Element>) {
    let node = desc
        .node(index)
        .unwrap_or_else(|| panic!("{}: node {index} out of range", desc.site));
    match node {
        Node::Prim { kind, props, children } => {
            let kids = build_children(desc, children, slots);
            out.push(build_prim(desc, index, *kind, props, kids, slots));
        }
        Node::Component { ctor, literals, children, .. } => {
            let kids = build_children(desc, children, slots);
            let ctor = take_ctor(desc, *ctor, slots);
            out.push(ctor(literals, kids));
        }
        Node::Dyn { cond, then, otherwise } => {
            let cond = take_cond(desc, *cond, slots);
            let then = take_branch(desc, *then, slots);
            let otherwise = take_branch(desc, *otherwise, slots);
            out.push(glue::when(
                move || cond(),
                move || then(),
                move || otherwise(),
            ));
        }
        Node::Select { selector, arms } => {
            let choose = match take(desc, *selector, slots) {
                SlotValue::Selector(f) => f,
                other => panic!(
                    "{}: slot {selector} must be a selector, got `{}`",
                    desc.site,
                    other.shape()
                ),
            };
            let taken = choose();
            let slot = *arms.get(taken).unwrap_or_else(|| {
                panic!(
                    "{}: selector chose arm {taken} of {} — the emission and the \
                     descriptor disagree about the arm count",
                    desc.site,
                    arms.len()
                )
            });
            // Only the taken arm's thunk runs, so the untaken arms'
            // work — their slot preludes included — never happens, as
            // in the direct lowering. The rest are dropped unused.
            for (i, &a) in arms.iter().enumerate() {
                if i != taken {
                    let _ = take(desc, a, slots);
                }
            }
            match take(desc, slot, slots) {
                SlotValue::Arm(f) => out.append(&mut f()),
                other => panic!(
                    "{}: slot {slot} must be an arm, got `{}`",
                    desc.site,
                    other.shape()
                ),
            }
        }
        // A children-slot escape carries N elements and flattens, the
        // way `ChildList::append_to` does in the direct lowering; a
        // single-slot escape carries one.
        Node::Escape { slot } => match take(desc, *slot, slots) {
            SlotValue::Element(e) => out.push(e),
            SlotValue::Elements(mut v) => out.append(&mut v),
            other => panic!(
                "{}: escape slot {slot} must be an element or a child list, got `{}`",
                desc.site,
                other.shape()
            ),
        },
    }
}

fn build_children(desc: &Descriptor, children: &[u32], slots: &mut [SlotValue]) -> Vec<Element> {
    let mut out = Vec::with_capacity(children.len());
    for &child in children {
        append_node(desc, child, slots, &mut out);
    }
    out
}

// ===========================================================================
// Primitives
// ===========================================================================

/// Apply the props every glue wrapper shares. Returns `true` when the
/// prop was one of them, so the per-kind arm can fall through to its own
/// names.
///
/// Every one of these is applied by `emit_component` for the primitives
/// that HAVE the setters — the wrappers taking `glue_wrapper_common!`,
/// plus `presence`'s hand-rolled copy. `overlay` / `anchored_overlay`
/// have neither `test_id` nor the a11y setters, so the emitter does not
/// declare those props native for them (they do not compile under the
/// direct lowering either) and the arms below are never reached for
/// those two kinds.
///
/// `accessibility` / `a11y_role` / `a11y_traits` / `live_region` take an
/// enum or struct, which a descriptor records as source text
/// (`LiteralValue::Path`) and cannot rebuild — so they arrive through an
/// erased SLOT, and the enum itself stays compiled in.
macro_rules! common_prop {
    ($desc:expr, $node:expr, $w:expr, $entry:expr, $slots:expr) => {{
        let name: &str = &$entry.name;
        match name {
            "style" => {
                $w = $w.with_style(style_of($desc, $node, $entry, $slots));
                true
            }
            "test_id" => {
                $w = $w.test_id(static_str($desc, $node, $entry, $slots));
                true
            }
            "a11y_label" => {
                $w = $w.a11y_label(string_of($desc, $node, $entry, $slots));
                true
            }
            "a11y_hint" => {
                $w = $w.a11y_hint(string_of($desc, $node, $entry, $slots));
                true
            }
            "a11y_hidden" => {
                $w = $w.a11y_hidden(plain_flag($desc, $node, $entry, $slots));
                true
            }
            "accessibility" => {
                $w = $w.accessibility(erased($desc, $node, $entry, $slots));
                true
            }
            "a11y_role" => {
                $w = $w.a11y_role(erased($desc, $node, $entry, $slots));
                true
            }
            "a11y_traits" => {
                $w = $w.a11y_traits(erased($desc, $node, $entry, $slots));
                true
            }
            "live_region" => {
                $w = $w.live_region(erased($desc, $node, $entry, $slots));
                true
            }
            _ => false,
        }
    }};
}

fn build_prim(
    desc: &Descriptor,
    node: u32,
    kind: PrimKind,
    props: &[PropEntry],
    children: Vec<Element>,
    slots: &mut [SlotValue],
) -> Element {
    match kind {
        PrimKind::View => {
            let mut w = glue::view(children);
            for entry in props {
                if common_prop!(desc, node, w, entry, slots) {
                    continue;
                }
                unknown_prop(desc, node, kind, entry);
            }
            w.into_element()
        }
        PrimKind::Text => {
            // `content` is positional on the constructor, so it is read
            // before the chain rather than in prop order. Both
            // lowerings do this (`glue::text(content)` then setters), so
            // the ordering is shared, not a template quirk.
            let content = props
                .iter()
                .find(|p| p.name == "content")
                .map(|p| text_of(desc, node, p, slots))
                .unwrap_or_else(|| TextSourceProp::Value(Value::Const(String::new())));
            let mut w = glue::text(Prepared(content));
            for entry in props {
                if entry.name == "content" {
                    continue;
                }
                if common_prop!(desc, node, w, entry, slots) {
                    continue;
                }
                unknown_prop(desc, node, kind, entry);
            }
            w.into_element()
        }
        PrimKind::Button => {
            let label = props
                .iter()
                .find(|p| p.name == "label")
                .map(|p| text_of(desc, node, p, slots))
                .unwrap_or_else(|| TextSourceProp::Value(Value::Const(String::new())));
            let on_click = props
                .iter()
                .find(|p| p.name == "on_click")
                .map(|p| press_of(desc, node, p, slots))
                .unwrap_or_else(|| IntoAction::into_action(|| {}));
            let mut w = glue::button(Prepared(label), on_click);
            for entry in props {
                let name: &str = &entry.name;
                if name == "label" || name == "on_click" {
                    continue;
                }
                if name == "disabled" {
                    w = w.disabled(value_bool(desc, node, entry, slots));
                    continue;
                }
                if name == "leading_icon" {
                    w = w.leading_icon(erased(desc, node, entry, slots));
                    continue;
                }
                if name == "trailing_icon" {
                    w = w.trailing_icon(erased(desc, node, entry, slots));
                    continue;
                }
                if common_prop!(desc, node, w, entry, slots) {
                    continue;
                }
                unknown_prop(desc, node, kind, entry);
            }
            w.into_element()
        }
        PrimKind::Image => {
            let src = props
                .iter()
                .find(|p| p.name == "src")
                .map(|p| string_of(desc, node, p, slots))
                .unwrap_or_default();
            let mut w = glue::primitives::image::image(src);
            for entry in props {
                let name: &str = &entry.name;
                if name == "src" {
                    continue;
                }
                if name == "alt" {
                    w = w.alt(string_of(desc, node, entry, slots));
                    continue;
                }
                if common_prop!(desc, node, w, entry, slots) {
                    continue;
                }
                unknown_prop(desc, node, kind, entry);
            }
            w.into_element()
        }
        PrimKind::ActivityIndicator => {
            let mut w = glue::primitives::activity_indicator::activity_indicator();
            for entry in props {
                if common_prop!(desc, node, w, entry, slots) {
                    continue;
                }
                unknown_prop(desc, node, kind, entry);
            }
            w.into_element()
        }
        PrimKind::ScrollView => {
            let mut w = glue::primitives::scroll_view::scroll_view(children);
            for entry in props {
                let name: &str = &entry.name;
                match name {
                    "horizontal" => w = w.horizontal(plain_flag(desc, node, entry, slots)),
                    "bounces" => w = w.bounces(plain_flag(desc, node, entry, slots)),
                    "always_bounce" => {
                        w = w.always_bounce(plain_flag(desc, node, entry, slots))
                    }
                    "end_reached_threshold" => {
                        w = w.end_reached_threshold(plain_f32(desc, node, entry, slots))
                    }
                    "safe_area" => w = w.safe_area(erased(desc, node, entry, slots)),
                    "on_scroll" => {
                        let f = callback::<dyn Fn(f32, f32)>(desc, node, entry, slots);
                        w = w.on_scroll(move |x, y| f(x, y));
                    }
                    "on_end_reached" => {
                        let f = callback::<dyn Fn()>(desc, node, entry, slots);
                        w = w.on_end_reached(move || f());
                    }
                    _ if common_prop!(desc, node, w, entry, slots) => {}
                    _ => unknown_prop(desc, node, kind, entry),
                }
            }
            w.into_element()
        }

        PrimKind::Icon => {
            let data: IconData = props
                .iter()
                .find(|p| p.name == "data")
                .map(|p| erased(desc, node, p, slots))
                .unwrap_or_else(|| missing_positional(desc, node, kind, "data"));
            let mut w = glue::icon(data);
            for entry in props {
                let name: &str = &entry.name;
                match name {
                    "data" => {}
                    "color" => w = w.color(f32_or_color(desc, node, entry, slots)),
                    "stroke" => w = w.stroke(f32_value_of(desc, node, entry, slots)),
                    "animate" => w = w.animate(erased(desc, node, entry, slots)),
                    "draw_in" => {
                        // Bound once — the two-argument setter reads one
                        // value, never two evaluations (see `emit_icon`).
                        let (ms, easing): (u32, Easing) = erased(desc, node, entry, slots);
                        w = w.draw_in(ms, easing);
                    }
                    _ if common_prop!(desc, node, w, entry, slots) => {}
                    _ => unknown_prop(desc, node, kind, entry),
                }
            }
            w.into_element()
        }

        PrimKind::TextInput => {
            let value = positional_string(desc, node, props, "value", slots);
            let on_change = positional_cb::<dyn Fn(String)>(desc, node, props, "on_change", slots, || {
                Rc::new(|_: String| {})
            });
            let mut w = glue::primitives::text_input::text_input(value, move |s| on_change(s));
            for entry in props {
                let name: &str = &entry.name;
                match name {
                    "value" | "on_change" => {}
                    "placeholder" => {
                        w = w.placeholder(string_of(desc, node, entry, slots))
                    }
                    "secure" => w = w.secure(bool_value_of(desc, node, entry, slots)),
                    _ if common_prop!(desc, node, w, entry, slots) => {}
                    _ => unknown_prop(desc, node, kind, entry),
                }
            }
            w.into_element()
        }

        PrimKind::Toggle => {
            let value = positional_bool(desc, node, props, "value", slots);
            let on_change = positional_cb::<dyn Fn(bool)>(desc, node, props, "on_change", slots, || {
                Rc::new(|_: bool| {})
            });
            let mut w = glue::primitives::toggle::toggle(value, move |b| on_change(b));
            for entry in props {
                let name: &str = &entry.name;
                match name {
                    "value" | "on_change" => {}
                    _ if common_prop!(desc, node, w, entry, slots) => {}
                    _ => unknown_prop(desc, node, kind, entry),
                }
            }
            w.into_element()
        }

        PrimKind::Slider => {
            let value = positional_f32(desc, node, props, "value", slots);
            let on_change = positional_cb::<dyn Fn(f32)>(desc, node, props, "on_change", slots, || {
                Rc::new(|_: f32| {})
            });
            let mut w = glue::primitives::slider::slider(value, move |v| on_change(v));
            // `.range(min, max)` is ONE setter over two props, so it is
            // applied together or not at all — the direct emitter makes
            // the same choice (a lone `min` reaches nothing there too).
            let min = props.iter().find(|p| p.name == "min");
            let max = props.iter().find(|p| p.name == "max");
            if let (Some(min), Some(max)) = (min, max) {
                let a = plain_f32(desc, node, min, slots);
                let b = plain_f32(desc, node, max, slots);
                w = w.range(a, b);
            }
            for entry in props {
                let name: &str = &entry.name;
                match name {
                    "value" | "on_change" | "min" | "max" => {}
                    "step" => w = w.step(plain_f32(desc, node, entry, slots)),
                    _ if common_prop!(desc, node, w, entry, slots) => {}
                    _ => unknown_prop(desc, node, kind, entry),
                }
            }
            w.into_element()
        }

        PrimKind::Link => {
            let url = props
                .iter()
                .find(|p| p.name == "external")
                .map(|p| string_value(desc, node, p, slots))
                .unwrap_or_else(|| missing_positional(desc, node, kind, "external"));
            let mut w = glue::primitives::link::external_link(url, children);
            for entry in props {
                let name: &str = &entry.name;
                match name {
                    "external" => {}
                    _ if common_prop!(desc, node, w, entry, slots) => {}
                    _ => unknown_prop(desc, node, kind, entry),
                }
            }
            w.into_element()
        }

        PrimKind::Overlay => {
            let mut w = glue::primitives::overlay::overlay(children);
            for entry in props {
                let name: &str = &entry.name;
                match name {
                    "placement" => w = w.placement(erased(desc, node, entry, slots)),
                    "backdrop" => w = w.backdrop(erased(desc, node, entry, slots)),
                    "backdrop_style" => {
                        w = w.backdrop_style(style_of(desc, node, entry, slots))
                    }
                    "trap_focus" => w = w.trap_focus(plain_flag(desc, node, entry, slots)),
                    "on_dismiss" => {
                        let f = callback::<dyn Fn()>(desc, node, entry, slots);
                        w = w.on_dismiss(move || f());
                    }
                    "style" => w = w.with_style(style_of(desc, node, entry, slots)),
                    _ => unknown_prop(desc, node, kind, entry),
                }
            }
            w.into_element()
        }

        PrimKind::AnchoredOverlay => {
            let target: AnchorTarget = props
                .iter()
                .find(|p| p.name == "target")
                .map(|p| erased(desc, node, p, slots))
                .unwrap_or_else(|| missing_positional(desc, node, kind, "target"));
            let mut w = glue::primitives::overlay::anchored_overlay(target, children);
            for entry in props {
                let name: &str = &entry.name;
                match name {
                    "target" => {}
                    "side" => w = w.side(erased(desc, node, entry, slots)),
                    "align" => w = w.align(erased(desc, node, entry, slots)),
                    "offset" => w = w.offset(plain_f32(desc, node, entry, slots)),
                    "backdrop" => w = w.backdrop(erased(desc, node, entry, slots)),
                    "backdrop_style" => {
                        w = w.backdrop_style(style_of(desc, node, entry, slots))
                    }
                    "trap_focus" => w = w.trap_focus(plain_flag(desc, node, entry, slots)),
                    "on_dismiss" => {
                        let f = callback::<dyn Fn()>(desc, node, entry, slots);
                        w = w.on_dismiss(move || f());
                    }
                    "style" => w = w.with_style(style_of(desc, node, entry, slots)),
                    _ => unknown_prop(desc, node, kind, entry),
                }
            }
            w.into_element()
        }

        PrimKind::Presence => {
            // The child is a branch THUNK, not a realized child list:
            // `presence(move || child)` rebuilds it per mount, so it is
            // a nested template exactly like a `Dyn` branch.
            let child = props
                .iter()
                .find(|p| p.name == "child")
                .map(|p| match &p.value {
                    PropValue::Slot(i) => take_branch(desc, *i, slots),
                    PropValue::Lit(other) => {
                        mismatch(desc, node, p, "branch slot", other.kind())
                    }
                })
                .unwrap_or_else(|| missing_positional(desc, node, kind, "child"));
            let mut w = glue::primitives::presence::presence(move || child());
            for entry in props {
                let name: &str = &entry.name;
                match name {
                    "child" => {}
                    "present" => w = w.present(bool_value_of(desc, node, entry, slots)),
                    "enter" => w = w.enter(erased(desc, node, entry, slots)),
                    "exit" => w = w.exit(erased(desc, node, entry, slots)),
                    _ if common_prop!(desc, node, w, entry, slots) => {}
                    _ => unknown_prop(desc, node, kind, entry),
                }
            }
            w.into_element()
        }

        PrimKind::Graphics => {
            let mut on_ready: Box<dyn FnMut(OnReadyEvent)> = props
                .iter()
                .find(|p| p.name == "on_ready")
                .map(|p| erased::<Box<dyn FnMut(OnReadyEvent)>>(desc, node, p, slots))
                .unwrap_or_else(|| Box::new(|_| {}));
            let mut w = glue::primitives::graphics::graphics(move |e| on_ready(e));
            for entry in props {
                let name: &str = &entry.name;
                match name {
                    "on_ready" => {}
                    "on_resize" => {
                        let mut f: Box<dyn FnMut(OnResizeEvent)> =
                            erased(desc, node, entry, slots);
                        w = w.on_resize(move |e| f(e));
                    }
                    "on_lost" => {
                        let mut f: Box<dyn FnMut()> = erased(desc, node, entry, slots);
                        w = w.on_lost(move || f());
                    }
                    _ if common_prop!(desc, node, w, entry, slots) => {}
                    _ => unknown_prop(desc, node, kind, entry),
                }
            }
            w.into_element()
        }
    }
}

/// `icon`'s `color` takes `impl IntoValue<Color>`; the slot carries a
/// `Value<Color>` (which implements it).
fn f32_or_color(
    desc: &Descriptor,
    node: u32,
    entry: &PropEntry,
    slots: &mut [SlotValue],
) -> Value<Color> {
    erased(desc, node, entry, slots)
}

fn positional_string(
    desc: &Descriptor,
    node: u32,
    props: &[PropEntry],
    name: &str,
    slots: &mut [SlotValue],
) -> Value<String> {
    props
        .iter()
        .find(|p| p.name == name)
        .map(|p| string_value(desc, node, p, slots))
        .unwrap_or(Value::Const(String::new()))
}

fn positional_bool(
    desc: &Descriptor,
    node: u32,
    props: &[PropEntry],
    name: &str,
    slots: &mut [SlotValue],
) -> Value<bool> {
    props
        .iter()
        .find(|p| p.name == name)
        .map(|p| bool_value_of(desc, node, p, slots))
        .unwrap_or(Value::Const(false))
}

fn positional_f32(
    desc: &Descriptor,
    node: u32,
    props: &[PropEntry],
    name: &str,
    slots: &mut [SlotValue],
) -> Value<f32> {
    props
        .iter()
        .find(|p| p.name == name)
        .map(|p| f32_value_of(desc, node, p, slots))
        .unwrap_or(Value::Const(0.0))
}

/// A constructor-positional callback. Absent means "no-op", matching the
/// direct emitter's `|_| {}` defaults — `fallback` supplies that no-op,
/// because `Rc<dyn Fn(T)>` has no `Default`.
fn positional_cb<F: 'static + ?Sized>(
    desc: &Descriptor,
    node: u32,
    props: &[PropEntry],
    name: &str,
    slots: &mut [SlotValue],
    fallback: impl FnOnce() -> Rc<F>,
) -> Rc<F> {
    match props.iter().find(|p| p.name == name) {
        Some(p) => callback::<F>(desc, node, p, slots),
        None => fallback(),
    }
}

/// A positional the emitter must always supply. Reaching this means the
/// emitter declared a node native without its required prop.
fn missing_positional(desc: &Descriptor, node: u32, kind: PrimKind, name: &str) -> ! {
    panic!(
        "{}: node {node} ({}) is missing its required `{name}` prop — the emission \
         must escape a node it cannot supply one for",
        desc.site,
        kind.tag()
    )
}

/// A [`TextSourceProp`] the caller already assembled, handed back to a
/// builder's `impl TextContent` parameter unchanged.
///
/// Why not `Value<String>`: an f-string's slot arrives as an
/// `AssembledText` whose `into_content_prop` surfaces the pre-decomposed
/// `JsBinding` fast path. Going through `into_content` would flatten it
/// to a plain closure and the template lowering would emit a *different*
/// (slower, differently-shaped) text binding than the direct one.
struct Prepared(TextSourceProp);

impl TextContent for Prepared {
    fn into_content(self) -> Value<String> {
        match self.0 {
            TextSourceProp::Value(v) => v,
            // The two structured forms have no `Value<String>`
            // equivalent; a caller that reaches this asked a builder for
            // plain content while holding a binding, which the
            // template emission never does.
            other => {
                let _ = other;
                Value::Const(String::new())
            }
        }
    }

    fn into_content_prop(self) -> TextSourceProp {
        self.0
    }
}

// ===========================================================================
// Slot / literal readers
// ===========================================================================

fn take(desc: &Descriptor, index: u32, slots: &mut [SlotValue]) -> SlotValue {
    let slot = slots
        .get_mut(index as usize)
        .unwrap_or_else(|| panic!("{}: slot {index} out of range", desc.site));
    match std::mem::replace(slot, SlotValue::Taken) {
        SlotValue::Taken => panic!(
            "{}: slot {index} taken twice — a descriptor must reference each slot once",
            desc.site
        ),
        v => v,
    }
}

fn mismatch(
    desc: &Descriptor,
    node: u32,
    entry: &PropEntry,
    want: &str,
    got: &str,
) -> ! {
    panic!(
        "{}: node {node} prop `{}` needs a {want}, got {got}. This is a `ui!` \
         emission bug: the descriptor and the slot array disagree.",
        desc.site, entry.name
    )
}

fn string_of(desc: &Descriptor, node: u32, entry: &PropEntry, slots: &mut [SlotValue]) -> String {
    match &entry.value {
        PropValue::Lit(LiteralValue::Str(s)) => s.to_string(),
        PropValue::Lit(other) => mismatch(desc, node, entry, "string", other.kind()),
        PropValue::Slot(i) => match take(desc, *i, slots) {
            SlotValue::Str(s) => s,
            other => mismatch(desc, node, entry, "string", other.shape()),
        },
    }
}

/// `test_id` takes `&'static str` all the way down to the robot
/// registry. A compiled-in descriptor's literal already is one; an owned
/// one (deserialized) is leaked, which is bounded — a site's test id is
/// registered once and lives as long as the program.
fn static_str(
    desc: &Descriptor,
    node: u32,
    entry: &PropEntry,
    slots: &mut [SlotValue],
) -> &'static str {
    match &entry.value {
        PropValue::Lit(LiteralValue::Str(std::borrow::Cow::Borrowed(s))) => s,
        PropValue::Lit(LiteralValue::Str(std::borrow::Cow::Owned(s))) => {
            Box::leak(s.clone().into_boxed_str())
        }
        PropValue::Lit(other) => mismatch(desc, node, entry, "string literal", other.kind()),
        PropValue::Slot(i) => match take(desc, *i, slots) {
            SlotValue::Str(s) => Box::leak(s.into_boxed_str()),
            other => mismatch(desc, node, entry, "string", other.shape()),
        },
    }
}

fn value_bool(
    desc: &Descriptor,
    node: u32,
    entry: &PropEntry,
    slots: &mut [SlotValue],
) -> Value<bool> {
    match &entry.value {
        PropValue::Lit(LiteralValue::Bool(b)) => Value::Const(*b),
        PropValue::Lit(other) => mismatch(desc, node, entry, "bool", other.kind()),
        PropValue::Slot(i) => match take(desc, *i, slots) {
            SlotValue::Bool(v) => v,
            other => mismatch(desc, node, entry, "bool", other.shape()),
        },
    }
}

fn plain_bool(desc: &Descriptor, node: u32, entry: &PropEntry, slots: &mut [SlotValue]) -> bool {
    match value_bool(desc, node, entry, slots) {
        Value::Const(b) => b,
        // A setter that takes a plain `bool` cannot carry a reactive
        // value; the emission only routes literals here.
        Value::Dyn(f) => f(),
    }
}

fn text_of(
    desc: &Descriptor,
    node: u32,
    entry: &PropEntry,
    slots: &mut [SlotValue],
) -> TextSourceProp {
    match &entry.value {
        PropValue::Lit(LiteralValue::Str(s)) => TextSourceProp::Value(Value::Const(s.to_string())),
        PropValue::Lit(other) => mismatch(desc, node, entry, "text", other.kind()),
        PropValue::Slot(i) => match take(desc, *i, slots) {
            SlotValue::Text(t) => t,
            SlotValue::Str(s) => TextSourceProp::Value(Value::Const(s)),
            other => mismatch(desc, node, entry, "text", other.shape()),
        },
    }
}

fn style_of(desc: &Descriptor, node: u32, entry: &PropEntry, slots: &mut [SlotValue]) -> StyleProp {
    match &entry.value {
        PropValue::Slot(i) => match take(desc, *i, slots) {
            SlotValue::Style(s) => s,
            other => mismatch(desc, node, entry, "style", other.shape()),
        },
        // There is no literal spelling of a style: `stylesheet!`
        // applications and rules closures are always code.
        PropValue::Lit(other) => mismatch(desc, node, entry, "style slot", other.kind()),
    }
}

fn press_of(desc: &Descriptor, node: u32, entry: &PropEntry, slots: &mut [SlotValue]) -> Action {
    match &entry.value {
        PropValue::Slot(i) => match take(desc, *i, slots) {
            SlotValue::Press(a) => a,
            other => mismatch(desc, node, entry, "press action", other.shape()),
        },
        PropValue::Lit(other) => mismatch(desc, node, entry, "press slot", other.kind()),
    }
}

/// Read an erased slot for a prop that has no literal spelling (an
/// enum, a handle, a callback). A literal there is an emitter bug.
fn erased<T: 'static>(
    desc: &Descriptor,
    node: u32,
    entry: &PropEntry,
    slots: &mut [SlotValue],
) -> T {
    match &entry.value {
        PropValue::Slot(i) => take(desc, *i, slots).take_as::<T>(&entry.name),
        PropValue::Lit(other) => mismatch(desc, node, entry, "slot", other.kind()),
    }
}

/// Read a prop that is native BOTH as a literal and as a slot, where the
/// setter takes a plain (non-reactive) `f32`.
fn plain_f32(desc: &Descriptor, node: u32, entry: &PropEntry, slots: &mut [SlotValue]) -> f32 {
    match &entry.value {
        PropValue::Lit(LiteralValue::Float(f)) => *f as f32,
        PropValue::Lit(LiteralValue::Int(i)) => *i as f32,
        PropValue::Lit(other) => mismatch(desc, node, entry, "number", other.kind()),
        PropValue::Slot(i) => take(desc, *i, slots).take_as::<f32>(&entry.name),
    }
}

/// As [`plain_f32`], for a setter taking a plain `bool`.
fn plain_flag(desc: &Descriptor, node: u32, entry: &PropEntry, slots: &mut [SlotValue]) -> bool {
    match &entry.value {
        PropValue::Lit(LiteralValue::Bool(b)) => *b,
        PropValue::Lit(other) => mismatch(desc, node, entry, "bool", other.kind()),
        PropValue::Slot(i) => take(desc, *i, slots).take_as::<bool>(&entry.name),
    }
}

/// As [`plain_f32`], for a setter taking a reactive `Value<String>` —
/// a literal becomes `Value::Const`.
fn string_value(
    desc: &Descriptor,
    node: u32,
    entry: &PropEntry,
    slots: &mut [SlotValue],
) -> Value<String> {
    match &entry.value {
        PropValue::Lit(LiteralValue::Str(v)) => Value::Const(v.to_string()),
        PropValue::Lit(other) => mismatch(desc, node, entry, "string", other.kind()),
        PropValue::Slot(i) => take(desc, *i, slots).take_as::<Value<String>>(&entry.name),
    }
}

/// As [`string_value`], for a reactive `bool`.
fn bool_value_of(
    desc: &Descriptor,
    node: u32,
    entry: &PropEntry,
    slots: &mut [SlotValue],
) -> Value<bool> {
    match &entry.value {
        PropValue::Lit(LiteralValue::Bool(b)) => Value::Const(*b),
        PropValue::Lit(other) => mismatch(desc, node, entry, "bool", other.kind()),
        PropValue::Slot(i) => take(desc, *i, slots).take_as::<Value<bool>>(&entry.name),
    }
}

/// As [`string_value`], for a reactive `f32`.
fn f32_value_of(
    desc: &Descriptor,
    node: u32,
    entry: &PropEntry,
    slots: &mut [SlotValue],
) -> Value<f32> {
    match &entry.value {
        PropValue::Lit(LiteralValue::Float(f)) => Value::Const(*f as f32),
        PropValue::Lit(LiteralValue::Int(i)) => Value::Const(*i as f32),
        PropValue::Lit(other) => mismatch(desc, node, entry, "number", other.kind()),
        PropValue::Slot(i) => take(desc, *i, slots).take_as::<Value<f32>>(&entry.name),
    }
}

/// A `Fn`-shaped callback slot, cloned out of its `Rc` so the builder can
/// hand the setter an owning closure.
fn callback<F: 'static + ?Sized>(
    desc: &Descriptor,
    node: u32,
    entry: &PropEntry,
    slots: &mut [SlotValue],
) -> Rc<F> {
    match &entry.value {
        PropValue::Slot(i) => take(desc, *i, slots).take_as::<Rc<F>>(&entry.name),
        PropValue::Lit(other) => mismatch(desc, node, entry, "callback slot", other.kind()),
    }
}

fn take_ctor(desc: &Descriptor, index: u32, slots: &mut [SlotValue]) -> ComponentCtor {
    match take(desc, index, slots) {
        SlotValue::Ctor(c) => c,
        other => panic!(
            "{}: slot {index} must be a component constructor, got `{}`",
            desc.site,
            other.shape()
        ),
    }
}

fn take_cond(desc: &Descriptor, index: u32, slots: &mut [SlotValue]) -> Rc<dyn Fn() -> bool> {
    match take(desc, index, slots) {
        SlotValue::Cond(c) => c,
        other => panic!(
            "{}: slot {index} must be a condition, got `{}`",
            desc.site,
            other.shape()
        ),
    }
}

fn take_branch(desc: &Descriptor, index: u32, slots: &mut [SlotValue]) -> Rc<dyn Fn() -> Element> {
    match take(desc, index, slots) {
        SlotValue::Branch(b) => b,
        other => panic!(
            "{}: slot {index} must be a branch thunk, got `{}`",
            desc.site,
            other.shape()
        ),
    }
}

fn unknown_prop(desc: &Descriptor, node: u32, kind: PrimKind, entry: &PropEntry) -> ! {
    panic!(
        "{}: node {node} ({}) has no descriptor-native prop `{}`. The emission must \
         escape a node whose props it cannot model.",
        desc.site,
        kind.tag(),
        entry.name
    )
}

// ===========================================================================
// Literal application on component props
// ===========================================================================

/// The fallback half of `#[component]`'s generated
/// `__apply_literal(&mut Props, name, value) -> bool`.
///
/// A blanket impl so a component-props type that does NOT have the
/// generated inherent method still compiles at the call site. Rust
/// resolves inherent methods before trait methods, so a `#[component]`
/// / `#[props]` struct uses its own generated one and everything else
/// silently answers "not applied" — which is the contract: a component
/// without the macro is slot-only under the template lowering, never an
/// error.
pub trait ApplyLiteralFallback {
    /// Returns whether the literal was applied.
    fn __apply_literal(&mut self, _name: &str, _value: &LiteralValue) -> bool {
        false
    }
}

impl<T> ApplyLiteralFallback for T {}

/// Re-exported so the emission can name the descriptor types without
/// the consumer crate depending on `runtime-template` directly.
pub use runtime_template::{
    check_well_formed, validate, Descriptor as TemplateDescriptor, LiteralValue as TemplateLiteral,
    Node as TemplateNode, Patch as TemplatePatch, PrimKind as TemplatePrimKind,
    PropEntry as TemplatePropEntry, PropValue as TemplatePropValue, Registry as TemplateRegistry,
    SiteId as TemplateSiteId, SlotInfo as TemplateSlotInfo, SlotSig as TemplateSlotSig,
};
