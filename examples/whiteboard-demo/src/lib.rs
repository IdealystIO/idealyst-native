//! `whiteboard-demo` — a slick cross-platform whiteboard.
//!
//! The app is a **stack navigator** (`stack_navigator`) with three routes: the
//! `BOARD` (root, full-bleed canvas), `SETTINGS`, and `PREVIEW`. The board sets
//! `unmount_on_blur(false)`, so pushing Settings/Preview leaves it mounted
//! underneath — the camera keeps running and strokes persist — with native
//! push/pop on iOS/Android/web (a child-swap on macOS). The board also sets
//! `back_enabled(false)` so an edge-swipe-back mid-stroke can't pop it (or, on
//! the Android root, background the app); Settings/Preview keep normal back. The
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
mod settings;
mod style;

pub(crate) use settings::CanvasBg;

use camera::MediaStream;
use runtime_core::primitives::navigator::use_can_go_back;
use runtime_core::{component, signal, ui, Element, Ref, Route, Screen, Signal};
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
}

#[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
pub fn register_extensions(backend: &mut backend_ios::IosBackend) {
    canvas_native::register(backend);
    // GPU canvas via vello/Metal. iOS uses host Metal (f16/compute), so vello
    // runs; register AFTER native so it wins (last-registration). Same uniform
    // registration as macOS/Android.
    canvas_vello::register(backend);
    video::register(backend);
}

#[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
pub fn register_extensions(backend: &mut backend_android::AndroidBackend) {
    canvas_native::register(backend);
    // GPU canvas: vello self-gates on f16 support — it wins on a real device
    // (Adreno/Mali have f16) and steps aside on the emulator's Vulkan (no f16),
    // leaving canvas-native. Same uniform registration as macOS.
    canvas_vello::register(backend);
    video::register(backend);
}

