//! The board screen's recordable content: the drawable canvas + drawing
//! surface, and the draggable camera widget. The floating chrome (in-tree
//! sibling overlays) lives in [`crate::chrome`].

use crate::style::{reactive_style, static_style, token};
use crate::{
    parse_rgba, paint_stroke, BoardState, CanvasCapture, CanvasStore, MicHandle, RecHandle, Stroke,
    Strokes,
};
use runtime_core::{
    component, ui, Element, IntoElement, Length, Overflow, Position, Signal, StyleRules, Tokenized,
    TouchPhase, TouchResponse,
};
use runtime_core::animation::{AnimatedValue, TweenTo};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;


// ============================================================================
// Board screen root
// ============================================================================

/// Props for [`BoardScreen`]. Bundles the whole (Copy) [`BoardState`] plus the
/// `!Copy` shared drawing model. None of the field types derive
/// `IdealystSchema`, so the props struct skips the derive (the macro only needs
/// `Default`).
pub struct BoardScreenProps {
    pub state: BoardState,
    pub strokes: Strokes,
    /// The saved canvas documents — threaded to the chrome so the Layers popover
    /// can list / switch / add / delete them.
    pub canvases: CanvasStore,
    pub rec_handle: RecHandle,
    /// The microphone stream slot, threaded to the record chrome so START can
    /// open a mic and STOP can drop it. See [`MicHandle`].
    pub mic_handle: MicHandle,
    pub version: Signal<u64>,
    /// The canvas self-capture bundle: the writer is fed to the Canvas, the
    /// whole bundle is threaded to the record chrome.
    pub capture: CanvasCapture,
    /// `true` while the board is the active stack root (no Settings/Preview
    /// pushed). Drives the floating chrome's mount/unmount.
    pub focused: Rc<dyn Fn() -> bool>,
}

impl Default for BoardScreenProps {
    fn default() -> Self {
        // The board always receives a real `CanvasCapture` from `app()`; this
        // Default exists only to satisfy the props contract.
        Self {
            state: BoardState::default(),
            strokes: Rc::new(RefCell::new(Vec::new())),
            canvases: Rc::new(RefCell::new(Vec::new())),
            rec_handle: Rc::new(RefCell::new(None)),
            mic_handle: Rc::new(RefCell::new(None)),
            version: Signal::new(0),
            capture: CanvasCapture::default(),
            focused: Rc::new(|| true),
        }
    }
}

