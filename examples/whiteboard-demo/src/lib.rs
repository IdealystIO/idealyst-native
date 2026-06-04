//! `whiteboard-demo` — a slick cross-platform whiteboard.
//!
//! Pieces, and how they fit:
//!
//! 1. **Drawable canvas** (`canvas` SDK). A full-screen `canvas::Canvas`
//!    is the base layer. Freehand drawing is driven by a raw `on_touch`
//!    handler on the wrapping `view`: `Began` starts a stroke with the
//!    current width + color, `Moved` appends a point, `Ended`/`Cancelled`
//!    finalizes it. Strokes live in a shared `Rc<RefCell<Vec<Stroke>>>`; a
//!    `version` signal ticks on every mutation so the canvas painter (which
//!    reads `version`) repaints through the renderer's reactive `Effect`.
//!
//! 2. **Floating tool rail** (right edge, inside `screen_recorder::
//!    PrivateLayer` so it's excluded from the recording). A collapse toggle
//!    expands/animates in the tools (stroke widths, a color button that
//!    opens an in-tree popover, clear, camera). Everything is `presence`-
//!    animated and reactive.
//!
//! 3. **Camera widget** (NORMAL recordable content): `Camera::open` →
//!    `MediaStream` → cover-fit `video::Video`. Draggable anywhere on the
//!    canvas (clamped to the safe area), so it appears in the recording
//!    wherever the user parks it.
//!
//! 4. **Record control**: a camera-style start/stop button docked
//!    bottom-center; while recording it becomes a stop button and slides to
//!    the bottom-right. A separate **recording overlay** (top-right, inside
//!    the PrivateLayer) shows a REC pill + a live, capture-excluded preview
//!    of the very `MediaStream` being recorded — no infinite-mirror.
//!
//! On iOS/Android the floating chrome + camera bounds are kept inside the
//! `safe_area_insets()` even though the app is full-screen.

use camera::{Camera, CameraConfig, CameraFacing, MediaStream};
use icons_lucide::{CAMERA, SETTINGS, TRASH_2, X};
use runtime_core::{
    icon, presence, safe_area_insets, signal, text, view, viewport_size, AlignItems, Color, Easing,
    Element, FlexDirection, IntoElement, JustifyContent, Length, Overflow, Position, PresenceAnim,
    PresenceState, Signal, StyleApplication, StyleRules, StyleSheet, Tokenized, TouchPhase,
    TouchResponse,
};
use screen_recorder::{PrivateLayer, RecordingConfig, ScreenRecorder};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

// ============================================================================
// Per-platform external registration
// ============================================================================
//
// The CLI-generated wrapper hands us the concrete backend. We register THREE
// externals: the canvas renderer (so the drawable surface paints), the video
// display (camera + recording-preview), and the screen-recorder (which installs
// the `PrivateLayer` capture-excluded overlay window). `camera` needs no
// register. Several backends now self-register via `inventory`, so these are
// belt-and-suspenders for the ones that don't.

#[cfg(target_arch = "wasm32")]
pub fn register_extensions(backend: &mut backend_web::WebBackend) {
    canvas_native::register(backend);
    video::register(backend);
    screen_recorder::register(backend);
}

#[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
pub fn register_extensions(backend: &mut backend_ios::IosBackend) {
    canvas_native::register(backend);
    video::register(backend);
    screen_recorder::register(backend);
}

#[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
pub fn register_extensions(backend: &mut backend_android::AndroidBackend) {
    canvas_native::register(backend);
    video::register(backend);
    screen_recorder::register(backend);
}

#[cfg(all(target_os = "macos", not(target_arch = "wasm32")))]
pub fn register_extensions(backend: &mut backend_macos::MacosBackend) {
    canvas_native::register(backend);
    video::register(backend);
    screen_recorder::register(backend);
}

#[cfg(not(any(
    target_arch = "wasm32",
    target_os = "ios",
    target_os = "android",
    target_os = "macos"
)))]
pub fn register_extensions<B: runtime_core::Backend>(_backend: &mut B) {}

// ============================================================================
// Drawing model
// ============================================================================

/// One completed (or in-progress) freehand stroke: a polyline plus the width +
/// color it was drawn with. Stored renderer-agnostically as logical pixels in
/// the canvas's coordinate space (which equals the wrapping view's local space,
/// so `TouchEvent::position` maps 1:1).
#[derive(Clone)]
struct Stroke {
    points: Vec<(f32, f32)>,
    width: f32,
    rgba: (u8, u8, u8, u8),
}

/// Shared mutable list of strokes. The `on_touch` handler mutates it and the
/// canvas painter reads it; a `version` signal bridges the two so a mutation
/// triggers a reactive repaint without cloning the whole vec into a signal.
type Strokes = Rc<RefCell<Vec<Stroke>>>;

/// The live media-writer recording handle, shared between the record button's
/// start (sets it) and stop (consumes it). `!Send`, main-thread only.
type RecHandle = Rc<RefCell<Option<media_writer::Recording>>>;

/// Which full-screen surface is showing. The board is always mounted; Settings
/// and Preview are overlays on top of it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum AppScreen {
    Board,
    Settings,
    Preview,
}

/// The palette of swatch colors, as `(label, css)`. Black is first (default).
const PALETTE: &[(&str, &str)] = &[
    ("black", "#111827"),
    ("red", "#ef4444"),
    ("orange", "#f59e0b"),
    ("green", "#22c55e"),
    ("blue", "#3b82f6"),
    ("violet", "#8b5cf6"),
];

/// Stroke-width presets for the thin / medium / thick buttons.
const WIDTH_THIN: f32 = 2.0;
const WIDTH_MEDIUM: f32 = 6.0;
const WIDTH_THICK: f32 = 14.0;

/// Camera widget box (draggable, recordable content).
const CAM_W: f32 = 132.0;
const CAM_H: f32 = 176.0;
/// Keep dragged content this far from the safe-area edges.
const DRAG_MARGIN: f32 = 8.0;

/// Tool-rail metrics — a button is square; the rail is the button + padding.
const TOOL_BTN: f32 = 44.0;
const RAIL_EDGE: f32 = 14.0; // gap from the screen edge (added to safe inset)

