//! `canvas3d-demo` — the `canvas3d` SDK end to end.
//!
//! One view, the three layers a 3D screen is built from:
//!
//! - **3D scene** (`draw`): a glTF model (Khronos' CC0 WaterBottle — fetch it
//!   with `./scripts/fetch-canvas3d-demo-model.sh`), a ground grid, and — once
//!   the model is tapped — a selection box drawn on top of everything.
//! - **2D overlay** (`overlay`): a marker at the tapped point, projected from
//!   3D each frame, composited into the same frame by the renderer.
//! - **Views** stacked over the canvas: a HUD naming the GPU path the renderer
//!   brought up (WebGPU, WebGL2, Metal, …).
//!
//! Input: drag to orbit, shift-drag or two fingers to pan, wheel / pinch to
//! zoom, tap to pick.

use canvas3d::prelude::*;
use runtime_core::{
    component, signal, ui, AlignItems, Color as StyleColor, Element, Length, PointerEvents, Position,
    SafeAreaSides, Signal, StyleRules, StyleSheet, TouchEvent, TouchPhase, TouchResponse,
};
use std::cell::Cell;
use std::rc::Rc;

/// The demo model. Fetched (pinned + checksummed), never committed — see
/// `assets/README.md`.
static MODEL_GLB: &[u8] = include_bytes!("../assets/WaterBottle.glb");

/// Pick id the model is drawn with.
const MODEL_PICK_ID: u32 = 1;
/// World-space height the model is scaled to (the grid is in the same units).
const MODEL_HEIGHT: f32 = 2.0;
/// A press that travels less than this many logical px is a tap (pick), not
/// a drag (orbit).
const TAP_SLOP_PX: f32 = 6.0;

/// Scene-registry seam: the 3D renderer handles the `Canvas3d` payload.
pub fn register_scene_extensions<H>(registry: &mut runtime_scene::Registry<H>)
where
    H: runtime_vocabulary::caps::GraphicsOps + runtime_vocabulary::style_attach::StyleServices + 'static,
{
    canvas3d_wgpu::register(registry);
}

/// Android entry (mounted by the generated wrapper).
pub fn scene_app() -> Element {
    app()
}

pub fn app() -> Element {
    ui! { Viewer() }
}

/// Scale the model to `MODEL_HEIGHT` and stand it on the grid, centred.
fn placement(model: &Model) -> Mat4 {
    let b = model.bounds();
    let scale = MODEL_HEIGHT / b.size().y.max(f32::EPSILON);
    let base_centre = Vec3::new(b.center().x, b.min.y, b.center().z);
    Mat4::from_scale(Vec3::splat(scale)) * Mat4::from_translation(-base_centre)
}

/// Wrap the orbit controller's touch handler so a press that barely moves
/// picks instead: the orbit handler still sees every event (a tap rotates by
/// a few pixels at most), and on release a short press picks under the
/// finger. A second finger turns the press into a gesture, never a tap.
fn tap_to_pick(
    orbit: impl Fn(&TouchEvent) -> TouchResponse + 'static,
    view: Canvas3dHandle,
    picked: Signal<Option<[f32; 3]>>,
) -> impl Fn(&TouchEvent) -> TouchResponse + 'static {
    // (start x, start y, still a tap)
    let press: Rc<Cell<Option<(f32, f32, bool)>>> = Rc::new(Cell::new(None));
    move |ev: &TouchEvent| {
        let response = orbit(ev);
        let (x, y) = (ev.position.x, ev.position.y);
        match ev.phase {
            TouchPhase::Began if response.consumed => {
                press.set(match press.get() {
                    None => Some((x, y, true)),
                    Some((sx, sy, _)) => Some((sx, sy, false)),
                });
            }
            TouchPhase::Moved => {
                if let Some((sx, sy, tap)) = press.get() {
                    let moved = ((x - sx).powi(2) + (y - sy).powi(2)).sqrt() > TAP_SLOP_PX;
                    press.set(Some((sx, sy, tap && !moved)));
                }
            }
            TouchPhase::Ended => {
                if let Some((sx, sy, true)) = press.take() {
                    picked.set(view.pick(sx, sy).map(|hit| hit.world_pos.to_array()));
                }
            }
            TouchPhase::Cancelled => press.set(None),
            _ => {}
        }
        response
    }
}

fn fill_style() -> Rc<StyleSheet> {
    Rc::new(StyleSheet::r#static(StyleRules {
        width: Some(Length::pct(100.0).into()),
        height: Some(Length::pct(100.0).into()),
        position: Some(Position::Relative),
        ..Default::default()
    }))
}

