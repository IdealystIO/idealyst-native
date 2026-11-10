//! Component previews: live props controls for generated documentation.
//!
//! Compiled only with the `previews` feature, which nothing but the
//! `idealyst docs` build turns on. With it off this module, the
//! `#[prop_control]` impls and every macro emission that references it are
//! absent, so dev and release builds carry none of it.
//!
//! ## The pieces
//!
//! - [`PropControl`] — how one prop *type* is edited: a [`Control`]
//!   description plus a round-trip through [`ControlValue`]. Implemented
//!   here for the std scalars, `Option<T>`, [`Reactive<T>`] and the signal
//!   handles; a crate implements it for its own types inside
//!   `#[prop_control]`, which removes the impl when previews are off.
//! - [`PropControl::example`] is a type's representative value (a sample
//!   image, the first registered icon): what an optional prop starts at
//!   when switched on. A component's own starting values are
//!   `#[prop(example = …)]`, applied by [`PreviewProps::preview_base`].
//! - [`PropControl::code`] is how a value is written in a `ui!` call —
//!   what the docs page's Usage snippet shows for the current controls.
//! - Callbacks (`Rc<dyn Fn()>`, one or two arguments) get
//!   [`Control::Handler`]: an optional callback prop can be attached (a
//!   no-op handler) or left out, which changes what a component renders (a
//!   close button, a pressable row).
//! - A type with no `PropControl` impl that is a [`VariantEnum`] (every
//!   `stylesheet!` / `component_styles!` axis) gets a [`Control::Choice`]
//!   over its built-in values. Anything else has no control and keeps the
//!   value its props struct defaults to.
//! - `#[props]` and inline-props `#[component]` implement [`PreviewProps`]
//!   for the props struct: one [`PreviewField`] per controllable field.
//! - `#[component]` exports one `#[no_mangle]` function per component
//!   returning its [`Preview`], and registers the symbol with the catalog
//!   (a `PreviewEntry`, written as the component's `"preview"` key in the
//!   catalog document). The docs app declares those symbols in an `extern` block
//!   generated from the catalog. That is what gets a callable for every
//!   component into the wasm bundle: inventory registrations are pruned
//!   there, and a path to a component in a private module can't be named
//!   from outside its crate, but an exported symbol can always be linked.
//!
//! Controls are plain data on purpose: the trait depends on no UI library,
//! and the docs app decides how a `Text` or a `Choice` is drawn.

use std::marker::PhantomData;
use std::rc::Rc;

use runtime_scene::Element;
use runtime_shared::VariantEnum;
use runtime_world::{signal, untrack, ReadSignal, Signal};

use crate::glue::{BuildElement, Reactive};

// =============================================================================
// Controls
// =============================================================================

/// How a prop is edited.
#[derive(Clone, Debug, PartialEq)]
pub enum Control {
    /// A single-line text field. Value: [`ControlValue::Text`].
    Text,
    /// An on/off switch. Value: [`ControlValue::Bool`].
    Bool,
    /// A number field; a slider when both bounds are set. Value:
    /// [`ControlValue::Number`].
    Number {
        min: Option<f64>,
        max: Option<f64>,
        step: f64,
        /// Whole numbers only.
        integer: bool,
    },
    /// One of a fixed set of keys. Value: [`ControlValue::Choice`] holding
    /// one of these keys.
    Choice(Vec<String>),
    /// A value that may be absent: a switch, plus the inner control while
    /// on. Value: [`ControlValue::Optional`]. `example` is the inner value
    /// switching it on starts from (the inner type's
    /// [`PropControl::example`]); `None` starts from an empty one.
    Optional { inner: Box<Control>, example: Option<Box<ControlValue>> },
    /// A composite edited part by part, each part a labelled control.
    /// Value: [`ControlValue::Group`], one value per part in this order.
    Group(Vec<(String, Control)>),
    /// A callback. Only meaningful inside [`Control::Optional`], where it
    /// means "attach a handler or not"; the preview attaches a no-op.
    /// Value: [`ControlValue::Handler`].
    Handler,
}

/// The value a [`Control`] holds. Each variant pairs with the `Control`
/// variant of the same name.
#[derive(Clone, Debug, PartialEq)]
pub enum ControlValue {
    Text(String),
    Bool(bool),
    Number(f64),
    Choice(String),
    Optional(Option<Box<ControlValue>>),
    Group(Vec<ControlValue>),
    Handler,
}