pub fn app() -> Element {
    idea_ui::install_idea_theme(idea_ui::light_theme());

    // ---- State -----------------------------------------------------------
    let width: Signal<f32> = signal!(WIDTH_MEDIUM);
    let color_css: Signal<&'static str> = signal!(PALETTE[0].1);

    let cam_on: Signal<bool> = signal!(false);
    let cam_stream: Signal<Option<MediaStream>> = signal!(None);
    // Camera widget top-left, in viewport points. `-1` = "not yet placed"; an
    // Effect drops it bottom-right once the viewport size is known.
    let cam_x: Signal<f32> = signal!(-1.0);
    let cam_y: Signal<f32> = signal!(-1.0);

    let recording: Signal<bool> = signal!(false);
    let rec_stream: Signal<Option<MediaStream>> = signal!(None);
    // Recording → file. `rec_handle` holds the live media-writer `Recording`
    // (consumed on stop); `rec_path` is the finished file's store-relative path.
    let rec_handle: RecHandle = Rc::new(RefCell::new(None));
    let rec_path: Signal<Option<String>> = signal!(None);

    // Which screen is showing. The board is always mounted underneath; Settings
    // and Preview are full-screen overlays in the capture-excluded layer.
    let screen: Signal<AppScreen> = signal!(AppScreen::Board);

    // UI chrome state.
    let palette_open: Signal<bool> = signal!(false);

    // Strokes + a repaint tick.
    let strokes: Strokes = Rc::new(RefCell::new(Vec::new()));
    let version: Signal<u64> = signal!(0);

    // Place the camera widget bottom-right the first time we learn the viewport
    // size (it's 0×0 before the first layout). Guarded so a later drag isn't
    // reset; re-runs on viewport/inset change but no-ops once placed.
    {
        let placed = Rc::new(Cell::new(false));
        let _ = runtime_core::Effect::new(move || {
            let vp = viewport_size().get();
            let ins = safe_area_insets().get();
            if !placed.get() && vp.width > 1.0 && vp.height > 1.0 {
                // Default bottom-LEFT — the tool rail + record button live on
                // the right, so the left corner stays clear and grabbable.
                cam_x.set(ins.left + 16.0);
                cam_y.set((vp.height - ins.bottom - CAM_H - 16.0).max(ins.top + DRAG_MARGIN));
                placed.set(true);
            }
        });
    }

    // ---- The drawable canvas (base layer) --------------------------------
    let canvas_el = build_canvas(strokes.clone(), version);
    let canvas_surface =
        build_drawing_surface(canvas_el, strokes.clone(), version, width, color_css);

    // ---- Camera widget (draggable, recordable) ---------------------------
    let camera_widget = build_camera_widget(cam_on, cam_stream, cam_x, cam_y);

    // ---- Capture-excluded chrome (inside the PrivateLayer) ---------------
    let tool_rail = build_tool_rail(
        width,
        color_css,
        cam_on,
        cam_stream,
        palette_open,
        strokes.clone(),
        version,
    );
    let palette = build_palette_popover(color_css, palette_open);
    let rec_dock = build_record_dock(recording, rec_stream, rec_handle.clone(), rec_path, screen);
    let rec_indicator = build_rec_indicator(recording);
    let settings_btn = build_settings_button(recording, screen);
    let settings_screen = build_settings_screen(screen);
    let preview_screen = build_preview_screen(rec_path, screen);
    // Order matters: the full-screen Settings / Preview overlays go LAST so
    // they sit on top of the board chrome when active.
    let private_layer = PrivateLayer(vec![
        rec_indicator,
        palette,
        tool_rail,
        rec_dock,
        settings_btn,
        settings_screen,
        preview_screen,
    ])
    .into_element();

    // ---- Root: canvas base + camera over it + the private overlay --------
    let fill_root = StyleRules {
        width: Some(Length::pct(100.0).into()),
        height: Some(Length::pct(100.0).into()),
        position: Some(Position::Relative),
        ..Default::default()
    };
    view(vec![canvas_surface, camera_widget, private_layer])
        .with_style(Rc::new(StyleSheet::r#static(fill_root)))
        .into_element()
}

// ============================================================================
// Canvas + drawing surface (unchanged from the original — it works)
// ============================================================================

fn build_canvas(strokes: Strokes, version: Signal<u64>) -> Element {
    use canvas::prelude::*;

    let fill = StyleRules {
        width: Some(Length::pct(100.0).into()),
        height: Some(Length::pct(100.0).into()),
        ..Default::default()
    };

    canvas::Canvas(CanvasProps {
        draw: canvas::draw(move |s: &mut Scene| {
            let _ = version.get(); // reactive repaint dependency

            s.path().add_path(Path::rect(0.0, 0.0, 100_000.0, 100_000.0));
            s.fill(Color::new(255, 255, 255, 255));

            for stroke in strokes.borrow().iter() {
                paint_stroke(s, stroke);
            }
        }),
        ..Default::default()
    })
    .with_style(Rc::new(StyleSheet::r#static(fill)))
    .into_element()
}

fn paint_stroke(s: &mut canvas::Scene, stroke: &Stroke) {
    use canvas::prelude::*;

    let (r, g, b, a) = stroke.rgba;
    let col = Color::new(r, g, b, a);

    if stroke.points.len() == 1 {
        let (x, y) = stroke.points[0];
        s.path()
            .add_path(Path::circle(x, y, (stroke.width * 0.5).max(1.0)));
        s.fill(col);
        return;
    }

    let mut first = true;
    for &(x, y) in &stroke.points {
        if first {
            s.path().move_to(x, y);
            first = false;
        } else {
            s.line_to(x, y);
        }
    }
    s.stroke(
        col,
        Stroke::width(stroke.width)
            .cap(LineCap::Round)
            .join(LineJoin::Round),
    );
}

fn build_drawing_surface(
    canvas_el: Element,
    strokes: Strokes,
    version: Signal<u64>,
    width: Signal<f32>,
    color_css: Signal<&'static str>,
) -> Element {
    let fill = StyleRules {
        width: Some(Length::pct(100.0).into()),
        height: Some(Length::pct(100.0).into()),
        position: Some(Position::Absolute),
        top: Some(Length::Px(0.0).into()),
        left: Some(Length::Px(0.0).into()),
        ..Default::default()
    };

    let active: Rc<RefCell<Option<runtime_core::TouchId>>> = Rc::new(RefCell::new(None));

    view(vec![canvas_el])
        .with_style(Rc::new(StyleSheet::r#static(fill)))
        .on_touch(move |ev| {
            let mut active = active.borrow_mut();
            match ev.phase {
                TouchPhase::Began => {
                    if active.is_some() {
                        return TouchResponse::IGNORED;
                    }
                    *active = Some(ev.id);
                    let rgba = parse_rgba(color_css.get());
                    strokes.borrow_mut().push(Stroke {
                        points: vec![(ev.position.x, ev.position.y)],
                        width: width.get(),
                        rgba,
                    });
                    version.set(version.get().wrapping_add(1));
                    TouchResponse::CLAIMED
                }
                TouchPhase::Moved => {
                    if *active != Some(ev.id) {
                        return TouchResponse::IGNORED;
                    }
                    if let Some(last) = strokes.borrow_mut().last_mut() {
                        last.points.push((ev.position.x, ev.position.y));
                    }
                    version.set(version.get().wrapping_add(1));
                    TouchResponse::CONSUMED
                }
                TouchPhase::Ended | TouchPhase::Cancelled => {
                    if *active != Some(ev.id) {
                        return TouchResponse::IGNORED;
                    }
                    *active = None;
                    if ev.phase == TouchPhase::Ended {
                        if let Some(last) = strokes.borrow_mut().last_mut() {
                            last.points.push((ev.position.x, ev.position.y));
                        }
                        version.set(version.get().wrapping_add(1));
                    }
                    TouchResponse::CONSUMED
                }
            }
        })
        .into_element()
}

fn parse_rgba(css: &str) -> (u8, u8, u8, u8) {
    let c = runtime_core::color::parse_or(css, runtime_core::color::Rgba::BLACK);
    (c.r, c.g, c.b, c.a)
}

// ============================================================================
// Reactive-style helper (closure form → StyleSource::Reactive)
// ============================================================================

/// Wrap a signal-reading `StyleRules` builder into a REACTIVE style source.
/// `.with_style(Rc<StyleSheet>)` resolves once and memoizes; a closure
/// `Fn() -> StyleApplication` re-runs whenever a signal it reads changes.
fn reactive_style(f: impl Fn() -> StyleRules + 'static) -> impl Fn() -> StyleApplication {
    move || StyleApplication::new(Rc::new(StyleSheet::r#static(f())))
}

/// Shorthand: equal border-radius on all corners.
fn radius(px: f32) -> StyleRules {
    StyleRules {
        border_top_left_radius: Some(Length::Px(px).into()),
        border_top_right_radius: Some(Length::Px(px).into()),
        border_bottom_left_radius: Some(Length::Px(px).into()),
        border_bottom_right_radius: Some(Length::Px(px).into()),
        ..Default::default()
    }
}

/// Equal border on all sides.
fn border_all(px: f32, color: &str) -> StyleRules {
    let c = Tokenized::Literal(Color(color.to_string()));
    StyleRules {
        border_top_width: Some(px.into()),
        border_bottom_width: Some(px.into()),
        border_left_width: Some(px.into()),
        border_right_width: Some(px.into()),
        border_top_color: Some(c.clone()),
        border_bottom_color: Some(c.clone()),
        border_left_color: Some(c.clone()),
        border_right_color: Some(c),
        ..Default::default()
    }
}

/// Overlay `extra`'s set fields onto `base`. Lets the radius/border shorthands
/// compose with a base `StyleRules` literal.
fn merge(base: &mut StyleRules, extra: StyleRules) {
    macro_rules! take {
        ($($f:ident),* $(,)?) => { $( if extra.$f.is_some() { base.$f = extra.$f; } )* };
    }
    take!(
        border_top_left_radius,
        border_top_right_radius,
        border_bottom_left_radius,
        border_bottom_right_radius,
        border_top_width,
        border_bottom_width,
        border_left_width,
        border_right_width,
        border_top_color,
        border_bottom_color,
        border_left_color,
        border_right_color,
    );
}

// ============================================================================
// Camera widget (draggable, cover-fit, recordable content)
// ============================================================================

fn build_camera_widget(
    cam_on: Signal<bool>,
    cam_stream: Signal<Option<MediaStream>>,
    cam_x: Signal<f32>,
    cam_y: Signal<f32>,
) -> Element {
    // The live feed, cover-fit so it fills the rounded box (no letterboxing).
    let fill = StyleRules {
        width: Some(Length::pct(100.0).into()),
        height: Some(Length::pct(100.0).into()),
        ..Default::default()
    };
    let video_el = video::Video(video::VideoProps {
        source: video::stream(move || cam_stream.get()),
        autoplay: true,
        object_fit: video::ObjectFit::Cover,
        ..Default::default()
    })
    .with_style(Rc::new(StyleSheet::r#static(fill)))
    .into_element();

    // Drag state: (start_touch_x, start_touch_y, start_cam_x, start_cam_y).
    let drag: Rc<RefCell<Option<(f32, f32, f32, f32)>>> = Rc::new(RefCell::new(None));

    // A TRANSPARENT overlay that fills the widget and carries the drag handler.
    // Putting the handler here (not on the wrapper) makes the overlay the
    // direct hit-test target on every backend — the video child can't swallow
    // the press (the earlier "can't move the camera" bug: on macOS the video
    // NSView was the hit target and the wrapper never saw the drag). It claims
    // the gesture so the canvas underneath doesn't draw.
    let drag_overlay = view(vec![])
        .with_style(Rc::new(StyleSheet::r#static(StyleRules {
            position: Some(Position::Absolute),
            top: Some(Length::Px(0.0).into()),
            left: Some(Length::Px(0.0).into()),
            right: Some(Length::Px(0.0).into()),
            bottom: Some(Length::Px(0.0).into()),
            ..Default::default()
        })))
        .on_touch(move |ev| match ev.phase {
            TouchPhase::Began => {
                *drag.borrow_mut() = Some((
                    ev.window_position.x,
                    ev.window_position.y,
                    cam_x.get(),
                    cam_y.get(),
                ));
                TouchResponse::CLAIMED
            }
            TouchPhase::Moved => {
                let start = *drag.borrow();
                if let Some((sx, sy, cx, cy)) = start {
                    let vp = viewport_size().get();
                    let ins = safe_area_insets().get();
                    let min_x = ins.left + DRAG_MARGIN;
                    let min_y = ins.top + DRAG_MARGIN;
                    let max_x = (vp.width - ins.right - CAM_W - DRAG_MARGIN).max(min_x);
                    let max_y = (vp.height - ins.bottom - CAM_H - DRAG_MARGIN).max(min_y);
                    cam_x.set((cx + (ev.window_position.x - sx)).clamp(min_x, max_x));
                    cam_y.set((cy + (ev.window_position.y - sy)).clamp(min_y, max_y));
                }
                TouchResponse::CONSUMED
            }
            TouchPhase::Ended | TouchPhase::Cancelled => {
                *drag.borrow_mut() = None;
                TouchResponse::CONSUMED
            }
        })
        .into_element();

    view(vec![video_el, drag_overlay])
        .with_style(reactive_style(move || {
            if !cam_on.get() {
                // Collapsed + parked off-flow while the camera is off.
                return StyleRules {
                    position: Some(Position::Absolute),
                    width: Some(Length::Px(0.0).into()),
                    height: Some(Length::Px(0.0).into()),
                    overflow: Some(Overflow::Hidden),
                    ..Default::default()
                };
            }
            let mut s = StyleRules {
                position: Some(Position::Absolute),
                left: Some(Length::Px(cam_x.get().max(0.0)).into()),
                top: Some(Length::Px(cam_y.get().max(0.0)).into()),
                width: Some(Length::Px(CAM_W).into()),
                height: Some(Length::Px(CAM_H).into()),
                background: Some(Tokenized::Literal(Color("#0b1220".into()))),
                overflow: Some(Overflow::Hidden),
                ..Default::default()
            };
            merge(&mut s, radius(18.0));
            merge(&mut s, border_all(2.0, "rgba(255,255,255,0.9)"));
            s
        }))
        .into_element()
}

// ============================================================================
// Floating tool rail (right edge, inside the PrivateLayer)
// ============================================================================

#[allow(clippy::too_many_arguments)]
fn build_tool_rail(
    width: Signal<f32>,
    color_css: Signal<&'static str>,
    cam_on: Signal<bool>,
    cam_stream: Signal<Option<MediaStream>>,
    palette_open: Signal<bool>,
    strokes: Strokes,
    version: Signal<u64>,
) -> Element {
    // Bare icon/shape buttons on a soft frosted rail — always visible (no
    // collapse). The icon IS the affordance; no per-button background.
    let pill = view(vec![
        width_button(WIDTH_THIN, width),
        width_button(WIDTH_MEDIUM, width),
        width_button(WIDTH_THICK, width),
        rail_divider(),
        color_button(color_css, palette_open),
        clear_button(strokes, version),
        rail_divider(),
        camera_toggle(cam_on, cam_stream),
    ])
    .with_style(Rc::new(StyleSheet::r#static({
        let mut s = StyleRules {
            flex_direction: Some(FlexDirection::Column),
            align_items: Some(AlignItems::Center),
            gap: Some(Length::Px(2.0).into()),
            padding_top: Some(Length::Px(8.0).into()),
            padding_bottom: Some(Length::Px(8.0).into()),
            padding_left: Some(Length::Px(6.0).into()),
            padding_right: Some(Length::Px(6.0).into()),
            background: Some(Tokenized::Literal(Color("rgba(255,255,255,0.92)".into()))),
            ..Default::default()
        };
        merge(&mut s, radius(24.0));
        merge(&mut s, border_all(1.0, "rgba(17,24,39,0.08)"));
        s
    })))
    .into_element();

    dock_right(pill)
}

/// Position a child vertically centered against the right edge, inset by the
/// safe area. The dock fills the screen but only lays the child out at center-
/// right; the empty area passes touches through (it has no background).
fn dock_right(child: Element) -> Element {
    view(vec![child])
        .with_style(reactive_style(move || {
            let ins = safe_area_insets().get();
            StyleRules {
                position: Some(Position::Absolute),
                top: Some(Length::Px(0.0).into()),
                bottom: Some(Length::Px(0.0).into()),
                right: Some(Length::Px(RAIL_EDGE + ins.right).into()),
                flex_direction: Some(FlexDirection::Column),
                justify_content: Some(JustifyContent::Center),
                align_items: Some(AlignItems::FlexEnd),
                ..Default::default()
            }
        }))
        .into_element()
}

/// A bare `TOOL_BTN`-sized tap target — no background, content centered. The
/// icon/shape inside is the whole affordance.
fn bare_btn(content: Element, on_press: impl Fn() + 'static) -> Element {
    view(vec![content])
        .with_style(Rc::new(StyleSheet::r#static(StyleRules {
            width: Some(Length::Px(TOOL_BTN).into()),
            height: Some(Length::Px(TOOL_BTN).into()),
            align_items: Some(AlignItems::Center),
            justify_content: Some(JustifyContent::Center),
            ..Default::default()
        })))
        .on_touch(move |ev| {
            if ev.phase == TouchPhase::Ended {
                on_press();
            }
            TouchResponse::CONSUMED
        })
        .into_element()
}

/// Wrap an `icon(...)` element so it renders at a consistent 22×22 box.
fn icon_box(el: Element) -> Element {
    view(vec![el])
        .with_style(Rc::new(StyleSheet::r#static(StyleRules {
            width: Some(Length::Px(22.0).into()),
            height: Some(Length::Px(22.0).into()),
            align_items: Some(AlignItems::Center),
            justify_content: Some(JustifyContent::Center),
            ..Default::default()
        })))
        .into_element()
}

/// A horizontal divider inside the vertical rail.
fn rail_divider() -> Element {
    view(vec![])
        .with_style(Rc::new(StyleSheet::r#static(StyleRules {
            width: Some(Length::Px(24.0).into()),
            height: Some(Length::Px(1.0).into()),
            background: Some(Tokenized::Literal(Color("rgba(17,24,39,0.12)".into()))),
            ..Default::default()
        })))
        .into_element()
}

