//! The board screen's recordable content: the drawable canvas + drawing
//! surface, and the draggable camera widget. The capture-excluded floating
//! chrome lives in [`crate::chrome`].

use crate::style::{border_all, radius, reactive_style, static_style, styled};
use crate::{parse_rgba, paint_stroke, BoardState, CanvasCapture, RecHandle, Stroke, Strokes};
use runtime_core::{
    component, safe_area_insets, ui, viewport_size, Color, Element, IntoElement, Length, Overflow,
    Position, Signal, StyleRules, Tokenized, TouchPhase, TouchResponse,
};
use std::cell::RefCell;
use std::rc::Rc;

use crate::{CAM_H, CAM_RADIUS, CAM_W, DRAG_MARGIN};

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
    pub rec_handle: RecHandle,
    pub version: Signal<u64>,
    /// The canvas self-capture bundle: the writer is fed to the Canvas, the
    /// whole bundle is threaded to the record chrome.
    pub capture: CanvasCapture,
    /// `true` while the board is the active stack root (no Settings/Preview
    /// pushed). Drives the capture-excluded chrome's mount/unmount.
    pub focused: Rc<dyn Fn() -> bool>,
}

impl Default for BoardScreenProps {
    fn default() -> Self {
        // The board always receives a real `CanvasCapture` from `app()`; this
        // Default exists only to satisfy the props contract.
        Self {
            state: BoardState::default(),
            strokes: Rc::new(RefCell::new(Vec::new())),
            rec_handle: Rc::new(RefCell::new(None)),
            version: Signal::new(0),
            capture: CanvasCapture::default(),
            focused: Rc::new(|| true),
        }
    }
}