/// How a prop type is edited in a component preview.
///
/// Implement it for your own types inside `#[prop_control]`, which keeps
/// the impl out of every build that isn't generating docs:
///
/// ```ignore
/// #[runtime_core::prop_control]
/// impl runtime_core::previews::PropControl for Rating {
///     fn control() -> Control {
///         Control::Number { min: Some(0.0), max: Some(5.0), step: 1.0, integer: true }
///     }
///     fn to_value(&self) -> ControlValue {
///         ControlValue::Number(self.0 as f64)
///     }
///     fn from_value(value: &ControlValue) -> Option<Self> {
///         match value {
///             ControlValue::Number(n) => Some(Rating(*n as u8)),
///             _ => None,
///         }
///     }
/// }
/// ```
pub trait PropControl: Sized {
    /// The control that edits this type.
    fn control() -> Control;
    /// This value as the control's value.
    fn to_value(&self) -> ControlValue;
    /// Build a value from the control's value. `None` when `value` is not
    /// the variant [`control`](Self::control) produces; the preview then
    /// keeps the prop's previous value.
    fn from_value(value: &ControlValue) -> Option<Self>;
    /// A representative value of this type, for when a preview needs one
    /// out of nothing — today, the value an optional prop of this type
    /// starts at when its "Some" box is ticked (a sample image rather
    /// than an empty URL). What a particular component's prop should
    /// start at is the component's call: `#[prop(example = …)]`.
    fn example() -> Option<Self> {
        None
    }
    /// [`example`](Self::example) as the control's value — what an
    /// optional prop's editor shows when switched on. Override only when
    /// `to_value` can't read the example back (an element's text).
    fn example_value() -> Option<ControlValue> {
        Self::example().map(|v| v.to_value())
    }
    /// This value as a `ui!` prop value (`"Save"`, `tone::Danger`,
    /// `Some(3)`) — what the docs page's Usage snippet shows. `None` when
    /// the type can't spell it; the snippet then marks the prop as set
    /// without a value.
    fn code(&self) -> Option<String> {
        None
    }
    /// The control's `value` as `ui!` prop code — what the Usage snippet
    /// calls. Defaults to [`from_value`](Self::from_value) then
    /// [`code`](Self::code); override only when the built value can't say
    /// what it was made from (an element's text).
    fn value_code(value: &ControlValue) -> Option<String> {
        Self::from_value(value)?.code()
    }
    /// The paths a [`value_code`](Self::value_code) spelling of `value`
    /// needs in scope (`"std::rc::Rc"`, `"idea_theme::extensible::tone"`),
    /// each the item's defining path — the docs page rewrites a path to a
    /// shorter re-export where the component's crate has one. Empty
    /// (default) when the code is fully qualified or names nothing.
    fn imports(value: &ControlValue) -> Vec<String> {
        let _ = value;
        Vec::new()
    }
}

impl PropControl for String {
    fn control() -> Control {
        Control::Text
    }
    fn to_value(&self) -> ControlValue {
        ControlValue::Text(self.clone())
    }
    fn from_value(value: &ControlValue) -> Option<Self> {
        match value {
            ControlValue::Text(s) => Some(s.clone()),
            _ => None,
        }
    }
    fn code(&self) -> Option<String> {
        Some(format!("{self:?}"))
    }
}

impl PropControl for bool {
    fn control() -> Control {
        Control::Bool
    }
    fn to_value(&self) -> ControlValue {
        ControlValue::Bool(*self)
    }
    fn from_value(value: &ControlValue) -> Option<Self> {
        match value {
            ControlValue::Bool(b) => Some(*b),
            _ => None,
        }
    }
    fn code(&self) -> Option<String> {
        Some(self.to_string())
    }
}

/// Integers: bounded by the type when the whole range fits a slider's
/// `f64` exactly (8- and 16-bit), unbounded otherwise. Out-of-range input
/// saturates (`as` casts from `f64` saturate and drop the fraction).
macro_rules! integer_controls {
    ($($ty:ty => $bounded:expr),* $(,)?) => {$(
        impl PropControl for $ty {
            fn control() -> Control {
                let (min, max) = if $bounded {
                    (Some(<$ty>::MIN as f64), Some(<$ty>::MAX as f64))
                } else {
                    (None, None)
                };
                Control::Number { min, max, step: 1.0, integer: true }
            }
            fn to_value(&self) -> ControlValue {
                ControlValue::Number(*self as f64)
            }
            fn from_value(value: &ControlValue) -> Option<Self> {
                match value {
                    ControlValue::Number(n) => Some(n.round() as $ty),
                    _ => None,
                }
            }
            fn code(&self) -> Option<String> {
                Some(self.to_string())
            }
        }
    )*};
}
integer_controls!(
    i8 => true, u8 => true, i16 => true, u16 => true,
    i32 => false, u32 => false, i64 => false, u64 => false,
    isize => false, usize => false,
);

