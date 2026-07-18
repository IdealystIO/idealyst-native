//! `external-export-suite` — two `#[component(external)]` components,
//! exported to every supported front-end framework via `idealyst export`.
//! See `consumers/` for a standalone app per framework.
//!
//! Between them the two components exercise every prop kind that can cross
//! the JS boundary:
//!
//! - [`Greeter`]: a reactive **string** prop + a **void callback**.
//! - [`Stepper`]: reactive **string** + **number** props + a callback that
//!   **carries a value** (`i32` → `event.detail` / `EventEmitter<number>`).
//!
//! Both are "controlled": JS owns the state, sets props in, and updates
//! them in response to the components' callbacks — the idiomatic shape for
//! a component embedded in a foreign framework.

use std::rc::Rc;

use runtime_core::{
    button, component, text, ui, AlignItems, Color, Cursor, Element, FlexDirection, FontWeight,
    IdealystSchema, JustifyContent, Length, Reactive, Shadow, StyleRules, StyleSheet, Tokenized,
};

// ---------------------------------------------------------------------------
// Tiny style vocabulary (kept local so the example needs only runtime-core —
// no theme / idea-ui dependency).
// ---------------------------------------------------------------------------

fn px(v: f32) -> Tokenized<Length> {
    Tokenized::Literal(Length::Px(v))
}
fn col(s: &str) -> Tokenized<Color> {
    Tokenized::Literal(Color(s.to_string()))
}
fn sheet(r: StyleRules) -> Rc<StyleSheet> {
    Rc::new(StyleSheet::r#static(r))
}
fn radius(r: &mut StyleRules, v: f32) {
    r.border_top_left_radius = Some(px(v));
    r.border_top_right_radius = Some(px(v));
    r.border_bottom_left_radius = Some(px(v));
    r.border_bottom_right_radius = Some(px(v));
}
fn padding(r: &mut StyleRules, x: f32, y: f32) {
    r.padding_left = Some(px(x));
    r.padding_right = Some(px(x));
    r.padding_top = Some(px(y));
    r.padding_bottom = Some(px(y));
}
fn border(r: &mut StyleRules, w: f32, c: &str) {
    r.border_top_width = Some(Tokenized::Literal(w));
    r.border_right_width = Some(Tokenized::Literal(w));
    r.border_bottom_width = Some(Tokenized::Literal(w));
    r.border_left_width = Some(Tokenized::Literal(w));
    r.border_top_color = Some(col(c));
    r.border_right_color = Some(col(c));
    r.border_bottom_color = Some(col(c));
    r.border_left_color = Some(col(c));
}

/// The outer card every component renders into.
fn card_sheet() -> Rc<StyleSheet> {
    let mut r = StyleRules {
        background: Some(col("#ffffff")),
        flex_direction: Some(FlexDirection::Column),
        align_items: Some(AlignItems::Stretch),
        gap: Some(px(12.0)),
        min_width: Some(px(220.0)),
        max_width: Some(px(320.0)),
        shadow: Some(Shadow { x: 0.0, y: 10.0, blur: 30.0, color: Color("rgba(17,24,39,0.10)".into()) }),
        ..Default::default()
    };
    padding(&mut r, 22.0, 20.0);
    radius(&mut r, 16.0);
    border(&mut r, 1.0, "#eceef3");
    sheet(r)
}

/// Primary heading text (Greeter).
fn title_sheet() -> Rc<StyleSheet> {
    sheet(StyleRules {
        font_size: Some(px(19.0)),
        font_weight: Some(FontWeight::SemiBold),
        color: Some(col("#0f172a")),
        ..Default::default()
    })
}

/// Small muted eyebrow label (Stepper).
fn eyebrow_sheet() -> Rc<StyleSheet> {
    sheet(StyleRules {
        font_size: Some(px(13.0)),
        font_weight: Some(FontWeight::Medium),
        color: Some(col("#6b7280")),
        ..Default::default()
    })
}

/// The big accent value (Stepper).
fn value_sheet() -> Rc<StyleSheet> {
    sheet(StyleRules {
        font_size: Some(px(30.0)),
        font_weight: Some(FontWeight::Bold),
        color: Some(col("#4f46e5")),
        ..Default::default()
    })
}

/// A row that spreads the value and the button apart.
fn row_sheet() -> Rc<StyleSheet> {
    sheet(StyleRules {
        flex_direction: Some(FlexDirection::Row),
        align_items: Some(AlignItems::Center),
        justify_content: Some(JustifyContent::SpaceBetween),
        gap: Some(px(16.0)),
        ..Default::default()
    })
}

/// A left-aligned row so a button keeps its natural width inside a
/// stretch-aligned card (instead of spanning the full width).
fn button_row_sheet() -> Rc<StyleSheet> {
    sheet(StyleRules {
        flex_direction: Some(FlexDirection::Row),
        justify_content: Some(JustifyContent::FlexStart),
        ..Default::default()
    })
}

/// The accent button shared by both components.
fn button_sheet() -> Rc<StyleSheet> {
    let mut r = StyleRules {
        background: Some(col("#4f46e5")),
        color: Some(col("#ffffff")),
        font_size: Some(px(14.0)),
        font_weight: Some(FontWeight::SemiBold),
        align_items: Some(AlignItems::Center),
        justify_content: Some(JustifyContent::Center),
        cursor: Some(Cursor::Pointer),
        shadow: Some(Shadow { x: 0.0, y: 4.0, blur: 12.0, color: Color("rgba(79,70,229,0.35)".into()) }),
        ..Default::default()
    };
    padding(&mut r, 18.0, 10.0);
    radius(&mut r, 10.0);
    sheet(r)
}

// ---------------------------------------------------------------------------
// Components
// ---------------------------------------------------------------------------

/// A friendly greeter. String prop + void callback.
#[derive(Default, IdealystSchema)]
pub struct GreeterProps {
    /// Who to greet. Updating it from JS re-renders the text.
    pub name: Reactive<String>,
    /// Fired when the "Greet" button is pressed.
    pub on_greet: Option<Rc<dyn Fn()>>,
}

#[component(external)]
pub fn Greeter(props: &GreeterProps) -> Element {
    let name = props.name.clone();
    let on_greet = props.on_greet.clone();
    ui! {
        view(style = card_sheet()) {
            text(move || format!("Hello, {}!", name.get())).with_style(title_sheet())
            view(style = button_row_sheet()) {
                button("Greet".to_string(), move || {
                    if let Some(cb) = &on_greet {
                        cb();
                    }
                })
                .with_style(button_sheet())
            }
        }
    }
}

/// A controlled stepper. Reactive string + number props, and a callback
/// that carries the requested next value. The host owns the count: pressing
/// "+1" doesn't mutate anything locally — it asks JS for `value + 1`, and
/// JS sets `value` back.
#[derive(Default, IdealystSchema)]
pub struct StepperProps {
    /// Label shown above the value.
    pub label: Reactive<String>,
    /// The current value (controlled by the host).
    pub value: Reactive<i32>,
    /// Fired with the requested next value when "+1" is pressed.
    pub on_step: Option<Rc<dyn Fn(i32)>>,
}

#[component(external)]
pub fn Stepper(props: &StepperProps) -> Element {
    let label = props.label.clone();
    let value = props.value.clone();
    let value_for_btn = value.clone();
    let on_step = props.on_step.clone();
    ui! {
        view(style = card_sheet()) {
            text(move || label.get()).with_style(eyebrow_sheet())
            view(style = row_sheet()) {
                text(move || format!("{}", value.get())).with_style(value_sheet())
                button("+1".to_string(), move || {
                    if let Some(cb) = &on_step {
                        cb(value_for_btn.get() + 1);
                    }
                })
                .with_style(button_sheet())
            }
        }
    }
}
