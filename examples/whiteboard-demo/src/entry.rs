//! The app component (`app`), its root-scoped [`BoardState`], the recording /
//! self-capture bundles, and external registration — the surface the CLI host
//! wrapper calls into.

// Link anchor: `canvas-native` self-registers its renderer via `inventory` at
// backend construction, but only if the crate is actually linked — an otherwise-
// unreferenced rlib dep gets dropped and its `inventory::submit!` never runs (the
// canvas would then not render, notably on web where native is the only renderer).
// `use … as _` forces the link without a concrete-typed call. `video` needs no
// anchor (it's referenced directly via `video::Video` in `screens.rs`). See
// [[project_inventory_self_registration]]. Mirrors `examples/canvas-demo`.
use canvas_native as _;

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use camera::MediaStream;
use runtime_core::primitives::navigator::use_can_go_back;
use runtime_core::{component, node_ref, signal, ui, Element, Ref, Screen, Signal};
use stack_navigator::{Navigator, StackBuilder, StackHandle, StackScreenExt};

use crate::settings::{CameraShape, CameraSize, CanvasBg};
use crate::{BoardScreen, CanvasDoc, CanvasStore, PreviewScreen, SettingsScreen, Strokes};

// ============================================================================
// External registration
// ============================================================================

/// Register the externals the board needs: the canvas renderer (so the drawable
/// surface paints) and the video display (camera + recording-preview). The
/// chrome is plain in-tree views, so no screen-recorder/PrivateLayer
/// registration is needed; `camera` needs none.
///
/// One generic function over `Backend` — not four concrete-typed copies. The
/// CLI host wrapper calls this with the platform's concrete backend, which binds
/// to `B` directly.
///
/// `canvas-native` and `video` self-register via `inventory` at backend
/// construction (which runs before this is called), so they need no explicit
/// call here. `canvas-vello` has no inventory hook — it's generic over `Backend`
/// and pulls the GPU surface from `create_graphics` — so it's the only one
/// registered here, last (last-registration-wins over native where vello is
/// viable; it self-gates off on devices without f16). The lone `cfg` mirrors the
/// dependency table: `canvas-vello` is only compiled for ios/android/macos.
pub fn register_extensions<B: runtime_core::RegisterExternal>(backend: &mut B) {
    #[cfg(any(target_os = "ios", target_os = "android", target_os = "macos"))]
    canvas_vello::register(backend);
    #[cfg(not(any(target_os = "ios", target_os = "android", target_os = "macos")))]
    let _ = backend; // web + desktop: inventory-only; vello absent from the build
}

// ============================================================================
// Recording / self-capture bundles
// ============================================================================

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
    /// Whether the Layers popover (canvas list) is open. Mutually exclusive with
    /// `palette_open` — opening one closes the other (both dock by the rail).
    pub layers_open: Signal<bool>,
    /// Index of the active canvas in the [`CanvasStore`].
    pub active_canvas: Signal<usize>,
    /// The canvas ids, in store order — the reactive source the Layers list
    /// iterates (`for id in canvas_ids, key = id`). Mutated only when membership
    /// changes (add/delete/reset), so a plain switch doesn't rebuild the list.
    /// The heavy stroke docs live in the `!Copy` [`CanvasStore`] alongside it.
    pub canvas_ids: Signal<Vec<u64>>,
    /// Monotonic id source for new [`CanvasDoc`]s (stable list-reconciliation keys).
    pub next_id: Signal<u64>,
    /// Board aspect ratio `(width, height)` — drives the centered canvas "stage".
    pub aspect: Signal<(u32, u32)>,
    /// Canvas drawing-surface background (`Auto` follows the app theme).
    pub canvas_bg: Signal<CanvasBg>,
    /// App theme: `true` = dark. Drives the reactive theme install + the `Auto`
    /// canvas background.
    pub dark: Signal<bool>,
    /// Camera widget shape (rounded rect / circle).
    pub camera_shape: Signal<CameraShape>,
    /// Camera widget size (S / M / L).
    pub camera_size: Signal<CameraSize>,
    /// Whether the app-level keyboard shortcuts (←/→/+/−) are active.
    pub keys_enabled: Signal<bool>,
    /// Whether the two-finger swipe-between-canvases gesture is active.
    pub gestures_enabled: Signal<bool>,
    /// Canvas-change cross-dissolve progress, `0` (just swapped) → `1` (settled).
    /// Read by the canvas scene draw to fade the outgoing strokes out and the live
    /// strokes in; tweened by an Effect that fires on each `active_canvas` change.
    /// See [`crate::CANVAS_FADE_MS`].
    pub canvas_anim: Signal<f32>,
    /// The bound navigator handle — `push(&SETTINGS, ())` from the FAB,
    /// `push(&PREVIEW, ())` when a recording stops.
    pub nav: Ref<StackHandle>,
}