#[cfg(all(target_os = "macos", not(target_arch = "wasm32")))]
pub fn register_extensions(backend: &mut backend_macos::MacosBackend) {
    canvas_native::register(backend);
    // GPU canvas: register vello AFTER native so it wins (last-registration).
    canvas_vello::register(backend);
    video::register(backend);
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
    /// The color snapshotted at draw time. For `ink` strokes this is the contrast
    /// color *as resolved when drawn* — a fallback only; the live paint re-resolves
    /// against the CURRENT backdrop so the stroke stays readable (see `ink`).
    pub rgba: (u8, u8, u8, u8),
    /// Drawn with the adaptive [`INK`] slot. Such strokes re-resolve their color
    /// against the live backdrop every paint (so they flip light↔dark when the
    /// canvas color or theme changes), instead of using the snapshotted `rgba`.
    pub ink: bool,
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
/// Self-capture is wired on macOS (vello, zero-copy IOSurface), web
/// (`canvas.captureStream()`), AND Android (canvas-native reads the composited
/// bitmap back into `writer` while recording → MediaCodec). iOS canvas-native
/// does not yet read back (a follow-up). We do NOT branch per-platform here; an
/// unsupported backend simply records an empty stream.
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

/// Paint one stroke into the canvas scene with an explicit color. A single point
/// → a filled dot; a polyline → a round-capped/joined stroke. The caller resolves
/// the color (so `ink` strokes get the live backdrop contrast, others their
/// snapshotted `rgba`).
pub(crate) fn paint_stroke(s: &mut canvas::Scene, stroke: &Stroke, rgba: (u8, u8, u8, u8)) {
    use canvas::prelude::*;

    let (r, g, b, a) = rgba;
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

/// Sentinel for the first palette slot: "ink" — an adaptive default that resolves
/// at use time to whichever of [`INK_ON_LIGHT`] / [`INK_ON_DARK`] contrasts the
/// current canvas backdrop. So the default stroke is always visible, including on
/// a dark canvas (where a fixed black would vanish). Every other palette entry is
/// a literal CSS color.
pub(crate) const INK: &str = "ink";
const INK_ON_LIGHT: &str = "#111827"; // near-black ink for a light backdrop
const INK_ON_DARK: &str = "#f9fafb"; // near-white ink for a dark backdrop

/// Resolve a palette color (possibly the [`INK`] sentinel) to a concrete CSS
/// color, given the current canvas backdrop. Non-sentinel entries pass through
/// unchanged. Used by the draw path (snapshotted into a stroke) and by the swatch
/// / color-button display, so all three agree on what "ink" currently is.
pub(crate) fn resolve_color(css: &'static str, canvas_bg: CanvasBg, dark: bool) -> &'static str {
    if css != INK {
        return css;
    }
    let (r, g, b) = canvas_bg.rgb(dark);
    // Rec. 601 perceived luminance; a dark backdrop (low luma) gets light ink.
    let luma = 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32;
    if luma < 128.0 {
        INK_ON_DARK
    } else {
        INK_ON_LIGHT
    }
}

/// The effective paint color for a stroke against the CURRENT backdrop. An `ink`
/// stroke re-resolves its contrast color every paint (stays readable when the
/// canvas color / theme changes); a fixed-hue stroke uses its snapshot.
pub(crate) fn stroke_color(stroke: &Stroke, canvas_bg: CanvasBg, dark: bool) -> (u8, u8, u8, u8) {
    if stroke.ink {
        parse_rgba(resolve_color(INK, canvas_bg, dark))
    } else {
        stroke.rgba
    }
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

/// The palette of swatch colors, as `(label, css)`. The first slot is the
/// adaptive [`INK`] default (contrasts the backdrop); the rest are literal hues.
pub(crate) const PALETTE: &[(&str, &str)] = &[
    ("ink", INK),
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
/// Keep dragged content this far from the stage edges.
pub(crate) const DRAG_MARGIN: f32 = 8.0;

/// Clamp a camera position (STAGE-local points) so the `CAM_W × CAM_H` widget
/// stays fully inside a `sw × sh` stage, with a [`DRAG_MARGIN`] inset.
pub(crate) fn clamp_cam(cx: f32, cy: f32, sw: f32, sh: f32) -> (f32, f32) {
    let m = DRAG_MARGIN;
    let max_x = (sw - CAM_W - m).max(m);
    let max_y = (sh - CAM_H - m).max(m);
    (cx.clamp(m, max_x), cy.clamp(m, max_y))
}

/// The camera widget's clamped top-left for the current aspect/viewport — reads
/// `aspect`/`cam_x`/`cam_y` (+ viewport/insets via `stage_geom`) reactively, so
/// every read site (widget box, composited layer rect) agrees and stays in
/// bounds even across an aspect change. Returns `(x, y)` in STAGE-local points.
pub(crate) fn clamped_cam(
    aspect: Signal<(u32, u32)>,
    cam_x: Signal<f32>,
    cam_y: Signal<f32>,
) -> (f32, f32) {
    let (aw, ah) = aspect.get();
    let (_x, _y, sw, sh) = settings::stage_geom(aw, ah);
    clamp_cam(cam_x.get(), cam_y.get(), sw, sh)
}

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
    /// Camera widget top-left, in STAGE-local points (the canvas's own coordinate
    /// space), so the composited layer rect and the widget box agree and the
    /// camera lives inside the aspect-locked board.
    pub cam_x: Signal<f32>,
    pub cam_y: Signal<f32>,
    pub recording: Signal<bool>,
    pub rec_path: Signal<Option<String>>,
    pub palette_open: Signal<bool>,
    /// Board aspect ratio `(width, height)` — drives the centered canvas "stage".
    pub aspect: Signal<(u32, u32)>,
    /// Canvas drawing-surface background (`Auto` follows the app theme).
    pub canvas_bg: Signal<CanvasBg>,
    /// App theme: `true` = dark. Drives `set_idea_theme` + the `Auto` canvas bg.
    pub dark: Signal<bool>,
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
            aspect: Signal::new(settings::DEFAULT_ASPECT),
            canvas_bg: Signal::new(CanvasBg::Auto),
            dark: Signal::new(false),
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
    // Open matching the OS appearance (the Settings toggle still overrides). On
    // `Auto` (no platform preference) we fall back to light. `color_scheme()` is
    // stashed at mount like `platform()`, so it's readable here in the app body.
    let start_dark =
        matches!(runtime_core::color_scheme(), runtime_core::ColorScheme::Dark);
    idea_ui::install_idea_theme(if start_dark {
        idea_ui::dark_theme()
    } else {
        idea_ui::light_theme()
    });

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
        aspect: signal!(settings::DEFAULT_ASPECT),
        canvas_bg: signal!(CanvasBg::Auto),
        dark: signal!(start_dark),
        nav,
    };

    // Light/dark: `install_idea_theme` (above) installed the component sheets;
    // this Effect swaps the ACTIVE theme whenever `dark` flips. Every token-based
    // style (idea-ui components + our own token colors) re-resolves, so the whole
    // app adapts. Root-scoped so it survives navigation.
    {
        let dark = state.dark;
        runtime_core::effect!({
            if dark.get() {
                idea_ui::set_idea_theme(idea_ui::dark_theme());
            } else {
                idea_ui::set_idea_theme(idea_ui::light_theme());
            }
        });
    }

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
        runtime_core::effect!({
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
    // navigation and isn't dropped between recordings.
    //
    // `with_surface_capture` wires the zero-copy GPU path on macOS: the vello
    // canvas renders into an IOSurface and publishes it as the stream's native
    // source; media-writer wraps that IOSurface in a CVPixelBuffer and encodes
    // it directly — no CPU read-back, no swizzle. On Android/web it's plain
    // `MediaStream::new()` and the canvas-native renderer pushes each composited
    // frame through the CPU read-back path.
    let (capture_stream, capture_writer) = media_stream::MediaStream::with_surface_capture();
    let capture = CanvasCapture {
        stream: capture_stream,
        writer: capture_writer,
        raf: Rc::new(RefCell::new(None)),
    };

    // Drop the camera widget bottom-left INSIDE the stage the first time we know
    // the stage size. STAGE-LOCAL coords (origin = stage top-left) so the widget
    // and the canvas-composited camera agree. Initial placement only — read-site
    // clamping (board.rs) keeps the camera in bounds when the aspect changes.
    {
        let placed = Rc::new(Cell::new(false));
        let cam_x = state.cam_x;
        let cam_y = state.cam_y;
        let aspect = state.aspect;
        runtime_core::effect!({
            let (aw, ah) = aspect.get();
            let (_sx, _sy, sw, sh) = settings::stage_geom(aw, ah);
            if !placed.get() && sw > 1.0 && sh > 1.0 {
                cam_x.set(DRAG_MARGIN);
                cam_y.set((sh - CAM_H - DRAG_MARGIN).max(DRAG_MARGIN));
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
                // The board IS the drawing surface — an edge-swipe-back
                // mid-stroke is exactly the accidental gesture we want to
                // suppress (on Android root it would otherwise background
                // the app; on a pushed board it would pop). Lock the
                // system back affordance here; the in-content chrome still
                // drives navigation to Settings/Preview explicitly.
                .back_enabled(false)
                // Full-screen while the board is active: on Android this
                // hides the bars AND lifts the gesture-exclusion cap so the
                // whole canvas's edge swipes become strokes (no back
                // chevron); on iOS it hides the status bar + home indicator.
                // The navigator restores chrome for Settings/Preview, which
                // don't set it, and re-enters on pop-back.
                .fullscreen(true)
            }
        })
        .screen(SETTINGS, move |_| {
            Screen::new(ui! { SettingsScreen(state = state) }).header_shown(false)
        })
        .screen(PREVIEW, move |_| {
            Screen::new(ui! {
                PreviewScreen(rec_path = state.rec_path, playback_url = preview_url, nav = nav)
            })
            .header_shown(false)
        });

    ui! { builder.bind(nav) }
}

#[cfg(test)]
mod tests {
    use super::{
        clamp_cam, parse_rgba, resolve_color, stroke_color, CanvasBg, Stroke, CAM_H, CAM_W,
        DRAG_MARGIN, INK, INK_ON_DARK, INK_ON_LIGHT,
    };

    // Regression for "the first palette color should contrast the backdrop": the
    // `INK` slot resolves to a light ink on a dark canvas and a dark ink on a light
    // one, so the default stroke is always visible. Non-ink entries pass through.
    #[test]
    fn ink_contrasts_explicit_canvas_colors() {
        assert_eq!(resolve_color(INK, CanvasBg::White, false), INK_ON_LIGHT);
        assert_eq!(resolve_color(INK, CanvasBg::Paper, false), INK_ON_LIGHT);
        assert_eq!(resolve_color(INK, CanvasBg::Slate, false), INK_ON_LIGHT);
        assert_eq!(resolve_color(INK, CanvasBg::Charcoal, false), INK_ON_DARK);
        assert_eq!(resolve_color(INK, CanvasBg::Black, false), INK_ON_DARK);
    }

    #[test]
    fn ink_follows_auto_canvas_through_theme() {
        // Auto canvas tracks the theme: white in light → dark ink; near-black in
        // dark → light ink.
        assert_eq!(resolve_color(INK, CanvasBg::Auto, false), INK_ON_LIGHT);
        assert_eq!(resolve_color(INK, CanvasBg::Auto, true), INK_ON_DARK);
    }

    #[test]
    fn non_ink_entries_pass_through_unchanged() {
        assert_eq!(resolve_color("#ef4444", CanvasBg::Black, true), "#ef4444");
        assert_eq!(resolve_color("#3b82f6", CanvasBg::White, false), "#3b82f6");
    }

    // Regression for settings requirement #3 ("the camera should stay in the
    // bounds"): `clamp_cam` is the single enforcer — every read site
    // (`clamped_cam`) funnels through it, so the widget can never escape the
    // stage regardless of a stale drag position or an aspect change shrinking
    // the stage under it.
    const STAGE_W: f32 = 400.0;
    const STAGE_H: f32 = 700.0;

    #[test]
    fn camera_inside_bounds_is_unchanged() {
        let (x, y) = clamp_cam(120.0, 300.0, STAGE_W, STAGE_H);
        assert_eq!((x, y), (120.0, 300.0));
    }

    #[test]
    fn camera_past_right_and_bottom_clamps_inside() {
        // Way past the far corner → pinned to the max inset, fully inside.
        let (x, y) = clamp_cam(9_999.0, 9_999.0, STAGE_W, STAGE_H);
        assert_eq!(x, STAGE_W - CAM_W - DRAG_MARGIN);
        assert_eq!(y, STAGE_H - CAM_H - DRAG_MARGIN);
        // The whole widget rect sits within the stage.
        assert!(x + CAM_W + DRAG_MARGIN <= STAGE_W);
        assert!(y + CAM_H + DRAG_MARGIN <= STAGE_H);
    }

    #[test]
    fn camera_past_top_left_clamps_to_margin() {
        let (x, y) = clamp_cam(-50.0, -50.0, STAGE_W, STAGE_H);
        assert_eq!((x, y), (DRAG_MARGIN, DRAG_MARGIN));
    }

    #[test]
    fn stage_smaller_than_widget_pins_to_margin() {
        // An aspect change can shrink the stage below the widget size; the
        // `.max(m)` floor keeps the position valid (top-left margin) instead of
        // producing a negative clamp range that would invert.
        let (x, y) = clamp_cam(200.0, 200.0, CAM_W - 10.0, CAM_H - 10.0);
        assert_eq!((x, y), (DRAG_MARGIN, DRAG_MARGIN));
    }
}
