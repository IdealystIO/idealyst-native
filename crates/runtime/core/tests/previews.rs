//! Component previews (`previews` feature): the `PropControl` built-ins,
//! the per-field control dispatch the macros emit, `#[prop(example = …)]`,
//! `#[prop_control]` on an app type, the children placeholder, and the
//! preview symbol recorded in the catalog.
//!
//!   cargo test -p runtime-core --features previews --test previews
//!
//! The symbol itself is linked through an `extern "Rust"` block generated
//! from the catalog; that path is exercised by docs-app's tests, which
//! link idea-ui's components by their recorded symbols.

#![cfg(feature = "previews")]

use std::cell::RefCell;
use std::rc::Rc;

use runtime_core::previews::{Control, ControlValue, Preview, PropControl};
use runtime_core::{component, prop_control, props, ui, Element, Reactive, Signal, VariantEnum};

thread_local! {
    static SEEN: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

fn record(s: String) {
    SEEN.with(|v| v.borrow_mut().push(s));
}

fn take() -> Vec<String> {
    SEEN.with(|v| std::mem::take(&mut *v.borrow_mut()))
}

/// An axis-style enum with no `PropControl` of its own: previews fall back
/// to a choice over `all_variants`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Size {
    Sm,
    #[default]
    Md,
    Lg,
}

impl VariantEnum for Size {
    fn as_variant_str(self) -> &'static str {
        match self {
            Size::Sm => "sm",
            Size::Md => "md",
            Size::Lg => "lg",
        }
    }
    fn all_variants() -> &'static [Self] {
        &[Size::Sm, Size::Md, Size::Lg]
    }
}

/// An app type with its own control.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rating(pub u8);

#[prop_control]
impl runtime_core::previews::PropControl for Rating {
    fn control() -> Control {
        Control::Number { min: Some(0.0), max: Some(5.0), step: 1.0, integer: true }
    }
    fn to_value(&self) -> ControlValue {
        ControlValue::Number(self.0 as f64)
    }
    fn from_value(value: &ControlValue) -> Option<Self> {
        match value {
            ControlValue::Number(n) => Some(Rating(*n as u8)),
            _ => None,
        }
    }
}

/// An axis enum that ALSO has its own control: the `PropControl` impl
/// must win over the `VariantEnum` fallback, bare or `Reactive`-wrapped.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Level(pub u8);

impl VariantEnum for Level {
    fn as_variant_str(self) -> &'static str {
        "level"
    }
    fn all_variants() -> &'static [Self] {
        &[Level(0)]
    }
}

#[prop_control]
impl runtime_core::previews::PropControl for Level {
    fn control() -> Control {
        Control::Number { min: Some(0.0), max: Some(9.0), step: 1.0, integer: true }
    }
    fn to_value(&self) -> ControlValue {
        ControlValue::Number(self.0 as f64)
    }
    fn from_value(value: &ControlValue) -> Option<Self> {
        match value {
            ControlValue::Number(n) => Some(Level(*n as u8)),
            _ => None,
        }
    }
}

/// Props written by hand — no `#[props]`, so no `PreviewProps`.
#[derive(Default)]
pub struct HandProps {
    pub note: String,
}

#[component]
fn Hand(props: HandProps) -> Element {
    record(format!("hand note={:?}", props.note));
    ui! { view() }
}

/// A type nothing knows how to edit.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Opaque(pub u32);

#[component]
fn Badge(
    #[prop(example = "New")] label: String,
    #[prop(default = 3)] count: i32,
    size: Size,
    rating: Rating,
    opaque: Opaque,
    #[prop(static)] on_press: Option<Rc<dyn Fn()>>,
) -> Element {
    let _ = (opaque, on_press);
    record(format!(
        "label={} count={} size={:?} rating={:?}",
        label.get(),
        count.get(),
        size.get(),
        rating.get()
    ));
    ui! { view() }
}

#[props]
#[derive(Default)]
pub struct PanelProps {
    #[prop(example = "Settings")]
    pub title: String,
    pub open: bool,
    pub children: Vec<Element>,
}

#[component]
fn Panel(props: PanelProps) -> Element {
    record(format!("title={} open={} children={}", props.title.get(), props.open.get(), props.children.len()));
    ui! { view() }
}

#[component]
fn Toggle(value: Signal<bool>) -> Element {
    record(format!("value={}", value.get()));
    ui! { view() }
}