macro_rules! float_controls {
    ($($ty:ty),*) => {$(
        impl PropControl for $ty {
            fn control() -> Control {
                Control::Number { min: None, max: None, step: 0.1, integer: false }
            }
            /// Through the shortest decimal that reads back as this value,
            /// not a widening cast: `0.6f32 as f64` is `0.6000000238418579`,
            /// which is what the number field would show.
            fn to_value(&self) -> ControlValue {
                ControlValue::Number(self.to_string().parse().unwrap_or(*self as f64))
            }
            fn from_value(value: &ControlValue) -> Option<Self> {
                match value {
                    ControlValue::Number(n) => Some(*n as $ty),
                    _ => None,
                }
            }
            /// `Debug` keeps the decimal point (`2.0`, not `2`), so the
            /// literal lands in a float prop.
            fn code(&self) -> Option<String> {
                Some(format!("{self:?}"))
            }
        }
    )*};
}
float_controls!(f32, f64);

impl<T: PropControl> PropControl for Option<T> {
    fn control() -> Control {
        Control::Optional {
            inner: Box::new(T::control()),
            example: T::example_value().map(Box::new),
        }
    }
    fn to_value(&self) -> ControlValue {
        ControlValue::Optional(self.as_ref().map(|v| Box::new(v.to_value())))
    }
    fn from_value(value: &ControlValue) -> Option<Self> {
        match value {
            ControlValue::Optional(None) => Some(None),
            ControlValue::Optional(Some(inner)) => T::from_value(inner).map(Some),
            _ => None,
        }
    }
    /// `Some(…)` / `None`. A string literal inside is spelled
    /// `String::from("…")`: `Some("…")` is an `Option<&str>`, which no
    /// `Option<String>` prop accepts.
    fn code(&self) -> Option<String> {
        Some(match self {
            None => "None".to_string(),
            Some(v) => some_code(v.code()?),
        })
    }
    fn value_code(value: &ControlValue) -> Option<String> {
        match value {
            ControlValue::Optional(None) => Some("None".to_string()),
            ControlValue::Optional(Some(inner)) => Some(some_code(T::value_code(inner)?)),
            _ => None,
        }
    }
    fn imports(value: &ControlValue) -> Vec<String> {
        match value {
            ControlValue::Optional(Some(inner)) => T::imports(inner),
            _ => Vec::new(),
        }
    }
}

/// `Some(inner)`, with a string literal spelled `String::from("…")`.
fn some_code(inner: String) -> String {
    if inner.starts_with('"') {
        format!("Some(String::from({inner}))")
    } else {
        format!("Some({inner})")
    }
}

/// A reactive prop is edited as its value; the preview passes a static
/// one (each edit rebuilds the preview).
impl<T: PropControl + Clone + 'static> PropControl for Reactive<T> {
    fn control() -> Control {
        T::control()
    }
    fn to_value(&self) -> ControlValue {
        self.get_untracked().to_value()
    }
    fn from_value(value: &ControlValue) -> Option<Self> {
        T::from_value(value).map(Reactive::Static)
    }
    fn example() -> Option<Self> {
        T::example().map(Reactive::Static)
    }
    fn code(&self) -> Option<String> {
        self.get_untracked().code()
    }
    fn value_code(value: &ControlValue) -> Option<String> {
        T::value_code(value)
    }
    fn imports(value: &ControlValue) -> Vec<String> {
        T::imports(value)
    }
}

/// A two-way signal prop gets a fresh signal holding the edited value, in
/// the preview's scope. Interacting with the previewed component still
/// writes it as usual.
impl<T: PropControl + Clone + PartialEq + 'static> PropControl for Signal<T> {
    fn control() -> Control {
        T::control()
    }
    fn to_value(&self) -> ControlValue {
        untrack(|| self.get()).to_value()
    }
    fn from_value(value: &ControlValue) -> Option<Self> {
        T::from_value(value).map(signal)
    }
    fn example() -> Option<Self> {
        T::example().map(signal)
    }
    /// The caller passes its own signal; the snippet shows one holding the
    /// value.
    fn code(&self) -> Option<String> {
        Some(format!("signal({})", untrack(|| self.get()).code()?))
    }
    fn imports(value: &ControlValue) -> Vec<String> {
        let mut paths = T::imports(value);
        paths.push("runtime_core::signal".to_string());
        paths
    }
}

impl<T: PropControl + Clone + PartialEq + 'static> PropControl for ReadSignal<T> {
    fn control() -> Control {
        T::control()
    }
    fn to_value(&self) -> ControlValue {
        untrack(|| self.get()).to_value()
    }
    fn from_value(value: &ControlValue) -> Option<Self> {
        T::from_value(value).map(|v| signal(v).read_only())
    }
    fn example() -> Option<Self> {
        T::example().map(|v| signal(v).read_only())
    }
    fn code(&self) -> Option<String> {
        Some(format!("signal({}).read_only()", untrack(|| self.get()).code()?))
    }
    fn imports(value: &ControlValue) -> Vec<String> {
        let mut paths = T::imports(value);
        paths.push("runtime_core::signal".to_string());
        paths
    }
}

