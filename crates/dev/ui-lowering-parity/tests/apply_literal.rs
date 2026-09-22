//! The generated `__apply_literal`, run against real `#[component]` /
//! `#[props]` structs.
//!
//! `runtime-macros`' own tests pin the emitted TOKENS; these pin the
//! behaviour — that a descriptor literal lands in the prop, that the
//! `Reactive<T>` wrap is bridged, that a refusal is reported rather than
//! silently swallowed, and that a props type without the macro falls
//! back to the blanket impl instead of failing to compile.

use std::borrow::Cow;

use runtime_macros::{component, props};
use runtime_vocabulary::glue::__template::{ApplyLiteralFallback, TemplateLiteral};
use runtime_vocabulary::glue::{Element, Reactive};

use runtime_macros::ui;

/// Inline-props form: the macro generates the struct, its `Default`, and
/// `__apply_literal`.
#[component]
fn Chip(
    /// Reactive-by-default string prop.
    label: String,
    /// Integer prop, narrower than the descriptor's `i64`.
    #[prop(default = 1)]
    weight: u8,
    /// Float prop.
    #[prop(default = 0.5)]
    ratio: f32,
    /// Static bool.
    #[prop(static, default = false)]
    loud: bool,
    /// A path-typed prop: nothing a generated applier can build.
    #[prop(static, default = Tone::Neutral)]
    tone: Tone,
) -> Element {
    ui! { text { move || label.get() } }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Tone {
    #[default]
    Neutral,
    Danger,
}

/// Explicit-struct form: `#[props]` owns the struct, so it emits
/// `__apply_literal` too.
#[props]
#[derive(Default)]
pub struct PanelProps {
    pub title: String,
    pub rows: i64,
}

/// A hand-rolled props type with NO macro — the "slot-only, never an
/// error" case.
#[derive(Default)]
pub struct BareProps {
    pub title: String,
}

fn str_lit(s: &'static str) -> TemplateLiteral {
    TemplateLiteral::Str(Cow::Borrowed(s))
}

#[test]
fn applies_a_string_literal_through_the_reactive_wrap() {
    let mut props = ChipProps::default();
    assert!(props.__apply_literal("label", &str_lit("alpha")));
    assert_eq!(props.label.get_untracked(), "alpha");
}

#[test]
fn applies_an_integer_literal_cast_to_the_field_width() {
    let mut props = ChipProps::default();
    assert!(props.__apply_literal("weight", &TemplateLiteral::Int(7)));
    assert_eq!(props.weight.get_untracked(), 7u8);
}

#[test]
fn applies_a_float_and_a_bool_literal() {
    let mut props = ChipProps::default();
    assert!(props.__apply_literal("ratio", &TemplateLiteral::Float(0.25)));
    assert_eq!(props.ratio.get_untracked(), 0.25f32);
    // A `#[prop(static)]` bool is unwrapped, so it assigns directly.
    assert!(props.__apply_literal("loud", &TemplateLiteral::Bool(true)));
    assert!(props.loud);
}

/// Wrong shape, unknown name, and `Path` all report a refusal. Reporting
/// rather than swallowing is the contract: the template emission's
/// site-local fallback is what runs next.
#[test]
fn refuses_a_mismatched_shape_an_unknown_name_and_a_path() {
    let mut props = ChipProps::default();
    assert!(!props.__apply_literal("weight", &str_lit("seven")));
    assert!(!props.__apply_literal("nonexistent", &str_lit("x")));
    assert!(!props.__apply_literal("tone", &TemplateLiteral::Path(Cow::Borrowed("Tone::Danger"))));
    // Nothing was written.
    assert_eq!(props.weight.get_untracked(), 1u8);
    assert_eq!(props.tone, Tone::Neutral);
}

/// The explicit `#[props]` form gets the same method.
#[test]
fn the_explicit_props_form_applies_literals_too() {
    let mut props = PanelProps::default();
    assert!(props.__apply_literal("title", &str_lit("Overview")));
    assert!(props.__apply_literal("rows", &TemplateLiteral::Int(12)));
    assert_eq!(props.title.get_untracked(), "Overview");
    assert_eq!(props.rows.get_untracked(), 12i64);
}

/// A props type the macro never touched still COMPILES at the call site
/// and answers "not applied" — inherent-before-trait resolution means
/// the blanket impl only ever serves types with no generated method.
#[test]
fn a_props_type_without_the_macro_falls_back_to_the_blanket_impl() {
    let mut bare = BareProps::default();
    assert!(!bare.__apply_literal("title", &str_lit("x")));
    assert_eq!(bare.title, "");
}

/// Compile-time proof that the generated method is INHERENT: taking its
/// function pointer by path resolves to the struct's own item, which
/// would fail if only the trait method existed.
#[test]
fn the_generated_method_is_inherent() {
    let f: fn(&mut ChipProps, &str, &TemplateLiteral) -> bool = ChipProps::__apply_literal;
    let mut props = ChipProps::default();
    assert!(f(&mut props, "label", &str_lit("via-pointer")));
    assert_eq!(props.label.get_untracked(), "via-pointer");
}

/// Keeps the `Reactive` import honest — the wrap is what the bridging
/// `.into()` targets.
#[test]
fn reactive_props_are_wrapped() {
    let r: Reactive<String> = "x".into();
    assert_eq!(r.get_untracked(), "x");
}
