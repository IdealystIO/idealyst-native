//! `whiteboard-demo` — a slick cross-platform whiteboard.
//!
//! The app is a **stack navigator** (`stack_navigator`) with three routes: the
//! `BOARD` (root, full-bleed canvas), `SETTINGS`, and `PREVIEW`. The board sets
//! `unmount_on_blur(false)`, so pushing Settings/Preview leaves it mounted
//! underneath — the camera keeps running and strokes persist — with native
//! push/pop + back gesture on iOS/Android/web (a child-swap on macOS). The
//! Settings/Preview screens carry their own in-content header (`header_shown` is
//! off everywhere) so they're navigable on macOS too, where the stack handler
//! renders no native chrome.
//!
//! Pieces of the board screen, and how they fit:
//!
//! 1. **Drawable canvas** (`canvas` SDK). A full-screen `canvas::Canvas`
//!    is the base layer. Freehand drawing is driven by a raw `on_touch`
//!    handler on the wrapping `view`: `Began` starts a stroke with the
//!    current width + color, `Moved` appends a point, `Ended`/`Cancelled`
//!    finalizes it. Strokes live in a shared `Rc<RefCell<Vec<Stroke>>>`; a
//!    `version` signal ticks on every mutation so the canvas painter (which
//!    reads `version`) repaints through the renderer's reactive `Effect`.
//!
//! 2. **Floating chrome** (tool rail, palette, record dock, REC pill, settings
//!    FAB), inside `screen_recorder::PrivateLayer` so it's excluded from the
//!    recording. Each dock's content is `!use_can_go_back()`-gated so it mounts
//!    only while the board is the active route — otherwise its always-on-top
//!    capture-excluded window would float over a pushed screen.
//!
//! 3. **Camera widget** (NORMAL recordable content): `Camera::open` →
//!    `MediaStream` → cover-fit `video::Video`. Draggable anywhere on the
//!    canvas (clamped to the safe area), so it appears in the recording
//!    wherever the user parks it.
//!
//! 4. **Record control**: a camera-style start/stop button docked
//!    bottom-center; while recording it becomes a stop button and slides to
//!    the bottom-right, and a REC pill shows top-center. Stopping finalizes the
//!    file and pushes the `PREVIEW` screen (play back · discard · export).
//!
//! On iOS/Android the floating chrome + camera bounds are kept inside the
//! `safe_area_insets()` even though the app is full-screen.
//!
//! ## Module layout
//!
//! - [`board`] — the recordable board content: `BoardScreen`, `DrawingSurface`,
//!   `CameraWidget`.
//! - [`chrome`] — the capture-excluded `PrivateLayer` chrome: tool rail, palette,
//!   record dock, REC pill, settings FAB (`build_chrome` assembles them).
//! - [`screens`] — the navigator-pushed `SettingsScreen` / `PreviewScreen` and
//!   the shared `ScreenScaffold` / `Label`.
//! - [`style`] — shared `StyleRules` helpers (`radius`, `border_all`,
//!   `reactive_style`, `focus_gate`, …).

// Lints inherent to the `#[component]` + `ui!` + closure-heavy idiom, emitted
// the same way across the framework's own component crates (idea-ui, runtime-core
// primitives, screen-recorder): the `ui!` macro appends `..Default::default()`
// to every props literal (`needless_update`); props own `Rc<dyn Fn>` /
// `Rc<RefCell<…>>` state (`type_complexity`); and props carry an explicit manual
// `Default` matching the idea-ui convention (`derivable_impls`). Allowed
// crate-wide to match that posture rather than diverge per call site.
#![allow(clippy::needless_update, clippy::type_complexity, clippy::derivable_impls)]

mod board;
mod chrome;
mod screens;
mod style;

use camera::MediaStream;
use runtime_core::primitives::navigator::use_can_go_back;
use runtime_core::{
    component, safe_area_insets, signal, ui, viewport_size, Element, Ref, Route, Screen, Signal,
};
use stack_navigator::{Navigator, StackBuilder, StackHandle, StackScreenExt};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

pub(crate) use board::BoardScreen;
pub(crate) use screens::{PreviewScreen, SettingsScreen};

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
    // GPU canvas: register vello AFTER native so it wins (last-registration).
    canvas_vello::register(backend);
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
pub(crate) struct Stroke {
    pub points: Vec<(f32, f32)>,
    pub width: f32,
    pub rgba: (u8, u8, u8, u8),
}