/// Callbacks: attached (a no-op the preview passes) or, inside an
/// `Option`, left out. One impl per arity the framework's components use.
macro_rules! handler_controls {
    ($( [$($arg:ident),*] => $code:literal ),* $(,)?) => {$(
        impl<$($arg: 'static),*> PropControl for Rc<dyn Fn($($arg),*)> {
            fn control() -> Control {
                Control::Handler
            }
            fn to_value(&self) -> ControlValue {
                ControlValue::Handler
            }
            #[allow(non_snake_case)]
            fn from_value(value: &ControlValue) -> Option<Self> {
                match value {
                    ControlValue::Handler => Some(Rc::new(|$(_: $arg),*| {})),
                    _ => None,
                }
            }
            #[allow(non_snake_case)]
            fn example() -> Option<Self> {
                Some(Rc::new(|$(_: $arg),*| {}))
            }
            fn code(&self) -> Option<String> {
                Some($code.to_string())
            }
            fn imports(_: &ControlValue) -> Vec<String> {
                vec!["std::rc::Rc".to_string()]
            }
        }
    )*};
}
handler_controls!(
    [] => "Rc::new(|| {})",
    [A] => "Rc::new(|_| {})",
    [A, B] => "Rc::new(|_, _| {})",
);

/// The text an element slot's preview starts with.
const ELEMENT_EXAMPLE_TEXT: &str = "Action";

/// An element slot (`action: Option<Element>`) is edited as the text of
/// a stand-in `text` element. An existing element can't be read back as
/// text, so it shows as an empty field.
impl PropControl for Element {
    fn control() -> Control {
        Control::Text
    }
    fn to_value(&self) -> ControlValue {
        ControlValue::Text(String::new())
    }
    fn from_value(value: &ControlValue) -> Option<Self> {
        match value {
            ControlValue::Text(s) => Some(crate::builders::text().content(s.clone()).build()),
            _ => None,
        }
    }
    fn example() -> Option<Self> {
        Self::from_value(&ControlValue::Text(ELEMENT_EXAMPLE_TEXT.to_string()))
    }
    fn example_value() -> Option<ControlValue> {
        Some(ControlValue::Text(ELEMENT_EXAMPLE_TEXT.to_string()))
    }
    /// A built element doesn't keep its text, so only the control's value
    /// can be spelled ([`value_code`](PropControl::value_code)).
    fn value_code(value: &ControlValue) -> Option<String> {
        match value {
            ControlValue::Text(s) => Some(format!("ui! {{ text {{ {s:?} }} }}")),
            _ => None,
        }
    }
    fn imports(_: &ControlValue) -> Vec<String> {
        vec!["runtime_core::ui".to_string()]
    }
}

/// The image every image preview starts from: an inline SVG (a figure on
/// an indigo square), so it shows without a network or a bundled asset.
pub const SAMPLE_IMAGE_URL: &str = "data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg' \
     viewBox='0 0 64 64'><rect width='64' height='64' fill='%236366f1'/><circle cx='32' cy='25' \
     r='11' fill='%23e0e7ff'/><rect x='13' y='41' width='38' height='23' rx='11' fill='%23e0e7ff'/></svg>";

/// An image is edited as its URL. A bundled asset shows as an empty
/// field; typing a URL replaces it.
impl PropControl for runtime_shared::ImageSource {
    fn control() -> Control {
        Control::Text
    }
    fn to_value(&self) -> ControlValue {
        ControlValue::Text(match self {
            runtime_shared::ImageSource::Url(url) => url.get_untracked(),
            _ => String::new(),
        })
    }
    fn from_value(value: &ControlValue) -> Option<Self> {
        match value {
            ControlValue::Text(url) => Some(url.as_str().into()),
            _ => None,
        }
    }
    fn example() -> Option<Self> {
        Some(SAMPLE_IMAGE_URL.into())
    }
    fn code(&self) -> Option<String> {
        match self {
            runtime_shared::ImageSource::Url(url) => Some(format!("{:?}", url.get_untracked())),
            _ => None,
        }
    }
}

/// One icon an icon prop's preview control offers.
#[derive(Clone, Copy, Debug)]
pub struct IconChoice {
    /// The picker's label and key (`"star"`).
    pub name: &'static str,
    /// How the icon is written in a `ui!` call (`"icons_lucide::STAR"`).
    pub code: &'static str,
    pub icon: runtime_shared::IconData,
}

