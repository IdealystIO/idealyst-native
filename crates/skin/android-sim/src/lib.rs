//! Material 3 (Android) skin for the wgpu preview renderer.
//!
//! `AndroidSim` is a unit struct: there's no per-instance
//! state. Construct it once at host startup, hand it as
//! `Rc<dyn Skin>` to the renderer, and every per-frame widget +
//! keyboard paint call routes through the trait impl below.
//!
//! Targets M3 baseline-light: primary `#6750A4`, surface-variant
//! `#E7E0EC`, outline `#79747E`. Ripple, elevation tones, and
//! emphasized motion curves are out of scope.

use std::collections::HashMap;

use glyphon::{Buffer, TextBounds};

use render_wgpu::keyboard::{KeyAction, KeySpec, LaidKey, LayoutMetrics};
use render_wgpu::pipeline::Instance as RectInstance;
use render_wgpu::text::StagedText;
use render_wgpu::widgets::{rect_inst, rect_inst_rotated, TEXT_INPUT_HPAD, TEXT_INPUT_VPAD};
use render_wgpu::{
    Skin, KEYBOARD_KEY_FONT_SIZE, SLIDER_THUMB_SIZE, SLIDER_TRACK_HEIGHT,
    TEXT_INPUT_CARET_WIDTH, TOGGLE_THUMB_INSET,
};

// ---------------------------------------------------------------------------
// M3 baseline light palette (sRGB; `rect_inst` converts to linear)
// ---------------------------------------------------------------------------

pub const M3_PRIMARY: [f32; 4] = [
    0x67 as f32 / 255.0,
    0x50 as f32 / 255.0,
    0xA4 as f32 / 255.0,
    1.0,
]; // #6750A4
const M3_ON_PRIMARY: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
const M3_SURFACE_VARIANT: [f32; 4] = [
    0xE7 as f32 / 255.0,
    0xE0 as f32 / 255.0,
    0xEC as f32 / 255.0,
    1.0,
]; // #E7E0EC
const M3_OUTLINE: [f32; 4] = [
    0x79 as f32 / 255.0,
    0x74 as f32 / 255.0,
    0x7E as f32 / 255.0,
    1.0,
]; // #79747E
const M3_ON_SURFACE_VARIANT: [f32; 4] = [
    0x49 as f32 / 255.0,
    0x45 as f32 / 255.0,
    0x4F as f32 / 255.0,
    0.6,
]; // #49454F @ placeholder alpha
const M3_SURFACE: [f32; 4] = [
    0xFF as f32 / 255.0,
    0xFB as f32 / 255.0,
    0xFE as f32 / 255.0,
    1.0,
]; // #FFFBFE
const M3_KB_BG: [f32; 4] = [
    0xEC as f32 / 255.0,
    0xE6 as f32 / 255.0,
    0xF0 as f32 / 255.0,
    1.0,
]; // surface-container
const M3_KEY_BG_MODIFIER: [f32; 4] = M3_SURFACE_VARIANT;
const M3_KEY_LABEL: [f32; 4] = [
    0x1C as f32 / 255.0,
    0x1B as f32 / 255.0,
    0x1F as f32 / 255.0,
    1.0,
]; // on-surface

// Switch sizing.
const M3_SWITCH_THUMB_DIAM_OFF: f32 = 16.0;
const M3_SWITCH_THUMB_DIAM_ON: f32 = 24.0;
const M3_SWITCH_TRACK_BORDER: f32 = 2.0;

// Slider sizing.
const M3_SLIDER_TRACK_HEIGHT_INACTIVE: f32 = SLIDER_TRACK_HEIGHT;
const M3_SLIDER_TRACK_HEIGHT_ACTIVE: f32 = SLIDER_TRACK_HEIGHT + 2.0;
const M3_SLIDER_THUMB_GAP: f32 = 6.0;
const M3_SLIDER_THUMB_WIDTH: f32 = 4.0;
const M3_SLIDER_THUMB_HEIGHT: f32 = SLIDER_THUMB_SIZE - 4.0;
const M3_SLIDER_THUMB_RADIUS: f32 = 2.0;

// Text field sizing.
const M3_TEXT_BORDER_W_UNFOCUSED: f32 = 1.0;
const M3_TEXT_BORDER_W_FOCUSED: f32 = 2.0;
const M3_TEXT_CORNER_RADIUS: f32 = 4.0;