/// What `#[component]`'s exported preview fn returns: the expression it
/// emits, with every probe trait in scope as it has them — so the
/// specialization order under test is the real one.
macro_rules! preview_of {
    ($tag:ty) => {{
        #[allow(unused_imports)]
        use runtime_core::previews::__probe::{ViaBuildElement as _, ViaPreviewProps as _};
        (&&runtime_core::previews::__probe::PropsProbe::<$tag>::new()).__preview()
    }};
}

/// The per-field dispatch `#[props]` emits, likewise verbatim.
macro_rules! adapter_of {
    ($ty:ty) => {{
        #[allow(unused_imports)]
        use runtime_core::previews::__probe::{
            ViaNone as _, ViaPropControl as _, ViaReactiveVariant as _, ViaVariant as _,
        };
        (&&&&runtime_core::previews::__probe::Probe::<$ty>::new()).__adapter()
    }};
}

fn field<'a>(preview: &'a Preview, name: &str) -> &'a runtime_core::previews::FieldSpec {
    preview.fields.iter().find(|f| f.name == name).unwrap_or_else(|| panic!("no `{name}` control"))
}

#[test]
fn builtin_controls_round_trip() {
    assert_eq!(String::control(), Control::Text);
    assert_eq!(String::from_value(&"hi".to_string().to_value()), Some("hi".to_string()));
    assert_eq!(bool::from_value(&true.to_value()), Some(true));
    assert_eq!(
        u8::control(),
        Control::Number { min: Some(0.0), max: Some(255.0), step: 1.0, integer: true }
    );
    assert_eq!(u8::from_value(&ControlValue::Number(300.0)), Some(255), "saturates");
    assert_eq!(i32::from_value(&ControlValue::Number(2.6)), Some(3), "rounds");
    assert_eq!(f32::from_value(&ControlValue::Number(0.5)), Some(0.5));
    assert_eq!(
        <Option<String>>::control(),
        Control::Optional { inner: Box::new(Control::Text), example: None },
        "String has no example, so switching an optional one on starts empty"
    );
    assert_eq!(<Option<String>>::from_value(&ControlValue::Optional(None)), Some(None));
    assert_eq!(
        <Option<i32>>::from_value(&ControlValue::Optional(Some(Box::new(ControlValue::Number(4.0))))),
        Some(Some(4))
    );
    let r = <Reactive<String>>::from_value(&ControlValue::Text("x".into())).unwrap();
    assert!(r.is_static());
    assert_eq!(r.get(), "x");
    assert_eq!(
        runtime_core::TextAlign::from_value(&runtime_core::TextAlign::Center.to_value()),
        Some(runtime_core::TextAlign::Center)
    );
    assert_eq!(String::from_value(&ControlValue::Bool(true)), None, "wrong variant is rejected");
}

#[test]
fn inline_props_preview_picks_a_control_per_field_type() {
    let preview = preview_of!(Badge);
    let names: Vec<_> = preview.fields.iter().map(|f| f.name).collect();
    // `opaque` has no control and keeps its default; the optional
    // `on_press` callback gets an "attach a handler" control.
    assert_eq!(names, ["label", "count", "size", "rating", "on_press"]);
    assert_eq!(field(&preview, "label").control, Control::Text);
    assert_eq!(
        field(&preview, "size").control,
        Control::Choice(vec!["sm".into(), "md".into(), "lg".into()]),
        "a VariantEnum without PropControl becomes a choice over all_variants"
    );
    assert_eq!(
        field(&preview, "rating").control,
        Control::Number { min: Some(0.0), max: Some(5.0), step: 1.0, integer: true },
        "an app type's #[prop_control] impl is used"
    );
}

#[test]
fn preview_starts_from_defaults_with_examples_applied() {
    let preview = preview_of!(Badge);
    assert_eq!(field(&preview, "label").initial, ControlValue::Text("New".into()), "example wins");
    assert_eq!(field(&preview, "count").initial, ControlValue::Number(3.0), "#[prop(default)] kept");
    assert_eq!(field(&preview, "size").initial, ControlValue::Choice("md".into()));
}

#[test]
fn preview_renders_the_component_with_the_control_values() {
    let preview = preview_of!(Badge);
    take();
    let _ = preview.render(&preview.initial_values());
    assert_eq!(take(), ["label=New count=3 size=Md rating=Rating(0)"]);

    let edited = vec![
        ControlValue::Text("Sale".into()),
        ControlValue::Number(9.0),
        ControlValue::Choice("lg".into()),
        ControlValue::Number(4.0),
    ];
    let _ = preview.render(&edited);
    assert_eq!(take(), ["label=Sale count=9 size=Lg rating=Rating(4)"]);
}

