//! iOS / UIKit skin for the wgpu preview renderer.
//!
//! `IosSim` is a unit struct: there's no per-instance state.
//! Construct it once at host startup, hand it as
//! `Rc<dyn Skin>` to the renderer, and every per-frame widget +
//! keyboard paint call routes through the trait impl below.
//!
//! All paint methods append into the renderer's shared
//! `RectInstance` / `StagedText` Vecs. sRGB→linear color
//! conversion lives in [`render_wgpu::widgets::rect_inst`]; this
//! crate only declares the iOS palette in sRGB.

use std::collections::HashMap;

use glyphon::{Buffer, TextBounds};

use render_wgpu::keyboard::{KeyAction, KeySpec, LaidKey, LayoutMetrics};
use render_wgpu::pipeline::Instance as RectInstance;
use render_wgpu::text::StagedText;
use render_wgpu::widgets::{rect_inst, rect_inst_rotated, TEXT_INPUT_HPAD, TEXT_INPUT_VPAD};
use render_wgpu::{
    lerp_color, Skin, KEYBOARD_KEY_FONT_SIZE, KEYBOARD_KEY_GAP, KEYBOARD_KEY_RADIUS,
    KEYBOARD_ROW_GAP, KEYBOARD_SIDE_MARGIN, KEYBOARD_VERT_MARGIN, SLIDER_THUMB_SIZE,
    SLIDER_TRACK_HEIGHT, TEXT_INPUT_CARET_WIDTH, TOGGLE_THUMB_INSET,
};

// ---------------------------------------------------------------------------
// iOS palette (mirrors UIKit's system colors)
// ---------------------------------------------------------------------------

pub const IOS_BLUE: [f32; 4] = [0.0, 0x7a as f32 / 255.0, 1.0, 1.0]; // #007AFF
pub const IOS_GREEN: [f32; 4] = [
    0x34 as f32 / 255.0,
    0xC7 as f32 / 255.0,
    0x59 as f32 / 255.0,
    1.0,
]; // #34C759
const IOS_TRACK_OFF: [f32; 4] = [
    0x78 as f32 / 255.0,
    0x78 as f32 / 255.0,
    0x80 as f32 / 255.0,
    0.32,
]; // systemGray w/ alpha
const IOS_THUMB: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
const IOS_THUMB_SHADOW: [f32; 4] = [0.0, 0.0, 0.0, 0.12];
const IOS_TEXT_BORDER: [f32; 4] = [
    0xC7 as f32 / 255.0,
    0xC7 as f32 / 255.0,
    0xCC as f32 / 255.0,
    1.0,
]; // systemGray3
const IOS_TEXT_INPUT_BG: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
const IOS_PLACEHOLDER: [f32; 4] = [60.0 / 255.0, 60.0 / 255.0, 67.0 / 255.0, 0.6];
/// Default spinner tint — `UIColor.systemGray`.
const IOS_SPINNER: [f32; 4] = [
    0x8E as f32 / 255.0,
    0x8E as f32 / 255.0,
    0x93 as f32 / 255.0,
    1.0,
];

// Keyboard palette.
const IOS_KB_BG: [f32; 4] = [
    0xD1 as f32 / 255.0,
    0xD4 as f32 / 255.0,
    0xDB as f32 / 255.0,
    1.0,
];
const IOS_KEY_BG: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
const IOS_KEY_BG_DARK: [f32; 4] = [
    0xAD as f32 / 255.0,
    0xB3 as f32 / 255.0,
    0xBE as f32 / 255.0,
    1.0,
];
const IOS_KEY_LABEL: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

// Spinner geometry.
const SPINNER_BARS: usize = 8;
const SPINNER_BAR_WIDTH_RATIO: f32 = 0.10;
const SPINNER_BAR_LENGTH_RATIO: f32 = 0.28;
const SPINNER_ORBIT_RATIO: f32 = 0.35;
const SPINNER_MIN_BAR_ALPHA: f32 = 0.15;

/// The iOS / UIKit skin. Stateless — instantiate once, wrap in
/// `Rc<dyn Skin>`, hand to the host.
pub struct IosSim;

impl IosSim {
    pub fn new() -> Self {
        Self
    }
}

impl Default for IosSim {
    fn default() -> Self {
        Self::new()
    }
}

impl Skin for IosSim {
    // -----------------------------------------------------------
    // Toggle — UISwitch
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
        let on_color = tint.unwrap_or(IOS_GREEN);
        let track_color = lerp_color(IOS_TRACK_OFF, on_color, t);

        rects.push(rect_inst(
            x,
            y,
            w,
            h,
            track_color,
            [track_radius; 4],
            [0.0; 4],
            0.0,
        ));

        let diameter = h - TOGGLE_THUMB_INSET * 2.0;
        let thumb_x_off = x + TOGGLE_THUMB_INSET;
        let thumb_x_on = x + w - TOGGLE_THUMB_INSET - diameter;
        let thumb_x = thumb_x_off + (thumb_x_on - thumb_x_off) * t;
        let thumb_y = y + TOGGLE_THUMB_INSET;