impl Default for BoardState {
    fn default() -> Self {
        Self {
            width: Signal::new(crate::WIDTH_MEDIUM),
            color_css: Signal::new(crate::PALETTE[0].1),
            cam_on: Signal::new(false),
            cam_stream: Signal::new(None),
            cam_x: Signal::new(-1.0),
            cam_y: Signal::new(-1.0),
            recording: Signal::new(false),
            rec_path: Signal::new(None),
            palette_open: Signal::new(false),
            layers_open: Signal::new(false),
            active_canvas: Signal::new(0),
            canvas_ids: Signal::new(vec![0]),
            next_id: Signal::new(1),
            aspect: Signal::new(crate::settings::DEFAULT_ASPECT),
            canvas_bg: Signal::new(CanvasBg::Auto),
            dark: Signal::new(false),
            camera_shape: Signal::new(CameraShape::RoundedRect),
            camera_size: Signal::new(CameraSize::Medium),
            keys_enabled: Signal::new(true),
            gestures_enabled: Signal::new(true),
            canvas_anim: Signal::new(1.0),
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

    // ---- State (root scope → survives navigation) ------------------------
    let nav: Ref<StackHandle> = node_ref!();
    let state = BoardState {
        width: signal!(crate::WIDTH_MEDIUM),
        color_css: signal!(crate::PALETTE[0].1),
        cam_on: signal!(false),
        cam_stream: signal!(None),
        // Camera widget top-left, in viewport points. `-1` = "not yet placed";
        // an Effect drops it bottom-left once the viewport size is known.
        cam_x: signal!(-1.0),
        cam_y: signal!(-1.0),
        recording: signal!(false),
        rec_path: signal!(None),
        palette_open: signal!(false),
        layers_open: signal!(false),
        active_canvas: signal!(0),
        // Canvas 0 is seeded below (store + this id list); next new canvas = id 1.
        canvas_ids: signal!(vec![0u64]),
        next_id: signal!(1),
        aspect: signal!(crate::settings::DEFAULT_ASPECT),
        canvas_bg: signal!(CanvasBg::Auto),
        dark: signal!(start_dark),
        camera_shape: signal!(CameraShape::RoundedRect),
        camera_size: signal!(CameraSize::Medium),
        keys_enabled: signal!(true),
        gestures_enabled: signal!(true),
        canvas_anim: signal!(1.0),
        nav,
    };

    // Theme, driven reactively by `state.dark` (the Settings toggle is the single
    // source of truth). One framework call installs the component sheets, the
    // initial theme, AND an internal effect that swaps the active theme whenever
    // `dark` flips — re-resolving every token-based style and repainting the host
    // window background (the theme system routes `color-background` through
    // `set_app_background`, which on macOS paints `NSWindow.backgroundColor`
    // without detaching the canvas's `CAMetalLayer`). Replaces a hand-rolled
    // `effect!` + a separate `set_app_background` effect.
    {
        let dark = state.dark;
        idea_ui::install_idea_theme_reactive(move || {
            if dark.get() { idea_ui::dark_theme() } else { idea_ui::light_theme() }
        });
    }

    // `rec_handle` holds the live media-writer `Recording` (consumed on stop).
    // It's `!Send` + non-`Copy`, so it lives outside `BoardState` and is cloned
    // into the board builder. Strokes + a repaint tick are likewise shared.
    let rec_handle: RecHandle = Rc::new(RefCell::new(None));
    let strokes: Strokes = Rc::new(RefCell::new(Vec::new()));
    let version: Signal<u64> = signal!(0);

    // The saved canvas documents. Seeded with the active doc (id 0); the live
    // `strokes` Rc above is canvas 0's working copy. `!Copy`, so it's cloned into
    // the board chrome + Settings builders like `strokes`/`rec_handle`.
    let canvases: CanvasStore = Rc::new(RefCell::new(vec![CanvasDoc { strokes: Vec::new() }]));

    // App-level keyboard shortcuts (desktop): ←/→ switch canvases, +/- add/remove.
    // Installed once via the cross-backend `set_app_key_handler` hook (web
    // `document` listener, macOS `NSEvent` monitor, …) — fires regardless of
    // focus. Gated on `keys_enabled` (a Settings toggle) and the canvas store, so
    // it survives navigation (root-scoped capture). Returns `PreventDefault` only
    // when it acts, so other keys route normally and macOS doesn't beep.
    {
        let canvases = canvases.clone();
        let strokes = strokes.clone();
        let active = state.active_canvas;
        let canvas_ids = state.canvas_ids;
        let next_id = state.next_id;
        let keys_enabled = state.keys_enabled;
        runtime_core::set_app_key_handler(Some(Rc::new(move |ev: &runtime_core::KeyEvent| {
            if !keys_enabled.get() {
                return runtime_core::KeyOutcome::Default;
            }
            match crate::key_action(&ev.key) {
                Some(action) => {
                    crate::apply_canvas_action(
                        action, &canvases, &strokes, active, version, canvas_ids, next_id,
                    );
                    runtime_core::KeyOutcome::PreventDefault
                }
                None => runtime_core::KeyOutcome::Default,
            }
        })));
    }

    // Drive a frame-rate repaint while the camera is on, so its composited
    // texture shows live frames (the canvas otherwise repaints only on a stroke
    // or drag). Root-scoped so it survives navigation.
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

    // Canvas self-capture (app-owned, root-scoped so it survives between
    // recordings): the Canvas writes each frame into `writer`, the record button
    // records `stream`. `with_surface_capture` takes the zero-copy IOSurface path
    // on macOS and the CPU read-back path elsewhere — no per-platform branch here.
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
        let camera_shape = state.camera_shape;
        let camera_size = state.camera_size;
        runtime_core::effect!({
            let (aw, ah) = aspect.get();
            let (_sx, _sy, sw, sh) = crate::settings::stage_geom(aw, ah);
            let (_cw, ch, _r) = crate::settings::camera_dims(camera_shape.get(), camera_size.get());
            if !placed.get() && sw > 1.0 && sh > 1.0 {
                cam_x.set(crate::DRAG_MARGIN);
                cam_y.set((sh - ch - crate::DRAG_MARGIN).max(crate::DRAG_MARGIN));
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
    let builder = Navigator::new(&crate::BOARD)
        .screen(crate::BOARD, {
            let strokes = strokes.clone();
            let rec_handle = rec_handle.clone();
            let capture = capture.clone();
            let canvases = canvases.clone();
            move |_| {
                // `focused` is computed INSIDE the board-route builder so
                // `use_can_go_back()` resolves in the navigator scope. `true`
                // while the board is the stack root (no Settings/Preview pushed):
                // the chrome is an in-tree sibling of the canvas, so it must
                // vanish when a screen is pushed or it lingers over
                // Settings/Preview. We gate on `!use_can_go_back()` rather
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
                        canvases = canvases.clone(),
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
        .screen(crate::SETTINGS, {
            let strokes = strokes.clone();
            let canvases = canvases.clone();
            move |_| {
                Screen::new(ui! {
                    SettingsScreen(state = state, strokes = strokes.clone(), canvases = canvases.clone(), version = version)
                })
                .header_shown(false)
            }
        })
        .screen(crate::PREVIEW, move |_| {
            Screen::new(ui! {
                PreviewScreen(rec_path = state.rec_path, playback_url = preview_url, aspect = state.aspect, nav = nav)
            })
            .header_shown(false)
        });

    ui! { builder.bind(nav) }
}