/// The board screen — the whiteboard itself: drawable canvas, a draggable
/// camera, and the floating in-tree chrome.
///
/// Root carries `overflow: hidden` so the full-screen app clips to the viewport
/// and a stray sub-pixel of chrome can't leak into a page-level scrollbar (the
/// board "starts scrolling" report) — a whiteboard never scrolls its root.
#[component]
pub fn BoardScreen(props: &BoardScreenProps) -> Element {
    let s = props.state;
    let strokes = props.strokes.clone();
    let canvases = props.canvases.clone();
    let rec_handle = props.rec_handle.clone();
    let mic_handle = props.mic_handle.clone();
    let version = props.version;
    let capture = props.capture.clone();
    let focused = props.focused.clone();

    // The Canvas writes each rendered frame into this writer (macOS/vello only).
    let capture_writer = capture.writer.clone();

    // The chrome (tool rail, palette, record dock, REC pill, settings FAB) as
    // individually-positioned overlays over the canvas — plain in-tree siblings,
    // no separate window. (Recording captures the canvas/GPU stream directly, so
    // the chrome is never in it.) As normal siblings the navigator also hides them
    // automatically when a screen is pushed (they belong to the board screen).
    let chrome = crate::chrome::build_chrome(
        focused,
        s,
        strokes.clone(),
        canvases.clone(),
        rec_handle,
        mic_handle,
        version,
        capture,
    );

    // Reactive so the letterbox around the stage follows the app theme (light/dark).
    let root_style = reactive_style(|| StyleRules {
        width: Some(Length::pct(100.0).into()),
        height: Some(Length::pct(100.0).into()),
        position: Some(Position::Relative),
        overflow: Some(Overflow::Hidden),
        background: Some(Tokenized::Literal(token(|c| c.background.clone()))),
        ..Default::default()
    });

    // The canvas "stage": an aspect-locked box, centered, as large as fits inside
    // the safe area — sized with explicit px from `stage_geom`. This makes
    // `stage_geom` (which the camera clamp + placement also call) the EXACT stage
    // size, so the camera bound matches the canvas across every aspect — unlike a
    // Taffy `aspect_ratio` box, whose laid-out size can drift from the formula.
    // The vello Metal surface resizes correctly when this px size changes because
    // `MetalView` now fires `on_resize` on `setFrameSize:` (the macOS backend fix).
    let aspect = s.aspect;
    let stage_style = reactive_style(move || {
        let (aw, ah) = aspect.get();
        let (x, y, w, h) = crate::settings::stage_geom(aw, ah);
        StyleRules {
            position: Some(Position::Absolute),
            left: Some(Length::Px(x).into()),
            top: Some(Length::Px(y).into()),
            width: Some(Length::Px(w).into()),
            height: Some(Length::Px(h).into()),
            // Cosmetic clip — uniform across backends now that the macOS backend
            // honors `overflow: Hidden` on plain views too (it routes to the
            // layer's `masksToBounds`, which clips the GPU canvas to bounds
            // without detaching its `CAMetalLayer`). The canvas fills the stage
            // exactly, so this is a no-op visually today, but it keeps the clip
            // correct if the stage ever gains a rounded-card radius — and with no
            // per-platform branch.
            overflow: Some(Overflow::Hidden),
            ..Default::default()
        }
    });

    ui! {
        view(style = root_style) {
            view(style = stage_style) {
                DrawingSurface(state = s, strokes = strokes, canvases = canvases, version = version, capture_writer = Some(capture_writer))
                CameraWidget(state = s)
            }
            chrome
        }
    }
}

// ============================================================================
// Canvas + drawing surface
// ============================================================================

/// Props for [`DrawingSurface`]. Holds the shared stroke list + repaint tick and
/// the current width/color (read in the `on_touch` handler).
pub struct DrawingSurfaceProps {
    pub state: BoardState,
    pub strokes: Strokes,
    /// The saved canvas docs — the two-finger swipe switches between them.
    pub canvases: CanvasStore,
    pub version: Signal<u64>,
    /// The canvas self-capture sink. The renderer reads back each frame into this
    /// writer while a recorder is subscribed (macOS/vello only).
    pub capture_writer: Option<media_stream::FrameWriter>,
}

impl Default for DrawingSurfaceProps {
    fn default() -> Self {
        Self {
            state: BoardState::default(),
            strokes: Rc::new(RefCell::new(Vec::new())),
            canvases: Rc::new(RefCell::new(Vec::new())),
            version: Signal::new(0),
            capture_writer: None,
        }
    }
}