// Keyboard layout.
const M3_KEY_GAP: f32 = 4.0;
const M3_ROW_GAP: f32 = 8.0;
const M3_SIDE_MARGIN: f32 = 4.0;
const M3_VERT_MARGIN: f32 = 6.0;
const M3_KEY_CORNER_RADIUS: f32 = 8.0;

// Spinner geometry — matches the iOS spinner so M3 has a
// visible loading state until the proper M3 circular
// indeterminate (single growing/shrinking arc) is written.
const SPINNER_BARS: usize = 8;
const SPINNER_BAR_WIDTH_RATIO: f32 = 0.10;
const SPINNER_BAR_LENGTH_RATIO: f32 = 0.28;
const SPINNER_ORBIT_RATIO: f32 = 0.35;
const SPINNER_MIN_BAR_ALPHA: f32 = 0.15;

/// The Material 3 / Android skin. Stateless — instantiate once,
/// wrap in `Rc<dyn Skin>`, hand to the host.
pub struct AndroidSim;

impl AndroidSim {
    pub fn new() -> Self {
        Self
    }
}

impl Default for AndroidSim {
    fn default() -> Self {
        Self::new()
    }
}

impl Skin for AndroidSim {
    // -----------------------------------------------------------
    // Toggle — M3 Switch
    // -----------------------------------------------------------

    fn paint_toggle(
        &self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        t: f32,
        tint: Option<[f32; 4]>,
        rects: &mut Vec<RectInstance>,
    ) {
        let t = t.clamp(0.0, 1.0);
        let track_radius = h * 0.5;
        let is_on = t > 0.5;
        let on_color = tint.unwrap_or(M3_PRIMARY);

        let (track_bg, track_border, track_border_w) = if is_on {
            (on_color, [0.0; 4], 0.0)
        } else {
            (M3_SURFACE_VARIANT, M3_OUTLINE, M3_SWITCH_TRACK_BORDER)
        };

        rects.push(rect_inst(
            x,
            y,
            w,
            h,
            track_bg,
            [track_radius; 4],
            track_border,
            track_border_w,
        ));

        let diameter =
            M3_SWITCH_THUMB_DIAM_OFF + (M3_SWITCH_THUMB_DIAM_ON - M3_SWITCH_THUMB_DIAM_OFF) * t;

        let inset = TOGGLE_THUMB_INSET;
        let thumb_cx_off = x + inset + M3_SWITCH_THUMB_DIAM_OFF * 0.5;
        let thumb_cx_on = x + w - inset - M3_SWITCH_THUMB_DIAM_ON * 0.5;
        let thumb_cx = thumb_cx_off + (thumb_cx_on - thumb_cx_off) * t;
        let thumb_cy = y + h * 0.5;
        let thumb_x = thumb_cx - diameter * 0.5;
        let thumb_y = thumb_cy - diameter * 0.5;

        let thumb_color = if is_on { M3_ON_PRIMARY } else { M3_OUTLINE };

        rects.push(rect_inst(
            thumb_x,
            thumb_y,
            diameter,
            diameter,
            thumb_color,
            [diameter * 0.5; 4],
            [0.0; 4],
            0.0,
        ));
    }

