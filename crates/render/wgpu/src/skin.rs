//! The pluggable platform-skin contract.
//!
//! A `Skin` is a stateless palette + paint policy: how to draw
//! each native widget chrome (toggle, slider, text input,
//! activity indicator) plus the on-screen keyboard layout and
//! its key chrome. The renderer holds an `Rc<dyn Skin>` and
//! dispatches every per-frame paint call through it.
//!
//! Skins live in their own crates (`ios-sim`, `android-sim`)
//! and depend on this one for the trait + helper types
//! (`RectInstance`, `StagedText`, `KeySpec`, `LayoutMetrics`,
//! `rect_inst`). The preview variant crates instantiate a
//! concrete skin and hand it to the host shell at `run` time.
//!
//! No `SimulatedPlatform` matching here — the trait IS the
//! dispatch. Two-platform parity is enforced by the trait, not
//! by an enum that future skins would have to extend.

use std::collections::HashMap;

use glyphon::Buffer;

use crate::keyboard::{KeySpec, LaidKey, LayoutMetrics};
use crate::pipeline::Instance as RectInstance;
use crate::text::StagedText;

/// A platform skin: every paint method any of the native widgets
/// or the on-screen keyboard could need, plus the keyboard's
/// row content + inter-key spacing.
///
/// Skins are stateless w.r.t. per-widget animation — `t`,
/// `value`, `phase`, focus flags, etc. are passed in by the
/// renderer's walk. Use `Rc<dyn Skin>` to hand a skin to the
/// host; the renderer holds one for the lifetime of the frame.
pub trait Skin {
    // -----------------------------------------------------------
    // Native widgets
    // -----------------------------------------------------------

    /// Append rect instances for a Toggle at the given frame.
    /// `t` is the thumb position in `0..=1`; `tint` overrides the
    /// ON-state track color (else use the skin's default accent).
    fn paint_toggle(
        &self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        t: f32,
        tint: Option<[f32; 4]>,
        rects: &mut Vec<RectInstance>,
    );

    /// Append rect instances for a Slider at the given frame.
    /// `tint` overrides the active-track color.
    #[allow(clippy::too_many_arguments)]
    fn paint_slider(
        &self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        value: f32,
        min: f32,
        max: f32,
        tint: Option<[f32; 4]>,
        rects: &mut Vec<RectInstance>,
    );

    /// Append rect + text instances for a TextInput. `field_bg`
    /// overrides the field's fill color.
    #[allow(clippy::too_many_arguments)]
    fn paint_text_input<'a>(
        &self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        is_focused: bool,
        draw_caret: bool,
        is_placeholder: bool,
        buffer: &'a Buffer,
        caret_x_local: f32,
        text_color: [f32; 4],
        field_bg: Option<[f32; 4]>,
        rects: &mut Vec<RectInstance>,
        texts: &mut Vec<StagedText<'a>>,
    );

    /// Append rect instances for an ActivityIndicator. `phase`
    /// is the rotation phase in `[0.0, 1.0)`; `tint` overrides
    /// the skin's default spinner color.
    fn paint_activity_indicator(
        &self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        phase: f32,
        tint: Option<[f32; 4]>,
        rects: &mut Vec<RectInstance>,
    );

    // -----------------------------------------------------------
    // On-screen keyboard
    // -----------------------------------------------------------

    /// Row content for the keyboard. Called by the shared
    /// keyboard layout engine on every frame the keyboard is
    /// visible. Keep allocations light — the `Vec`s are
    /// short-lived.
    fn keyboard_rows(&self) -> Vec<Vec<KeySpec>>;

    /// Inter-key spacing knobs. Consumed by the shared layout
    /// engine; lets each skin tighten or loosen its key gaps
    /// without touching the layout math.
    fn keyboard_layout_metrics(&self) -> LayoutMetrics;

    /// Paint the keyboard overlay. The shared layout engine has
    /// already produced `keyboard_rect` (full panel) and
    /// `laid_keys` (each key's absolute screen rect). The skin
    /// is responsible for the panel background, every key's
    /// chrome, and the label glyphs.
    fn paint_keyboard<'a>(
        &self,
        keyboard_rect: (f32, f32, f32, f32),
        laid_keys: &[LaidKey],
        glyphs: &'a HashMap<&'static str, Buffer>,
        rects: &mut Vec<RectInstance>,
        texts: &mut Vec<StagedText<'a>>,
    );
}
