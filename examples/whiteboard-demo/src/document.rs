//! The whiteboard's drawing + document model: strokes, the multi-canvas store,
//! the canvas-navigation commands (keyboard / swipe), and stroke color
//! resolution. All renderer-agnostic and free of UI — the unit tests in
//! [`crate::tests`] exercise this module directly.

use std::cell::RefCell;
use std::rc::Rc;

use runtime_core::Signal;

use crate::settings::CanvasBg;

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

// ============================================================================
// Multi-canvas documents
// ============================================================================

/// One whiteboard "canvas" — a saved document of strokes. The *active*
/// document's working copy lives in the shared [`Strokes`] Rc that the
/// painter/`on_touch`/capture all use; the store holds the inactive docs (and a
/// snapshot of the active one, refreshed on every switch/add/delete). This keeps
/// ONE canvas surface + ONE capture stream, so a switch is just swapping the live
/// strokes' contents — the recording sees no seam.
///
/// Stable ids for the Layers list live in the parallel `canvas_ids: Signal<Vec<u64>>`
/// (positionally aligned with this store); display labels are positional
/// ("Canvas {i+1}").
#[derive(Clone, Default)]
pub(crate) struct CanvasDoc {
    pub strokes: Vec<Stroke>,
}

/// The saved canvas documents. `!Copy` (like [`Strokes`]), so it lives outside
/// `BoardState` and is threaded into the builders that need it.
pub(crate) type CanvasStore = Rc<RefCell<Vec<CanvasDoc>>>;

/// Snapshot the live strokes into the active document so an out-of-document op
/// (switch/add/delete) doesn't lose the current canvas's edits.
pub(crate) fn save_active(store: &CanvasStore, strokes: &Strokes, active: Signal<usize>) {
    let idx = active.get();
    if let Some(doc) = store.borrow_mut().get_mut(idx) {
        doc.strokes = strokes.borrow().clone();
    }
}

/// Jump to canvas `target`: save the current doc, load the target's strokes into
/// the live Rc, and bump `version` so the (single) canvas surface repaints the
/// loaded document — including mid-recording, where the capture loop is already
/// ticking `version`. Membership is unchanged, so `canvas_ids` is NOT touched
/// (the Layers list keeps its rows; only the active highlight, driven by
/// `active`, moves).
pub(crate) fn switch_canvas(
    store: &CanvasStore,
    strokes: &Strokes,
    active: Signal<usize>,
    version: Signal<u64>,
    target: usize,
) {
    if target == active.get() || target >= store.borrow().len() {
        return;
    }
    save_active(store, strokes, active);
    let loaded = store.borrow()[target].strokes.clone();
    *strokes.borrow_mut() = loaded;
    active.set(target);
    version.set(version.get().wrapping_add(1));
}

/// Append a fresh empty canvas and switch to it. `canvas_ids` (the reactive
/// Layers-list source) gains the new id in lock-step with the store.
pub(crate) fn add_canvas(
    store: &CanvasStore,
    strokes: &Strokes,
    active: Signal<usize>,
    version: Signal<u64>,
    canvas_ids: Signal<Vec<u64>>,
    next_id: Signal<u64>,
) {
    save_active(store, strokes, active);
    let id = next_id.get();
    next_id.set(id + 1);
    store.borrow_mut().push(CanvasDoc { strokes: Vec::new() });
    let idx = store.borrow().len() - 1;
    *strokes.borrow_mut() = Vec::new();
    active.set(idx);
    let mut ids = canvas_ids.get();
    ids.push(id);
    canvas_ids.set(ids);
    version.set(version.get().wrapping_add(1));
}

/// Remove canvas `idx`. No-op when only one canvas remains (a board always has at
/// least one). The active index re-clamps so it keeps pointing at a live doc, its
/// strokes reload into the live Rc, and `canvas_ids` drops the id in lock-step.
pub(crate) fn delete_canvas(
    store: &CanvasStore,
    strokes: &Strokes,
    active: Signal<usize>,
    version: Signal<u64>,
    canvas_ids: Signal<Vec<u64>>,
    idx: usize,
) {
    if store.borrow().len() <= 1 || idx >= store.borrow().len() {
        return;
    }
    // Persist the active doc first so deleting a DIFFERENT canvas keeps the
    // current canvas's in-progress edits.
    save_active(store, strokes, active);
    let cur = active.get();
    store.borrow_mut().remove(idx);
    let len = store.borrow().len();
    // Shift the active pointer to still address a live doc.
    let new_active = if idx < cur {
        cur - 1
    } else if idx == cur {
        cur.min(len - 1)
    } else {
        cur
    };
    let loaded = store.borrow()[new_active].strokes.clone();
    *strokes.borrow_mut() = loaded;
    active.set(new_active);
    let mut ids = canvas_ids.get();
    if idx < ids.len() {
        ids.remove(idx);
    }
    canvas_ids.set(ids);
    version.set(version.get().wrapping_add(1));
}