/// A stroke-width button: a bare filled dot whose size tracks the stroke width
/// it sets. Accent-blue when selected, muted grey otherwise — color, not a
/// background box, carries the state.
fn width_button(w: f32, width: Signal<f32>) -> Element {
    let dot = view(vec![])
        .with_style(reactive_style(move || {
            let selected = (width.get() - w).abs() < f32::EPSILON;
            let d = 6.0 + w; // dot grows with the stroke width it represents
            let mut s = StyleRules {
                width: Some(Length::Px(d).into()),
                height: Some(Length::Px(d).into()),
                background: Some(Tokenized::Literal(Color(
                    if selected { "#2563eb" } else { "#9ca3af" }.to_string(),
                ))),
                ..Default::default()
            };
            merge(&mut s, radius(d / 2.0));
            s
        }))
        .into_element();
    bare_btn(dot, move || width.set(w))
}

/// The color button: a bare disc of the current color with a thin ring (so a
/// light color still reads on the rail). Tapping toggles the palette popover.
fn color_button(color_css: Signal<&'static str>, palette_open: Signal<bool>) -> Element {
    let disc = view(vec![])
        .with_style(reactive_style(move || {
            let open = palette_open.get();
            let mut s = StyleRules {
                width: Some(Length::Px(22.0).into()),
                height: Some(Length::Px(22.0).into()),
                background: Some(Tokenized::Literal(Color(color_css.get().to_string()))),
                ..Default::default()
            };
            merge(&mut s, radius(11.0));
            merge(
                &mut s,
                border_all(
                    if open { 2.0 } else { 1.5 },
                    if open { "#2563eb" } else { "rgba(17,24,39,0.28)" },
                ),
            );
            s
        }))
        .into_element();
    bare_btn(disc, move || palette_open.set(!palette_open.get()))
}