/// The drawable canvas (base layer) plus the freehand `on_touch` handler. The
/// canvas is a `canvas::Canvas` expression child; the wrapping `view` carries
/// the gesture: `Began` starts a stroke, `Moved` appends, `Ended`/`Cancelled`
/// finalizes.
#[component]
pub fn DrawingSurface(props: &DrawingSurfaceProps) -> Element {
    let width = props.state.width;
    let color_css = props.state.color_css;
    let strokes = props.strokes.clone();
    let canvases = props.canvases.clone();
    let version = props.version;
    let capture_writer = props.capture_writer.clone();
    // Two-finger-swipe state (canvas switching).
    let active_canvas = props.state.active_canvas;
    let canvas_ids = props.state.canvas_ids;
    let next_id = props.state.next_id;
    let gestures_enabled = props.state.gestures_enabled;

    // Camera-as-texture: hand the canvas a reactive view of the camera stream +
    // its drag rect as a `TextureLayer`, so the renderer composites the live
    // camera INTO the canvas (on-screen AND in the recording) — cover-fit, with
    // rounded corners to match the widget frame. macOS/vello only; elsewhere the
    // canvas ignores it and the `CameraWidget` shows the video itself.
    let cam_stream = props.state.cam_stream;
    let cam_x = props.state.cam_x;
    let cam_y = props.state.cam_y;
    let aspect = props.state.aspect;
    let cam_shape = props.state.camera_shape;
    let cam_size = props.state.camera_size;
    let canvas_anim = props.state.canvas_anim;
    let camera_layer = canvas::TextureLayer::new(
        Rc::new(move || cam_stream.get()),
        // Clamped to the stage so the camera can't leave the board — and the same
        // clamp the widget box uses, so frame and image agree. Size follows the
        // chosen `CameraSize`.
        Rc::new(move || {
            let (cx, cy) = crate::clamped_cam(aspect, cam_x, cam_y, cam_shape, cam_size);
            let (cw, ch, _r) = crate::settings::camera_dims(cam_shape.get(), cam_size.get());
            (cx, cy, cw, ch)
        }),
    )
    .fit(canvas::Fit::Cover)
    // Reactive radius: rounded-rect vs full-circle (and the circle's radius scales
    // with size), updated live without rebuilding the layer.
    .corner_radius_fn(move || crate::settings::camera_dims(cam_shape.get(), cam_size.get()).2)
    // The frame is drawn by the canvas WITH the camera image, so it stays locked
    // to the picture while dragging (a separate framework-view border lagged the
    // moving image — they update on different clocks).
    .border(2.0, canvas::Color::new(255, 255, 255, 230));

    let canvas_bg = props.state.canvas_bg;
    let dark = props.state.dark;
    // Cross-dissolve buffers for the canvas-change transition. `fade_from` holds
    // the OUTGOING canvas's strokes (drawn fading out beneath the incoming live
    // strokes); `last_rendered` tracks the last settled content so the trigger can
    // capture it as the next `fade_from` — content-based, so it's correct for
    // switch / add / delete alike (no store-index bookkeeping).
    let fade_from: Strokes = Rc::new(RefCell::new(Vec::new()));
    let last_rendered: Strokes = Rc::new(RefCell::new(strokes.borrow().clone()));
    let canvas_el = build_canvas(
        strokes.clone(),
        fade_from.clone(),
        canvas_anim,
        version,
        canvas_bg,
        dark,
        capture_writer,
        vec![camera_layer],
    );

    // The surface fills the stage exactly and never moves. The canvas-change
    // transition is a cross-dissolve INSIDE the scene (see `build_canvas`), so
    // nothing here slides or fades — that's precisely what keeps the composited
    // camera out of the animation (only the background strokes cross-fade).
    let surface_style = static_style(StyleRules {
        width: Some(Length::pct(100.0).into()),
        height: Some(Length::pct(100.0).into()),
        position: Some(Position::Absolute),
        top: Some(Length::Px(0.0).into()),
        left: Some(Length::Px(0.0).into()),
        ..Default::default()
    });

    // Drive `canvas_anim` with an AnimatedValue tween on every active-canvas
    // change (add / switch / swipe / arrow key all move `active_canvas`).
    // AnimatedValue supplies the eased, clock-driven timeline and auto-stops on
    // settle; we mirror its value into the reactive signal so the actual move
    // goes through the normal restyle path. The first run only PRIMES (mount
    // isn't a transition); without the guard the canvas would slide in on load.
    {
        let anim = AnimatedValue::new(1.0f32);
        // Mirror the tween into the reactive signal; once settled, record the
        // now-current content as the next transition's fade-out source.
        let strokes_settle = strokes.clone();
        let last_rendered_settle = last_rendered.clone();
        let sub = anim.subscribe(move |v, _| {
            canvas_anim.set(*v);
            if *v >= 0.999 {
                *last_rendered_settle.borrow_mut() = strokes_settle.borrow().clone();
            }
        });
        runtime_core::on_cleanup(move || drop(sub));
        let active_idx = props.state.active_canvas;
        let primed = Rc::new(Cell::new(false));
        runtime_core::effect!({
            let _ = active_idx.get(); // re-run on every canvas change
            if !primed.get() {
                primed.set(true);
            } else {
                // Capture the just-displaced content to fade OUT, then cross-fade
                // the freshly-swapped live strokes IN (driven by the 0→1 tween).
                *fade_from.borrow_mut() = last_rendered.borrow().clone();
                anim.set(0.0);
                anim.animate(
                    TweenTo::new(1.0, std::time::Duration::from_millis(crate::CANVAS_FADE_MS))
                        .ease_out(),
                );
            }
        });
    }

    // `active` = the touch id currently drawing a stroke (single finger).
    let active: Rc<RefCell<Option<runtime_core::TouchId>>> = Rc::new(RefCell::new(None));
    // All down touches in WINDOW coords: id → (start_x, start_y, cur_x, cur_y).
    // A second concurrent touch turns the gesture into a canvas swipe.
    let touches: Rc<RefCell<HashMap<runtime_core::TouchId, (f32, f32, f32, f32)>>> =
        Rc::new(RefCell::new(HashMap::new()));
    // One switch per swipe gesture (reset when all fingers lift).
    let swipe_fired = Rc::new(Cell::new(false));

    ui! {
        view(style = surface_style) {
            canvas_el
        }
        .on_touch(move |ev| {
            let (wx, wy) = (ev.window_position.x, ev.window_position.y);
            match ev.phase {
                TouchPhase::Began => {
                    touches.borrow_mut().insert(ev.id, (wx, wy, wx, wy));
                    let multi = touches.borrow().len() >= 2 && gestures_enabled.get();
                    if multi {
                        // A 2nd finger landed → this is a swipe, not a stroke. Cancel
                        // the in-progress stroke the first finger started so a swipe
                        // never leaves an accidental line.
                        if active.borrow().is_some() {
                            strokes.borrow_mut().pop();
                            *active.borrow_mut() = None;
                            version.set(version.get().wrapping_add(1));
                        }
                        return TouchResponse::CLAIMED;
                    }
                    if active.borrow().is_some() {
                        return TouchResponse::IGNORED;
                    }
                    *active.borrow_mut() = Some(ev.id);
                    // Flag ink strokes so they re-resolve against the backdrop every
                    // paint; snapshot the current resolution as the `rgba` fallback.
                    let raw = color_css.get();
                    let ink = raw == crate::INK;
                    let rgba = parse_rgba(crate::resolve_color(raw, canvas_bg.get(), dark.get()));
                    strokes.borrow_mut().push(Stroke {
                        points: vec![(ev.position.x, ev.position.y)],
                        width: width.get(),
                        rgba,
                        ink,
                    });
                    version.set(version.get().wrapping_add(1));
                    TouchResponse::CLAIMED
                }
                TouchPhase::Moved => {
                    if let Some(t) = touches.borrow_mut().get_mut(&ev.id) {
                        t.2 = wx;
                        t.3 = wy;
                    }
                    let multi = touches.borrow().len() >= 2 && gestures_enabled.get();
                    if multi {
                        if !swipe_fired.get() {
                            // Average the per-touch deltas from gesture start.
                            let (mut sdx, mut sdy, mut n) = (0.0f32, 0.0f32, 0u32);
                            for (_, (sx, sy, cx, cy)) in touches.borrow().iter() {
                                sdx += cx - sx;
                                sdy += cy - sy;
                                n += 1;
                            }
                            let (avg_dx, avg_dy) = (sdx / n as f32, sdy / n as f32);
                            if let Some(action) =
                                crate::swipe_action(avg_dx, avg_dy, crate::SWIPE_THRESHOLD)
                            {
                                crate::apply_canvas_action(
                                    action, &canvases, &strokes, active_canvas, version,
                                    canvas_ids, next_id,
                                );
                                swipe_fired.set(true);
                            }
                        }
                        return TouchResponse::CONSUMED;
                    }
                    if *active.borrow() != Some(ev.id) {
                        return TouchResponse::IGNORED;
                    }
                    if let Some(last) = strokes.borrow_mut().last_mut() {
                        last.points.push((ev.position.x, ev.position.y));
                    }
                    version.set(version.get().wrapping_add(1));
                    TouchResponse::CONSUMED
                }
                TouchPhase::Ended | TouchPhase::Cancelled => {
                    touches.borrow_mut().remove(&ev.id);
                    if touches.borrow().is_empty() {
                        swipe_fired.set(false);
                    }
                    if *active.borrow() == Some(ev.id) {
                        *active.borrow_mut() = None;
                        if ev.phase == TouchPhase::Ended {
                            if let Some(last) = strokes.borrow_mut().last_mut() {
                                last.points.push((ev.position.x, ev.position.y));
                            }
                            version.set(version.get().wrapping_add(1));
                        }
                    }
                    TouchResponse::CONSUMED
                }
            }
        })
    }
}