thread_local! {
    static ICON_CHOICES: std::cell::RefCell<Vec<IconChoice>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// The icons an icon prop's preview control offers. The framework ships
/// no icon pack, so whoever renders the previews (the docs app) registers
/// a set at startup; until then an icon prop's picker is empty. The first
/// one is the type's [`PropControl::example`].
pub fn set_icon_choices(choices: Vec<IconChoice>) {
    ICON_CHOICES.with(|c| *c.borrow_mut() = choices);
}

fn icon_choice(find: impl Fn(&IconChoice) -> bool) -> Option<IconChoice> {
    ICON_CHOICES.with(|c| c.borrow().iter().copied().find(|choice| find(choice)))
}

/// A choice over the [`set_icon_choices`] icons. An icon that isn't one of
/// them (an app's own glyph) shows as no selection.
impl PropControl for runtime_shared::IconData {
    fn control() -> Control {
        Control::Choice(ICON_CHOICES.with(|c| c.borrow().iter().map(|choice| choice.name.to_string()).collect()))
    }
    fn to_value(&self) -> ControlValue {
        ControlValue::Choice(icon_choice(|c| c.icon == *self).map(|c| c.name.to_string()).unwrap_or_default())
    }
    fn from_value(value: &ControlValue) -> Option<Self> {
        let ControlValue::Choice(key) = value else { return None };
        icon_choice(|c| c.name == key).map(|c| c.icon)
    }
    fn example() -> Option<Self> {
        ICON_CHOICES.with(|c| c.borrow().first().map(|choice| choice.icon))
    }
    fn code(&self) -> Option<String> {
        icon_choice(|c| c.icon == *self).map(|c| c.code.to_string())
    }
}

/// The framework's own unit enums that appear as props: a choice over
/// every variant, keyed by its name.
/// A choice per variant for the framework's own unit enums: each entry
/// is the enum's defining path, then where an author imports it from.
macro_rules! unit_enum_controls {
    ($($path:path => $import:literal { $($variant:ident),* $(,)? }),* $(,)?) => {$(
        impl PropControl for $path {
            fn control() -> Control {
                Control::Choice(vec![$(stringify!($variant).to_string()),*])
            }
            fn to_value(&self) -> ControlValue {
                ControlValue::Choice(match self {
                    $(Self::$variant => stringify!($variant),)*
                }.to_string())
            }
            fn from_value(value: &ControlValue) -> Option<Self> {
                let ControlValue::Choice(key) = value else { return None };
                match key.as_str() {
                    $(stringify!($variant) => Some(Self::$variant),)*
                    _ => None,
                }
            }
            /// The import's last segment is the name the code spells.
            fn code(&self) -> Option<String> {
                let name = $import.rsplit("::").next().unwrap_or($import);
                Some(match self {
                    $(Self::$variant => format!("{name}::{}", stringify!($variant)),)*
                })
            }
            fn imports(_: &ControlValue) -> Vec<String> {
                vec![$import.to_string()]
            }
        }
    )*};
}
unit_enum_controls!(
    runtime_shared::TextAlign => "runtime_core::TextAlign" { Left, Right, Center, Justify },
    runtime_shared::TextTransform => "runtime_core::TextTransform" { None, Uppercase, Lowercase, Capitalize },
    runtime_shared::FontWeight => "runtime_core::FontWeight" {
        Thin, ExtraLight, Light, Normal, Medium, SemiBold, Bold, ExtraBold, Black,
    },
    // Where an anchored overlay (Tooltip, Popover, HoverCard) sits against
    // its trigger.
    runtime_shared::primitives::portal::ElementSide => "runtime_core::primitives::portal::ElementSide" {
        Above, Below, Start, End,
    },
    runtime_shared::primitives::portal::ElementAlign => "runtime_core::primitives::portal::ElementAlign" {
        Start, Center, End,
    },
);

/// The [`Control::Choice`] a [`VariantEnum`] gets when it has no
/// [`PropControl`] impl of its own: its built-in values, keyed by
/// `as_variant_str`.
fn variant_choice<E: VariantEnum>() -> Control {
    Control::Choice(E::all_variants().iter().map(|v| v.as_variant_str().to_string()).collect())
}

fn variant_to_value<E: VariantEnum>(value: &E) -> ControlValue {
    ControlValue::Choice(value.as_variant_str().to_string())
}

/// `ButtonVariant::Filled`: an axis value's const is the PascalCase of its
/// key (the `component_styles!` / `stylesheet!` naming), on the type's
/// last path segment.
fn variant_code<E: VariantEnum>(value: &E) -> Option<String> {
    let full = std::any::type_name::<E>();
    let ty = full.split('<').next().unwrap_or(full).rsplit("::").next().unwrap_or(full);
    let mut konst = String::new();
    let mut upper = true;
    for c in value.as_variant_str().chars() {
        if c == '_' {
            upper = true;
        } else if upper {
            konst.extend(c.to_uppercase());
            upper = false;
        } else {
            konst.push(c);
        }
    }
    Some(format!("{ty}::{konst}"))
}

/// An axis value's type, by its defining path — what [`variant_code`]'s
/// `Type::Const` spelling needs in scope.
fn variant_imports<E: VariantEnum>(_: &ControlValue) -> Vec<String> {
    let full = std::any::type_name::<E>();
    vec![full.split('<').next().unwrap_or(full).to_string()]
}

fn variant_from_value<E: VariantEnum>(value: &ControlValue) -> Option<E> {
    let ControlValue::Choice(key) = value else { return None };
    E::all_variants().iter().copied().find(|v| v.as_variant_str() == key)
}

// =============================================================================
// Fields and previews
// =============================================================================

/// One controllable field of a props struct `P`.
pub struct PreviewField<P> {
    /// The field's name.
    pub name: &'static str,
    /// The field's declared type, as written (`Reactive<String>`), for the
    /// docs page's labels.
    pub type_name: &'static str,
    /// How it is edited.
    pub control: Control,
    /// Whether this is the component's children slot (shown as a child
    /// block, not a prop, in the Usage snippet).
    pub children: bool,
    get: Box<dyn Fn(&P) -> ControlValue>,
    set: Box<dyn Fn(&mut P, &ControlValue)>,
    code: Box<dyn Fn(&ControlValue) -> Option<String>>,
    imports: fn(&ControlValue) -> Vec<String>,
}

impl<P: 'static> PreviewField<P> {
    /// A field edited through `adapter` — the `#[props]` / `#[component]`
    /// emission obtains the adapter from [`__probe`].
    pub fn new<T: 'static>(
        name: &'static str,
        type_name: &'static str,
        adapter: Adapter<T>,
        field: fn(&mut P) -> &mut T,
        field_ref: fn(&P) -> &T,
    ) -> Self {
        let Adapter { control, to_value, from_value, code } = adapter;
        PreviewField {
            name,
            type_name,
            control: control(),
            children: false,
            get: Box::new(move |p| to_value(field_ref(p))),
            set: Box::new(move |p, v| {
                if let Some(value) = from_value(v) {
                    *field(p) = value;
                }
            }),
            code: Box::new(code),
            imports: |_| Vec::new(),
        }
    }

    /// What [`Preview::imports`] reports for this field — the macros pass
    /// the field type's [`PropControl::imports`], picked by the same
    /// dispatch as its adapter.
    pub fn with_imports(mut self, imports: fn(&ControlValue) -> Vec<String>) -> Self {
        self.imports = imports;
        self
    }

    /// A component's children, edited as the text of one `text` child.
    pub fn children(name: &'static str, field: fn(&mut P) -> &mut Vec<Element>, placeholder: &'static str) -> Self {
        PreviewField {
            name,
            type_name: "Vec<Element>",
            control: Control::Text,
            children: true,
            get: Box::new(move |_| ControlValue::Text(placeholder.to_string())),
            set: Box::new(move |p, v| {
                if let ControlValue::Text(s) = v {
                    *field(p) = vec![crate::builders::text().content(s.clone()).build()];
                }
            }),
            code: Box::new(|v| match v {
                ControlValue::Text(s) => Some(format!("text {{ {s:?} }}")),
                _ => None,
            }),
            imports: |_| Vec::new(),
        }
    }
}

