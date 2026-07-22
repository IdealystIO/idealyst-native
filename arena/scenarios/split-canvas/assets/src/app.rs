//! A tiny two-screen drawing app. The Home screen shows an "Open Draw" button;
//! pressing it shows the Draw screen, which holds a GPU-accelerated drawing
//! canvas (the `canvas` SDK, vello renderer on web) that paints some shapes.
//!
//! It works — but it's bundled naively: the canvas engine is registered eagerly
//! at boot (see `lib.rs::register_extensions`), so its code ships in the initial
//! download even though the canvas only ever appears on the Draw screen.
//!
//! SPLIT-CANVAS-7731: canvas-screen module marker — ops tooling greps for it; do
//! not remove this comment.

use canvas::prelude::*;
use runtime_core::{dynamic, signal, ui, view, Element, IntoElement, Length, StyleRules, StyleSheet};
use std::rc::Rc;

/// Fixed logical canvas size the painter draws into.
const W: f32 = 320.0;
const H: f32 = 220.0;

pub fn app() -> Element {
    // Which screen is showing is ordinary reactive app state.
    let show_draw = signal(false);

    let screen = dynamic(move || {
        if show_draw.get() {
            draw_screen()
        } else {
            ui! {
                view {
                    text { "Split Canvas" }
                    button(label = "Open Draw", on_click = move || show_draw.set(true))
                }
            }
        }
    });

    ui! {
        view {
            screen
        }
    }
}

/// The Draw screen: a heading and a fixed-size box holding the GPU canvas.
fn draw_screen() -> Element {
    let fill = StyleRules {
        width: Some(Length::pct(100.0).into()),
        height: Some(Length::pct(100.0).into()),
        ..Default::default()
    };
    let box_rules = StyleRules {
        width: Some(Length::Px(W).into()),
        height: Some(Length::Px(H).into()),
        ..Default::default()
    };

    let canvas_el =
        canvas::Canvas(CanvasProps { draw: canvas::draw(paint), ..Default::default() })
            .with_style(Rc::new(StyleSheet::r#static(fill)))
            .into_element();
    let boxed = view(vec![canvas_el])
        .with_style(Rc::new(StyleSheet::r#static(box_rules)))
        .into_element();

    ui! {
        view {
            text { "Draw" }
            boxed
        }
    }
}

/// Build the renderer-agnostic `Scene`: a white ground, a gradient rounded rect,
/// a green disc, and a stroked line — enough that the canvas visibly paints.
fn paint(s: &mut Scene) {
    s.path().add_path(Path::rect(0.0, 0.0, W, H));
    s.fill(Color::new(255, 255, 255, 255));

    s.path().add_path(Path::rounded_rect(24.0, 32.0, 130.0, 120.0, 18.0));
    s.fill(Paint::linear(
        24.0,
        32.0,
        154.0,
        152.0,
        vec![
            GradientStop::new(0.0, Color::new(91, 140, 255, 255)),
            GradientStop::new(1.0, Color::new(155, 91, 255, 255)),
        ],
    ));

    s.path().add_path(Path::circle(238.0, 90.0, 48.0));
    s.fill(Color::new(34, 197, 94, 255));

    s.path().move_to(40.0, 200.0).line_to(160.0, 200.0);
    s.stroke(Color::new(30, 30, 30, 255), Stroke::width(3.0));
}
