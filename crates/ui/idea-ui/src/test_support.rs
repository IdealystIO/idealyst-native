//! Core-agnostic BUILD-TREE introspection for idea-ui's (and
//! idea-ui-nav's) unit tests.
//!
//! The old core's `Element` is a public enum the tests pattern-matched
//! directly (`Element::View { children, style, .. }`). The new core's
//! `Element::Item { data: Box<dyn Any>, .. }` erases the payload, so
//! direct matching is impossible — and the two shapes can't be unified
//! by the `runtime_facade` alias (pattern syntax binds to the concrete
//! enum). This module is the shared assertion surface instead:
//! [`classify`] maps ONE element to the normalized [`P`] mirror, with a
//! per-core backend (the sanctioned fork location — everything else in
//! the tests stays same-source).
//!
//! Normalizations (identical on both cores):
//! - reactive props are EVALUATED to their current value (`disabled`,
//!   icon `color`, text-input `secure`) — assertions compare values,
//!   not reactive wrappers;
//! - handler presence collapses to `bool` (`on_touch`, `on_hover`,
//!   `ref_fill`) — tests only ever asserted `is_some()`;
//! - styles normalize to [`TStyle`] (sheet application vs raw rules,
//!   static vs closure), resolvable via [`TStyle::resolve`].
//!
//! Hidden from docs: test support, not component surface.

use std::rc::Rc;

use runtime_core::accessibility::AccessibilityProps;
use runtime_core::primitives::portal::PortalTarget;
use runtime_core::{resolve_style, Color, Element, IconData, StyleApplication, StyleRules};

/// Normalized style slot — see module docs.
pub enum TStyle {
    /// A static sheet application (`stylesheet!` builder output).
    App(StyleApplication),
    /// A reactive sheet application.
    AppFn(Box<dyn Fn() -> StyleApplication>),
    /// Pre-resolved rules (no sheet).
    Rules(Rc<StyleRules>),
    /// A reactive resolved-rules closure.
    RulesFn(Box<dyn Fn() -> Rc<StyleRules>>),
}

impl TStyle {
    /// Resolve to concrete rules (through the shared sheet engine for
    /// the application variants).
    pub fn resolve(&self) -> Rc<StyleRules> {
        match self {
            TStyle::App(app) => resolve_style(app),
            TStyle::AppFn(f) => resolve_style(&f()),
            TStyle::Rules(r) => r.clone(),
            TStyle::RulesFn(f) => f(),
        }
    }

    /// The sheet application (static or evaluated). Panics for raw-rules
    /// styles — a test reaching for variants/sheet data on a rules-only
    /// style is asserting the wrong thing.
    pub fn application(&self) -> StyleApplication {
        match self {
            TStyle::App(app) => app.clone(),
            TStyle::AppFn(f) => f(),
            TStyle::Rules(_) | TStyle::RulesFn(_) => {
                panic!("test_support: style is raw rules, not a sheet application")
            }
        }
    }

    /// Whether this is one of the REACTIVE variants.
    pub fn is_reactive(&self) -> bool {
        matches!(self, TStyle::AppFn(_) | TStyle::RulesFn(_))
    }
}

/// The normalized primitive mirror (field names follow the old enum).
pub enum P {
    View {
        children: Vec<Element>,
        style: Option<TStyle>,
        preserves_focus: bool,
        on_touch: bool,
        on_hover: bool,
        ref_fill: bool,
        accessibility: AccessibilityProps,
    },
    Text {
        /// Current content when statically knowable (`None` for bound /
        /// styled-runs sources).
        text: Option<String>,
        style: Option<TStyle>,
        accessibility: AccessibilityProps,
    },
    Pressable {
        children: Vec<Element>,
        on_click: Rc<dyn Fn()>,
        style: Option<TStyle>,
        /// Evaluated current value.
        disabled: Option<bool>,
        preserves_focus: bool,
        ref_fill: bool,
        accessibility: AccessibilityProps,
    },
    Icon {
        data: IconData,
        /// Evaluated current value.
        color: Option<Color>,
        style: Option<TStyle>,
    },
    TextInput {
        /// Current controlled value.
        value: String,
        on_change: Rc<dyn Fn(String)>,
        on_focus: bool,
        /// Evaluated current value.
        secure: bool,
        style: Option<TStyle>,
    },
    TextArea {
        min_rows: Option<u32>,
        max_rows: Option<u32>,
        style: Option<TStyle>,
    },
    ScrollView {
        children: Vec<Element>,
        style: Option<TStyle>,
    },
    ActivityIndicator,
    Portal {
        children: Vec<Element>,
        target: PortalTarget,
        style: Option<TStyle>,
    },
    Fragment {
        children: Vec<Element>,
    },
    /// Anything the mirror doesn't model (reactive holes, keyed lists,
    /// external, …) — carries a diagnostic kind name.
    Other(&'static str),
}

impl P {
    /// Children for the container-shaped kinds; panics otherwise (use in
    /// tests that KNOW the shape).
    pub fn children(self) -> Vec<Element> {
        match self {
            P::View { children, .. }
            | P::Pressable { children, .. }
            | P::ScrollView { children, .. }
            | P::Portal { children, .. }
            | P::Fragment { children } => children,
            _ => panic!("test_support: node kind carries no children"),
        }
    }