#[test]
fn explicit_props_struct_preview_and_children_placeholder() {
    let preview = preview_of!(Panel);
    let names: Vec<_> = preview.fields.iter().map(|f| f.name).collect();
    assert_eq!(names, ["title", "open", "children"]);
    assert_eq!(field(&preview, "title").initial, ControlValue::Text("Settings".into()));
    assert_eq!(field(&preview, "children").control, Control::Text);
    take();
    let _ = preview.render(&preview.initial_values());
    assert_eq!(take(), ["title=Settings open=false children=1"], "children get one text child");
}

#[test]
fn signal_prop_gets_a_fresh_signal_holding_the_edited_value() {
    runtime_core::__with_fresh_world(|| {
        let preview = preview_of!(Toggle);
        assert_eq!(field(&preview, "value").control, Control::Bool);
        take();
        let _ = preview.render(&[ControlValue::Bool(true)]);
        assert_eq!(take(), ["value=true"]);
    });
}

#[test]
fn catalog_entry_records_the_exported_preview_symbol() {
    let entry = runtime_core::__mcp::entries()
        .find(|e| e.name == "Badge" && e.module_path == module_path!())
        .expect("Badge is in the catalog");
    assert!(
        entry.preview.starts_with("__idealyst_preview__previews__Badge__"),
        "symbol names crate and component: {}",
        entry.preview
    );
    let panel = runtime_core::__mcp::entries().find(|e| e.name == "Panel").unwrap();
    assert_ne!(entry.preview, panel.preview);
}

/// Regression: the props probe's `BuildElement` fallback once sat at the
/// same autoderef step as the `PreviewProps` impl and won, so every
/// component — idea-ui's Button included — previewed with no controls.
#[test]
fn regression_preview_props_outranks_the_uncontrolled_fallback() {
    assert!(!preview_of!(Badge).fields.is_empty());
    assert!(!preview_of!(Panel).fields.is_empty());
}

#[test]
fn hand_written_props_preview_uncontrolled_from_defaults() {
    let preview = preview_of!(Hand);
    assert!(preview.fields.is_empty());
    take();
    let _ = preview.render(&[]);
    assert_eq!(take(), ["hand note=\"\""]);
}

/// Regression: field dispatch had `Reactive<VariantEnum>` ahead of the
/// type's own `PropControl`, so an axis with a custom control got the
/// generic choice instead.
#[test]
fn regression_own_prop_control_outranks_the_variant_fallback() {
    let custom = Control::Number { min: Some(0.0), max: Some(9.0), step: 1.0, integer: true };
    assert_eq!(adapter_of!(Level).map(|a| (a.control)()), Some(custom.clone()));
    assert_eq!(adapter_of!(Reactive<Level>).map(|a| (a.control)()), Some(custom));
    assert_eq!(
        adapter_of!(Reactive<Size>).map(|a| (a.control)()),
        Some(Control::Choice(vec!["sm".into(), "md".into(), "lg".into()]))
    );
    assert_eq!(adapter_of!(Size).map(|a| (a.control)()), Some(Control::Choice(vec!["sm".into(), "md".into(), "lg".into()])));
    assert!(adapter_of!(Opaque).is_none());
}

/// An optional image switched on starts at the sample image, not an empty
/// URL — `PropControl::example` carried in the `Optional` control.
#[test]
fn optional_image_starts_from_the_sample_image() {
    use runtime_core::previews::SAMPLE_IMAGE_URL;
    assert_eq!(
        <Option<runtime_core::ImageSource>>::control(),
        Control::Optional {
            inner: Box::new(Control::Text),
            example: Some(Box::new(ControlValue::Text(SAMPLE_IMAGE_URL.into()))),
        }
    );
    let edited = runtime_core::ImageSource::from_value(&ControlValue::Text("https://x/y.png".into())).unwrap();
    assert_eq!(edited.to_value(), ControlValue::Text("https://x/y.png".into()));
}