/// Shared mutable list of strokes. The `on_touch` handler mutates it and the
/// canvas painter reads it; a `version` signal bridges the two so a mutation
/// triggers a reactive repaint without cloning the whole vec into a signal.
pub(crate) type Strokes = Rc<RefCell<Vec<Stroke>>>;

/// The live media-writer recording handle, shared between the record button's
/// start (sets it) and stop (consumes it). `!Send`, main-thread only.
pub(crate) type RecHandle = Rc<RefCell<Option<media_writer::Recording>>>;

/// The canvas self-capture bundle. The Canvas writes each rendered frame into
/// `writer`; the app records `stream` with `media-writer`. `raf` holds the
/// capture-cadence loop that ticks the canvas `version` signal at frame rate
/// while recording, so the renderer re-renders (and reads back a frame) every
/// frame instead of only on a stroke mutation.
///
/// NOTE: this is **macOS (vello) self-capture** only. On web/iOS the canvas uses
/// canvas-native, which ignores `CanvasProps::capture`, so recording produces no
/// frames there — a known follow-up (web = `canvas.captureStream()`, iOS = vello
/// later). We do NOT branch per-platform here; the unsupported backends simply
/// record an empty stream.
///
/// `MediaStream` is `!Send`/`!Copy` but `Clone`; `FrameWriter` is `Clone`. Like
/// `strokes`/`rec_handle`, it lives outside `BoardState` and is `.clone()`d into
/// builders.
#[derive(Clone)]
pub struct CanvasCapture {
    pub stream: media_stream::MediaStream,
    pub writer: media_stream::FrameWriter,
    pub raf: Rc<RefCell<Option<runtime_core::scheduling::RafLoop>>>,
}

impl Default for CanvasCapture {
    fn default() -> Self {
        // A throwaway stream/writer — only exists to satisfy props `Default`. The
        // real bundle is created once in `app()` and threaded through.
        let (stream, writer) = media_stream::MediaStream::new();
        Self { stream, writer, raf: Rc::new(RefCell::new(None)) }
    }
}