    /// The style slot for the styled kinds; panics for kinds without one.
    pub fn style(self) -> Option<TStyle> {
        match self {
            P::View { style, .. }
            | P::Text { style, .. }
            | P::Pressable { style, .. }
            | P::Icon { style, .. }
            | P::TextInput { style, .. }
            | P::TextArea { style, .. }
            | P::ScrollView { style, .. }
            | P::Portal { style, .. } => style,
            _ => panic!("test_support: node kind carries no style slot"),
        }
    }
}

pub use imp::classify;

#[cfg(not(feature = "new-core"))]
mod imp {
    use super::*;
    use runtime_core::StyleSource;

    fn style(src: Option<StyleSource>) -> Option<TStyle> {
        src.map(|s| match s {
            StyleSource::Static(app) => TStyle::App(app),
            StyleSource::Reactive(f) => TStyle::AppFn(f),
            _ => panic!("test_support: style source kind not modeled by the test mirror"),
        })
    }

    /// Old-core backend: a thin field mapping off the public enum.
    pub fn classify(el: Element) -> P {
        match el {
            Element::View {
                children,
                style: st,
                ref_fill,
                on_touch,
                on_hover,
                preserves_focus,
                accessibility,
                ..
            } => P::View {
                children,
                style: style(st),
                preserves_focus,
                on_touch: on_touch.is_some(),
                on_hover: on_hover.is_some(),
                ref_fill: ref_fill.is_some(),
                accessibility,
            },
            Element::Text { source, style: st, accessibility, .. } => P::Text {
                text: match source {
                    runtime_core::TextSource::Static(s) => Some(s),
                    _ => None,
                },
                style: style(st),
                accessibility,
            },
            Element::Pressable {
                children,
                on_click,
                style: st,
                ref_fill,
                disabled,
                preserves_focus,
                accessibility,
                ..
            } => P::Pressable {
                children,
                on_click,
                style: style(st),
                disabled: disabled.map(|f| f()),
                preserves_focus,
                ref_fill: ref_fill.is_some(),
                accessibility,
            },
            Element::Icon { data, color, style: st, .. } => P::Icon {
                data,
                color: color.map(|f| f()),
                style: style(st),
            },
            Element::TextInput {
                value, on_change, on_focus, secure, style: st, ..
            } => P::TextInput {
                value: value.get(),
                on_change,
                on_focus: on_focus.is_some(),
                secure: secure.get(),
                style: style(st),
            },
            Element::TextArea { min_rows, max_rows, style: st, .. } => {
                P::TextArea { min_rows, max_rows, style: style(st) }
            }
            Element::ScrollView { children, style: st, .. } => {
                P::ScrollView { children, style: style(st) }
            }
            Element::ActivityIndicator { .. } => P::ActivityIndicator,
            Element::Portal { children, target, style: st, .. } => {
                P::Portal { children, target, style: style(st) }
            }
            Element::Fragment { children } => P::Fragment { children },
            Element::When { .. } => P::Other("when"),
            Element::Each { .. } => P::Other("each"),
            Element::External { .. } => P::Other("external"),
            _ => P::Other("unmodeled"),
        }
    }
}

#[cfg(feature = "new-core")]
mod imp {
    use super::*;
    use runtime_core::{Owned, __Value as Value};
    use runtime_vocabulary::prims::{self, PrimCell};
    use std::cell::RefCell;

