//! `whiteboard-demo` — a slick cross-platform whiteboard.
//!
//! Pieces, and how they fit:
//!
//! 1. **Drawable canvas** (`canvas` SDK). A full-screen `canvas::Canvas`
//!    is the base layer. Freehand drawing is driven by a raw `on_touch`
//!    handler on the `view` that wraps the canvas: `Began` starts a
//!    stroke with the current width + color, `Moved` appends a point,
//!    `Ended`/`Cancelled` finalizes it. Strokes live in a shared
//!    `Rc<RefCell<Vec<Stroke>>>`; a `version` signal ticks on every
//!    mutation so the canvas painter (which reads `version`) re-runs
//!    through the renderer's reactive `Effect` and repaints.
//!
//! 2. **Overlay toolbar** inside `screen_recorder::PrivateLayer` — so it
//!    renders in a separate, capture-excluded window on iOS/Android and
//!    does NOT appear in the recording. Holds 3 stroke-width buttons, a
//!    color-swatch row, a camera toggle and a record button.
//!
//! 3. **Camera widget** (bottom-right, NORMAL recordable content):
//!    `Camera::open` → `MediaStream` → `video::Video` in a fixed box.
//!
//! 4. **Record button**: `ScreenRecorder::start` → `MediaStream` held in
//!    a signal (keeps capture alive); pressing again drops it.
//!
//! 5. **Live recording preview** (center-left), rendered INSIDE the
//!    `PrivateLayer` so the preview is itself excluded from capture —
//!    avoiding the infinite-mirror feedback loop. It shows the very
//!    `MediaStream` being recorded.
//!
//! Canvas-SDK note: the canvas drawing surface here is the renderer-
//! agnostic `Scene` replayed by `canvas-native` (CoreGraphics on iOS,
//! `android.graphics` on Android, Canvas2D on web). No canvas call here
//! is anything but `Scene` building + the `Canvas` external — both
//! pure-Rust and stable. See the report for the on-device verification
//! checklist (the Android canvas renderer is actively under construction).

use camera::{Camera, CameraConfig, MediaStream};
use runtime_core::{
    signal, text, view, Color, Element, IntoElement, Length, Position, Signal, StyleRules,
    StyleSheet, Tokenized, TouchPhase, TouchResponse,
};
use screen_recorder::{PrivateLayer, RecordingConfig, ScreenRecorder};
use std::cell::RefCell;
use std::rc::Rc;

// ============================================================================
// Per-platform external registration
// ============================================================================
//
// The CLI-generated wrapper hands us the concrete backend. We register
// THREE externals: the canvas renderer (so the drawable surface paints),
// the video display (so the camera + recording-preview show), and the
// screen-recorder (which installs the `PrivateLayer` capture-excluded
// overlay window). `camera` needs no register.

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
    // macOS uses `canvas-native`'s fallback (no native module yet) — the
    // framework draws its "not supported" placeholder for the canvas, but
    // the toolbar / camera / record wiring still compiles and runs.
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

/// One completed (or in-progress) freehand stroke: a polyline plus the
/// width + color it was drawn with. Stored renderer-agnostically as
/// logical pixels in the canvas's coordinate space (which equals the
/// wrapping view's local space, so `TouchEvent::position` maps 1:1).
#[derive(Clone)]
struct Stroke {
    points: Vec<(f32, f32)>,
    width: f32,
    /// RGBA bytes (kept as a plain tuple so this module doesn't depend on
    /// a particular color type at the storage layer).
    rgba: (u8, u8, u8, u8),
}

/// Shared mutable list of strokes. The `on_touch` handler mutates it and
/// the canvas painter reads it; a `version` signal bridges the two so a
/// mutation triggers a reactive repaint without cloning the whole vec
/// into a signal on every pointer-move.
type Strokes = Rc<RefCell<Vec<Stroke>>>;