        rects.push(rect_inst(
            thumb_x - 0.5,
            thumb_y + 1.0,
            diameter + 1.0,
            diameter,
            IOS_THUMB_SHADOW,
            [diameter * 0.5; 4],
            [0.0; 4],
            0.0,
        ));
        rects.push(rect_inst(
            thumb_x,
            thumb_y,
            diameter,
            diameter,
            IOS_THUMB,
            [diameter * 0.5; 4],
            [0.0; 4],
            0.0,
        ));
    }

    // -----------------------------------------------------------
    // Slider — UISlider
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
        let fill_color = tint.unwrap_or(IOS_BLUE);
        let thumb_size = SLIDER_THUMB_SIZE;
        let track_h = SLIDER_TRACK_HEIGHT;
        let inset = thumb_size * 0.5;
        let track_x = x + inset;
        let track_y = y + (h - track_h) * 0.5;
        let track_w = (w - inset * 2.0).max(0.0);

        let t = if max > min {
            ((value - min) / (max - min)).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let fill_w = track_w * t;

        rects.push(rect_inst(
            track_x,
            track_y,
            track_w,
            track_h,
            IOS_TRACK_OFF,
            [track_h * 0.5; 4],
            [0.0; 4],
            0.0,
        ));
        if fill_w > 0.0 {
            rects.push(rect_inst(
                track_x,
                track_y,
                fill_w,
                track_h,
                fill_color,
                [track_h * 0.5; 4],
                [0.0; 4],
                0.0,
            ));
        }

        let thumb_x = track_x + fill_w - thumb_size * 0.5;
        let thumb_y = y + (h - thumb_size) * 0.5;

        rects.push(rect_inst(
            thumb_x - 0.5,
            thumb_y + 1.5,
            thumb_size + 1.0,
            thumb_size,
            IOS_THUMB_SHADOW,
            [thumb_size * 0.5; 4],
            [0.0; 4],
            0.0,
        ));
        rects.push(rect_inst(
            thumb_x,
            thumb_y,
            thumb_size,
            thumb_size,
            IOS_THUMB,
            [thumb_size * 0.5; 4],
            [0.0; 4],
            0.0,
        ));
    }

    // -----------------------------------------------------------
    // TextInput — UITextField
    // -----------------------------------------------------------

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
    ) {
        let border = if is_focused { IOS_BLUE } else { IOS_TEXT_BORDER };
        rects.push(rect_inst(
            x,
            y,
            w,
            h,
            field_bg.unwrap_or(IOS_TEXT_INPUT_BG),
            [8.0; 4],
            border,
            1.0,
        ));

        let glyph_color = if is_placeholder { IOS_PLACEHOLDER } else { text_color };
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

        if is_focused && draw_caret && !is_placeholder {
            let caret_x = text_x + caret_x_local;
            let caret_y = y + TEXT_INPUT_VPAD;
            let caret_h = h - TEXT_INPUT_VPAD * 2.0;
            rects.push(rect_inst(
                caret_x,
                caret_y,
                TEXT_INPUT_CARET_WIDTH,
                caret_h,
                IOS_BLUE,
                [0.0; 4],
                [0.0; 4],
                0.0,
            ));
        }
    }

    // -----------------------------------------------------------
    // ActivityIndicator — UIActivityIndicatorView
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
        let base_color = tint.unwrap_or(IOS_SPINNER);
        let cx = x + w * 0.5;
        let cy = y + h * 0.5;
        let bar_w = diameter * SPINNER_BAR_WIDTH_RATIO;
        let bar_h = diameter * SPINNER_BAR_LENGTH_RATIO;
        let orbit_r = diameter * SPINNER_ORBIT_RATIO;
        let n = SPINNER_BARS as f32;

        for i in 0..SPINNER_BARS {
            // `slot=0` is the head (brightest). Each subsequent slot
            // is one step counter-rotated, with `phase` shifting the
            // whole ring around once per spin period.
            let slot = i as f32;
            let slot_angle = (phase - slot / n) * std::f32::consts::TAU;
            // Screen y is down; rotate so `slot_angle = 0` puts the
            // leading bar at 12 o'clock (top). Position the bar's
            // *center* on the orbit; the bar's own rotation will
            // align its long axis with the radial direction.
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
    // On-screen keyboard
    // -----------------------------------------------------------

    fn keyboard_rows(&self) -> Vec<Vec<KeySpec>> {
        // iOS QWERTY portrait — three letter rows + bottom row
        // with space + return. Numbers/symbols/shift are out of
        // scope for the MVP.
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
            key_gap: KEYBOARD_KEY_GAP,
            row_gap: KEYBOARD_ROW_GAP,
            side_margin: KEYBOARD_SIDE_MARGIN,
            vert_margin: KEYBOARD_VERT_MARGIN,
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

        // Background panel.
        rects.push(rect_inst(
            kb_x,
            kb_y,
            kb_w,
            kb_h,
            IOS_KB_BG,
            [0.0; 4],
            [0.0; 4],
            0.0,
        ));

        for k in laid_keys {
            let is_special = !matches!(k.action, KeyAction::Character(_));
            let bg = if is_special { IOS_KEY_BG_DARK } else { IOS_KEY_BG };
            rects.push(rect_inst(
                k.x,
                k.y,
                k.w,
                k.h,
                bg,
                [KEYBOARD_KEY_RADIUS; 4],
                [0.0; 4],
                0.0,
            ));

            if let Some(buf) = glyphs.get(k.label) {
                // Glyphon doesn't expose a measured-width API;
                // the first layout run's `line_w` is close enough
                // for centered labels.
                let label_w = buf.layout_runs().next().map(|r| r.line_w).unwrap_or(0.0);
                let label_h = KEYBOARD_KEY_FONT_SIZE * 1.2;
                let tx = k.x + (k.w - label_w) * 0.5;
                let ty = k.y + (k.h - label_h) * 0.5;
                texts.push(StagedText {
                    buffer: buf,
                    x: tx,
                    y: ty,
                    color: IOS_KEY_LABEL,
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
