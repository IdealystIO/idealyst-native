//! `canvas-demo` — exercises the `canvas` SDK end to end.
//!
//! Every card is a `canvas::Canvas` whose `draw` closure builds a
//! renderer-agnostic [`Scene`](canvas::Scene); `canvas-native` replays it
//! into a `<canvas>` 2D context on web. The cards cover the full Phase-2
//! drawing surface: paths + fills, strokes + caps/joins, linear/radial
//! gradients, and transforms + clip driven by a per-frame animation
//! signal (proving the reactive repaint path).
//!
//! Canvas boxes are a fixed logical size (320×200) so painters draw in
//! known coordinates — size-aware/responsive painting is a deliberate
//! later enhancement, not needed to demonstrate the renderer.

use canvas::prelude::*;
// link the chosen canvas renderer so its inventory self-registration survives DCE
use canvas_native as _;
use idea_ui::{install_idea_theme, light_theme, Stack, StackGap, StackPadding, Typography};
use runtime_core::{
    raf_loop_scoped, signal, ui, view, Element, IntoElement, Length, StyleRules, StyleSheet,
};
use std::rc::Rc;

/// No per-platform registration needed: the canvas renderer external
/// self-registers via `inventory::submit!` at backend construction (see
/// [[project_inventory_self_registration]]). The `use canvas_native as _;`
/// above keeps the renderer crate linked so its inventory entry survives DCE.
pub fn register_extensions<B: runtime_core::Backend>(_backend: &mut B) {}

/// Fixed logical canvas size every card draws into.
const W: f32 = 320.0;
const H: f32 = 200.0;

pub fn app() -> Element {
    install_idea_theme(light_theme());

    // Animation clock for the last card: one shared scope-owned rAF loop
    // ticks a signal; the canvas painter reads it, so each frame re-runs
    // the painter through the renderer's reactive Effect.
    let t = signal!(0.0_f32);
    raf_loop_scoped(move || t.set(t.get() + 0.015));

    let body: Vec<Element> = vec![
        ui! { Typography(content = "Canvas".to_string(), kind = idea_ui::typography_kind::H1) },
        ui! {
            Typography(
                content = "Programmatic 2D drawing. Each card builds a renderer-agnostic \
                    `Scene`; `canvas-native` replays it into a `<canvas>` 2D context."
                    .to_string(),
                muted = true,
            )
        },
        card("Paths & fills", draw_paths),
        card("Strokes, caps & joins", draw_strokes),
        card("Gradients (linear + radial)", draw_gradients),
        card("Transforms · clip · animation", move |s: &mut Scene| draw_animated(s, t.get())),
    ];

    ui! {
        scroll_view {
            Stack(gap = StackGap::Lg, padding = StackPadding::Lg) { body }
        }
    }
}