/// Paint one stroke into the canvas scene. A single point → a filled dot; a
/// polyline → a round-capped/joined stroke.
pub(crate) fn paint_stroke(s: &mut canvas::Scene, stroke: &Stroke) {
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

/// Parse a CSS color string into RGBA bytes (black on failure).
pub(crate) fn parse_rgba(css: &str) -> (u8, u8, u8, u8) {
    let c = runtime_core::color::parse_or(css, runtime_core::color::Rgba::BLACK);
    (c.r, c.g, c.b, c.a)
}

// ----------------------------------------------------------------------------
// Routes — one stack-navigator screen each. The board is the root; Settings and
// Preview are pushed onto the stack (native push/pop + back gesture on
// iOS/Android/web; a child-swap on macOS). The board screen sets
// `unmount_on_blur(false)` (the default) so it stays alive under a pushed
// screen — the camera keeps running and strokes persist — but its
// capture-excluded `PrivateLayer` chrome is hidden via `!use_can_go_back()` so
// the toolbar doesn't float over Settings.
// ----------------------------------------------------------------------------

pub(crate) const BOARD: Route<()> = Route::<()>::new("board", "/");
pub(crate) const SETTINGS: Route<()> = Route::<()>::new("settings", "/settings");
pub(crate) const PREVIEW: Route<()> = Route::<()>::new("preview", "/preview");

/// The palette of swatch colors, as `(label, css)`. Black is first (default).
pub(crate) const PALETTE: &[(&str, &str)] = &[
    ("black", "#111827"),
    ("red", "#ef4444"),
    ("orange", "#f59e0b"),
    ("green", "#22c55e"),
    ("blue", "#3b82f6"),
    ("violet", "#8b5cf6"),
];

/// Stroke-width presets for the thin / medium / thick buttons.
pub(crate) const WIDTH_THIN: f32 = 2.0;
pub(crate) const WIDTH_MEDIUM: f32 = 6.0;
pub(crate) const WIDTH_THICK: f32 = 14.0;

/// Camera widget box (draggable, recordable content).
pub(crate) const CAM_W: f32 = 132.0;
pub(crate) const CAM_H: f32 = 176.0;
/// Corner radius of the camera, in logical points — used for the composited
/// layer's rounded mask AND the widget frame's border, so they line up.
pub(crate) const CAM_RADIUS: f32 = 18.0;
/// Keep dragged content this far from the safe-area edges.
pub(crate) const DRAG_MARGIN: f32 = 8.0;

/// Tool-rail metrics — a button is square; the rail is the button + padding.
pub(crate) const TOOL_BTN: f32 = 44.0;
pub(crate) const RAIL_EDGE: f32 = 14.0; // gap from the screen edge (added to safe inset)

/// The `files` store + filename a recording is written to.
pub(crate) const REC_STORE: &str = "recordings";
pub(crate) const REC_FILE: &str = "recording.mp4";

// ============================================================================
// App-level state
// ============================================================================

/// All app-level state, created once in [`app`] and threaded into the board
/// screen builder. Because these live in the root scope (not a per-screen one),
/// they survive navigation — the board's `unmount_on_blur(false)` keeps the
/// screen mounted, and even on the macOS/web child-swap the signals outlive the
/// detach. `Copy` (every field is a `Signal`/`Ref`), so the board builder can
/// re-capture them on each mount.
#[derive(Clone, Copy)]
pub(crate) struct BoardState {
    pub width: Signal<f32>,
    pub color_css: Signal<&'static str>,
    pub cam_on: Signal<bool>,
    pub cam_stream: Signal<Option<MediaStream>>,
    pub cam_x: Signal<f32>,
    pub cam_y: Signal<f32>,
    pub recording: Signal<bool>,
    pub rec_path: Signal<Option<String>>,
    pub palette_open: Signal<bool>,
    /// The bound navigator handle — `push(&SETTINGS, ())` from the FAB,
    /// `push(&PREVIEW, ())` when a recording stops.
    pub nav: Ref<StackHandle>,
}

impl Default for BoardState {
    fn default() -> Self {
        Self {
            width: Signal::new(WIDTH_MEDIUM),
            color_css: Signal::new(PALETTE[0].1),
            cam_on: Signal::new(false),
            cam_stream: Signal::new(None),
            cam_x: Signal::new(-1.0),
            cam_y: Signal::new(-1.0),
            recording: Signal::new(false),
            rec_path: Signal::new(None),
            palette_open: Signal::new(false),
            nav: Ref::new(),
        }
    }
}

// ============================================================================
// App entry point
// ============================================================================

/// The whiteboard app: install the theme, create root-scoped state, and build
/// the 3-route stack navigator (board + Settings + Preview).
#[component]
pub fn app() -> Element {
    idea_ui::install_idea_theme(idea_ui::light_theme());

    // ---- State (root scope → survives navigation) ------------------------
    let nav: Ref<StackHandle> = Ref::new();
    let state = BoardState {
        width: signal!(WIDTH_MEDIUM),
        color_css: signal!(PALETTE[0].1),
        cam_on: signal!(false),
        cam_stream: signal!(None),
        // Camera widget top-left, in viewport points. `-1` = "not yet placed";
        // an Effect drops it bottom-left once the viewport size is known.
        cam_x: signal!(-1.0),
        cam_y: signal!(-1.0),
        recording: signal!(false),
        rec_path: signal!(None),
        palette_open: signal!(false),
        nav,
    };

    // `rec_handle` holds the live media-writer `Recording` (consumed on stop).
    // It's `!Send` + non-`Copy`, so it lives outside `BoardState` and is cloned
    // into the board builder. Strokes + a repaint tick are likewise shared.
    let rec_handle: RecHandle = Rc::new(RefCell::new(None));
    let strokes: Strokes = Rc::new(RefCell::new(Vec::new()));
    let version: Signal<u64> = signal!(0);

    // Keep the canvas re-rendering while the camera is on, so its composited
    // texture shows live frames (the canvas otherwise only repaints on a stroke
    // or drag). An Effect starts a frame-rate `raf_loop` bumping `version` when
    // the camera turns on and drops it when off. Root-scoped so it survives
    // navigation. (During recording the record button drives its own raf too;
    // both just bump the same tick — harmless.)
    {
        let cam_on = state.cam_on;
        let cam_raf: Rc<RefCell<Option<runtime_core::scheduling::RafLoop>>> =
            Rc::new(RefCell::new(None));
        let _ = runtime_core::Effect::new(move || {
            if cam_on.get() {
                if cam_raf.borrow().is_none() {
                    *cam_raf.borrow_mut() =
                        Some(runtime_core::scheduling::raf_loop(move || {
                            version.set(version.get().wrapping_add(1));
                        }));
                }
            } else {
                *cam_raf.borrow_mut() = None;
            }
        });
    }

    // Canvas self-capture: the Canvas writes each rendered frame into `writer`;
    // the record button records `stream`. Root-scoped (app-owned) so it survives
    // navigation and isn't dropped between recordings. See `CanvasCapture` for
    // the macOS-only (vello) caveat.
    //
    // `with_surface_capture` wires the zero-copy GPU path on macOS: the vello
    // canvas renders into an IOSurface and publishes it as the stream's native
    // source; media-writer wraps that IOSurface in a CVPixelBuffer and encodes
    // it directly — no CPU read-back, no swizzle. On other platforms it's plain
    // `MediaStream::new()` (the CPU read-back path).
    let (capture_stream, capture_writer) = media_stream::MediaStream::with_surface_capture();
    let capture = CanvasCapture {
        stream: capture_stream,
        writer: capture_writer,
        raf: Rc::new(RefCell::new(None)),
    };

    // Place the camera widget bottom-left the first time we learn the viewport
    // size (it's 0×0 before the first layout). In the root scope so it runs once
    // regardless of which screen is mounted; guarded so a later drag isn't
    // reset.
    {
        let placed = Rc::new(Cell::new(false));
        let cam_x = state.cam_x;
        let cam_y = state.cam_y;
        let _ = runtime_core::Effect::new(move || {
            let vp = viewport_size().get();
            let ins = safe_area_insets().get();
            if !placed.get() && vp.width > 1.0 && vp.height > 1.0 {
                cam_x.set(ins.left + 16.0);
                cam_y.set((vp.height - ins.bottom - CAM_H - 16.0).max(ins.top + DRAG_MARGIN));
                placed.set(true);
            }
        });
    }

    // The Preview screen's resolved playback URL. Created in the ROOT scope (not
    // the per-mount preview scope) because it's set from an async IndexedDB read
    // on web that may land AFTER the preview is popped — writing a signal whose
    // scope was already freed would panic. Root-scoped, the late write is a
    // harmless no-op.
    let preview_url: Signal<String> = signal!(String::new());

    // ---- The stack navigator: board (root) + Settings + Preview ----------
    // `header_shown(false)` everywhere — the board is full-bleed canvas, and the
    // Settings/Preview screens carry their own in-content header (a back button
    // that `pop`s) so they're navigable on every backend, including macOS where
    // the stack handler renders no native chrome.
    let builder = Navigator::new(&BOARD)
        .screen(BOARD, {
            let strokes = strokes.clone();
            let rec_handle = rec_handle.clone();
            let capture = capture.clone();
            move |_| {
                // `focused` is computed INSIDE the board-route builder so
                // `use_can_go_back()` resolves in the navigator scope. `true`
                // while the board is the stack root (no Settings/Preview pushed):
                // the chrome lives in a separate, always-on-top capture-excluded
                // window, so it must vanish when a screen is pushed or it floats
                // over Settings/Preview. We gate on `!use_can_go_back()` rather
                // than `use_focus()`: `use_focus` reads `active_route`, which
                // native stack handlers leave STALE after a bare `pop`, so the
                // chrome would never come back on return (the macOS "private
                // layer goes missing" bug). `can_go_back` is depth-derived and
                // every backend updates it on push AND pop.
                let can_go_back = use_can_go_back();
                let focused: Rc<dyn Fn() -> bool> = Rc::new(move || !can_go_back());
                Screen::new(ui! {
                    BoardScreen(
                        state = state,
                        strokes = strokes.clone(),
                        rec_handle = rec_handle.clone(),
                        version = version,
                        capture = capture.clone(),
                        focused = focused,
                    )
                })
                .header_shown(false)
            }
        })
        .screen(SETTINGS, move |_| {
            Screen::new(ui! { SettingsScreen(nav = nav) }).header_shown(false)
        })
        .screen(PREVIEW, move |_| {
            Screen::new(ui! {
                PreviewScreen(rec_path = state.rec_path, playback_url = preview_url, nav = nav)
            })
            .header_shown(false)
        });

    ui! { builder.bind(nav) }
}