/// The `canvas::Canvas` painter. Reactive repaint deps: `version` (stroke
/// mutations) and `canvas_anim` (the canvas-change cross-dissolve).
#[allow(clippy::too_many_arguments)]
fn build_canvas(
    strokes: Strokes,
    fade_from: Strokes,
    canvas_anim: Signal<f32>,
    version: Signal<u64>,
    canvas_bg: Signal<crate::CanvasBg>,
    dark: Signal<bool>,
    capture_writer: Option<media_stream::FrameWriter>,
    layers: Vec<canvas::TextureLayer>,
) -> Element {
    use canvas::prelude::*;

    let fill = static_style(StyleRules {
        width: Some(Length::pct(100.0).into()),
        height: Some(Length::pct(100.0).into()),
        ..Default::default()
    });

    canvas::Canvas(CanvasProps {
        draw: canvas::draw(move |s: &mut Scene| {
            let _ = version.get(); // reactive repaint dependency (stroke edits)
            let p = canvas_anim.get().clamp(0.0, 1.0); // dep: the cross-dissolve tween
            // The drawing-surface background — the chosen `CanvasBg` (Auto follows
            // the app theme). Reading both signals here makes a color/theme change
            // repaint the canvas.
            let cb = canvas_bg.get();
            let dk = dark.get();
            let (r, g, b) = cb.rgb(dk);

            s.path().add_path(Path::rect(0.0, 0.0, 100_000.0, 100_000.0));
            s.fill(Color::new(r, g, b, 255));

            // Canvas-change cross-dissolve: the OUTGOING strokes fade out
            // (alpha·(1-p)) and the live strokes fade in (alpha·p), both over the
            // constant background. Nothing moves, and the camera — a `TextureLayer`
            // composited ON TOP of these scene ops — is untouched, so only the
            // background animates. Alpha blends against the opaque bg drawn above,
            // so this works on the opaque Metal surface (a compositor-level fade
            // would instead fade the camera too). `p == 1` is the steady state:
            // live strokes paint at full alpha and the fade-out layer is skipped.
            if p < 1.0 {
                for stroke in fade_from.borrow().iter() {
                    let (sr, sg, sb, sa) = crate::stroke_color(stroke, cb, dk);
                    paint_stroke(s, stroke, (sr, sg, sb, (sa as f32 * (1.0 - p)) as u8));
                }
            }
            for stroke in strokes.borrow().iter() {
                // `ink` strokes re-resolve against the live backdrop so they stay
                // readable across canvas-color / theme changes.
                let (sr, sg, sb, sa) = crate::stroke_color(stroke, cb, dk);
                let a = if p < 1.0 { (sa as f32 * p) as u8 } else { sa };
                paint_stroke(s, stroke, (sr, sg, sb, a));
            }
        }),
        // Self-capture sink: the vello renderer reads back each frame here while
        // a recorder is subscribed (macOS only; canvas-native ignores it).
        capture: capture_writer,
        // Live texture layers (the camera) composited into the canvas (macOS).
        layers,
        ..Default::default()
    })
    .with_style(fill)
    .into_element()
}

