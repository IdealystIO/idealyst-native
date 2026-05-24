//! iOS / UIKit skin for the wgpu preview renderer.
//!
//! `IosSim` is a unit struct: there's no per-instance state.
//! Construct it once at host startup, hand it as
//! `Rc<dyn Painter>` to the renderer, and every per-frame widget +
//! keyboard paint call routes through the trait impl below.
//!
//! All paint methods append into the renderer's shared
//! `RectInstance` / `StagedText` Vecs. sRGB→linear color
//! conversion lives in [`render_wgpu::widgets::rect_inst`]; this
//! crate only declares the iOS palette in sRGB.

mod chrome_icons;
mod device;

pub use device::{BezelStyle, DeviceConfig, DeviceModel, NotchStyle, StatusBarStyle};

use std::collections::HashMap;

use glyphon::{Buffer, TextBounds};

use render_wgpu::keyboard::{KeyAction, KeySpec, LaidKey, LayoutMetrics};
use render_wgpu::pipeline::Instance as RectInstance;
use render_wgpu::text::StagedText;
use render_wgpu::widgets::{rect_inst, rect_inst_rotated, TEXT_INPUT_HPAD, TEXT_INPUT_VPAD};
use render_wgpu::{
    lerp_color, paint_icon, NavigatorHeaderAction, NavigatorHeaderChrome, NavigatorHeaderHit,
    Painter, KEYBOARD_KEY_FONT_SIZE, KEYBOARD_KEY_GAP, KEYBOARD_KEY_RADIUS, KEYBOARD_ROW_GAP,
    KEYBOARD_SIDE_MARGIN, KEYBOARD_VERT_MARGIN, SLIDER_THUMB_SIZE, SLIDER_TRACK_HEIGHT,
    TEXT_INPUT_CARET_WIDTH, TOGGLE_THUMB_INSET,
};

// ---------------------------------------------------------------------------
// iOS navigator header constants
// ---------------------------------------------------------------------------

/// iOS navigation bar background — opaque white with a hint of
/// system gray. Real UIKit uses a translucent material here;
/// the simulator paints an opaque approximation that reads
/// correctly on both light + dark scrollable content.
const IOS_HEADER_BG: [f32; 4] = [
    0xF9 as f32 / 255.0,
    0xF9 as f32 / 255.0,
    0xF9 as f32 / 255.0,
    1.0,
];
/// Hairline separator at the bottom of the header. UIKit's
/// default is `UIColor.separator` (`#3C3C43` @ 36% alpha).
const IOS_HEADER_SEPARATOR: [f32; 4] = [
    0x3C as f32 / 255.0,
    0x3C as f32 / 255.0,
    0x43 as f32 / 255.0,
    0.36,
];
/// Title text color — `UIColor.label` (near-black in light mode).
const IOS_HEADER_TITLE: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
/// Slot square edge — back chevron + header_left/right icons
/// render inside this many logical px in each header corner.
const IOS_HEADER_SLOT_SIZE: f32 = 28.0;
/// Inset from the header's left/right edge to the slot square.
const IOS_HEADER_SLOT_INSET: f32 = 8.0;
/// Title font size — matches iOS' inline title style. Kept
/// here for reference; the actual buffer is shaped in the
/// renderer's `attach_screen_metadata`, which picks a
/// font_size shared across skins.
#[allow(dead_code)]
const IOS_HEADER_TITLE_SIZE: f32 = 17.0;

// ---------------------------------------------------------------------------
// iOS device chrome (status bar + home indicator)
// ---------------------------------------------------------------------------

/// Logical-px height of the iOS status bar (notch-era phones).
/// `safeAreaInsets.top` on a 390x844 iPhone 13 = 47pt.
pub const IOS_STATUS_BAR_HEIGHT: f32 = 47.0;
/// Logical-px height of the home-indicator strip at the bottom
/// of every Face-ID iPhone. `safeAreaInsets.bottom` = 34pt.
pub const IOS_HOME_INDICATOR_HEIGHT: f32 = 34.0;
/// Width of the home-indicator pill, in logical px. iOS draws a
/// 134pt wide black pill on the home strip; we use a slightly
/// shorter neutral fill so light/dark both look reasonable.
const IOS_HOME_PILL_WIDTH: f32 = 134.0;
/// Height of the pill itself (the indicator strip is taller; the
/// pill sits in the middle).
const IOS_HOME_PILL_HEIGHT: f32 = 5.0;
/// Pill color — full-black at high opacity reads correctly over
/// both light and dark screen content underneath. Real iOS
/// modulates this against the screen content; the simulator
/// keeps a constant value.
const IOS_HOME_PILL_COLOR: [f32; 4] = [0.0, 0.0, 0.0, 0.85];
/// Status-bar foreground (clock + status glyphs). `UIColor.label`
/// — near-black in light mode. Real iOS picks dark or light
/// based on the screen's chrome contrast; the simulator keeps
/// it dark always for light-app readability.
const IOS_STATUS_FG: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
/// Status-bar font size. iOS uses 17pt for the clock.
const IOS_STATUS_FONT_SIZE: f32 = 15.0;
/// Inset from the bar's edges to the clock / status-icon group.
const IOS_STATUS_INSET: f32 = 24.0;

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