/// Clear the board — a bare trash icon.
fn clear_button(strokes: Strokes, version: Signal<u64>) -> Element {
    let glyph = icon_box(icon(TRASH_2).color(|| Color::from("#374151")).into_element());
    bare_btn(glyph, move || {
        strokes.borrow_mut().clear();
        version.set(version.get().wrapping_add(1));
    })
}

/// Camera on/off toggle: a bare camera icon, green when live, grey when off.
fn camera_toggle(cam_on: Signal<bool>, cam_stream: Signal<Option<MediaStream>>) -> Element {
    let glyph = icon_box(
        icon(CAMERA)
            .color(move || {
                if cam_on.get() {
                    Color::from("#16a34a")
                } else {
                    Color::from("#374151")
                }
            })
            .into_element(),
    );
    bare_btn(glyph, move || {
        if cam_on.get() {
            cam_on.set(false);
            cam_stream.set(None);
        } else {
            cam_on.set(true);
            runtime_core::driver::spawn_async(async move {
                let config = CameraConfig {
                    facing: CameraFacing::Front,
                    ..Default::default()
                };
                match Camera::new().open(config).await {
                    Ok(stream) => cam_stream.set(Some(stream)),
                    Err(_) => cam_on.set(false),
                }
            });
        }
    })
}