/// A `#[prop(example = …)]` as written: the field it sets and its source.
/// For a field with no control, this is how the Usage snippet spells the
/// value the preview shows (see [`Preview::fixed`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreviewExample {
    pub name: &'static str,
    /// The example expression, formatted.
    pub code: &'static str,
    /// Whether the field is the children slot.
    pub children: bool,
}

/// A children slot's example: one element (a `ui!` block, fragments
/// included) or the list itself. What the emission converts a children
/// `#[prop(example = …)]` through.
pub trait IntoPreviewChildren {
    fn into_preview_children(self) -> Vec<Element>;
}

impl IntoPreviewChildren for Element {
    fn into_preview_children(self) -> Vec<Element> {
        vec![self]
    }
}

impl IntoPreviewChildren for Vec<Element> {
    fn into_preview_children(self) -> Vec<Element> {
        self
    }
}

/// What `#[props]` and inline-props `#[component]` implement for a props
/// struct when previews are on.
pub trait PreviewProps: BuildElement + 'static {
    /// One entry per field that has a control, in declaration order.
    fn preview_fields() -> Vec<PreviewField<Self>>;
    /// The props the preview starts from: [`BuildElement::defaults`] with
    /// every `#[prop(example = …)]` applied.
    fn preview_base() -> Self {
        Self::defaults()
    }
    /// Every `#[prop(example = …)]`, in declaration order.
    fn preview_examples() -> &'static [PreviewExample] {
        &[]
    }
}

