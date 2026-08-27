//! BUILD-TREE introspection for idea-ui's (and idea-ui-nav's) unit
//! tests.
//!
//! `Element::Item { data: Box<dyn Any>, .. }` erases the primitive
//! payload, so a test can't pattern-match a built tree directly. This
//! module is the assertion surface instead: [`classify`] downcasts ONE
//! element's payload against each modeled primitive and maps it to the
//! normalized [`P`] mirror.
//!
//! Normalizations:
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
    /// A build-time-minted class stamp (premint-cfg builds only). Carries
    /// no resolvable rules — assertions compare the class list and the
    /// optional inline layer.
    Preminted {
        class: String,
        inline: Option<Rc<StyleRules>>,
    },
    /// The reactive preminted counterpart — evaluate for the current
    /// class list.
    PremintedFn(Box<dyn Fn() -> String>),
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
            TStyle::Preminted { .. } | TStyle::PremintedFn(_) => panic!(
                "test_support: a preminted class stamp carries no resolvable \
                 rules — assert on the class list instead"
            ),
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
            TStyle::Preminted { .. } | TStyle::PremintedFn(_) => {
                panic!("test_support: style is a preminted class stamp, not a sheet application")
            }
        }
    }

    /// Whether this is one of the REACTIVE variants.
    pub fn is_reactive(&self) -> bool {
        matches!(self, TStyle::AppFn(_) | TStyle::RulesFn(_) | TStyle::PremintedFn(_))
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
    ActivityIndicator {
        /// The explicit accent, `None` = the backend default (web:
        /// `currentColor`).
        color: Option<Color>,
    },
    Portal {
        children: Vec<Element>,
        target: PortalTarget,
        /// Whether the portal installs a focus trap. Modelled because a
        /// trap on the WRONG portal is invisible in the tree shape and
        /// very visible to a user: an empty trapping portal bounces focus
        /// out of every sibling portal (see
        /// `popover::dismiss_catcher`'s note).
        trap_focus: bool,
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
            SP::Preminted { class, inline, .. } => {
                TStyle::Preminted { class: class.into_owned(), inline }
            }
            SP::PremintedDynamic { class_of, .. } => TStyle::PremintedFn(class_of),
            _ => panic!("test_support: style prop kind not modeled by the test mirror"),
        })
    }

    fn eval<T: 'static>(v: Value<T>) -> T {
        match v {
            Value::Const(t) => t,
            Value::Dyn(f) => f(),
        }
    }

    /// Peel `Owned`/single-`Fragment` wrappers, then downcast the erased
    /// `Item` payload against each modeled prim.
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
            let p = cell.take();
            return P::ActivityIndicator { color: p.color };
        }
        if let Some(cell) = data.downcast_ref::<PrimCell<prims::PortalPrim>>() {
            let p = cell.take();
            return P::Portal {
                children,
                target: p.target,
                trap_focus: p.trap_focus,
                style: style(p.style),
            };
        }
        P::Other("unmodeled-item")
    }
}