/// A full-screen, pointer-transparent layer over the canvas that holds the
/// HUD. It asks for the safe-area insets (status bar, notch, home indicator),
/// so the HUD never sits under system chrome, and drags anywhere still reach
/// the orbit handler underneath.
fn chrome_style() -> Rc<StyleSheet> {
    Rc::new(StyleSheet::r#static(StyleRules {
        position: Some(Position::Absolute),
        top: Some(Length::Px(0.0).into()),
        left: Some(Length::Px(0.0).into()),
        right: Some(Length::Px(0.0).into()),
        bottom: Some(Length::Px(0.0).into()),
        align_items: Some(AlignItems::FlexStart),
        pointer_events: Some(PointerEvents::None),
        ..Default::default()
    }))
}

/// The HUD card, inset from the safe area's top-left corner.
fn hud_style() -> Rc<StyleSheet> {
    Rc::new(StyleSheet::r#static(StyleRules {
        margin_top: Some(Length::Px(12.0).into()),
        margin_left: Some(Length::Px(12.0).into()),
        max_width: Some(Length::Px(360.0).into()),
        padding_top: Some(Length::Px(8.0).into()),
        padding_bottom: Some(Length::Px(8.0).into()),
        padding_left: Some(Length::Px(12.0).into()),
        padding_right: Some(Length::Px(12.0).into()),
        gap: Some(Length::Px(4.0).into()),
        background: Some(StyleColor("rgba(255, 255, 255, 0.85)".into()).into()),
        color: Some(StyleColor("#1b1d22".into()).into()),
        font_size: Some(Length::Px(13.0).into()),
        pointer_events: Some(PointerEvents::None),
        ..Default::default()
    }))
}

#[component]
pub fn Viewer() -> Element {
    let model = Model::from_gltf(MODEL_GLB).expect("the bundled WaterBottle.glb is a valid glTF");
    let place = placement(&model);
    let orbit = OrbitCamera::new(OrbitConfig {
        target: Vec3::new(0.0, MODEL_HEIGHT * 0.5, 0.0),
        distance: 5.0,
        yaw: 0.6,
        pitch: 0.3,
        ..Default::default()
    });
    let view3d = Canvas3dHandle::new();
    let picked: Signal<Option<[f32; 3]>> = signal(None);
    let grid = Lines::grid(3.0, 0.25);
    let selection = Lines::box_edges(model.bounds(), place);

    let on_touch = tap_to_pick(orbit.touch_handler(&view3d), view3d.clone(), picked);
    let on_wheel = orbit.wheel_handler();

    let scene_orbit = orbit.clone();
    let draw = canvas3d::draw(move |s: &mut Scene3d| {
        s.camera(scene_orbit.camera())
            .clear(Color::rgb(0.93, 0.94, 0.96))
            .ambient(Color::WHITE, 0.35)
            .light(Light::directional(Vec3::new(-0.6, -1.0, -0.4), Color::WHITE, 2.5))
            .light(Light::directional(Vec3::new(0.8, -0.3, 0.6), Color::rgb(0.75, 0.82, 1.0), 0.8))
            .lines(&grid, Color::rgb(0.72, 0.74, 0.8), LineDepth::Tested);
        s.model(&model, place).pick_id(MODEL_PICK_ID);
        if picked.get().is_some() {
            s.lines(&selection, Color::rgb(1.0, 0.45, 0.1), LineDepth::OnTop);
        }
    });

    // 2D marker at the picked point, re-projected whenever the camera moves.
    let overlay = canvas::draw(move |s: &mut canvas::Scene| {
        let Some(p) = picked.get() else { return };
        let Some(at) = orbit.camera().project(Vec3::from(p), s.size()) else { return };
        let accent = canvas::Color::new(255, 115, 25, 255);
        s.path().add_path(canvas::Path::circle(at.x, at.y, 10.0));
        s.stroke(accent, canvas::Stroke::width(2.0));
        s.path().move_to(at.x - 16.0, at.y).line_to(at.x - 6.0, at.y).move_to(at.x + 6.0, at.y).line_to(at.x + 16.0, at.y);
        s.stroke(accent, canvas::Stroke::width(2.0));
        s.path().add_path(canvas::Path::circle(at.x, at.y, 2.5));
        s.fill(accent);
    });

    let renderer = view3d.clone();
    ui! {
        view(style = fill_style(), on_touch = on_touch, on_wheel = on_wheel) {
            { Canvas3d(Canvas3dProps { draw, overlay: Some(overlay), handle: Some(view3d) }) }
            view(style = chrome_style(), safe_area = SafeAreaSides::ALL) {
                view(style = hud_style()) {
                    text { move || format!("canvas3d · {}", renderer.renderer_info().unwrap_or_else(|| "starting…".to_string())) }
                    text { "drag: orbit · shift-drag / two fingers: pan · wheel / pinch: zoom · tap: pick" }
                }
            }
        }
    }
}