// ============================================================================
// Color palette popover (in-tree presence panel, left of the rail)
// ============================================================================

fn build_palette_popover(color_css: Signal<&'static str>, palette_open: Signal<bool>) -> Element {
    let panel = presence(move || {
        let mut swatches: Vec<Element> = Vec::new();
        for (_label, css) in PALETTE {
            swatches.push(swatch(css, color_css, palette_open));
        }
        let grid = view(swatches)
            .with_style(Rc::new(StyleSheet::r#static(StyleRules {
                flex_direction: Some(FlexDirection::Row),
                flex_wrap: Some(runtime_core::FlexWrap::Wrap),
                width: Some(Length::Px(108.0).into()),
                gap: Some(Length::Px(10.0).into()),
                align_items: Some(AlignItems::Center),
                justify_content: Some(JustifyContent::Center),
                ..Default::default()
            })))
            .into_element();
        view(vec![grid])
            .with_style(Rc::new(StyleSheet::r#static({
                let mut s = StyleRules {
                    padding_top: Some(Length::Px(12.0).into()),
                    padding_bottom: Some(Length::Px(12.0).into()),
                    padding_left: Some(Length::Px(12.0).into()),
                    padding_right: Some(Length::Px(12.0).into()),
                    background: Some(Tokenized::Literal(Color("rgba(255,255,255,0.97)".into()))),
                    ..Default::default()
                };
                merge(&mut s, radius(18.0));
                merge(&mut s, border_all(1.0, "rgba(17,24,39,0.08)"));
                s
            })))
            .into_element()
    })
    .present(move || palette_open.get())
    .enter(PresenceAnim::new(
        PresenceState {
            opacity: Some(0.0),
            translate_x: Some(12.0),
            scale: Some(0.96),
            ..Default::default()
        },
        170,
        Easing::EaseOut,
    ))
    .exit(PresenceAnim::new(
        PresenceState {
            opacity: Some(0.0),
            translate_x: Some(12.0),
            scale: Some(0.96),
            ..Default::default()
        },
        130,
        Easing::EaseIn,
    ))
    .into_element();

    // Dock to the right edge, offset left of the rail so it sits beside the
    // color button. Vertically centered, safe-area aware.
    view(vec![panel])
        .with_style(reactive_style(move || {
            let ins = safe_area_insets().get();
            let rail_w = TOOL_BTN + 16.0 + 12.0; // button + rail padding + gap
            StyleRules {
                position: Some(Position::Absolute),
                top: Some(Length::Px(0.0).into()),
                bottom: Some(Length::Px(0.0).into()),
                right: Some(Length::Px(RAIL_EDGE + ins.right + rail_w).into()),
                flex_direction: Some(FlexDirection::Column),
                justify_content: Some(JustifyContent::Center),
                align_items: Some(AlignItems::FlexEnd),
                ..Default::default()
            }
        }))
        .into_element()
}

/// A color swatch in the popover. Tapping sets the color and closes the popover.
fn swatch(css: &'static str, color_css: Signal<&'static str>, palette_open: Signal<bool>) -> Element {
    view(vec![])
        .with_style(reactive_style(move || {
            let selected = color_css.get() == css;
            let mut s = StyleRules {
                width: Some(Length::Px(28.0).into()),
                height: Some(Length::Px(28.0).into()),
                background: Some(Tokenized::Literal(Color(css.to_string()))),
                ..Default::default()
            };
            merge(&mut s, radius(14.0));
            merge(&mut s, border_all(if selected { 3.0 } else { 0.0 }, "#1f2937"));
            s
        }))
        .on_touch(move |ev| {
            if ev.phase == TouchPhase::Ended {
                color_css.set(css);
                palette_open.set(false);
            }
            TouchResponse::CONSUMED
        })
        .into_element()
}

// ============================================================================
// Record control — camera-style start/stop button
// ============================================================================

fn build_record_dock(
    recording: Signal<bool>,
    rec_stream: Signal<Option<MediaStream>>,
    rec_handle: RecHandle,
    rec_path: Signal<Option<String>>,
    screen: Signal<AppScreen>,
) -> Element {
    let button = record_button(recording, rec_stream, rec_handle, rec_path, screen);

    // A full-width bottom dock; idle → centered, recording → slid to the right.
    view(vec![button])
        .with_style(reactive_style(move || {
            let ins = safe_area_insets().get();
            StyleRules {
                position: Some(Position::Absolute),
                left: Some(Length::Px(0.0).into()),
                right: Some(Length::Px(0.0).into()),
                bottom: Some(Length::Px(28.0 + ins.bottom).into()),
                flex_direction: Some(FlexDirection::Row),
                align_items: Some(AlignItems::Center),
                justify_content: Some(if recording.get() {
                    JustifyContent::FlexEnd
                } else {
                    JustifyContent::Center
                }),
                padding_right: Some(
                    Length::Px(if recording.get() { 24.0 + ins.right } else { 0.0 }).into(),
                ),
                ..Default::default()
            }
        }))
        .into_element()
}