/// Icons are a choice over what the renderer registered; the first one is
/// the type's example. Nothing registered: an empty picker, no example.
#[test]
fn icon_control_offers_the_registered_icons() {
    use runtime_core::previews::{set_icon_choices, IconChoice};
    use runtime_core::{FillRule, IconData};
    const A: IconData = IconData { view_box: (24, 24), paths: &["M0 0h1"], fill_rule: FillRule::NonZero, filled: false };
    const B: IconData = IconData { view_box: (24, 24), paths: &["M0 0h2"], fill_rule: FillRule::NonZero, filled: false };
    const OTHER: IconData = IconData { view_box: (24, 24), paths: &["M0 0h3"], fill_rule: FillRule::NonZero, filled: false };

    set_icon_choices(Vec::new());
    assert_eq!(IconData::control(), Control::Choice(Vec::new()));
    assert!(IconData::example().is_none());

    set_icon_choices(vec![
        IconChoice { name: "a", code: "pack::A", icon: A },
        IconChoice { name: "b", code: "pack::B", icon: B },
    ]);
    assert_eq!(IconData::control(), Control::Choice(vec!["a".into(), "b".into()]));
    assert_eq!(B.to_value(), ControlValue::Choice("b".into()));
    assert_eq!(IconData::from_value(&ControlValue::Choice("a".into())), Some(A));
    assert_eq!(OTHER.to_value(), ControlValue::Choice(String::new()), "an unregistered glyph has no key");
    assert_eq!(IconData::example(), Some(A));
    assert_eq!(B.code().as_deref(), Some("pack::B"), "an icon is written as the choice's code");
    assert_eq!(OTHER.code(), None);
    assert_eq!(
        <Option<IconData>>::control(),
        Control::Optional {
            inner: Box::new(Control::Choice(vec!["a".into(), "b".into()])),
            example: Some(Box::new(ControlValue::Choice("a".into()))),
        }
    );
    set_icon_choices(Vec::new());
}

/// How the Usage snippet writes each kind of value. A snippet is pasted
/// into `ui!`, so each spelling must be what a prop of that type takes.
#[test]
fn values_are_written_as_ui_prop_code() {
    use std::rc::Rc;
    assert_eq!("Save".to_string().code().as_deref(), Some("\"Save\""));
    assert_eq!(true.code().as_deref(), Some("true"));
    assert_eq!(3i32.code().as_deref(), Some("3"));
    assert_eq!(2.0f32.code().as_deref(), Some("2.0"), "a float keeps its decimal point");
    assert_eq!(
        Some("x".to_string()).code().as_deref(),
        Some("Some(String::from(\"x\"))"),
        "Some(\"x\") would be an Option<&str>"
    );
    assert_eq!(Some(4u8).code().as_deref(), Some("Some(4)"));
    assert_eq!(<Option<String>>::None.code().as_deref(), Some("None"));
    assert_eq!(Reactive::Static("x".to_string()).code().as_deref(), Some("\"x\""));
    assert_eq!(runtime_core::TextAlign::Center.code().as_deref(), Some("TextAlign::Center"));
    let handler: Rc<dyn Fn()> = Rc::new(|| {});
    assert_eq!(handler.code().as_deref(), Some("Rc::new(|| {})"));
    let one: Rc<dyn Fn(String)> = Rc::new(|_| {});
    assert_eq!(one.code().as_deref(), Some("Rc::new(|_| {})"));
}

/// An axis value is written as its type's PascalCase const, through the
/// same dispatch the macros emit.
#[test]
fn an_axis_value_is_written_as_its_const() {
    let adapter = adapter_of!(Reactive<Size>).unwrap();
    assert_eq!((adapter.code)(&ControlValue::Choice("lg".into())).as_deref(), Some("Size::Lg"));
}

#[component]
fn Dismissable(#[prop(example = "Hi")] text: String, on_close: Option<std::rc::Rc<dyn Fn()>>) -> Element {
    record(format!("text={} closable={}", text.get(), on_close.is_some()));
    ui! { view() }
}

/// An optional callback is a "attach a handler" control: attaching passes
/// a no-op, so a component that renders differently with one (a close
/// button) shows it. Each field also carries its declared type and its
/// default (what a Usage snippet leaves out).
#[test]
fn optional_callback_can_be_attached_and_fields_carry_type_and_default() {
    let preview = preview_of!(Dismissable);
    let on_close = field(&preview, "on_close");
    assert_eq!(on_close.control, Control::Optional { inner: Box::new(Control::Handler), example: Some(Box::new(ControlValue::Handler)) });
    assert_eq!(on_close.initial, ControlValue::Optional(None));
    let text = field(&preview, "text");
    assert_eq!(text.default, ControlValue::Text(String::new()), "the default, not the example");
    assert_eq!(text.initial, ControlValue::Text("Hi".into()));
    assert!(text.type_name.contains("Reactive") && text.type_name.contains("String"), "{}", text.type_name);

    take();
    let _ = preview.render(&[ControlValue::Text("Hi".into()), ControlValue::Optional(Some(Box::new(ControlValue::Handler)))]);
    assert_eq!(take(), ["text=Hi closable=true"]);
    assert_eq!(preview.code(1, &ControlValue::Optional(Some(Box::new(ControlValue::Handler)))).as_deref(), Some("Some(Rc::new(|| {}))"));
}
