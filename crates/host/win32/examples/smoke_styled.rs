//! Styling smoke test: owner-drawn `IdealystView` cards that exercise
//! the GDI+ renderer — anti-aliased rounded corners, per-corner radius,
//! background alpha, linear + radial gradient fills, and both uniform
//! and per-side (asymmetric) borders.
//!
//! ```text
//! cargo run -p host-win32 --example smoke_styled
//! ```

use std::rc::Rc;

use runtime_core::{
    text, view, Color, Element, Gradient, GradientKind, GradientStop, StyleRules, StyleSheet,
};

/// Set all four border sides to the same width + color — a *uniform*
/// border, which the backend collapses to one anti-aliased stroke that
/// follows the corner radius. (Setting only `border_top_*` would render
/// a top-only bar, matching every other backend's per-side semantics.)
fn uniform_border(rules: &mut StyleRules, width: f32, color: &str) {
    rules.border_top_width = Some(width.into());
    rules.border_right_width = Some(width.into());
    rules.border_bottom_width = Some(width.into());
    rules.border_left_width = Some(width.into());
    rules.border_top_color = Some(color.into());
    rules.border_right_color = Some(color.into());
    rules.border_bottom_color = Some(color.into());
    rules.border_left_color = Some(color.into());
}

/// A solid-color rounded card with a uniform white border and fixed size.
fn card(bg: &str, radius: f32) -> Rc<StyleSheet> {
    let mut rules = StyleRules {
        background: Some(bg.into()),
        border_top_left_radius: Some(radius.into()),
        border_top_right_radius: Some(radius.into()),
        border_bottom_right_radius: Some(radius.into()),
        border_bottom_left_radius: Some(radius.into()),
        width: Some(280.0.into()),
        height: Some(110.0.into()),
        ..Default::default()
    };
    uniform_border(&mut rules, 3.0, "#ffffff");
    Rc::new(StyleSheet::r#static(rules))
}

/// A card filled with a linear gradient (angle in CSS degrees) instead of
/// a solid background.
fn gradient_card(angle_deg: f32, stops: &[(f32, &str)], radius: f32) -> Rc<StyleSheet> {
    Rc::new(StyleSheet::r#static(StyleRules {
        background_gradient: Some(Gradient {
            kind: GradientKind::Linear { angle_deg },
            stops: stops
                .iter()
                .map(|(offset, c)| GradientStop { offset: *offset, color: Color(c.to_string()) })
                .collect(),
        }),
        border_top_left_radius: Some(radius.into()),
        border_top_right_radius: Some(radius.into()),
        border_bottom_right_radius: Some(radius.into()),
        border_bottom_left_radius: Some(radius.into()),
        width: Some(280.0.into()),
        height: Some(110.0.into()),
        ..Default::default()
    }))
}

/// A light card with a thick colored BOTTOM accent bar only — the
/// asymmetric per-side border path (rendered as a straight bar, not a
/// corner-tracing stroke).
fn bottom_accent_card() -> Rc<StyleSheet> {
    Rc::new(StyleSheet::r#static(StyleRules {
        background: Some("#f2f2f7".into()),
        border_bottom_width: Some(6.0.into()),
        border_bottom_color: Some("#2f6fed".into()),
        width: Some(280.0.into()),
        height: Some(110.0.into()),
        ..Default::default()
    }))
}

fn app() -> Element {
    view(vec![
        // Opaque blue, big radius → clearly anti-aliased corners.
        view(vec![text("Rounded • uniform border").into()])
            .with_style(card("#2f6fed", 28.0))
            .into(),
        // Semi-transparent red (alpha 0.65) blends over the white window.
        view(vec![text("Alpha 0.65 • small radius").into()])
            .with_style(card("rgba(220, 50, 50, 0.65)", 8.0))
            .into(),
        // Linear gradient, top→bottom (180°), blue → purple.
        view(vec![text("Linear gradient 180°").into()])
            .with_style(gradient_card(
                180.0,
                &[(0.0, "#2f6fed"), (1.0, "#8b3fd4")],
                16.0,
            ))
            .into(),
        // Asymmetric per-side border: a bottom accent bar.
        view(vec![text("Bottom accent (per-side border)").into()])
            .with_style(bottom_accent_card())
            .into(),
    ])
    .into()
}

fn main() {
    let opts = host_win32::RunOptions {
        title: "Idealyst — Win32 styling smoke".to_string(),
        width: 360,
        height: 560,
    };
    std::process::exit(host_win32::run(opts, app));
}