/// The record button: a white ring with a red core. Idle = red disc (record);
/// recording = red rounded square (stop). Starts a screen capture AND a
/// media-writer recording of it to a file; stopping finalizes the file and
/// opens the Preview screen.
fn record_button(
    recording: Signal<bool>,
    rec_stream: Signal<Option<MediaStream>>,
    rec_handle: RecHandle,
    rec_path: Signal<Option<String>>,
    screen: Signal<AppScreen>,
) -> Element {
    // Inner core morphs disc ↔ square via reactive radius + size.
    let core = view(vec![])
        .with_style(reactive_style(move || {
            let rec = recording.get();
            let size = if rec { 26.0 } else { 44.0 };
            let mut s = StyleRules {
                width: Some(Length::Px(size).into()),
                height: Some(Length::Px(size).into()),
                background: Some(Tokenized::Literal(Color("#ef4444".into()))),
                ..Default::default()
            };
            merge(&mut s, radius(if rec { 7.0 } else { 22.0 }));
            s
        }))
        .into_element();

    view(vec![core])
        .with_style(Rc::new(StyleSheet::r#static({
            let mut s = StyleRules {
                width: Some(Length::Px(64.0).into()),
                height: Some(Length::Px(64.0).into()),
                align_items: Some(AlignItems::Center),
                justify_content: Some(JustifyContent::Center),
                background: Some(Tokenized::Literal(Color("rgba(255,255,255,0.96)".into()))),
                ..Default::default()
            };
            merge(&mut s, radius(32.0));
            merge(&mut s, border_all(3.0, "rgba(17,24,39,0.12)"));
            s
        })))
        .on_touch(move |ev| {
            if ev.phase != TouchPhase::Ended {
                return TouchResponse::CONSUMED;
            }
            if recording.get() {
                // STOP → finalize the file, then open the Preview screen.
                recording.set(false);
                let rec_handle = rec_handle.clone();
                runtime_core::driver::spawn_async(async move {
                    // Bind the take() out of the RefMut so we don't hold the
                    // borrow across `.await` (see refmut-lifetime memory).
                    let taken = rec_handle.borrow_mut().take();
                    if let Some(rec) = taken {
                        if let Ok(path) = rec.stop().await {
                            rec_path.set(Some(path));
                            screen.set(AppScreen::Preview);
                        }
                    }
                    rec_stream.set(None); // drop the stream → capture stops
                });
            } else {
                // START → screen capture + media-writer recording to a file.
                recording.set(true);
                let rec_handle = rec_handle.clone();
                runtime_core::driver::spawn_async(async move {
                    let stream = match ScreenRecorder::new().start(RecordingConfig::new()).await {
                        Ok(s) => s,
                        Err(_) => {
                            recording.set(false);
                            return;
                        }
                    };
                    let store = match files::app_files(REC_STORE) {
                        Ok(s) => s,
                        Err(_) => {
                            recording.set(false);
                            return;
                        }
                    };
                    let cfg = media_writer::RecordConfig::new(store, REC_FILE);
                    match media_writer::MediaWriter::new()
                        .record(media_writer::MediaInputs::video(&stream), cfg)
                        .await
                    {
                        Ok(rec) => {
                            *rec_handle.borrow_mut() = Some(rec);
                            // Keep the stream alive so capture (and the encoder
                            // subscription feeding off it) keeps running.
                            rec_stream.set(Some(stream));
                        }
                        Err(_) => recording.set(false), // stream drops → capture stops
                    }
                });
            }
            TouchResponse::CONSUMED
        })
        .into_element()
}

/// The `files` store + filename a recording is written to.
const REC_STORE: &str = "recordings";
const REC_FILE: &str = "recording.mp4";

// ============================================================================
// Minimal REC indicator (top-center, inside the PrivateLayer — NOT recorded)
// ============================================================================

fn build_rec_indicator(recording: Signal<bool>) -> Element {
    let pill = presence(move || {
        let dot = view(vec![])
            .with_style(Rc::new(StyleSheet::r#static({
                let mut s = StyleRules {
                    width: Some(Length::Px(9.0).into()),
                    height: Some(Length::Px(9.0).into()),
                    background: Some(Tokenized::Literal(Color("#ef4444".into()))),
                    ..Default::default()
                };
                merge(&mut s, radius(5.0));
                s
            })))
            .into_element();
        view(vec![dot, text("REC").into_element()])
            .with_style(Rc::new(StyleSheet::r#static({
                let mut s = StyleRules {
                    flex_direction: Some(FlexDirection::Row),
                    align_items: Some(AlignItems::Center),
                    gap: Some(Length::Px(7.0).into()),
                    padding_top: Some(Length::Px(6.0).into()),
                    padding_bottom: Some(Length::Px(6.0).into()),
                    padding_left: Some(Length::Px(12.0).into()),
                    padding_right: Some(Length::Px(12.0).into()),
                    background: Some(Tokenized::Literal(Color("rgba(17,24,39,0.82)".into()))),
                    color: Some(Tokenized::Literal(Color("#ffffff".into()))),
                    ..Default::default()
                };
                merge(&mut s, radius(13.0));
                s
            })))
            .into_element()
    })
    .present(move || recording.get())
    .enter(PresenceAnim::new(
        PresenceState { opacity: Some(0.0), translate_y: Some(-8.0), ..Default::default() },
        180,
        Easing::EaseOut,
    ))
    .exit(PresenceAnim::new(
        PresenceState { opacity: Some(0.0), translate_y: Some(-8.0), ..Default::default() },
        130,
        Easing::EaseIn,
    ))
    .into_element();

    // Dock top-center, safe-area aware.
    view(vec![pill])
        .with_style(reactive_style(move || {
            let ins = safe_area_insets().get();
            StyleRules {
                position: Some(Position::Absolute),
                top: Some(Length::Px(16.0 + ins.top).into()),
                left: Some(Length::Px(0.0).into()),
                right: Some(Length::Px(0.0).into()),
                flex_direction: Some(FlexDirection::Row),
                justify_content: Some(JustifyContent::Center),
                ..Default::default()
            }
        }))
        .into_element()
}

// ============================================================================
// Settings — a FAB (top-left, while not recording) + a placeholder screen
// ============================================================================