/// The palette of swatch colors, as `(label, css)` — parsed to `Rgba`
/// at use. Black is first so it's the default.
const PALETTE: &[(&str, &str)] = &[
    ("black", "#111827"),
    ("red", "#ef4444"),
    ("blue", "#3b82f6"),
    ("green", "#22c55e"),
    ("amber", "#f59e0b"),
    ("violet", "#8b5cf6"),
];

/// Stroke-width presets for the thin / medium / thick buttons.
const WIDTH_THIN: f32 = 2.0;
const WIDTH_MEDIUM: f32 = 6.0;
const WIDTH_THICK: f32 = 14.0;

/// Camera widget box (bottom-right, recordable content).
const CAM_W: f32 = 120.0;
const CAM_H: f32 = 160.0;

/// Recording-preview box (center-left, inside the private layer).
const PREVIEW_W: f32 = 140.0;
const PREVIEW_H: f32 = 200.0;

pub fn app() -> Element {
    idea_ui::install_idea_theme(idea_ui::light_theme());

    // ---- State -----------------------------------------------------------
    let width: Signal<f32> = signal!(WIDTH_MEDIUM);
    // Current draw color as a CSS string (parsed in the painter / swatch).
    let color_css: Signal<&'static str> = signal!(PALETTE[0].1);

    let cam_on: Signal<bool> = signal!(false);
    let cam_stream: Signal<Option<MediaStream>> = signal!(None);

    let recording: Signal<bool> = signal!(false);
    let rec_stream: Signal<Option<MediaStream>> = signal!(None);

    // Strokes + a repaint tick. `version` is read by the canvas painter so
    // every mutation re-runs it through the renderer's reactive Effect.
    let strokes: Strokes = Rc::new(RefCell::new(Vec::new()));
    let version: Signal<u64> = signal!(0);

    // ---- The drawable canvas (base layer) --------------------------------
    let canvas_el = build_canvas(strokes.clone(), version);
    let canvas_surface = build_drawing_surface(canvas_el, strokes.clone(), version, width, color_css);

    // ---- Camera widget (bottom-right, recordable) ------------------------
    let camera_widget = build_camera_widget(cam_on, cam_stream);

    // ---- Toolbar + recording preview (inside the PrivateLayer) -----------
    let toolbar = build_toolbar(
        width,
        color_css,
        cam_on,
        cam_stream,
        recording,
        rec_stream,
        strokes.clone(),
        version,
    );
    let rec_preview = build_recording_preview(recording, rec_stream);
    let private_layer = PrivateLayer(vec![rec_preview, toolbar]).into_element();

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
// Canvas + drawing surface
// ============================================================================

/// The `canvas::Canvas` external whose painter replays every stored
/// stroke. Reads `version` so a stroke mutation re-runs the painter.
fn build_canvas(strokes: Strokes, version: Signal<u64>) -> Element {
    use canvas::prelude::*;

    let fill = StyleRules {
        width: Some(Length::pct(100.0).into()),
        height: Some(Length::pct(100.0).into()),
        ..Default::default()
    };

    canvas::Canvas(CanvasProps {
        draw: canvas::draw(move |s: &mut Scene| {
            // Reactive dependency: bumping `version` repaints.
            let _ = version.get();

            // Paint the board background so the surface is opaque white
            // (and the strokes read clearly).
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

/// Replay one stored stroke as a rounded polyline. A single point draws a
/// filled dot (a tap), so dotting the board leaves a mark.
fn paint_stroke(s: &mut canvas::Scene, stroke: &Stroke) {
    use canvas::prelude::*;

    let (r, g, b, a) = stroke.rgba;
    let col = Color::new(r, g, b, a);

    if stroke.points.len() == 1 {
        let (x, y) = stroke.points[0];
        s.path().add_path(Path::circle(x, y, (stroke.width * 0.5).max(1.0)));
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
        Stroke::width(stroke.width).cap(LineCap::Round).join(LineJoin::Round),
    );
}

/// Wrap the canvas in a full-screen `view` that captures raw touches and
/// turns them into strokes. Touch coordinates are view-local, which match
/// the canvas's logical coordinate space 1:1.
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

    // `drawing` tracks whether the active TouchId is one we're drawing
    // with. We only ever draw the single primary touch (multi-touch
    // strokes aren't part of the MVP); a `RefCell<Option<TouchId>>` gates
    // it so a second finger doesn't fork the in-progress stroke.
    let active: Rc<RefCell<Option<runtime_core::TouchId>>> = Rc::new(RefCell::new(None));

    view(vec![canvas_el])
        .with_style(Rc::new(StyleSheet::r#static(fill)))
        .on_touch(move |ev| {
            let mut active = active.borrow_mut();
            match ev.phase {
                TouchPhase::Began => {
                    if active.is_some() {
                        // Already drawing with another finger — ignore.
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
                    // Claim so a parent scroll/gesture can't steal the drag.
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
                    // Final point for Ended; Cancelled leaves the stroke as-is.
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

/// Parse a CSS color string into RGBA bytes via the framework's canonical
/// parser, falling back to opaque black.
fn parse_rgba(css: &str) -> (u8, u8, u8, u8) {
    let c = runtime_core::color::parse_or(css, runtime_core::color::Rgba::BLACK);
    (c.r, c.g, c.b, c.a)
}

// ============================================================================
// Camera widget (recordable content, bottom-right)
// ============================================================================

fn build_camera_widget(cam_on: Signal<bool>, cam_stream: Signal<Option<MediaStream>>) -> Element {
    // Reactive presence: mount the camera box only when `cam_on`. The Video
    // carries a reactive stream source, so it populates once `cam_stream` is
    // set. The dynamically-mounted box gets sized because the backend kicks a
    // layout pass for inserts into a live (window-attached) parent — see the
    // Android `insert` / `layout_policy::insert_needs_layout_pass`.
    runtime_core::when(
        move || cam_on.get(),
        move || stream_box(cam_stream, camera_box_rules()),
        empty_view,
    )
}

/// Fixed camera box, absolutely positioned bottom-right.
fn camera_box_rules() -> StyleRules {
    StyleRules {
        position: Some(Position::Absolute),
        right: Some(Length::Px(16.0).into()),
        bottom: Some(Length::Px(16.0).into()),
        width: Some(Length::Px(CAM_W).into()),
        height: Some(Length::Px(CAM_H).into()),
        background: Some(Tokenized::Literal(Color("#000000".into()))),
        border_top_left_radius: Some(Length::Px(12.0).into()),
        border_top_right_radius: Some(Length::Px(12.0).into()),
        border_bottom_left_radius: Some(Length::Px(12.0).into()),
        border_bottom_right_radius: Some(Length::Px(12.0).into()),
        overflow: Some(runtime_core::Overflow::Hidden),
        ..Default::default()
    }
}

/// Build a `Video` that fills `box_rules`, sourced from a reactive stream
/// signal. The Video external has no intrinsic size on native, so it's
/// wrapped in an explicitly-sized box (the camera-preview-demo fix).
/// Build a `Video` that fills `box_rules`, sourced from a reactive stream
/// signal. The Video external has no intrinsic size on native, so it's wrapped
/// in an explicitly-sized box (the camera-preview-demo fix).
fn stream_box(stream_sig: Signal<Option<MediaStream>>, box_rules: StyleRules) -> Element {
    let fill = StyleRules {
        width: Some(Length::pct(100.0).into()),
        height: Some(Length::pct(100.0).into()),
        ..Default::default()
    };
    let video_el = video::Video(video::VideoProps {
        source: video::stream(move || stream_sig.get()),
        autoplay: true,
        ..Default::default()
    })
    .with_style(Rc::new(StyleSheet::r#static(fill)))
    .into_element();
    view(vec![video_el])
        .with_style(Rc::new(StyleSheet::r#static(box_rules)))
        .into_element()
}

fn empty_view() -> Element {
    view(vec![]).into_element()
}

/// An empty, zero-size view — the inert `otherwise` branch for `when`.

// ============================================================================
// Recording preview (inside PrivateLayer, center-left) — NOT recorded
// ============================================================================

fn build_recording_preview(
    recording: Signal<bool>,
    rec_stream: Signal<Option<MediaStream>>,
) -> Element {
    runtime_core::when(
        move || recording.get(),
        move || stream_box(rec_stream, preview_box_rules()),
        empty_view,
    )
}

/// Fixed recording-preview box, absolutely positioned center-left.
fn preview_box_rules() -> StyleRules {
    StyleRules {
        position: Some(Position::Absolute),
        left: Some(Length::Px(16.0).into()),
        top: Some(Length::pct(50.0).into()),
        width: Some(Length::Px(PREVIEW_W).into()),
        height: Some(Length::Px(PREVIEW_H).into()),
        background: Some(Tokenized::Literal(Color("rgba(0,0,0,0.85)".into()))),
        border_top_left_radius: Some(Length::Px(10.0).into()),
        border_top_right_radius: Some(Length::Px(10.0).into()),
        border_bottom_left_radius: Some(Length::Px(10.0).into()),
        border_bottom_right_radius: Some(Length::Px(10.0).into()),
        overflow: Some(runtime_core::Overflow::Hidden),
        ..Default::default()
    }
}

// ============================================================================
// Toolbar (inside PrivateLayer, bottom-center) — NOT recorded
// ============================================================================

#[allow(clippy::too_many_arguments)]
fn build_toolbar(
    width: Signal<f32>,
    color_css: Signal<&'static str>,
    cam_on: Signal<bool>,
    cam_stream: Signal<Option<MediaStream>>,
    recording: Signal<bool>,
    rec_stream: Signal<Option<MediaStream>>,
    strokes: Strokes,
    version: Signal<u64>,
) -> Element {
    let mut row: Vec<Element> = Vec::new();

    // -- Stroke-width buttons (thin / medium / thick) ----------------------
    row.push(width_button("·", WIDTH_THIN, width));
    row.push(width_button("—", WIDTH_MEDIUM, width));
    row.push(width_button("▬", WIDTH_THICK, width));
    row.push(separator());

    // -- Color swatches ----------------------------------------------------
    for (_label, css) in PALETTE {
        row.push(swatch(css, color_css));
    }
    row.push(separator());

    // -- Clear board -------------------------------------------------------
    {
        let strokes = strokes.clone();
        let on_clear = move || {
            strokes.borrow_mut().clear();
            version.set(version.get().wrapping_add(1));
        };
        row.push(
            view(vec![text("🗑").into_element()])
                .with_style(Rc::new(StyleSheet::r#static(tool_btn_rules("rgba(31,41,55,0.06)"))))
                .on_touch(move |ev| {
                    if ev.phase == TouchPhase::Ended {
                        on_clear();
                    }
                    TouchResponse::CONSUMED
                })
                .into_element(),
        );
    }
    row.push(separator());

    // -- Camera toggle -----------------------------------------------------
    row.push(camera_toggle(cam_on, cam_stream));

    // -- Record button -----------------------------------------------------
    row.push(record_button(recording, rec_stream));

    // The toolbar bar: a rounded pill, bottom-center, absolutely
    // positioned inside the full-screen private layer.
    let bar_rules = StyleRules {
        position: Some(Position::Absolute),
        bottom: Some(Length::Px(28.0).into()),
        // Span the width with side margins and WRAP, instead of auto-sizing +
        // centering: ~12 tools at 40px overflow a phone's width, so let the
        // row flow onto a second line and stay on-screen on any device.
        left: Some(Length::Px(12.0).into()),
        right: Some(Length::Px(12.0).into()),
        flex_direction: Some(runtime_core::FlexDirection::Row),
        flex_wrap: Some(runtime_core::FlexWrap::Wrap),
        align_items: Some(runtime_core::AlignItems::Center),
        justify_content: Some(runtime_core::JustifyContent::Center),
        gap: Some(Length::Px(8.0).into()),
        padding_top: Some(Length::Px(10.0).into()),
        padding_bottom: Some(Length::Px(10.0).into()),
        padding_left: Some(Length::Px(14.0).into()),
        padding_right: Some(Length::Px(14.0).into()),
        background: Some(Tokenized::Literal(Color("rgba(255,255,255,0.96)".into()))),
        border_top_left_radius: Some(Length::Px(22.0).into()),
        border_top_right_radius: Some(Length::Px(22.0).into()),
        border_bottom_left_radius: Some(Length::Px(22.0).into()),
        border_bottom_right_radius: Some(Length::Px(22.0).into()),
        ..Default::default()
    };

    view(row)
        .with_style(Rc::new(StyleSheet::r#static(bar_rules)))
        .into_element()
}

/// Shared base style for a round-ish tool button.
fn tool_btn_rules(bg: &str) -> StyleRules {
    StyleRules {
        width: Some(Length::Px(40.0).into()),
        height: Some(Length::Px(40.0).into()),
        align_items: Some(runtime_core::AlignItems::Center),
        justify_content: Some(runtime_core::JustifyContent::Center),
        background: Some(Tokenized::Literal(Color(bg.into()))),
        border_top_left_radius: Some(Length::Px(12.0).into()),
        border_top_right_radius: Some(Length::Px(12.0).into()),
        border_bottom_left_radius: Some(Length::Px(12.0).into()),
        border_bottom_right_radius: Some(Length::Px(12.0).into()),
        ..Default::default()
    }
}

/// A stroke-width button. Reactively highlights when `width` matches its
/// preset.
fn width_button(glyph: &'static str, w: f32, width: Signal<f32>) -> Element {
    let label = text(glyph).into_element();
    // Reactive background: selected → tinted.
    let bg = runtime_core::view(vec![label]);
    let style = Rc::new(StyleSheet::new(move |_| {
        let selected = (width.get() - w).abs() < f32::EPSILON;
        tool_btn_rules(if selected { "rgba(59,130,246,0.18)" } else { "rgba(31,41,55,0.06)" })
    }));
    bg.with_style(style)
        .on_touch(move |ev| {
            if ev.phase == TouchPhase::Ended {
                width.set(w);
            }
            TouchResponse::CONSUMED
        })
        .into_element()
}

/// A color swatch. Tapping sets `color_css`; a ring shows the selection.
fn swatch(css: &'static str, color_css: Signal<&'static str>) -> Element {
    let style = Rc::new(StyleSheet::new(move |_| {
        let selected = color_css.get() == css;
        let bw: f32 = if selected { 3.0 } else { 0.0 };
        let ring = Tokenized::Literal(Color("#1f2937".into()));
        StyleRules {
            width: Some(Length::Px(28.0).into()),
            height: Some(Length::Px(28.0).into()),
            background: Some(Tokenized::Literal(Color(css.to_string()))),
            border_top_left_radius: Some(Length::Px(14.0).into()),
            border_top_right_radius: Some(Length::Px(14.0).into()),
            border_bottom_left_radius: Some(Length::Px(14.0).into()),
            border_bottom_right_radius: Some(Length::Px(14.0).into()),
            border_top_width: Some(bw.into()),
            border_bottom_width: Some(bw.into()),
            border_left_width: Some(bw.into()),
            border_right_width: Some(bw.into()),
            border_top_color: Some(ring.clone()),
            border_bottom_color: Some(ring.clone()),
            border_left_color: Some(ring.clone()),
            border_right_color: Some(ring),
            ..Default::default()
        }
    }));
    view(vec![])
        .with_style(style)
        .on_touch(move |ev| {
            if ev.phase == TouchPhase::Ended {
                color_css.set(css);
            }
            TouchResponse::CONSUMED
        })
        .into_element()
}

/// A thin vertical separator between toolbar groups.
fn separator() -> Element {
    let rules = StyleRules {
        width: Some(Length::Px(1.0).into()),
        height: Some(Length::Px(28.0).into()),
        background: Some(Tokenized::Literal(Color("rgba(31,41,55,0.15)".into()))),
        ..Default::default()
    };
    view(vec![]).with_style(Rc::new(StyleSheet::r#static(rules))).into_element()
}

/// Camera on/off toggle. ON → open the camera and stash the stream;
/// OFF → drop the stream (stops capture).
fn camera_toggle(cam_on: Signal<bool>, cam_stream: Signal<Option<MediaStream>>) -> Element {
    let label = runtime_core::text(move || {
        if cam_on.get() { "📷●".to_string() } else { "📷".to_string() }
    })
    .into_element();
    let style = Rc::new(StyleSheet::new(move |_| {
        tool_btn_rules(if cam_on.get() { "rgba(34,197,94,0.18)" } else { "rgba(31,41,55,0.06)" })
    }));
    view(vec![label])
        .with_style(style)
        .on_touch(move |ev| {
            if ev.phase != TouchPhase::Ended {
                return TouchResponse::CONSUMED;
            }
            if cam_on.get() {
                // Turn off: drop the stream (last clone → capture stops).
                cam_on.set(false);
                cam_stream.set(None);
            } else {
                cam_on.set(true);
                runtime_core::driver::spawn_async(async move {
                    match Camera::new().open(CameraConfig::default()).await {
                        Ok(stream) => cam_stream.set(Some(stream)),
                        Err(_) => {
                            // Failed to open — revert the toggle.
                            cam_on.set(false);
                        }
                    }
                });
            }
            TouchResponse::CONSUMED
        })
        .into_element()
}

/// Record toggle. ON → start screen capture and hold the stream (keeps
/// capture alive); OFF → drop it (stops). Shows ● red while recording.
fn record_button(recording: Signal<bool>, rec_stream: Signal<Option<MediaStream>>) -> Element {
    let label = runtime_core::text(move || {
        if recording.get() { "● REC".to_string() } else { "● Rec".to_string() }
    })
    .into_element();
    let style = Rc::new(StyleSheet::new(move |_| {
        let bg = if recording.get() { "rgba(220,38,38,0.95)" } else { "rgba(220,38,38,0.12)" };
        StyleRules {
            height: Some(Length::Px(40.0).into()),
            align_items: Some(runtime_core::AlignItems::Center),
            justify_content: Some(runtime_core::JustifyContent::Center),
            padding_left: Some(Length::Px(14.0).into()),
            padding_right: Some(Length::Px(14.0).into()),
            background: Some(Tokenized::Literal(Color(bg.into()))),
            border_top_left_radius: Some(Length::Px(20.0).into()),
            border_top_right_radius: Some(Length::Px(20.0).into()),
            border_bottom_left_radius: Some(Length::Px(20.0).into()),
            border_bottom_right_radius: Some(Length::Px(20.0).into()),
            ..Default::default()
        }
    }));
    view(vec![label])
        .with_style(style)
        .on_touch(move |ev| {
            if ev.phase != TouchPhase::Ended {
                return TouchResponse::CONSUMED;
            }
            if recording.get() {
                recording.set(false);
                rec_stream.set(None);
            } else {
                recording.set(true);
                runtime_core::driver::spawn_async(async move {
                    // `ThisApp` (the default) → app-only capture: PixelCopy
                    // on Android (no consent), ReplayKit on iOS.
                    match ScreenRecorder::new().start(RecordingConfig::new()).await {
                        Ok(stream) => rec_stream.set(Some(stream)),
                        Err(_) => recording.set(false),
                    }
                });
            }
            TouchResponse::CONSUMED
        })
        .into_element()
}
