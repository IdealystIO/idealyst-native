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
//!    FAB), as in-tree sibling overlays of the canvas (no separate window — the
//!    recording captures the canvas/GPU stream directly, so the chrome is never
//!    in it). Each dock's content is `!use_can_go_back()`-gated so it mounts only
//!    while the board is the active route and doesn't linger over a pushed screen.
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
//! - [`entry`] — the `app` entry component, root-scoped `BoardState`, the
//!   recording/self-capture bundles, and `register_extensions`.
//! - [`document`] — the drawing + document model: strokes, the multi-canvas
//!   store and its ops, canvas-navigation commands, stroke color resolution.
//! - [`board`] — the recordable board content: `BoardScreen`, `DrawingSurface`,
//!   `CameraWidget`.
//! - [`chrome`] — the floating in-tree chrome: tool rail, palette, record dock,
//!   REC pill, settings FAB (`build_chrome` assembles them).
//! - [`screens`] — the navigator-pushed `SettingsScreen` / `PreviewScreen` and
//!   the shared `ScreenScaffold` / `Label`.
//! - [`style`] — shared `StyleRules` helpers (`radius`, `border_all`,
//!   `reactive_style`, `focus_gate`, …).
//! - `tests` — unit tests for [`document`] + the camera-clamp helpers below.

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
mod document;
mod entry;
mod screens;
mod settings;
mod style;
#[cfg(test)]
mod tests;

use runtime_core::{Route, Signal};

// Re-export each module's surface at the crate root so call sites stay
// `crate::X` regardless of which file an item lives in (and so the CLI host
// wrapper reaches `app` / `register_extensions`). The entry module is named
// `entry`, not `app`, because the `#[component] fn app` generates a type alias
// `app` that would collide with a module of the same name in the type namespace.
pub use entry::{app, register_extensions, CanvasCapture};
pub(crate) use entry::{BoardState, RecHandle};
pub(crate) use board::BoardScreen;
pub(crate) use document::*;
pub(crate) use screens::{PreviewScreen, SettingsScreen};
pub(crate) use settings::{CameraShape, CameraSize, CanvasBg};

// ============================================================================
// Routes
// ============================================================================

// One stack-navigator screen each. The board is the root; Settings and Preview
// are pushed onto the stack (native push/pop + back gesture on iOS/Android/web;
// a child-swap on macOS). The board screen sets `unmount_on_blur(false)` (the
// default) so it stays alive under a pushed screen — the camera keeps running and
// strokes persist — but its floating chrome is hidden via `!use_can_go_back()` so
// the toolbar doesn't linger over Settings.
pub(crate) const BOARD: Route<()> = Route::<()>::new("board", "/");
pub(crate) const SETTINGS: Route<()> = Route::<()>::new("settings", "/settings");
pub(crate) const PREVIEW: Route<()> = Route::<()>::new("preview", "/preview");

// ============================================================================
// Layout / animation / recording constants
// ============================================================================

/// Keep dragged content this far from the stage edges.
pub(crate) const DRAG_MARGIN: f32 = 8.0;

/// Layers popover open/close animation timings (ms). `EXIT` is also how long an
/// "add canvas" tap defers its store mutation, so the new row materializes only
/// AFTER the popover has fully animated out — adding it mid-exit made the row
/// pop into the closing panel (the stutter the user reported).
pub(crate) const LAYERS_ENTER_MS: u32 = 170;
pub(crate) const LAYERS_EXIT_MS: u32 = 130;

/// Canvas-change transition duration (ms): when the active canvas changes (add /
/// switch / swipe / arrow key) the drawing cross-dissolves — the outgoing
/// strokes fade out while the incoming ones fade in, over the constant
/// background. Driven by a `0→1` progress signal (`BoardState::canvas_anim`)
/// tweened over this duration.
///
/// The fade is done IN the canvas scene (per-stroke alpha over the opaque
/// background), NOT at the view/compositor level, so the live camera — a texture
/// layer composited on top — is excluded: only the background animates, and
/// nothing moves. (A compositor-level opacity fade would also fade the camera; a
/// layer transform would detach the GPU `CAMetalLayer` on macOS.)
pub(crate) const CANVAS_FADE_MS: u64 = 260;

/// Tool-rail metrics — a button is square; the rail is the button + padding.
pub(crate) const TOOL_BTN: f32 = 44.0;
pub(crate) const RAIL_EDGE: f32 = 14.0; // gap from the screen edge (added to safe inset)

/// The `files` store + filename a recording is written to.
pub(crate) const REC_STORE: &str = "recordings";
pub(crate) const REC_FILE: &str = "recording.mp4";

// ============================================================================
// Camera placement helpers
// ============================================================================

/// Clamp a camera position (STAGE-local points) so a `cam_w × cam_h` widget stays
/// fully inside a `sw × sh` stage, with a [`DRAG_MARGIN`] inset.
pub(crate) fn clamp_cam(
    cx: f32,
    cy: f32,
    sw: f32,
    sh: f32,
    cam_w: f32,
    cam_h: f32,
) -> (f32, f32) {
    let m = DRAG_MARGIN;
    let max_x = (sw - cam_w - m).max(m);
    let max_y = (sh - cam_h - m).max(m);
    (cx.clamp(m, max_x), cy.clamp(m, max_y))
}

/// The camera widget's clamped top-left for the current aspect/viewport + camera
/// shape/size — reads all five signals (+ viewport/insets via `stage_geom`)
/// reactively, so every read site (widget box, composited layer rect) agrees and
/// stays in bounds even across an aspect or size change. `(x, y)` in stage points.
pub(crate) fn clamped_cam(
    aspect: Signal<(u32, u32)>,
    cam_x: Signal<f32>,
    cam_y: Signal<f32>,
    cam_shape: Signal<CameraShape>,
    cam_size: Signal<CameraSize>,
) -> (f32, f32) {
    let (aw, ah) = aspect.get();
    let (_x, _y, sw, sh) = settings::stage_geom(aw, ah);
    let (cw, ch, _r) = settings::camera_dims(cam_shape.get(), cam_size.get());
    clamp_cam(cam_x.get(), cam_y.get(), sw, sh, cw, ch)
}
