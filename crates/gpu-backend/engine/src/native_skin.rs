//! `NativeSkin` — the no-chrome [`Painter`] for running the wgpu backend
//! as a *real native desktop renderer* (not an iOS/Android simulator).
//!
//! The `ios-sim` / `android-sim` skins exist to *approximate a phone*: a
//! device bezel, a status bar, an on-screen keyboard, and platform-styled
//! native widgets, so a developer on a desktop can preview a mobile app.
//! `NativeSkin` is the opposite intent — the desktop OS window itself is
//! the chrome, so this skin draws **no** bezel and no on-screen keyboard,
//! and no-ops the platform-widget paints. Views, text, buttons, images,
//! gradients, borders, shadows and animations all render via the
//! `Renderer` directly (skin-independent); only the cosmetic
//! platform-widget overlays (toggle knob, slider track, text-field caret
//! chrome, navigator title bar) are the skin's job, and on a real native
//! host those are absent by design.
//!
//! The one knob that matters is [`platform`](NativeSkin::platform): unlike
//! the sim skins (which report `Custom("Sim")` so author code thinks it's
//! on a simulator), `NativeSkin` reports the **actual host OS** it was
//! constructed for (`MacOs`, `Windows`, `Linux`). Author code reading
//! `runtime_core::platform()` then takes its genuine native branch — e.g.
//! idea-ui-docs renders its desktop custom-header + pinned-sidebar layout
//! under `Platform::MacOs`, exactly as the AppKit backend would. This is
//! what makes the wgpu backend a universal native target rather than a
//! mobile previewer.

use std::collections::HashMap;

use glyphon::Buffer;
use runtime_core::Platform;

use crate::keyboard::{KeySpec, LaidKey, LayoutMetrics};
use crate::painter::{NavigatorHeaderChrome, NavigatorHeaderHit, Painter};
use crate::pipeline::Instance as RectInstance;
use crate::text::StagedText;

/// A no-chrome [`Painter`] that reports a real host-OS [`Platform`]
/// identity. See the module docs for why the paints are no-ops and the
/// platform read-out matters.
pub struct NativeSkin {
    platform: Platform,
}

impl NativeSkin {
    /// Build a native skin reporting `platform` to author code. Pick the
    /// value matching the host the window actually runs on (`MacOs` on
    /// macOS, etc.) so `runtime_core::platform()` branches natively.
    pub fn new(platform: Platform) -> Self {
        Self { platform }
    }
}

impl Painter for NativeSkin {
    fn platform(&self) -> Platform {
        self.platform
    }

    // ---- Platform-widget overlays: absent on a real native host. The
    // layout box still exists (the framework laid it out and the renderer
    // drew its styled background/border); only the skin-drawn knob / track
    // / caret / title-bar is omitted. ----

    fn paint_toggle(
        &self,
        _x: f32,
        _y: f32,
        _w: f32,
        _h: f32,
        _t: f32,
        _tint: Option<[f32; 4]>,
        _rects: &mut Vec<RectInstance>,
    ) {
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_slider(
        &self,
        _x: f32,
        _y: f32,
        _w: f32,
        _h: f32,
        _value: f32,
        _min: f32,
        _max: f32,
        _tint: Option<[f32; 4]>,
        _rects: &mut Vec<RectInstance>,
    ) {
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_text_input<'a>(
        &self,
        _x: f32,
        _y: f32,
        _w: f32,
        _h: f32,
        _is_focused: bool,
        _draw_caret: bool,
        _is_placeholder: bool,
        _buffer: &'a Buffer,
        _caret_x_local: f32,
        _text_color: [f32; 4],
        _field_bg: Option<[f32; 4]>,
        _rects: &mut Vec<RectInstance>,
        _texts: &mut Vec<StagedText<'a>>,
    ) {
    }

    fn paint_activity_indicator(
        &self,
        _x: f32,
        _y: f32,
        _w: f32,
        _h: f32,
        _phase: f32,
        _tint: Option<[f32; 4]>,
        _rects: &mut Vec<RectInstance>,
    ) {
    }

    // On-screen keyboard — a desktop host has a physical keyboard, so
    // there are no rows to lay out and nothing to paint.
    fn keyboard_rows(&self) -> Vec<Vec<KeySpec>> {
        Vec::new()
    }

    fn keyboard_layout_metrics(&self) -> LayoutMetrics {
        LayoutMetrics {
            key_gap: 0.0,
            row_gap: 0.0,
            side_margin: 0.0,
            vert_margin: 0.0,
        }
    }

    fn paint_keyboard<'a>(
        &self,
        _keyboard_rect: (f32, f32, f32, f32),
        _laid_keys: &[LaidKey],
        _pressed_label: Option<&'static str>,
        _glyphs: &'a HashMap<&'static str, Buffer>,
        _rects: &mut Vec<RectInstance>,
        _texts: &mut Vec<StagedText<'a>>,
    ) {
    }

    // Navigator header — the app owns its chrome on desktop (idea-ui-docs
    // renders a custom header via `TopSlot::Custom`), so the skin draws no
    // native title bar. The screen body still renders.
    fn paint_navigator_header<'a, 'b>(
        &self,
        _rect: (f32, f32, f32, f32),
        _chrome: NavigatorHeaderChrome<'a, 'b>,
        _rects: &mut Vec<RectInstance>,
        _texts: &mut Vec<StagedText<'a>>,
        _hit_regions: &mut Vec<NavigatorHeaderHit>,
    ) {
    }
}