/// A titled card wrapping a fixed-size canvas that runs `draw_fn`.
fn card<F: Fn(&mut Scene) + 'static>(title: &str, draw_fn: F) -> Element {
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

    let canvas_el = canvas::Canvas(CanvasProps { draw: canvas::draw(draw_fn), ..Default::default() })
        .with_style(Rc::new(StyleSheet::r#static(fill)))
        .into_element();
    let boxed = view(vec![canvas_el])
        .with_style(Rc::new(StyleSheet::r#static(box_rules)))
        .into_element();

    let title_el =
        ui! { Typography(content = title.to_string(), kind = idea_ui::typography_kind::H3) };

    ui! {
        Stack(gap = StackGap::Sm) { vec![title_el, boxed] }
    }
}

/// Fill the canvas background and stroke a subtle border so each card's
/// bounds are visible. Called first by every painter.
fn frame(s: &mut Scene) {
    s.path().add_path(Path::rect(0.0, 0.0, W, H));
    s.fill(Color::new(255, 255, 255, 255));
    s.path().add_path(Path::rect(0.5, 0.5, W - 1.0, H - 1.0));
    s.stroke(Color::new(225, 228, 235, 255), Stroke::width(1.0));
}

fn draw_paths(s: &mut Scene) {
    frame(s);

    // Rounded rect with a gradient fill.
    s.path().add_path(Path::rounded_rect(24.0, 32.0, 120.0, 100.0, 18.0));
    s.fill(Paint::linear(
        24.0,
        32.0,
        144.0,
        132.0,
        vec![
            GradientStop::new(0.0, color("#5b8cff")),
            GradientStop::new(1.0, color("#9b5bff")),
        ],
    ));

    // A filled circle.
    s.path().add_path(Path::circle(228.0, 70.0, 42.0));
    s.fill(color("#22c55e"));

    // A cubic-Bézier "leaf" blob.
    s.path()
        .move_to(190.0, 150.0)
        .cubic_to(210.0, 110.0, 280.0, 110.0, 300.0, 150.0)
        .cubic_to(280.0, 190.0, 210.0, 190.0, 190.0, 150.0)
        .close();
    s.fill(color("#f59e0b"));

    // A stroked triangle.
    s.path().move_to(40.0, 180.0).line_to(95.0, 150.0).line_to(150.0, 180.0).close();
    s.stroke(color("#ef4444"), Stroke::width(3.0).join(LineJoin::Round));
}

fn draw_strokes(s: &mut Scene) {
    frame(s);

    // Three horizontal strokes, one per line cap, same width.
    let caps = [
        (LineCap::Butt, color("#5b8cff")),
        (LineCap::Round, color("#22c55e")),
        (LineCap::Square, color("#f59e0b")),
    ];
    for (i, (cap, c)) in caps.into_iter().enumerate() {
        let y = 40.0 + i as f32 * 36.0;
        s.path().move_to(48.0, y).line_to(200.0, y);
        s.stroke(c, Stroke::width(14.0).cap(cap));
    }

    // Miter vs round vs bevel joins on a zig-zag.
    let joins = [LineJoin::Miter, LineJoin::Round, LineJoin::Bevel];
    for (i, join) in joins.into_iter().enumerate() {
        let x = 232.0 + i as f32 * 0.0;
        let y = 40.0 + i as f32 * 36.0;
        s.path().move_to(x, y + 14.0).line_to(x + 24.0, y - 12.0).line_to(x + 48.0, y + 14.0);
        s.stroke(color("#9b5bff"), Stroke::width(8.0).join(join));
    }

    // A stroked star outline at the bottom.
    s.path().add_path(star(95.0, 165.0, 26.0, 11.0, 5));
    s.stroke(color("#ef4444"), Stroke::width(2.5).join(LineJoin::Round));
}

fn draw_gradients(s: &mut Scene) {
    frame(s);

    // Linear "rainbow" rectangle.
    s.path().add_path(Path::rounded_rect(24.0, 28.0, 130.0, 144.0, 12.0));
    s.fill(Paint::linear(
        24.0,
        28.0,
        24.0,
        172.0,
        vec![
            GradientStop::new(0.0, color("#ef4444")),
            GradientStop::new(0.5, color("#f59e0b")),
            GradientStop::new(1.0, color("#22c55e")),
        ],
    ));

    // Radial gradient disc — center-light to edge-dark.
    s.path().add_path(Path::circle(238.0, 100.0, 72.0));
    s.fill(Paint::radial(
        238.0,
        100.0,
        72.0,
        vec![
            GradientStop::new(0.0, color("#ffffff")),
            GradientStop::new(0.25, color("#9b5bff")),
            GradientStop::new(1.0, color("#1e1b4b")),
        ],
    ));
}

fn draw_animated(s: &mut Scene, t: f32) {
    frame(s);

    // Clip everything to a centered circle, then spin a gradient-filled
    // star inside it — exercises save/clip/translate/rotate/restore.
    s.save();
    s.path().add_path(Path::circle(W * 0.5, H * 0.5, 86.0));
    s.clip();

    s.translate(W * 0.5, H * 0.5);
    s.rotate(t);
    // Outer radius (118) overflows the 86px clip circle on purpose, so the
    // circular clip visibly crops the star's spikes as it spins.
    s.path().add_path(star(0.0, 0.0, 118.0, 46.0, 6));
    s.fill(Paint::linear(
        -118.0,
        -118.0,
        118.0,
        118.0,
        vec![
            GradientStop::new(0.0, color("#5b8cff")),
            GradientStop::new(1.0, color("#ec4899")),
        ],
    ));
    s.stroke(color("#1e293b"), Stroke::width(2.0).join(LineJoin::Round));

    s.restore();
}

/// Build a star/burst path with `points` spikes between `outer` and
/// `inner` radii, centered at `(cx, cy)`.
fn star(cx: f32, cy: f32, outer: f32, inner: f32, points: u32) -> Path {
    use std::f32::consts::{FRAC_PI_2, PI};
    let mut p = Path::new();
    let n = points * 2;
    for i in 0..n {
        let r = if i % 2 == 0 { outer } else { inner };
        let ang = -FRAC_PI_2 + i as f32 * PI / points as f32;
        let (x, y) = (cx + r * ang.cos(), cy + r * ang.sin());
        p = if i == 0 { p.move_to(x, y) } else { p.line_to(x, y) };
    }
    p.close()
}