    // -----------------------------------------------------------
    // Slider — M3 Slider
    // -----------------------------------------------------------

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
    ) {
        let active_color = tint.unwrap_or(M3_PRIMARY);
        let thumb_w = M3_SLIDER_THUMB_WIDTH;
        let inset = (SLIDER_THUMB_SIZE * 0.5).max(thumb_w * 0.5);
        let track_x = x + inset;
        let track_w = (w - inset * 2.0).max(0.0);
        let center_y = y + h * 0.5;

        let t = if max > min {
            ((value - min) / (max - min)).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let fill_w = track_w * t;

        let inactive_y = center_y - M3_SLIDER_TRACK_HEIGHT_INACTIVE * 0.5;
        rects.push(rect_inst(
            track_x,
            inactive_y,
            track_w,
            M3_SLIDER_TRACK_HEIGHT_INACTIVE,
            M3_SURFACE_VARIANT,
            [M3_SLIDER_TRACK_HEIGHT_INACTIVE * 0.5; 4],
            [0.0; 4],
            0.0,
        ));

        let active_w = (fill_w - M3_SLIDER_THUMB_GAP).max(0.0);
        if active_w > 0.0 {
            let active_y = center_y - M3_SLIDER_TRACK_HEIGHT_ACTIVE * 0.5;
            rects.push(rect_inst(
                track_x,
                active_y,
                active_w,
                M3_SLIDER_TRACK_HEIGHT_ACTIVE,
                active_color,
                [M3_SLIDER_TRACK_HEIGHT_ACTIVE * 0.5; 4],
                [0.0; 4],
                0.0,
            ));
        }

        let thumb_x = track_x + fill_w - thumb_w * 0.5;
        let thumb_y = center_y - M3_SLIDER_THUMB_HEIGHT * 0.5;
        rects.push(rect_inst(
            thumb_x,
            thumb_y,
            thumb_w,
            M3_SLIDER_THUMB_HEIGHT,
            active_color,
            [M3_SLIDER_THUMB_RADIUS; 4],
            [0.0; 4],
            0.0,
        ));
    }

    // -----------------------------------------------------------
    // TextInput — M3 Outlined Text Field
    // -----------------------------------------------------------

    fn paint_text_input<'a>(
        &self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        is_focused: bool,
        _draw_caret: bool,
        is_placeholder: bool,
        buffer: &'a Buffer,
        caret_x_local: f32,
        text_color: [f32; 4],
        field_bg: Option<[f32; 4]>,
        rects: &mut Vec<RectInstance>,
        texts: &mut Vec<StagedText<'a>>,
    ) {
        let (border_color, border_w) = if is_focused {
            (M3_PRIMARY, M3_TEXT_BORDER_W_FOCUSED)
        } else {
            (M3_OUTLINE, M3_TEXT_BORDER_W_UNFOCUSED)
        };

        rects.push(rect_inst(
            x,
            y,
            w,
            h,
            field_bg.unwrap_or(M3_SURFACE),
            [M3_TEXT_CORNER_RADIUS; 4],
            border_color,
            border_w,
        ));

        let glyph_color = if is_placeholder { M3_ON_SURFACE_VARIANT } else { text_color };
        let text_x = x + TEXT_INPUT_HPAD;
        let text_y = y + TEXT_INPUT_VPAD;
        let inner_w = (w - TEXT_INPUT_HPAD * 2.0).max(0.0);
        texts.push(StagedText {
            buffer,
            x: text_x,
            y: text_y,
            color: glyph_color,
            clip: TextBounds {
                left: text_x as i32,
                top: y as i32,
                right: (text_x + inner_w) as i32,
                bottom: (y + h) as i32,
            },
        });

        if is_focused && _draw_caret && !is_placeholder {
            let caret_x = text_x + caret_x_local;
            let caret_y = y + TEXT_INPUT_VPAD;
            let caret_h = h - TEXT_INPUT_VPAD * 2.0;
            rects.push(rect_inst(
                caret_x,
                caret_y,
                TEXT_INPUT_CARET_WIDTH,
                caret_h,
                M3_PRIMARY,
                [0.0; 4],
                [0.0; 4],
                0.0,
            ));
        }
    }

    // -----------------------------------------------------------
    // ActivityIndicator — temporary iOS-style dot ring tinted M3.
    // TODO: real M3 circular indeterminate progress (single arc
    // segment that grows + shrinks with the emphasized motion
    // curve).
    // -----------------------------------------------------------

    fn paint_activity_indicator(
        &self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        phase: f32,
        tint: Option<[f32; 4]>,
        rects: &mut Vec<RectInstance>,
    ) {
        let diameter = w.min(h);
        if diameter <= 0.0 {
            return;
        }
        let base_color = tint.unwrap_or(M3_PRIMARY);
        let cx = x + w * 0.5;
        let cy = y + h * 0.5;
        let bar_w = diameter * SPINNER_BAR_WIDTH_RATIO;
        let bar_h = diameter * SPINNER_BAR_LENGTH_RATIO;
        let orbit_r = diameter * SPINNER_ORBIT_RATIO;
        let n = SPINNER_BARS as f32;

        for i in 0..SPINNER_BARS {
            let slot = i as f32;
            let slot_angle = (phase - slot / n) * std::f32::consts::TAU;
            let dx = slot_angle.sin() * orbit_r;
            let dy = -slot_angle.cos() * orbit_r;
            let fade = 1.0 - slot / n;
            let alpha_factor =
                SPINNER_MIN_BAR_ALPHA + (1.0 - SPINNER_MIN_BAR_ALPHA) * fade;
            let bar_color = [
                base_color[0],
                base_color[1],
                base_color[2],
                base_color[3] * alpha_factor,
            ];
            rects.push(rect_inst_rotated(
                cx + dx - bar_w * 0.5,
                cy + dy - bar_h * 0.5,
                bar_w,
                bar_h,
                bar_color,
                [bar_w * 0.5; 4],
                [0.0; 4],
                0.0,
                slot_angle,
            ));
        }
    }

    // -----------------------------------------------------------
    // On-screen keyboard — Gboard-style flat letters on the
    // surface-container panel, modifier chips in surface-variant.
    // -----------------------------------------------------------

    fn keyboard_rows(&self) -> Vec<Vec<KeySpec>> {
        let l = render_wgpu::keyboard::letter;
        vec![
            vec![
                l('q', "q"), l('w', "w"), l('e', "e"), l('r', "r"), l('t', "t"),
                l('y', "y"), l('u', "u"), l('i', "i"), l('o', "o"), l('p', "p"),
            ],
            vec![
                l('a', "a"), l('s', "s"), l('d', "d"), l('f', "f"), l('g', "g"),
                l('h', "h"), l('j', "j"), l('k', "k"), l('l', "l"),
            ],
            vec![
                l('z', "z"), l('x', "x"), l('c', "c"), l('v', "v"), l('b', "b"),
                l('n', "n"), l('m', "m"),
                KeySpec { label: "⌫", action: KeyAction::Backspace, width_units: 1.5 },
            ],
            vec![
                KeySpec { label: "space", action: KeyAction::Space, width_units: 5.0 },
                KeySpec { label: "return", action: KeyAction::Enter, width_units: 2.0 },
            ],
        ]
    }

    fn keyboard_layout_metrics(&self) -> LayoutMetrics {
        LayoutMetrics {
            key_gap: M3_KEY_GAP,
            row_gap: M3_ROW_GAP,
            side_margin: M3_SIDE_MARGIN,
            vert_margin: M3_VERT_MARGIN,
        }
    }

    fn paint_keyboard<'a>(
        &self,
        keyboard_rect: (f32, f32, f32, f32),
        laid_keys: &[LaidKey],
        glyphs: &'a HashMap<&'static str, Buffer>,
        rects: &mut Vec<RectInstance>,
        texts: &mut Vec<StagedText<'a>>,
    ) {
        let (kb_x, kb_y, kb_w, kb_h) = keyboard_rect;

        rects.push(rect_inst(
            kb_x,
            kb_y,
            kb_w,
            kb_h,
            M3_KB_BG,
            [0.0; 4],
            [0.0; 4],
            0.0,
        ));

        for k in laid_keys {
            // Letter keys render flat — just glyph on the panel.
            let is_modifier = !matches!(k.action, KeyAction::Character(_));
            if is_modifier {
                rects.push(rect_inst(
                    k.x,
                    k.y,
                    k.w,
                    k.h,
                    M3_KEY_BG_MODIFIER,
                    [M3_KEY_CORNER_RADIUS; 4],
                    [0.0; 4],
                    0.0,
                ));
            }

            if let Some(buf) = glyphs.get(k.label) {
                let label_w = buf.layout_runs().next().map(|r| r.line_w).unwrap_or(0.0);
                let label_h = KEYBOARD_KEY_FONT_SIZE * 1.2;
                let tx = k.x + (k.w - label_w) * 0.5;
                let ty = k.y + (k.h - label_h) * 0.5;
                texts.push(StagedText {
                    buffer: buf,
                    x: tx,
                    y: ty,
                    color: M3_KEY_LABEL,
                    clip: TextBounds {
                        left: k.x as i32,
                        top: k.y as i32,
                        right: (k.x + k.w) as i32,
                        bottom: (k.y + k.h) as i32,
                    },
                });
            }
        }
    }
}