/// The iOS / UIKit skin. Holds a [`DeviceConfig`] that
/// controls the simulator's device-level chrome — notch /
/// dynamic island, corner radius, bezel frame, status-bar
/// foreground style. Defaults to [`DeviceModel::IPhone15Pro`];
/// pick a different preset with [`Self::with_device`] or tune
/// individual knobs with the `with_*` builders.
pub struct IosSim {
    config: DeviceConfig,
}

impl IosSim {
    /// New iOS skin with default chrome (iPhone 15 Pro).
    pub fn new() -> Self {
        Self { config: DeviceConfig::default_config() }
    }

    /// Replace the entire chrome config with the preset for a
    /// given device. Chain with `.with_notch(...)` etc. to
    /// override individual knobs.
    pub fn with_device(mut self, model: DeviceModel) -> Self {
        self.config = DeviceConfig::for_model(model);
        self
    }

    pub fn with_notch(mut self, notch: NotchStyle) -> Self {
        self.config.notch = notch;
        self
    }

    pub fn with_corner_radius(mut self, radius: f32) -> Self {
        self.config.corner_radius = radius;
        self
    }

    pub fn with_bezel(mut self, bezel: BezelStyle) -> Self {
        self.config.bezel = bezel;
        self
    }

    pub fn with_status_bar_style(mut self, style: StatusBarStyle) -> Self {
        self.config.status_bar_style = style;
        self
    }

    pub fn device_config(&self) -> DeviceConfig {
        self.config
    }
}

impl Default for IosSim {
    fn default() -> Self {
        Self::new()
    }
}

/// Default `StyleRules` for an unstyled `button(...)` on iOS.
/// Mirrors UIKit's plain `UIButton` look: tinted system-blue text
/// on a transparent background, 17 pt body font, 12×16 padding,
/// 10 pt corner radius (matches `UIButton.Configuration.plain()`).
/// Authors can override any field via `.with_style(...)`.
fn ios_button_defaults() -> runtime_core::StyleRules {
    use runtime_core::{Color, Length, StyleRules, Tokenized};
    StyleRules {
        background: Some(Tokenized::Literal(Color("#00000000".into()))),
        color: Some(Tokenized::Literal(Color("#007AFF".into()))),
        font_size: Some(Tokenized::Literal(Length::Px(17.0))),
        padding_top: Some(Tokenized::Literal(Length::Px(12.0))),
        padding_right: Some(Tokenized::Literal(Length::Px(16.0))),
        padding_bottom: Some(Tokenized::Literal(Length::Px(12.0))),
        padding_left: Some(Tokenized::Literal(Length::Px(16.0))),
        border_top_left_radius: Some(Tokenized::Literal(Length::Px(10.0))),
        border_top_right_radius: Some(Tokenized::Literal(Length::Px(10.0))),
        border_bottom_left_radius: Some(Tokenized::Literal(Length::Px(10.0))),
        border_bottom_right_radius: Some(Tokenized::Literal(Length::Px(10.0))),
        ..Default::default()
    }
}

impl Painter for IosSim {
    fn platform(&self) -> runtime_core::Platform {
        runtime_core::Platform::Custom("Sim")
    }

    fn button_defaults(&self) -> runtime_core::StyleRules {
        ios_button_defaults()
    }

    /// iOS press feedback for an unstyled `UIButton.plain()`:
    /// the label's alpha dims to ~50%. No background overlay —
    /// the resting background is transparent anyway, and UIKit
    /// doesn't paint a state layer on top.
    fn button_press_visual(&self, t: f32) -> render_wgpu::ButtonPressVisual {
        let t = t.clamp(0.0, 1.0);
        render_wgpu::ButtonPressVisual {
            text_alpha_factor: 1.0 - 0.5 * t,
            bg_overlay: None,
        }
    }

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