// ============================================================================
// Camera widget (draggable, cover-fit, recordable content)
// ============================================================================

/// Props for [`CameraWidget`]. Reads `cam_on`/`cam_stream` (display) plus
/// `cam_x`/`cam_y` (drag position) off the [`BoardState`].
pub struct CameraWidgetProps {
    pub state: BoardState,
}

impl Default for CameraWidgetProps {
    fn default() -> Self {
        Self { state: BoardState::default() }
    }
}

/// The draggable camera frame. Every backend composites the live camera INTO
/// the canvas (the [`DrawingSurface`]'s `TextureLayer` — GPU on macOS, native
/// 2D `drawImage`/`drawBitmap`/`CGImage` on web/iOS/Android), so this widget is
/// purely a transparent frame + drag handle positioned over that composited
/// region — identical on all platforms (no per-target branching). Dragging it
/// moves `cam_x`/`cam_y`, which the canvas layer's `rect` closure reads, so the
/// composited camera follows.
#[component]
pub fn CameraWidget(props: &CameraWidgetProps) -> Element {
    let cam_on = props.state.cam_on;
    let cam_x = props.state.cam_x;
    let cam_y = props.state.cam_y;
    let aspect = props.state.aspect;
    let cam_shape = props.state.camera_shape;
    let cam_size = props.state.camera_size;

    // Drag state: (start_touch_x, start_touch_y, start_cam_x, start_cam_y).
    let drag: Rc<RefCell<Option<(f32, f32, f32, f32)>>> = Rc::new(RefCell::new(None));

    // Full-fill transparent child that carries the drag gesture.
    let overlay_style = static_style(StyleRules {
        position: Some(Position::Absolute),
        top: Some(Length::Px(0.0).into()),
        left: Some(Length::Px(0.0).into()),
        right: Some(Length::Px(0.0).into()),
        bottom: Some(Length::Px(0.0).into()),
        ..Default::default()
    });

    let box_style = reactive_style(move || {
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
        // Invisible drag target only: the canvas composites the camera AND its
        // frame (rounded corners + border) at the same `cam_x/cam_y`, so they stay
        // pixel-locked while dragging. This box just provides the hit-test rect —
        // its position can lag a frame without any visible effect (nothing is
        // drawn here).
        // Position clamped to the stage — same clamp the composited layer rect
        // uses, so the (invisible) hit-test box always sits over the visible
        // camera even right after an aspect change.
        let (bx, by) = crate::clamped_cam(aspect, cam_x, cam_y, cam_shape, cam_size);
        let (cw, ch, _r) = crate::settings::camera_dims(cam_shape.get(), cam_size.get());
        StyleRules {
            position: Some(Position::Absolute),
            left: Some(Length::Px(bx).into()),
            top: Some(Length::Px(by).into()),
            width: Some(Length::Px(cw).into()),
            height: Some(Length::Px(ch).into()),
            ..Default::default()
        }
    });

    ui! {
        view(style = box_style) {
            view(style = overlay_style) {}
            .on_touch(move |ev| match ev.phase {
                TouchPhase::Began => {
                    // Start from the CLAMPED position (= what's visible), so a drag
                    // begun after an aspect change doesn't jump.
                    let (cx, cy) = crate::clamped_cam(aspect, cam_x, cam_y, cam_shape, cam_size);
                    *drag.borrow_mut() = Some((ev.window_position.x, ev.window_position.y, cx, cy));
                    TouchResponse::CLAIMED
                }
                TouchPhase::Moved => {
                    let start = *drag.borrow();
                    if let Some((sx, sy, cx, cy)) = start {
                        // Clamp to the STAGE (the camera lives in the board). The
                        // touch delta is viewport-space; the stage is unscaled, so
                        // the delta applies 1:1 to stage-local coords.
                        let (aw, ah) = aspect.get();
                        let (_x, _y, sw, sh) = crate::settings::stage_geom(aw, ah);
                        let (cw, ch, _r) = crate::settings::camera_dims(cam_shape.get(), cam_size.get());
                        let (nx, ny) = crate::clamp_cam(
                            cx + (ev.window_position.x - sx),
                            cy + (ev.window_position.y - sy),
                            sw,
                            sh,
                            cw,
                            ch,
                        );
                        cam_x.set(nx);
                        cam_y.set(ny);
                    }
                    TouchResponse::CONSUMED
                }
                TouchPhase::Ended | TouchPhase::Cancelled => {
                    *drag.borrow_mut() = None;
                    TouchResponse::CONSUMED
                }
            })
        }
    }
}
