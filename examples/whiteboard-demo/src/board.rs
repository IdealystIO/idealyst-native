//! The board screen's recordable content: the drawable canvas + drawing
//! surface, and the draggable camera widget. The floating chrome (in-tree
//! sibling overlays) lives in [`crate::chrome`].

use crate::style::{reactive_style, static_style, token};
use crate::{parse_rgba, paint_stroke, BoardState, CanvasCapture, RecHandle, Stroke, Strokes};
use runtime_core::{
    component, safe_area_insets, ui, viewport_size, AlignItems, Element, FlexDirection,
    IntoElement, JustifyContent, Length, Overflow, Position, Signal, StyleRules, Tokenized,
    TouchPhase, TouchResponse,
};
use std::cell::RefCell;
use std::rc::Rc;

use crate::{CAM_H, CAM_RADIUS, CAM_W};

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
            rec_handle: Rc::new(RefCell::new(None)),
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
    let rec_handle = props.rec_handle.clone();
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
    let chrome = crate::chrome::build_chrome(focused, s, strokes.clone(), rec_handle, version, capture);

    // Reactive so the letterbox around the stage follows the app theme (light/dark).
    let root_style = reactive_style(|| StyleRules {
        width: Some(Length::pct(100.0).into()),
        height: Some(Length::pct(100.0).into()),
        position: Some(Position::Relative),
        overflow: Some(Overflow::Hidden),
        background: Some(Tokenized::Literal(token(|c| c.background.clone()))),
        ..Default::default()
    });

    // The stage is centered inside this safe-area-inset container. Chrome stays a
    // direct child of the root (so its own absolute insets aren't double-counted).
    let center_style = reactive_style(move || {
        let ins = safe_area_insets().get();
        StyleRules {
            position: Some(Position::Absolute),
            top: Some(Length::Px(0.0).into()),
            left: Some(Length::Px(0.0).into()),
            width: Some(Length::pct(100.0).into()),
            height: Some(Length::pct(100.0).into()),
            flex_direction: Some(FlexDirection::Column),
            align_items: Some(AlignItems::Center),
            justify_content: Some(JustifyContent::Center),
            padding_top: Some(Length::Px(ins.top + crate::settings::STAGE_MARGIN).into()),
            padding_bottom: Some(Length::Px(ins.bottom + crate::settings::STAGE_MARGIN).into()),
            padding_left: Some(Length::Px(ins.left + crate::settings::STAGE_MARGIN).into()),
            padding_right: Some(Length::Px(ins.right + crate::settings::STAGE_MARGIN).into()),
            ..Default::default()
        }
    });

    // The canvas "stage": an aspect-locked box, as large as fits in the centered
    // container. Sized via Taffy `aspect_ratio` + a PERCENTAGE binding dimension
    // (NOT reactive px): the canvas then resizes through the layout pass, which is
    // the path that resizes the vello Metal surface on macOS. A reactive px size
    // strands that surface at its degenerate first-paint size (`vp=0`), so px
    // sizing renders nothing on macOS. See [[project_macos_appkit_uikit_diffs]].
    let aspect = s.aspect;
    let stage_style = reactive_style(move || {
        let (aw, ah) = aspect.get();
        let ar = aw as f32 / ah as f32;
        let ins = safe_area_insets().get();
        let vp = viewport_size().get();
        let avail_w = (vp.width - ins.left - ins.right - 2.0 * crate::settings::STAGE_MARGIN).max(1.0);
        let avail_h = (vp.height - ins.top - ins.bottom - 2.0 * crate::settings::STAGE_MARGIN).max(1.0);
        let mut st = StyleRules {
            aspect_ratio: Some(ar),
            // Positioned containing block: the absolute DrawingSurface + camera box
            // children resolve their `top/left` against THIS stage (not the outer
            // center container), so the camera's stage-local clamp lines up with the
            // canvas. `Relative` keeps it a normal flex item (no offset) and — unlike
            // `overflow`/`background` — doesn't force layer-backing on macOS.
            position: Some(Position::Relative),
            // Cosmetic clip (rounded card). NOT on macOS: `overflow: Hidden` forces
            // this view layer-backed, detaching the canvas child's `CAMetalLayer`.
            overflow: if cfg!(target_os = "macos") {
                None
            } else {
                Some(Overflow::Hidden)
            },
            ..Default::default()
        };
        // Largest fitting box: if the available area is wider than the target
        // ratio, height binds (full height, width derived); else width binds.
        if avail_w / avail_h > ar {
            st.height = Some(Length::pct(100.0).into());
        } else {
            st.width = Some(Length::pct(100.0).into());
        }
        st
    });

    ui! {
        view(style = root_style) {
            view(style = center_style) {
                view(style = stage_style) {
                    DrawingSurface(state = s, strokes = strokes, version = version, capture_writer = Some(capture_writer))
                    CameraWidget(state = s)
                }
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
    let aspect = props.state.aspect;
    let camera_layer = canvas::TextureLayer::new(
        Rc::new(move || cam_stream.get()),
        // Clamped to the stage so the camera can't leave the board — and the same
        // clamp the widget box uses, so frame and image agree.
        Rc::new(move || {
            let (cx, cy) = crate::clamped_cam(aspect, cam_x, cam_y);
            (cx, cy, CAM_W, CAM_H)
        }),
    )
    .fit(canvas::Fit::Cover)
    .corner_radius(CAM_RADIUS)
    // The frame is drawn by the canvas WITH the camera image, so it stays locked
    // to the picture while dragging (a separate framework-view border lagged the
    // moving image — they update on different clocks).
    .border(2.0, canvas::Color::new(255, 255, 255, 230));

    let canvas_bg = props.state.canvas_bg;
    let dark = props.state.dark;
    let canvas_el = build_canvas(strokes.clone(), version, canvas_bg, dark, capture_writer, vec![camera_layer]);

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
            let _ = version.get(); // reactive repaint dependency
            // The drawing-surface background — the chosen `CanvasBg` (Auto follows
            // the app theme). Reading both signals here makes a color/theme change
            // repaint the canvas.
            let (r, g, b) = canvas_bg.get().rgb(dark.get());

            s.path().add_path(Path::rect(0.0, 0.0, 100_000.0, 100_000.0));
            s.fill(Color::new(r, g, b, 255));

            let cb = canvas_bg.get();
            let dk = dark.get();
            for stroke in strokes.borrow().iter() {
                // `ink` strokes re-resolve against the live backdrop so they stay
                // readable across canvas-color / theme changes.
                paint_stroke(s, stroke, crate::stroke_color(stroke, cb, dk));
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
        let (bx, by) = crate::clamped_cam(aspect, cam_x, cam_y);
        StyleRules {
            position: Some(Position::Absolute),
            left: Some(Length::Px(bx).into()),
            top: Some(Length::Px(by).into()),
            width: Some(Length::Px(CAM_W).into()),
            height: Some(Length::Px(CAM_H).into()),
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
                    let (cx, cy) = crate::clamped_cam(aspect, cam_x, cam_y);
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
                        let (nx, ny) = crate::clamp_cam(
                            cx + (ev.window_position.x - sx),
                            cy + (ev.window_position.y - sy),
                            sw,
                            sh,
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