fn build_settings_button(recording: Signal<bool>, screen: Signal<AppScreen>) -> Element {
    let fab = presence(move || {
        let glyph = icon_box(icon(SETTINGS).color(|| Color::from("#374151")).into_element());
        view(vec![glyph])
            .with_style(Rc::new(StyleSheet::r#static({
                let mut s = StyleRules {
                    width: Some(Length::Px(44.0).into()),
                    height: Some(Length::Px(44.0).into()),
                    align_items: Some(AlignItems::Center),
                    justify_content: Some(JustifyContent::Center),
                    background: Some(Tokenized::Literal(Color("rgba(255,255,255,0.92)".into()))),
                    ..Default::default()
                };
                merge(&mut s, radius(22.0));
                merge(&mut s, border_all(1.0, "rgba(17,24,39,0.08)"));
                s
            })))
            .on_touch(move |ev| {
                if ev.phase == TouchPhase::Ended {
                    screen.set(AppScreen::Settings);
                }
                TouchResponse::CONSUMED
            })
            .into_element()
    })
    // Only when NOT recording.
    .present(move || !recording.get())
    .enter(PresenceAnim::new(
        PresenceState { opacity: Some(0.0), scale: Some(0.9), ..Default::default() },
        160,
        Easing::EaseOut,
    ))
    .exit(PresenceAnim::new(
        PresenceState { opacity: Some(0.0), scale: Some(0.9), ..Default::default() },
        120,
        Easing::EaseIn,
    ))
    .into_element();

    view(vec![fab])
        .with_style(reactive_style(move || {
            let ins = safe_area_insets().get();
            StyleRules {
                position: Some(Position::Absolute),
                top: Some(Length::Px(16.0 + ins.top).into()),
                left: Some(Length::Px(16.0 + ins.left).into()),
                ..Default::default()
            }
        }))
        .into_element()
}

fn build_settings_screen(screen: Signal<AppScreen>) -> Element {
    let header = screen_header("Settings", move || screen.set(AppScreen::Board));
    let rows = view(vec![
        setting_row("Smooth strokes", true),
        setting_row("Show grid", false),
        setting_row("Pressure sensitivity", false),
        setting_row("High-quality recording", true),
    ])
    .with_style(Rc::new(StyleSheet::r#static(StyleRules {
        flex_direction: Some(FlexDirection::Column),
        gap: Some(Length::Px(2.0).into()),
        padding_left: Some(Length::Px(20.0).into()),
        padding_right: Some(Length::Px(20.0).into()),
        ..Default::default()
    })))
    .into_element();
    let note = text("Placeholder — these don't do anything yet.").into_element();
    let note_box = view(vec![note])
        .with_style(Rc::new(StyleSheet::r#static(StyleRules {
            padding_left: Some(Length::Px(20.0).into()),
            padding_top: Some(Length::Px(8.0).into()),
            color: Some(Tokenized::Literal(Color("#9ca3af".into()))),
            ..Default::default()
        })))
        .into_element();
    screen_overlay(
        move || screen.get() == AppScreen::Settings,
        vec![header, rows, note_box],
    )
}

/// One placeholder settings row: a label + a static pill "switch".
fn setting_row(label: &'static str, on: bool) -> Element {
    let knob_x = if on { JustifyContent::FlexEnd } else { JustifyContent::FlexStart };
    let track_bg = if on { "#2563eb" } else { "#d1d5db" };
    let knob = view(vec![])
        .with_style(Rc::new(StyleSheet::r#static({
            let mut s = StyleRules {
                width: Some(Length::Px(18.0).into()),
                height: Some(Length::Px(18.0).into()),
                background: Some(Tokenized::Literal(Color("#ffffff".into()))),
                ..Default::default()
            };
            merge(&mut s, radius(9.0));
            s
        })))
        .into_element();
    let track = view(vec![knob])
        .with_style(Rc::new(StyleSheet::r#static({
            let mut s = StyleRules {
                width: Some(Length::Px(40.0).into()),
                height: Some(Length::Px(24.0).into()),
                padding_left: Some(Length::Px(3.0).into()),
                padding_right: Some(Length::Px(3.0).into()),
                flex_direction: Some(FlexDirection::Row),
                align_items: Some(AlignItems::Center),
                justify_content: Some(knob_x),
                background: Some(Tokenized::Literal(Color(track_bg.into()))),
                ..Default::default()
            };
            merge(&mut s, radius(12.0));
            s
        })))
        .into_element();
    view(vec![text(label).into_element(), track])
        .with_style(Rc::new(StyleSheet::r#static(StyleRules {
            flex_direction: Some(FlexDirection::Row),
            align_items: Some(AlignItems::Center),
            justify_content: Some(JustifyContent::SpaceBetween),
            padding_top: Some(Length::Px(14.0).into()),
            padding_bottom: Some(Length::Px(14.0).into()),
            // Explicit dark text — the macOS default label color follows the
            // SYSTEM appearance (white in dark mode), which would be invisible
            // on this light screen.
            color: Some(Tokenized::Literal(Color("#111827".into()))),
            ..Default::default()
        })))
        .into_element()
}

// ============================================================================
// Preview / export screen (after a recording stops)
// ============================================================================

fn build_preview_screen(rec_path: Signal<Option<String>>, screen: Signal<AppScreen>) -> Element {
    let header = screen_header("Recording", move || screen.set(AppScreen::Board));

    // The recorded file plays back via a REACTIVE url — resolved from `rec_path`
    // (a `file://` to the store's real path on native; empty on web, where blob
    // playback needs an async read, leaving the dark stage). Reactive so it
    // populates when a recording finishes, not at build time.
    let stage_video = video::Video(video::VideoProps {
        source: video::url(move || {
            rec_path
                .get()
                .and_then(|p| recording_url(&p))
                .unwrap_or_default()
        }),
        autoplay: true,
        controls: true,
        loop_playback: true,
        object_fit: video::ObjectFit::Contain,
    })
    .with_style(Rc::new(StyleSheet::r#static(StyleRules {
        width: Some(Length::pct(100.0).into()),
        height: Some(Length::pct(100.0).into()),
        ..Default::default()
    })))
    .into_element();
    let stage_box = view(vec![stage_video])
        .with_style(Rc::new(StyleSheet::r#static({
            let mut s = StyleRules {
                flex_grow: Some(Tokenized::Literal(1.0)),
                margin_left: Some(Length::Px(20.0).into()),
                margin_right: Some(Length::Px(20.0).into()),
                background: Some(Tokenized::Literal(Color("#0b1220".into()))),
                overflow: Some(Overflow::Hidden),
                align_items: Some(AlignItems::Center),
                justify_content: Some(JustifyContent::Center),
                ..Default::default()
            };
            merge(&mut s, radius(16.0));
            s
        })))
        .into_element();

    // Actions: Discard (delete + back) · Export (save via picker).
    let discard = action_button("Discard", false, move || {
        if let Some(p) = rec_path.get() {
            runtime_core::driver::spawn_async(async move {
                if let Ok(store) = files::app_files(REC_STORE) {
                    let _ = store.delete(&p).await;
                }
            });
        }
        rec_path.set(None);
        screen.set(AppScreen::Board);
    });
    let export = action_button("Export", true, move || {
        if let Some(p) = rec_path.get() {
            runtime_core::driver::spawn_async(async move {
                if let Ok(store) = files::app_files(REC_STORE) {
                    if let Ok(Some(bytes)) = store.read(&p).await {
                        let req = file_export::SaveRequest::bytes(REC_FILE, "video/mp4", bytes);
                        let _ = file_export::FileExport::new().save(req).await;
                    }
                }
            });
        }
    });
    let actions = view(vec![discard, export])
        .with_style(Rc::new(StyleSheet::r#static(StyleRules {
            flex_direction: Some(FlexDirection::Row),
            gap: Some(Length::Px(12.0).into()),
            justify_content: Some(JustifyContent::Center),
            padding_top: Some(Length::Px(16.0).into()),
            padding_bottom: Some(Length::Px(20.0).into()),
            padding_left: Some(Length::Px(20.0).into()),
            padding_right: Some(Length::Px(20.0).into()),
            ..Default::default()
        })))
        .into_element();

    screen_overlay(
        move || screen.get() == AppScreen::Preview,
        vec![header, stage_box, actions],
    )
}

