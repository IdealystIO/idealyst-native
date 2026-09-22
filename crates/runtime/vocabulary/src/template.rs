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

use std::rc::Rc;

use runtime_scene::Element;
use runtime_shared::{Action, IntoAction};
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
/// `__apply_literal(&mut Props, name, value) -> bool`, moves the built
/// children in, and calls `BuildElement::build`.
///
/// `Rc<dyn Fn>` rather than `Box<dyn FnOnce>` so a nested template's
/// branch thunk can hold one across re-invocations.
pub type ComponentCtor = Rc<dyn Fn(&[PropEntry], Vec<Element>) -> Element>;

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

    pub fn ctor(f: impl Fn(&[PropEntry], Vec<Element>) -> Element + 'static) -> SlotValue {
        SlotValue::Ctor(Rc::new(f))
    }

    pub fn cond(f: impl Fn() -> bool + 'static) -> SlotValue {
        SlotValue::Cond(Rc::new(f))
    }

    pub fn branch<E: IntoElement>(f: impl Fn() -> E + 'static) -> SlotValue {
        SlotValue::Branch(Rc::new(move || f().into_element()))
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
/// `a11y_role` / `a11y_traits` / `live_region` / `accessibility` are
/// absent on purpose: each takes an ENUM or struct value, which a
/// descriptor records as source text (`LiteralValue::Path`) and cannot
/// reconstruct. The emission escapes a node carrying one rather than
/// half-applying it.
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
                $w = $w.a11y_hidden(plain_bool($desc, $node, $entry, $slots));
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
                if name == "horizontal" {
                    w = w.horizontal(plain_bool(desc, node, entry, slots));
                    continue;
                }
                if name == "bounces" {
                    w = w.bounces(plain_bool(desc, node, entry, slots));
                    continue;
                }
                if common_prop!(desc, node, w, entry, slots) {
                    continue;
                }
                unknown_prop(desc, node, kind, entry);
            }
            w.into_element()
        }
    }
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