    thread_local! {
        /// Peeled component `Owned` scopes, retained so reactive style
        /// closures extracted from a classified subtree can still read
        /// the component body's signals. Entries become inert when the
        /// test's world drops (`Owned::drop` skips dead arenas), so the
        /// accumulation is bounded by the test process.
        static KEEPALIVE: RefCell<Vec<Owned>> = const { RefCell::new(Vec::new()) };
    }

    fn style(src: Option<runtime_vocabulary::StyleProp>) -> Option<TStyle> {
        use runtime_vocabulary::StyleProp as SP;
        src.map(|s| match s {
            SP::Sheet(app) => TStyle::App(*app),
            SP::SheetDynamic(f) => TStyle::AppFn(f),
            SP::Static(rules) => TStyle::Rules(rules),
            SP::Dynamic(f) => TStyle::RulesFn(f),
            _ => panic!("test_support: style prop kind not modeled by the test mirror"),
        })
    }

    fn eval<T: 'static>(v: Value<T>) -> T {
        match v {
            Value::Const(t) => t,
            Value::Dyn(f) => f(),
        }
    }

    /// New-core backend: peel `Owned`/single-`Fragment` wrappers, then
    /// downcast the erased `Item` payload against each modeled prim.
    pub fn classify(el: Element) -> P {
        match el {
            Element::Owned { element, owned } => {
                KEEPALIVE.with(|k| k.borrow_mut().push(owned));
                classify(*element)
            }
            Element::Fragment(children) => P::Fragment { children },
            Element::Dyn(_) => P::Other("dyn"),
            Element::Keyed { .. } => P::Other("keyed"),
            Element::Many { .. } => P::Other("many"),
            Element::Item { data, children } => classify_item(data, children),
            _ => P::Other("unmodeled"),
        }
    }

    fn classify_item(data: Box<dyn std::any::Any>, children: Vec<Element>) -> P {
        if let Some(cell) = data.downcast_ref::<PrimCell<prims::ViewPrim>>() {
            let p = cell.take();
            return P::View {
                children,
                style: style(p.style),
                preserves_focus: p.preserves_focus,
                on_touch: p.on_touch.is_some(),
                on_hover: p.on_hover.is_some(),
                ref_fill: p.ref_fill.is_some(),
                accessibility: p.a11y,
            };
        }
        if let Some(cell) = data.downcast_ref::<PrimCell<prims::TextPrim>>() {
            let p = cell.take();
            return P::Text {
                text: match p.content {
                    prims::TextSourceProp::Value(Value::Const(s)) => Some(s),
                    _ => None,
                },
                style: style(p.style),
                accessibility: p.a11y,
            };
        }
        if let Some(cell) = data.downcast_ref::<PrimCell<prims::PressablePrim>>() {
            let p = cell.take();
            return P::Pressable {
                children,
                on_click: p.on_press,
                style: style(p.style),
                disabled: p.disabled.map(eval),
                preserves_focus: p.preserves_focus,
                ref_fill: p.ref_fill.is_some(),
                accessibility: p.a11y,
            };
        }
        if let Some(cell) = data.downcast_ref::<PrimCell<prims::IconPrim>>() {
            let p = cell.take();
            return P::Icon {
                data: eval(p.data),
                color: p.color.map(eval),
                style: style(p.style),
            };
        }
        if let Some(cell) = data.downcast_ref::<PrimCell<prims::TextInputPrim>>() {
            let p = cell.take();
            return P::TextInput {
                value: eval(p.value),
                on_change: p.on_change,
                on_focus: p.on_focus.is_some(),
                secure: eval(p.secure),
                style: style(p.style),
            };
        }
        if let Some(cell) = data.downcast_ref::<PrimCell<prims::TextAreaPrim>>() {
            let p = cell.take();
            return P::TextArea {
                min_rows: p.min_rows,
                max_rows: p.max_rows,
                style: style(p.style),
            };
        }
        if let Some(cell) = data.downcast_ref::<PrimCell<prims::ScrollViewPrim>>() {
            let p = cell.take();
            return P::ScrollView { children, style: style(p.style) };
        }
        if let Some(cell) = data.downcast_ref::<PrimCell<prims::ActivityIndicatorPrim>>() {
            let _ = cell.take();
            return P::ActivityIndicator;
        }
        if let Some(cell) = data.downcast_ref::<PrimCell<prims::PortalPrim>>() {
            let p = cell.take();
            return P::Portal { children, target: p.target, style: style(p.style) };
        }
        P::Other("unmodeled-item")
    }
}