/// Resolve a played-back URL for a recorded file. Native → a `file://` URL via
/// the store's real path. Web → `None` (a blob URL needs an async read; the
/// Preview screen shows a card there instead).
fn recording_url(path: &str) -> Option<String> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let store = files::app_files(REC_STORE).ok()?;
        let p = store.local_path(path)?;
        Some(format!("file://{}", p.display()))
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = path;
        None
    }
}

// ============================================================================
// Shared screen chrome
// ============================================================================

/// An always-mounted, opaque full-screen Settings/Preview overlay that toggles
/// on `active`. When inactive it collapses to `0×0` (clipped) so it's invisible
/// AND passes touches through to the board — an always-present full-screen
/// layer would block every click. When active it fills the viewport.
///
/// We use this reactive-size pattern (the camera-widget idiom that lays out
/// reliably on every backend) instead of wrapping the content in a `presence`:
/// a `presence`'s absolutely-positioned child resolves its `inset:0` against
/// the presence PLACEHOLDER, not the viewport, so on macOS/Taffy the screen
/// never actually filled the window (the "no background" bug). As a direct
/// PrivateLayer child, `inset:0` fills the full-screen overlay correctly.
fn screen_overlay(active: impl Fn() -> bool + 'static, children: Vec<Element>) -> Element {
    view(children)
        .with_style(reactive_style(move || {
            if !active() {
                return StyleRules {
                    position: Some(Position::Absolute),
                    width: Some(Length::Px(0.0).into()),
                    height: Some(Length::Px(0.0).into()),
                    overflow: Some(Overflow::Hidden),
                    ..Default::default()
                };
            }
            let ins = safe_area_insets().get();
            StyleRules {
                position: Some(Position::Absolute),
                top: Some(Length::Px(0.0).into()),
                left: Some(Length::Px(0.0).into()),
                right: Some(Length::Px(0.0).into()),
                bottom: Some(Length::Px(0.0).into()),
                flex_direction: Some(FlexDirection::Column),
                padding_top: Some(Length::Px(12.0 + ins.top).into()),
                padding_bottom: Some(Length::Px(ins.bottom).into()),
                background: Some(Tokenized::Literal(Color("#f7f8fb".into()))),
                // Explicit dark text — macOS's default label color follows the
                // system appearance (white in dark mode).
                color: Some(Tokenized::Literal(Color("#111827".into()))),
                ..Default::default()
            }
        }))
        .into_element()
}

/// A screen header: a title + a close (×) button that runs `on_close`.
fn screen_header(title: &'static str, on_close: impl Fn() + 'static) -> Element {
    let close = view(vec![icon_box(icon(X).color(|| Color::from("#374151")).into_element())])
        .with_style(Rc::new(StyleSheet::r#static({
            let mut s = StyleRules {
                width: Some(Length::Px(40.0).into()),
                height: Some(Length::Px(40.0).into()),
                align_items: Some(AlignItems::Center),
                justify_content: Some(JustifyContent::Center),
                background: Some(Tokenized::Literal(Color("rgba(17,24,39,0.05)".into()))),
                ..Default::default()
            };
            merge(&mut s, radius(20.0));
            s
        })))
        .on_touch(move |ev| {
            if ev.phase == TouchPhase::Ended {
                on_close();
            }
            TouchResponse::CONSUMED
        })
        .into_element();
    view(vec![text(title).into_element(), close])
        .with_style(Rc::new(StyleSheet::r#static(StyleRules {
            flex_direction: Some(FlexDirection::Row),
            align_items: Some(AlignItems::Center),
            justify_content: Some(JustifyContent::SpaceBetween),
            padding_left: Some(Length::Px(20.0).into()),
            padding_right: Some(Length::Px(16.0).into()),
            padding_top: Some(Length::Px(8.0).into()),
            padding_bottom: Some(Length::Px(12.0).into()),
            // Explicit dark title text (macOS dark-mode default is white).
            color: Some(Tokenized::Literal(Color("#111827".into()))),
            ..Default::default()
        })))
        .into_element()
}

/// A labeled action button. `primary` → filled blue; else neutral.
fn action_button(label: &'static str, primary: bool, on_press: impl Fn() + 'static) -> Element {
    let (bg, fg) = if primary {
        ("#2563eb", "#ffffff")
    } else {
        ("rgba(17,24,39,0.06)", "#111827")
    };
    let lbl = view(vec![text(label).into_element()])
        .with_style(Rc::new(StyleSheet::r#static(StyleRules {
            color: Some(Tokenized::Literal(Color(fg.into()))),
            ..Default::default()
        })))
        .into_element();
    view(vec![lbl])
        .with_style(Rc::new(StyleSheet::r#static({
            let mut s = StyleRules {
                height: Some(Length::Px(46.0).into()),
                padding_left: Some(Length::Px(28.0).into()),
                padding_right: Some(Length::Px(28.0).into()),
                align_items: Some(AlignItems::Center),
                justify_content: Some(JustifyContent::Center),
                background: Some(Tokenized::Literal(Color(bg.into()))),
                ..Default::default()
            };
            merge(&mut s, radius(23.0));
            s
        })))
        .on_touch(move |ev| {
            if ev.phase == TouchPhase::Ended {
                on_press();
            }
            TouchResponse::CONSUMED
        })
        .into_element()
}