/// Collapse the whole board back to a single empty canvas — used when the aspect
/// ratio changes (every doc's stage-local strokes are invalidated by the new
/// stage size).
pub(crate) fn reset_canvases(
    store: &CanvasStore,
    strokes: &Strokes,
    active: Signal<usize>,
    version: Signal<u64>,
    canvas_ids: Signal<Vec<u64>>,
    next_id: Signal<u64>,
) {
    let id = next_id.get();
    next_id.set(id + 1);
    *store.borrow_mut() = vec![CanvasDoc { strokes: Vec::new() }];
    *strokes.borrow_mut() = Vec::new();
    active.set(0);
    canvas_ids.set(vec![id]);
    version.set(version.get().wrapping_add(1));
}

/// Does the board hold ANY strokes, across every canvas? The live Rc is the
/// source of truth for the active doc (the store's copy of it can be stale), so
/// check that plus every OTHER stored doc. Gates the aspect-change confirmation.
pub(crate) fn any_drawings(store: &CanvasStore, strokes: &Strokes, active: Signal<usize>) -> bool {
    if !strokes.borrow().is_empty() {
        return true;
    }
    let cur = active.get();
    store
        .borrow()
        .iter()
        .enumerate()
        .any(|(i, d)| i != cur && !d.strokes.is_empty())
}

// ============================================================================
// Canvas navigation (keyboard + swipe)
// ============================================================================

/// A canvas-navigation command, produced by [`key_action`] (keyboard) and the
/// two-finger swipe (board.rs), then applied against the canvas store.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum CanvasAction {
    /// Previous canvas (clamped at the first).
    Prev,
    /// Next canvas (clamped at the last).
    Next,
    /// Add a new blank canvas + switch to it.
    Add,
    /// Remove the current canvas — or clear it when it's the only one.
    Remove,
}

/// Map an app-level key (Web `KeyboardEvent.key` vocabulary, which every backend
/// normalizes to) to a [`CanvasAction`]. `=`/`_` are accepted alongside `+`/`-`
/// because a keyboard yields the unshifted glyph. Pure → unit-tested.
pub(crate) fn key_action(key: &str) -> Option<CanvasAction> {
    match key {
        "ArrowLeft" => Some(CanvasAction::Prev),
        "ArrowRight" => Some(CanvasAction::Next),
        "+" | "=" => Some(CanvasAction::Add),
        "-" | "_" => Some(CanvasAction::Remove),
        _ => None,
    }
}

/// How far (viewport px) a two-finger swipe must travel to fire.
pub(crate) const SWIPE_THRESHOLD: f32 = 48.0;

/// Decide whether an averaged two-finger drag (from the gesture start) is a
/// horizontal swipe, and which way. Requires horizontal dominance (`|dx| > |dy|`)
/// and clearing `threshold`. Swiping LEFT (fingers move −x, content advances)
/// goes to the Next canvas; RIGHT → Prev. Pure → unit-tested.
pub(crate) fn swipe_action(avg_dx: f32, avg_dy: f32, threshold: f32) -> Option<CanvasAction> {
    if avg_dx.abs() < threshold || avg_dx.abs() <= avg_dy.abs() {
        return None;
    }
    if avg_dx < 0.0 {
        Some(CanvasAction::Next)
    } else {
        Some(CanvasAction::Prev)
    }
}

/// Apply a [`CanvasAction`] to the canvas store. `Prev`/`Next` clamp at the ends;
/// `Remove` clears the sole canvas instead of deleting it. Shared by the keyboard
/// handler and the swipe gesture.
pub(crate) fn apply_canvas_action(
    action: CanvasAction,
    store: &CanvasStore,
    strokes: &Strokes,
    active: Signal<usize>,
    version: Signal<u64>,
    canvas_ids: Signal<Vec<u64>>,
    next_id: Signal<u64>,
) {
    match action {
        CanvasAction::Prev => {
            let i = active.get();
            if i > 0 {
                switch_canvas(store, strokes, active, version, i - 1);
            }
        }
        CanvasAction::Next => {
            let i = active.get();
            if i + 1 < store.borrow().len() {
                switch_canvas(store, strokes, active, version, i + 1);
            }
        }
        CanvasAction::Add => add_canvas(store, strokes, active, version, canvas_ids, next_id),
        CanvasAction::Remove => {
            if store.borrow().len() > 1 {
                delete_canvas(store, strokes, active, version, canvas_ids, active.get());
            } else {
                // The last canvas: clear it rather than delete (a board always
                // has at least one canvas).
                strokes.borrow_mut().clear();
                if let Some(d) = store.borrow_mut().get_mut(0) {
                    d.strokes.clear();
                }
                version.set(version.get().wrapping_add(1));
            }
        }
    }
}

// ============================================================================
// Palette + stroke painting / color resolution
// ============================================================================

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
pub(crate) const INK_ON_LIGHT: &str = "#111827"; // near-black ink for a light backdrop
pub(crate) const INK_ON_DARK: &str = "#f9fafb"; // near-white ink for a dark backdrop

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