        // Real shader-driven drop shadow (soft falloff) under the
        // thumb. Two stacked instances per UIKit's UISlider:
        //   - A wider, softer ambient shadow that lifts the thumb
        //     off light backgrounds.
        //   - A tighter, slightly-darker key shadow that gives a
        //     "weight" cue at the contact edge.
        // Both share the thumb's corner radius so the halo hugs
        // the circle.
        rects.push(render_wgpu::widgets::shadow_inst(
            thumb_x,
            thumb_y,
            thumb_size,
            thumb_size,
            0.0,    // offset x
            2.0,    // offset y
            8.0,    // blur
            [0.0, 0.0, 0.0, 0.18],
            [thumb_size * 0.5; 4],
        ));
        rects.push(render_wgpu::widgets::shadow_inst(
            thumb_x,
            thumb_y,
            thumb_size,
            thumb_size,
            0.0,
            1.0,
            3.0,
            [0.0, 0.0, 0.0, 0.22],
            [thumb_size * 0.5; 4],
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
        pressed_label: Option<&'static str>,
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
            let is_pressed = pressed_label == Some(k.label);
            // iOS press feedback: letter keys flash to the dark
            // modifier-key shade; modifier keys flash to white.
            let bg = match (is_special, is_pressed) {
                (false, true) => IOS_KEY_BG_DARK,
                (true, true) => IOS_KEY_BG,
                (false, false) => IOS_KEY_BG,
                (true, false) => IOS_KEY_BG_DARK,
            };
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

    fn paint_navigator_header<'a, 'b>(
        &self,
        rect: (f32, f32, f32, f32),
        chrome: NavigatorHeaderChrome<'a, 'b>,
        rects: &mut Vec<RectInstance>,
        texts: &mut Vec<StagedText<'a>>,
        hit_regions: &mut Vec<NavigatorHeaderHit>,
    ) {
        let (x, y, w, h) = rect;

        // Bar fill — author override beats system default.
        let bg = chrome.background.unwrap_or(IOS_HEADER_BG);
        // Extend the bg upward by `safe_area_top` so the
        // status-bar strip shares the header's color (and
        // therefore slides with the screen during a transition
        // instead of leaking the clear color through). Title +
        // icon positions still use the original `rect`, not
        // the extended one.
        let bg_y = y - chrome.safe_area_top;
        let bg_h = h + chrome.safe_area_top;
        rects.push(rect_inst(x, bg_y, w, bg_h, bg, [0.0; 4], [0.0; 4], 0.0));
        // Hairline separator at the bottom edge. UIKit calls
        // this the "shadow image"; one logical pixel.
        rects.push(rect_inst(
            x,
            y + h - 1.0,
            w,
            1.0,
            IOS_HEADER_SEPARATOR,
            [0.0; 4],
            [0.0; 4],
            0.0,
        ));

        let tint = chrome.tint.unwrap_or(IOS_BLUE);

        // Left slot — back chevron or author's header_left icon.
        let slot_y = y + (h - IOS_HEADER_SLOT_SIZE) * 0.5;
        if let Some(name) = chrome.header_left_icon {
            if let Some(icon) = chrome_icons::lookup(name) {
                let slot_x = x + IOS_HEADER_SLOT_INSET;
                paint_icon(
                    slot_x,
                    slot_y,
                    IOS_HEADER_SLOT_SIZE,
                    IOS_HEADER_SLOT_SIZE,
                    icon.paths,
                    icon.view_box,
                    tint,
                    1.0,
                    rects,
                );
                hit_regions.push(NavigatorHeaderHit {
                    rect: (slot_x, y, IOS_HEADER_SLOT_SIZE + IOS_HEADER_SLOT_INSET, h),
                    action: NavigatorHeaderAction::HeaderLeft,
                });
            }
        } else if chrome.show_back {
            let icon = chrome_icons::BACK_CHEVRON;
            let slot_x = x + IOS_HEADER_SLOT_INSET;
            paint_icon(
                slot_x,
                slot_y,
                IOS_HEADER_SLOT_SIZE,
                IOS_HEADER_SLOT_SIZE,
                icon.paths,
                icon.view_box,
                tint,
                1.0,
                rects,
            );
            // Extend the hit target leftward to the bar edge so
            // a glancing tap on the chevron's edge still pops
            // — same forgiving target UIKit ships.
            hit_regions.push(NavigatorHeaderHit {
                rect: (x, y, IOS_HEADER_SLOT_SIZE + IOS_HEADER_SLOT_INSET * 2.0, h),
                action: NavigatorHeaderAction::Back,
            });
        }

        // Right slot.
        if let Some(name) = chrome.header_right_icon {
            if let Some(icon) = chrome_icons::lookup(name) {
                let slot_x = x + w - IOS_HEADER_SLOT_SIZE - IOS_HEADER_SLOT_INSET;
                paint_icon(
                    slot_x,
                    slot_y,
                    IOS_HEADER_SLOT_SIZE,
                    IOS_HEADER_SLOT_SIZE,
                    icon.paths,
                    icon.view_box,
                    tint,
                    1.0,
                    rects,
                );
                hit_regions.push(NavigatorHeaderHit {
                    rect: (slot_x, y, IOS_HEADER_SLOT_SIZE + IOS_HEADER_SLOT_INSET, h),
                    action: NavigatorHeaderAction::HeaderRight,
                });
            }
        }

        // Title — centered horizontally, vertically.
        if let Some(buffer) = chrome.title {
            // Measure the glyph extent to recenter — glyphon's
            // own layout already wrapped to the buffer's max
            // width, so we read the first run's bounds.
            let (text_w, text_h) = measure_buffer(buffer);
            let tx = x + (w - text_w) * 0.5;
            let ty = y + (h - text_h) * 0.5;
            texts.push(StagedText {
                buffer,
                x: tx,
                y: ty,
                color: chrome.title_color.unwrap_or(IOS_HEADER_TITLE),
                clip: TextBounds {
                    left: x as i32,
                    top: y as i32,
                    right: (x + w) as i32,
                    bottom: (y + h) as i32,
                },
            });
        }
    }

    fn safe_area_insets(&self) -> runtime_core::EdgeInsets {
        runtime_core::EdgeInsets {
            top: IOS_STATUS_BAR_HEIGHT,
            right: 0.0,
            bottom: IOS_HOME_INDICATOR_HEIGHT,
            left: 0.0,
        }
    }

    fn device_corner_radius(&self) -> f32 {
        self.config.corner_radius
    }

    fn chrome_glyph_labels(&self) -> Vec<(&'static str, String, f32)> {
        // The host re-shapes "clock" on every minute boundary;
        // the initial string is irrelevant since the host
        // synthesizes the real one before first render.
        vec![("clock", String::new(), IOS_STATUS_FONT_SIZE)]
    }

    fn paint_device_chrome<'a>(
        &self,
        viewport: (f32, f32),
        insets: runtime_core::EdgeInsets,
        _now: web_time::Instant,
        glyphs: &'a HashMap<&'static str, Buffer>,
        rects: &mut Vec<RectInstance>,
        texts: &mut Vec<StagedText<'a>>,
    ) {
        let (vw, vh) = viewport;
        let cfg = self.config;
        // Status-bar foreground color follows the configured
        // light/dark style. Dark = near-black on light surfaces;
        // light = white on dark surfaces / over a notch.
        let fg = match cfg.status_bar_style {
            StatusBarStyle::Dark => IOS_STATUS_FG,
            StatusBarStyle::Light => [1.0, 1.0, 1.0, 1.0],
        };
        let bar_h = insets.top.max(IOS_STATUS_BAR_HEIGHT);

        if let Some(clock) = glyphs.get("clock") {
            let (_tw, th) = measure_buffer(clock);
            // Push the clock slightly down so it visually sits
            // below a tall notch on devices that have one.
            let tx = IOS_STATUS_INSET;
            let ty = (bar_h - th) * 0.5 + 4.0;
            texts.push(StagedText {
                buffer: clock,
                x: tx,
                y: ty,
                color: fg,
                clip: TextBounds {
                    left: 0,
                    top: 0,
                    right: (vw * 0.5) as i32,
                    bottom: bar_h as i32,
                },
            });
        }

        // Right-side status glyphs (signal / wifi / battery).
        let icon_y = (bar_h - 10.0) * 0.5 + 2.0;
        let right_anchor = vw - IOS_STATUS_INSET;
        let battery_w = 22.0;
        let battery_h = 10.0;
        let battery_x = right_anchor - battery_w;
        rects.push(rect_inst(
            battery_x, icon_y, battery_w, battery_h,
            [0.0; 4], [2.0; 4], fg, 1.0,
        ));
        rects.push(rect_inst(
            battery_x + battery_w, icon_y + 3.0, 2.0, battery_h - 6.0,
            fg, [1.0; 4], [0.0; 4], 0.0,
        ));
        let fill_inset = 1.5;
        let fill_w = (battery_w - fill_inset * 2.0) * 0.95;
        rects.push(rect_inst(
            battery_x + fill_inset, icon_y + fill_inset,
            fill_w, battery_h - fill_inset * 2.0,
            fg, [1.5; 4], [0.0; 4], 0.0,
        ));
        let wifi_size = 12.0;
        let wifi_x = battery_x - 6.0 - wifi_size;
        let wifi_y = (bar_h - wifi_size) * 0.5 + 4.0;
        rects.push(rect_inst(
            wifi_x, wifi_y, wifi_size, wifi_size,
            fg, [2.0; 4], [0.0; 4], 0.0,
        ));
        let bar_gap = 2.0;
        let bar_unit_w = 3.0;
        let sig_total_w = bar_unit_w * 4.0 + bar_gap * 3.0;
        let sig_anchor_x = wifi_x - 6.0 - sig_total_w;
        for i in 0..4 {
            let bx = sig_anchor_x + (bar_unit_w + bar_gap) * i as f32;
            let bh = 3.0 + i as f32 * 2.0;
            let by = icon_y + (10.0 - bh);
            rects.push(rect_inst(
                bx, by, bar_unit_w, bh,
                fg, [1.0; 4], [0.0; 4], 0.0,
            ));
        }

        // --- Notch / Dynamic Island ---
        // Always black, regardless of status bar style — the
        // cutout is "outside the screen" in physical reality.
        match cfg.notch {
            NotchStyle::None => {}
            NotchStyle::Notch { width, height, radius } => {
                let nx = (vw - width) * 0.5;
                let ny = 0.0;
                rects.push(rect_inst(
                    nx, ny, width, height,
                    [0.0, 0.0, 0.0, 1.0],
                    // Round only the bottom corners — the notch
                    // hangs from the top edge so the top corners
                    // are flush with the bezel.
                    [0.0, 0.0, radius, radius],
                    [0.0; 4],
                    0.0,
                ));
            }
            NotchStyle::DynamicIsland { width, height, top_offset } => {
                let ix = (vw - width) * 0.5;
                let iy = top_offset;
                rects.push(rect_inst(
                    ix, iy, width, height,
                    [0.0, 0.0, 0.0, 1.0],
                    [height * 0.5; 4],
                    [0.0; 4],
                    0.0,
                ));
            }
        }

        // Corner masking + the rounded device silhouette are
        // handled by the renderer's `device_frame` pipeline,
        // which paints opaque black outside the rounded path
        // via an inverse-SDF fullscreen draw. The skin no
        // longer needs corner-mask rects or a bezel-border
        // rect for this — `device_corner_radius()` is the only
        // surface area between skin and renderer. The
        // `BezelStyle` field stays on the config for future
        // use (e.g., painting a visible material color *inside*
        // the rounded path before the app draws), but is not
        // wired here.
        let _ = cfg.bezel;

        // --- Home indicator pill ---
        // Hidden when the device has no home indicator (SE
        // family). Painted in the configured fg color, but
        // weighted toward black since the home indicator on
        // real iPhones is always black-ish regardless of theme.
        if insets.bottom > 0.0 {
            let pill_x = (vw - IOS_HOME_PILL_WIDTH) * 0.5;
            let pill_y =
                vh - insets.bottom * 0.5 - IOS_HOME_PILL_HEIGHT * 0.5;
            let pill_color = match cfg.status_bar_style {
                StatusBarStyle::Dark => IOS_HOME_PILL_COLOR,
                StatusBarStyle::Light => [1.0, 1.0, 1.0, 0.85],
            };
            rects.push(rect_inst(
                pill_x, pill_y, IOS_HOME_PILL_WIDTH, IOS_HOME_PILL_HEIGHT,
                pill_color,
                [IOS_HOME_PILL_HEIGHT * 0.5; 4],
                [0.0; 4],
                0.0,
            ));
        }
    }
}

/// Measure a pre-laid glyphon buffer's tight bounding box. Used
/// to center the navigator header's title text — the renderer
/// already laid out the buffer at its natural wrap width, so we
/// read the lines' actual run extents instead of asking glyphon
/// to relayout. Returns `(width, height)` in logical px.
fn measure_buffer(buffer: &Buffer) -> (f32, f32) {
    let mut max_w: f32 = 0.0;
    let mut total_h: f32 = 0.0;
    for run in buffer.layout_runs() {
        max_w = max_w.max(run.line_w);
        total_h += run.line_height;
    }
    (max_w, total_h)
}