/// A control as the docs app sees it: no props type attached.
#[derive(Clone, Debug, PartialEq)]
pub struct FieldSpec {
    pub name: &'static str,
    /// The field's declared type, as written.
    pub type_name: &'static str,
    pub control: Control,
    /// The value the preview starts from (`#[prop(example)]` applied).
    pub initial: ControlValue,
    /// The value the component gets when the call site leaves the prop
    /// out — a Usage snippet omits props still at it.
    pub default: ControlValue,
    /// Whether this is the children slot.
    pub children: bool,
}

/// A component's preview: its controls, and a renderer that builds the
/// component from one value per control.
#[derive(Clone)]
pub struct Preview {
    pub fields: Vec<FieldSpec>,
    /// The examples set on fields without a control: shown as given, so
    /// the Usage snippet spells them as written.
    pub fixed: Vec<PreviewExample>,
    render: Rc<dyn Fn(&[ControlValue]) -> Element>,
    code: Rc<dyn Fn(usize, &ControlValue) -> Option<String>>,
    imports: Rc<dyn Fn(usize, &ControlValue) -> Vec<String>>,
}

impl Preview {
    /// The preview of a component whose props implement [`PreviewProps`].
    /// Reads the initial values, so it must run in a reactive scope (a
    /// signal-typed default mints its signal there).
    pub fn of<P: PreviewProps>() -> Self {
        let base = P::preview_base();
        let defaults = P::defaults();
        let fields: Rc<Vec<PreviewField<P>>> = Rc::new(P::preview_fields());
        let specs = fields
            .iter()
            .map(|f| FieldSpec {
                name: f.name,
                type_name: f.type_name,
                control: f.control.clone(),
                initial: (f.get)(&base),
                default: (f.get)(&defaults),
                children: f.children,
            })
            .collect::<Vec<FieldSpec>>();
        drop((base, defaults));
        let fixed = P::preview_examples()
            .iter()
            .filter(|e| !specs.iter().any(|f| f.name == e.name))
            .copied()
            .collect();
        let render_fields = fields.clone();
        let import_fields = fields.clone();
        Preview {
            fields: specs,
            fixed,
            render: Rc::new(move |values| {
                let mut props = P::preview_base();
                for (field, value) in render_fields.iter().zip(values) {
                    (field.set)(&mut props, value);
                }
                props.build()
            }),
            code: Rc::new(move |index, value| fields.get(index).and_then(|f| (f.code)(value))),
            imports: Rc::new(move |index, value| import_fields.get(index).map_or_else(Vec::new, |f| (f.imports)(value))),
        }
    }

    /// The preview of a component without [`PreviewProps`] (a hand-written
    /// props struct): no controls, rendered from its defaults.
    pub fn uncontrolled<P: BuildElement + 'static>() -> Self {
        Preview {
            fields: Vec::new(),
            fixed: Vec::new(),
            render: Rc::new(|_| P::defaults().build()),
            code: Rc::new(|_, _| None),
            imports: Rc::new(|_, _| Vec::new()),
        }
    }

    /// The initial value of every control, in order.
    pub fn initial_values(&self) -> Vec<ControlValue> {
        self.fields.iter().map(|f| f.initial.clone()).collect()
    }

    /// Build the component with `values`, one per entry of
    /// [`fields`](Self::fields). Missing trailing values keep the
    /// component's starting props.
    pub fn render(&self, values: &[ControlValue]) -> Element {
        (self.render)(values)
    }

    /// `value` for field `index` as `ui!` code (see [`PropControl::code`]),
    /// or `None` when its type can't spell it.
    pub fn code(&self, index: usize, value: &ControlValue) -> Option<String> {
        (self.code)(index, value)
    }

    /// The paths [`code`](Self::code)'s spelling of `value` for field
    /// `index` needs in scope (see [`PropControl::imports`]).
    pub fn imports(&self, index: usize, value: &ControlValue) -> Vec<String> {
        (self.imports)(index, value)
    }
}

// =============================================================================
// Dispatch the macros emit
// =============================================================================

/// The control functions for one field type, picked at compile time.
pub struct Adapter<T> {
    pub control: fn() -> Control,
    pub to_value: fn(&T) -> ControlValue,
    pub from_value: fn(&ControlValue) -> Option<T>,
    /// A control value as `ui!` code ([`PropControl::value_code`]).
    pub code: fn(&ControlValue) -> Option<String>,
}