/// The board screen — the whiteboard itself: drawable canvas, a draggable
/// camera, and the capture-excluded `PrivateLayer` chrome.
///
/// Root carries `overflow: hidden` so the full-screen app clips to the viewport
/// and a stray sub-pixel of chrome can't leak into a page-level scrollbar (the
/// board "starts scrolling" report) — a whiteboard never scrolls its root.
#[component]
pub fn BoardScreen(props: &BoardScreenProps) -> Element {
    let s = props.state;
    let strokes = props.strokes.clone();
    let rec_handle = props.rec_handle.clone();
    let version = props.version;
    let capture = props.capture.clone();
    let focused = props.focused.clone();

    // The Canvas writes each rendered frame into this writer (macOS/vello only).
    let capture_writer = capture.writer.clone();

    // The chrome (tool rail, palette, record dock, REC pill, settings FAB) as
    // individually-positioned absolute overlays over the canvas. No longer wrapped
    // in `screen_recorder::PrivateLayer`: that existed to exclude the toolbar from
    // a SCREEN recording, but we now record the canvas/GPU stream directly — the
    // chrome is never part of the canvas, so it's never in the recording anyway.
    // As normal in-tree siblings the navigator also hides them automatically when
    // a screen is pushed (they belong to the board screen).
    let chrome = crate::chrome::build_chrome(focused, s, strokes.clone(), rec_handle, version, capture);

    let root_style = static_style(StyleRules {
        width: Some(Length::pct(100.0).into()),
        height: Some(Length::pct(100.0).into()),
        position: Some(Position::Relative),
        overflow: Some(Overflow::Hidden),
        ..Default::default()
    });

    ui! {
        view(style = root_style) {
            DrawingSurface(state = s, strokes = strokes, version = version, capture_writer = Some(capture_writer))
            CameraWidget(state = s)
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
    let version = props.version;
    let capture_writer = props.capture_writer.clone();

    // Camera-as-texture: hand the canvas a reactive view of the camera stream +
    // its drag rect as a `TextureLayer`, so the renderer composites the live
    // camera INTO the canvas (on-screen AND in the recording) — cover-fit, with
    // rounded corners to match the widget frame. macOS/vello only; elsewhere the
    // canvas ignores it and the `CameraWidget` shows the video itself.
    let cam_stream = props.state.cam_stream;
    let cam_x = props.state.cam_x;
    let cam_y = props.state.cam_y;
    let camera_layer = canvas::TextureLayer::new(
        Rc::new(move || cam_stream.get()),
        Rc::new(move || (cam_x.get(), cam_y.get(), CAM_W, CAM_H)),
    )
    .fit(canvas::Fit::Cover)
    .corner_radius(CAM_RADIUS);

    let canvas_el = build_canvas(strokes.clone(), version, capture_writer, vec![camera_layer]);

    let surface_style = static_style(StyleRules {
        width: Some(Length::pct(100.0).into()),
        height: Some(Length::pct(100.0).into()),
        position: Some(Position::Absolute),
        top: Some(Length::Px(0.0).into()),
        left: Some(Length::Px(0.0).into()),
        ..Default::default()
    });

    let active: Rc<RefCell<Option<runtime_core::TouchId>>> = Rc::new(RefCell::new(None));

    ui! {
        view(style = surface_style) {
            canvas_el
        }
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
    }
}

/// The `canvas::Canvas` painter. A reactive repaint dependency (`version`)
/// re-runs the draw closure on every stroke mutation.
fn build_canvas(
    strokes: Strokes,
    version: Signal<u64>,
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
            let _ = version.get(); // reactive repaint dependency

            s.path().add_path(Path::rect(0.0, 0.0, 100_000.0, 100_000.0));
            s.fill(Color::new(255, 255, 255, 255));

            for stroke in strokes.borrow().iter() {
                paint_stroke(s, stroke);
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

/// The live camera feed: a cover-fit `video::Video` in a rounded box, draggable
/// anywhere on the canvas (clamped to the safe area). A transparent overlay
/// carries the drag handler so the video child can't swallow the press (the
/// macOS "can't move the camera" bug — the video NSView was the hit target).
#[component]
pub fn CameraWidget(props: &CameraWidgetProps) -> Element {
    let cam_on = props.state.cam_on;
    let cam_stream = props.state.cam_stream;
    let cam_x = props.state.cam_x;
    let cam_y = props.state.cam_y;

    // macOS (GPU LayerCompositor) AND web (canvas-native drawImage) composite the
    // camera INTO the canvas, so it lands in the recording and this widget is
    // just a transparent draggable frame over it. Other targets (iOS/Android)
    // can't composite yet, so the widget shows the live `video` itself
    // (display-only; not in the recording — a follow-up).
    let composited = cfg!(target_os = "macos") || cfg!(target_arch = "wasm32");
    let video_fill = static_style(StyleRules {
        width: Some(Length::pct(100.0).into()),
        height: Some(Length::pct(100.0).into()),
        ..Default::default()
    });
    let video_child: Option<Element> = if composited {
        None
    } else {
        Some(
            video::Video(video::VideoProps {
                source: video::stream(move || cam_stream.get()),
                autoplay: true,
                object_fit: video::ObjectFit::Cover,
                ..Default::default()
            })
            .with_style(video_fill)
            .into_element(),
        )
    };

    // Drag state: (start_touch_x, start_touch_y, start_cam_x, start_cam_y).
    let drag: Rc<RefCell<Option<(f32, f32, f32, f32)>>> = Rc::new(RefCell::new(None));

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
        styled(
            StyleRules {
                position: Some(Position::Absolute),
                left: Some(Length::Px(cam_x.get().max(0.0)).into()),
                top: Some(Length::Px(cam_y.get().max(0.0)).into()),
                width: Some(Length::Px(CAM_W).into()),
                height: Some(Length::Px(CAM_H).into()),
                // Transparent when the canvas composites the camera behind us
                // (macOS) so it shows through the frame; a dark fill otherwise
                // (the video sits on top).
                background: if composited {
                    None
                } else {
                    Some(Tokenized::Literal(Color("#0b1220".into())))
                },
                overflow: Some(Overflow::Hidden),
                ..Default::default()
            },
            [radius(18.0), border_all(2.0, "rgba(255,255,255,0.9)")],
        )
    });

    ui! {
        view(style = box_style) {
            if let Some(v) = video_child { v }
            view(style = overlay_style) {}
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
        }
    }
}