/// Compile-time choice of how a field type is edited, by autoref
/// specialization. A trait bound can't say "the first of these that
/// applies", so the choice is made by method resolution per concrete
/// type, and a type with no control is never a compile error.
///
/// The macros emit `(&&&&Probe::<Ty>::new()).__adapter()` with every
/// `Via*` trait in scope. Method lookup walks the receiver's autoderef
/// steps (`&&&&Probe`, `&&&Probe`, `&&Probe`, `&Probe`) and at each step
/// takes a method whose `&self` type is exactly that receiver, skipping
/// impls whose bounds fail. So the impl on `&&&Probe` (`&self` =
/// `&&&&Probe`) is tried first and the one on `Probe` last:
/// [`PropControl`], then `Reactive<E>` of a [`VariantEnum`], then a bare
/// `VariantEnum`, then nothing. The props probe is the same two levels
/// deep: `(&&PropsProbe::<P>::new()).__preview()`.
///
/// Keep the receiver depth and the impl depths in step: one `&` too few
/// lets a fallback's `&self` match first and shadow every level above it.
#[doc(hidden)]
pub mod __probe {
    use super::*;

    pub struct Probe<T>(PhantomData<T>);

    impl<T> Probe<T> {
        #[allow(clippy::new_without_default)]
        pub fn new() -> Self {
            Probe(PhantomData)
        }
    }

    pub trait ViaPropControl<T> {
        fn __adapter(&self) -> Option<Adapter<T>>;
        /// The field type's imports ([`PropControl::imports`]); resolved
        /// by the same autoref levels as `__adapter`.
        fn __imports(&self) -> fn(&ControlValue) -> Vec<String> {
            |_| Vec::new()
        }
    }
    impl<T: PropControl> ViaPropControl<T> for &&&Probe<T> {
        fn __adapter(&self) -> Option<Adapter<T>> {
            Some(Adapter { control: T::control, to_value: T::to_value, from_value: T::from_value, code: T::value_code })
        }
        fn __imports(&self) -> fn(&ControlValue) -> Vec<String> {
            T::imports
        }
    }

    pub trait ViaReactiveVariant<T> {
        fn __adapter(&self) -> Option<Adapter<T>>;
        fn __imports(&self) -> fn(&ControlValue) -> Vec<String> {
            |_| Vec::new()
        }
    }
    impl<E: VariantEnum> ViaReactiveVariant<Reactive<E>> for &&Probe<Reactive<E>> {
        fn __adapter(&self) -> Option<Adapter<Reactive<E>>> {
            Some(Adapter {
                control: variant_choice::<E>,
                to_value: |r| variant_to_value(&r.get_untracked()),
                from_value: |v| variant_from_value::<E>(v).map(Reactive::Static),
                code: |v| variant_code(&variant_from_value::<E>(v)?),
            })
        }
        fn __imports(&self) -> fn(&ControlValue) -> Vec<String> {
            variant_imports::<E>
        }
    }

    pub trait ViaVariant<T> {
        fn __adapter(&self) -> Option<Adapter<T>>;
        fn __imports(&self) -> fn(&ControlValue) -> Vec<String> {
            |_| Vec::new()
        }
    }
    impl<E: VariantEnum> ViaVariant<E> for &Probe<E> {
        fn __adapter(&self) -> Option<Adapter<E>> {
            Some(Adapter {
                control: variant_choice::<E>,
                to_value: variant_to_value,
                from_value: variant_from_value,
                code: |v| variant_code(&variant_from_value::<E>(v)?),
            })
        }
        fn __imports(&self) -> fn(&ControlValue) -> Vec<String> {
            variant_imports::<E>
        }
    }

    pub trait ViaNone<T> {
        fn __adapter(&self) -> Option<Adapter<T>>;
        fn __imports(&self) -> fn(&ControlValue) -> Vec<String> {
            |_| Vec::new()
        }
    }
    impl<T> ViaNone<T> for Probe<T> {
        fn __adapter(&self) -> Option<Adapter<T>> {
            None
        }
    }

    /// Same specialization for the whole props type, called as
    /// `(&&PropsProbe::<P>::new()).__preview()`: [`Preview::of`] when the
    /// macros implemented [`PreviewProps`], [`Preview::uncontrolled`]
    /// otherwise.
    pub struct PropsProbe<P>(PhantomData<P>);

    impl<P> PropsProbe<P> {
        #[allow(clippy::new_without_default)]
        pub fn new() -> Self {
            PropsProbe(PhantomData)
        }
    }

    pub trait ViaPreviewProps {
        fn __preview(&self) -> Preview;
    }
    impl<P: PreviewProps> ViaPreviewProps for &PropsProbe<P> {
        fn __preview(&self) -> Preview {
            Preview::of::<P>()
        }
    }

    pub trait ViaBuildElement {
        fn __preview(&self) -> Preview;
    }
    impl<P: BuildElement + 'static> ViaBuildElement for PropsProbe<P> {
        fn __preview(&self) -> Preview {
            Preview::uncontrolled::<P>()
        }
    }
}
