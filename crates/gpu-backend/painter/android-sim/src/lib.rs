//! Material 3 (Android) skin for the wgpu preview renderer.
//!
//! `AndroidSim` is a unit struct: there's no per-instance
//! state. Construct it once at host startup, hand it as
//! `Rc<dyn Painter>` to the renderer, and every per-frame widget +
//! keyboard paint call routes through the trait impl below.
//!
//! Targets M3 baseline-light: primary `#6750A4`, surface-variant
//! `#E7E0EC`, outline `#79747E`. Ripple, elevation tones, and
//! emphasized motion curves are out of scope.

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
    paint_icon, NavigatorHeaderAction, NavigatorHeaderChrome, NavigatorHeaderHit, Painter,
    KEYBOARD_KEY_FONT_SIZE, SLIDER_THUMB_SIZE, SLIDER_TRACK_HEIGHT, TEXT_INPUT_CARET_WIDTH,
    TOGGLE_THUMB_INSET,
};

// ---------------------------------------------------------------------------
// Material 3 device chrome (status bar + gesture nav line)
// ---------------------------------------------------------------------------

/// Logical-px height of the M3 status bar. Android stock is
/// 24dp; some OEMs go up to 28dp for cutout phones — the
/// simulator picks 28 so a deep-notch tester catches issues.
pub const M3_STATUS_BAR_HEIGHT: f32 = 28.0;
/// Logical-px height of the gesture-nav bar at the bottom of
/// every Android 10+ device using gesture navigation. Stock is
/// 24dp; this matches.
pub const M3_GESTURE_NAV_HEIGHT: f32 = 24.0;
/// Status-bar foreground (clock + status glyphs). On-surface
/// — near-black in M3 baseline-light.
const M3_STATUS_FG: [f32; 4] = [
    0x1C as f32 / 255.0,
    0x1B as f32 / 255.0,
    0x1F as f32 / 255.0,
    1.0,
];
/// Status-bar font size — M3 spec is ~14sp.
const M3_STATUS_FONT_SIZE: f32 = 14.0;
/// Edge inset from the bar's outer borders to the clock /
/// status-icon group. M3 spec is 16dp, but our simulated
/// viewport's edge sits directly at the bezel (real phones get
/// some breathing room from the bezel-to-screen seal that we
/// don't model), so the M3 value reads as visually cramped.
/// Bumped to match the iOS skin's 24dp for cross-simulator
/// consistency.
const M3_STATUS_INSET: f32 = 24.0;

// ---------------------------------------------------------------------------
// Material 3 top app bar (center-aligned variant)
// ---------------------------------------------------------------------------

/// Top app bar surface — M3 baseline-light uses surface
/// container at elevation 0. Keeping this distinct from the
/// generic `M3_SURFACE` so the skin can later tint it on scroll
/// (M3's "elevation overlay" trick).
const M3_HEADER_BG: [f32; 4] = M3_SURFACE;
/// Headline color on the bar — `on-surface`.
const M3_HEADER_TITLE: [f32; 4] = [
    0x1C as f32 / 255.0,
    0x1B as f32 / 255.0,
    0x1F as f32 / 255.0,
    1.0,
];
/// Icon tint — same `on-surface` as the title.
const M3_HEADER_TINT: [f32; 4] = [
    0x1C as f32 / 255.0,
    0x1B as f32 / 255.0,
    0x1F as f32 / 255.0,
    1.0,
];
/// Icon slot edge (M3 "icon button" is 40dp circle around a
/// 24dp icon; the renderer paints the icon glyph only — the
/// ripple background isn't a M3-faithful fixture in V1).
const M3_HEADER_SLOT_SIZE: f32 = 24.0;
/// Outer inset from the bar's edges to the slot square.
const M3_HEADER_SLOT_INSET: f32 = 12.0;

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
/// OFF-state track outline width. M3's spec is 1 dp; 2 dp
/// read as a heavy ring around the (small) thumb instead of a
/// subtle container edge.
const M3_SWITCH_TRACK_BORDER: f32 = 1.0;
/// M3 OFF-state thumb inset — half the gap between the (small)
/// thumb and the track's interior. Computed so the thumb sits
/// visually centered in the track instead of pinned to the left
/// edge. `(track_h - thumb_diam_off) / 2` for the 31×51 toggle:
/// (31 − 16) / 2 ≈ 7.5 → rounded to a whole pixel.
const M3_SWITCH_THUMB_INSET_OFF: f32 = 8.0;
/// ON-state thumb inset. The thumb is larger so we get closer
/// to the track edge by design — `(31 − 24) / 2 ≈ 3.5` →
/// half a pixel under the track's 2dp border feels too snug,
/// so we land on 4 for a 2dp visual gap.
const M3_SWITCH_THUMB_INSET_ON: f32 = 4.0;

// Slider sizing.
/// M3 expressive slider — matches the stock Android sliders
/// (Pixel / Moto Edge volume controls).
/// - Both active *and* inactive tracks are 16 dp chunky pills;
///   the inactive uses a muted surface-variant color rather
///   than the primary fill. Earlier rev painted the inactive
///   as a thin 4 dp line which made the thumb visually float
///   between two unrelated shapes.
/// - 4×22 dp vertical pill thumb at the value point, with a
///   small 2 dp gap on each side from the surrounding fills.
/// - A 4 dp white stop-indicator dot pinned to the far-right
///   of the inactive region per the M3 spec.
const M3_SLIDER_TRACK_HEIGHT: f32 = 16.0;
const M3_SLIDER_THUMB_WIDTH: f32 = 4.0;
const M3_SLIDER_THUMB_HEIGHT: f32 = 22.0;
const M3_SLIDER_THUMB_RADIUS: f32 = 2.0;
const M3_SLIDER_THUMB_GAP: f32 = 2.0;
/// Diameter of the stop-indicator dot painted on the trailing
/// end of the inactive track. Matches what shows on stock
/// Android (4 dp white circle).
const M3_SLIDER_STOP_DOT_DIAM: f32 = 4.0;
const M3_SLIDER_STOP_DOT_INSET: f32 = 6.0;

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

// Material 3 indeterminate progress geometry. M3 uses a single
// arc segment that rotates and (in the full spec) breathes in
// and out. We approximate the visual with a tight cluster of
// capsules forming a constant ~120° arc that rotates around the
// center — distinct from iOS's even ring of fading dots.
const M3_ARC_BARS: usize = 8;
/// Total angular span covered by the arc, in radians (~120°).
const M3_ARC_SPAN: f32 = std::f32::consts::FRAC_PI_2 * 1.33;
const M3_ARC_BAR_WIDTH_RATIO: f32 = 0.10;
const M3_ARC_BAR_LENGTH_RATIO: f32 = 0.22;
const M3_ARC_ORBIT_RATIO: f32 = 0.38;
/// Alpha falloff at the trailing tail of the arc. The leading
/// bar paints at full opacity.
const M3_ARC_TAIL_MIN_ALPHA: f32 = 0.45;

/// The Material 3 / Android skin. Holds a [`DeviceConfig`] that
/// controls the simulator's device-level chrome — hole-punch /
/// teardrop notch, corner radius, bezel frame, status-bar
/// foreground style. Defaults to [`DeviceModel::Pixel8`]; pick a
/// different preset with [`Self::with_device`] or tune
/// individual knobs with the `with_*` builders.
pub struct AndroidSim {
    config: DeviceConfig,
}

impl AndroidSim {
    /// New Android skin with default chrome (Pixel 8).
    pub fn new() -> Self {
        Self { config: DeviceConfig::default_config() }
    }

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

impl Default for AndroidSim {
    fn default() -> Self {
        Self::new()
    }
}

/// Default `StyleRules` for an unstyled `button(...)` on Material 3.
/// Matches the filled-button variant (M3's default emphasis):
/// primary-color background, on-primary text, 14 sp medium-weight
/// label, 10×24 padding for a 40 dp pill, full-pill 20 dp corners.
/// Authors override individual fields via `.with_style(...)`; the
/// merge in `Backend::apply_style` keeps any field they set.
fn m3_button_defaults() -> runtime_core::StyleRules {
    use runtime_core::{Color, FontWeight, Length, StyleRules, Tokenized};
    StyleRules {
        // M3 Primary (#6750A4). Hard-coded here rather than
        // reading the active theme palette — skins are
        // stateless and the theme system's tokens layer on
        // top via author overrides.
        background: Some(Tokenized::Literal(Color("#6750A4".into()))),
        color: Some(Tokenized::Literal(Color("#FFFFFF".into()))),
        font_size: Some(Tokenized::Literal(Length::Px(14.0))),
        font_weight: Some(FontWeight::Medium),
        padding_top: Some(Tokenized::Literal(Length::Px(10.0))),
        padding_right: Some(Tokenized::Literal(Length::Px(24.0))),
        padding_bottom: Some(Tokenized::Literal(Length::Px(10.0))),
        padding_left: Some(Tokenized::Literal(Length::Px(24.0))),
        border_top_left_radius: Some(Tokenized::Literal(Length::Px(20.0))),
        border_top_right_radius: Some(Tokenized::Literal(Length::Px(20.0))),
        border_bottom_left_radius: Some(Tokenized::Literal(Length::Px(20.0))),
        border_bottom_right_radius: Some(Tokenized::Literal(Length::Px(20.0))),
        ..Default::default()
    }
}

impl Painter for AndroidSim {
    fn platform(&self) -> runtime_core::Platform {
        runtime_core::Platform::Custom("Sim")
    }

    fn button_defaults(&self) -> runtime_core::StyleRules {
        m3_button_defaults()
    }

    /// M3 press feedback: paint an 8% on-primary state-layer
    /// over the resting background. Text stays at full alpha —
    /// Material's filled-button spec keeps the label
    /// opaque under the state layer.
    fn button_press_visual(&self, t: f32) -> render_wgpu::ButtonPressVisual {
        let t = t.clamp(0.0, 1.0);
        render_wgpu::ButtonPressVisual {
            text_alpha_factor: 1.0,
            // White state layer at 8% per the M3 state-layer
            // spec for hovered/pressed on a primary container.
            // Scaled by `t` so the overlay fades in/out with
            // the press progress tween.
            bg_overlay: Some([1.0, 1.0, 1.0, 0.08 * t]),
        }
    }

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

        // Track is a plain filled pill in both states — no
        // outline. The earlier M3 spec drew a 1–2 dp outline
        // around the OFF-state container; current Material
        // implementations drop it in favor of a contained
        // surface-container-highest fill, which reads cleaner
        // against the surface and avoids a stripe artifact
        // around the small OFF thumb.
        let track_bg = if is_on { on_color } else { M3_SURFACE_VARIANT };
        let track_border: [f32; 4] = [0.0; 4];
        let track_border_w: f32 = 0.0;

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

        // Inset differs by state — the small OFF thumb needs
        // more breathing room from the track edge than the
        // large ON thumb, otherwise the OFF state looks pinned
        // to the left edge instead of resting in the track.
        // `TOGGLE_THUMB_INSET` is the iOS value (sized for the
        // 27pt iOS thumb) and is wrong for M3; we ignore it.
        let _ = TOGGLE_THUMB_INSET;
        let thumb_cx_off = x + M3_SWITCH_THUMB_INSET_OFF + M3_SWITCH_THUMB_DIAM_OFF * 0.5;
        let thumb_cx_on = x + w - M3_SWITCH_THUMB_INSET_ON - M3_SWITCH_THUMB_DIAM_ON * 0.5;
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

        // Both track segments are the same chunky 16 dp pill,
        // separated by a 2 dp gap on each side of the thumb.
        // Inactive uses surface-variant, active uses primary
        // (or the author's tint).
        let thumb_x = track_x + fill_w - thumb_w * 0.5;
        let thumb_left = thumb_x;
        let thumb_right = thumb_x + thumb_w;
        let track_y = center_y - M3_SLIDER_TRACK_HEIGHT * 0.5;

        // Per-corner radii: only the *outer* edges of each
        // segment are rounded. The edges facing the thumb are
        // squared off so the thumb reads as a clean separator
        // between two block fills, not as the meeting point
        // of two half-pills. corner_radius is [tl, tr, br, bl].
        let r = M3_SLIDER_TRACK_HEIGHT * 0.5;
        let active_end = (thumb_left - M3_SLIDER_THUMB_GAP).max(track_x);
        let active_w = (active_end - track_x).max(0.0);
        if active_w > 0.0 {
            rects.push(rect_inst(
                track_x,
                track_y,
                active_w,
                M3_SLIDER_TRACK_HEIGHT,
                active_color,
                [r, 0.0, 0.0, r],
                [0.0; 4],
                0.0,
            ));
        }

        let inactive_start =
            (thumb_right + M3_SLIDER_THUMB_GAP).min(track_x + track_w);
        let inactive_w = (track_x + track_w - inactive_start).max(0.0);
        if inactive_w > 0.0 {
            rects.push(rect_inst(
                inactive_start,
                track_y,
                inactive_w,
                M3_SLIDER_TRACK_HEIGHT,
                M3_SURFACE_VARIANT,
                [0.0, r, r, 0.0],
                [0.0; 4],
                0.0,
            ));
        }

        // Stop indicator: small white dot pinned to the
        // trailing end of the inactive track. Only paint
        // when there's enough inactive room left to fit
        // the dot (avoids painting on top of the thumb at
        // value≈max).
        if inactive_w > M3_SLIDER_STOP_DOT_INSET + M3_SLIDER_STOP_DOT_DIAM {
            let dot_cx =
                track_x + track_w - M3_SLIDER_STOP_DOT_INSET - M3_SLIDER_STOP_DOT_DIAM * 0.5;
            let dot_cy = center_y;
            rects.push(rect_inst(
                dot_cx - M3_SLIDER_STOP_DOT_DIAM * 0.5,
                dot_cy - M3_SLIDER_STOP_DOT_DIAM * 0.5,
                M3_SLIDER_STOP_DOT_DIAM,
                M3_SLIDER_STOP_DOT_DIAM,
                M3_ON_PRIMARY,
                [M3_SLIDER_STOP_DOT_DIAM * 0.5; 4],
                [0.0; 4],
                0.0,
            ));
        }

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
    // ActivityIndicator — M3 circular indeterminate progress.
    // A compact ~120° arc of capsules rotates around the center
    // once per spin period. Real M3 also breathes the arc length
    // in and out; that's a future polish — the constant-length
    // arc already reads distinctly different from the iOS even
    // ring of fading dots.
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
        let bar_w = diameter * M3_ARC_BAR_WIDTH_RATIO;
        let bar_h = diameter * M3_ARC_BAR_LENGTH_RATIO;
        let orbit_r = diameter * M3_ARC_ORBIT_RATIO;
        let n = M3_ARC_BARS as f32;

        // Leading edge of the arc walks around the circle as
        // `phase` advances. Each subsequent bar trails behind by
        // a fraction of the total arc span.
        let leading_angle = phase * std::f32::consts::TAU;
        for i in 0..M3_ARC_BARS {
            let slot = i as f32;
            // 0 at the head, 1 at the tail.
            let tail_t = slot / (n - 1.0).max(1.0);
            let bar_angle = leading_angle - tail_t * M3_ARC_SPAN;
            let dx = bar_angle.sin() * orbit_r;
            let dy = -bar_angle.cos() * orbit_r;
            // Linear alpha fade along the arc tail.
            let alpha_factor =
                M3_ARC_TAIL_MIN_ALPHA + (1.0 - M3_ARC_TAIL_MIN_ALPHA) * (1.0 - tail_t);
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
                bar_angle,
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
        pressed_label: Option<&'static str>,
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

        // M3 press feedback: every key (letter or modifier)
        // gets a translucent surface-variant chip behind it
        // when pressed — same rounded rect, slightly darker
        // than the panel background. Letter keys are otherwise
        // flat; modifier chips stay surface-variant.
        for k in laid_keys {
            let is_modifier = !matches!(k.action, KeyAction::Character(_));
            let is_pressed = pressed_label == Some(k.label);
            if is_modifier || is_pressed {
                let bg = if is_pressed { M3_OUTLINE } else { M3_KEY_BG_MODIFIER };
                rects.push(rect_inst(
                    k.x,
                    k.y,
                    k.w,
                    k.h,
                    bg,
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

    fn paint_navigator_header<'a, 'b>(
        &self,
        rect: (f32, f32, f32, f32),
        chrome: NavigatorHeaderChrome<'a, 'b>,
        rects: &mut Vec<RectInstance>,
        texts: &mut Vec<StagedText<'a>>,
        hit_regions: &mut Vec<NavigatorHeaderHit>,
    ) {
        let (x, y, w, h) = rect;

        // Top-app-bar surface fill.
        let bg = chrome.background.unwrap_or(M3_HEADER_BG);
        // Extend the bg upward by `safe_area_top` so the
        // status-bar strip shares the header color — see the
        // iOS skin for the rationale.
        let bg_y = y - chrome.safe_area_top;
        let bg_h = h + chrome.safe_area_top;
        rects.push(rect_inst(x, bg_y, w, bg_h, bg, [0.0; 4], [0.0; 4], 0.0));

        let tint = chrome.tint.unwrap_or(M3_HEADER_TINT);
        let slot_y = y + (h - M3_HEADER_SLOT_SIZE) * 0.5;

        // Track how far in from the left the title text needs to
        // start. M3's small top app bar puts the title left-aligned
        // after the leading icon slot — not centered like iOS.
        let mut title_left = x + M3_HEADER_SLOT_INSET;

        if let Some(name) = chrome.header_left_icon {
            if let Some(icon) = chrome_icons::lookup(name) {
                let slot_x = x + M3_HEADER_SLOT_INSET;
                paint_icon(
                    slot_x,
                    slot_y,
                    M3_HEADER_SLOT_SIZE,
                    M3_HEADER_SLOT_SIZE,
                    icon.paths,
                    icon.view_box,
                    tint,
                    1.0,
                    false,
                    rects,
                );
                hit_regions.push(NavigatorHeaderHit {
                    rect: (
                        slot_x - M3_HEADER_SLOT_INSET,
                        y,
                        M3_HEADER_SLOT_SIZE + M3_HEADER_SLOT_INSET * 2.0,
                        h,
                    ),
                    action: NavigatorHeaderAction::HeaderLeft,
                });
                title_left = slot_x + M3_HEADER_SLOT_SIZE + M3_HEADER_SLOT_INSET;
            }
        } else if chrome.show_back {
            let icon = chrome_icons::BACK_ARROW;
            let slot_x = x + M3_HEADER_SLOT_INSET;
            paint_icon(
                slot_x,
                slot_y,
                M3_HEADER_SLOT_SIZE,
                M3_HEADER_SLOT_SIZE,
                icon.paths,
                icon.view_box,
                tint,
                1.0,
                    false,
                rects,
            );
            hit_regions.push(NavigatorHeaderHit {
                rect: (
                    x,
                    y,
                    M3_HEADER_SLOT_SIZE + M3_HEADER_SLOT_INSET * 2.0,
                    h,
                ),
                action: NavigatorHeaderAction::Back,
            });
            title_left = slot_x + M3_HEADER_SLOT_SIZE + M3_HEADER_SLOT_INSET;
        }

        // Right slot — same shape as iOS.
        let mut title_right = x + w - M3_HEADER_SLOT_INSET;
        if let Some(name) = chrome.header_right_icon {
            if let Some(icon) = chrome_icons::lookup(name) {
                let slot_x = x + w - M3_HEADER_SLOT_SIZE - M3_HEADER_SLOT_INSET;
                paint_icon(
                    slot_x,
                    slot_y,
                    M3_HEADER_SLOT_SIZE,
                    M3_HEADER_SLOT_SIZE,
                    icon.paths,
                    icon.view_box,
                    tint,
                    1.0,
                    false,
                    rects,
                );
                hit_regions.push(NavigatorHeaderHit {
                    rect: (
                        slot_x,
                        y,
                        M3_HEADER_SLOT_SIZE + M3_HEADER_SLOT_INSET,
                        h,
                    ),
                    action: NavigatorHeaderAction::HeaderRight,
                });
                title_right = slot_x - M3_HEADER_SLOT_INSET;
            }
        }

        // Title — left-aligned (M3 small top app bar) within the
        // remaining space between the leading slot and the
        // trailing slot.
        if let Some(buffer) = chrome.title {
            let (text_w, text_h) = measure_buffer(buffer);
            let available = (title_right - title_left).max(0.0);
            let tx = title_left;
            let ty = y + (h - text_h) * 0.5;
            texts.push(StagedText {
                buffer,
                x: tx,
                y: ty,
                color: chrome.title_color.unwrap_or(M3_HEADER_TITLE),
                // Clip to the title slot so a long title clips
                // before colliding with the trailing icon.
                clip: TextBounds {
                    left: tx as i32,
                    top: y as i32,
                    right: (tx + available.min(text_w)) as i32,
                    bottom: (y + h) as i32,
                },
            });
        }
    }

    fn safe_area_insets(&self) -> runtime_core::EdgeInsets {
        runtime_core::EdgeInsets {
            top: M3_STATUS_BAR_HEIGHT,
            right: 0.0,
            bottom: M3_GESTURE_NAV_HEIGHT,
            left: 0.0,
        }
    }

    fn device_corner_radius(&self) -> f32 {
        self.config.corner_radius
    }

    fn chrome_glyph_labels(&self) -> Vec<(&'static str, String, f32)> {
        vec![("clock", String::new(), M3_STATUS_FONT_SIZE)]
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
        // Status-bar foreground follows the configured style.
        let fg = match cfg.status_bar_style {
            StatusBarStyle::Dark => M3_STATUS_FG,
            StatusBarStyle::Light => [1.0, 1.0, 1.0, 1.0],
        };
        let bar_h = insets.top.max(M3_STATUS_BAR_HEIGHT);

        if let Some(clock) = glyphs.get("clock") {
            let (_tw, th) = measure_buffer(clock);
            // M3 puts the clock in the *top-left*, opposite iOS.
            let tx = M3_STATUS_INSET;
            let ty = (bar_h - th) * 0.5;
            texts.push(StagedText {
                buffer: clock,
                x: tx,
                y: ty,
                color: fg,
                clip: TextBounds {
                    left: 0,
                    top: 0,
                    right: (vw * 0.4) as i32,
                    bottom: bar_h as i32,
                },
            });
        }

        // Right-side: vertical-bar signal, wifi-square, battery.
        let icon_y = (bar_h - 10.0) * 0.5;
        let right_anchor = vw - M3_STATUS_INSET;
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
        let wifi_y = (bar_h - wifi_size) * 0.5;
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

        // --- Camera cutout ---
        match cfg.notch {
            NotchStyle::None => {}
            NotchStyle::HolePunchCentered { diameter, top_offset } => {
                let cx = (vw - diameter) * 0.5;
                let cy = top_offset;
                rects.push(rect_inst(
                    cx, cy, diameter, diameter,
                    [0.0, 0.0, 0.0, 1.0],
                    [diameter * 0.5; 4],
                    [0.0; 4],
                    0.0,
                ));
            }
            NotchStyle::HolePunchLeft { diameter, top_offset, left_inset } => {
                rects.push(rect_inst(
                    left_inset, top_offset, diameter, diameter,
                    [0.0, 0.0, 0.0, 1.0],
                    [diameter * 0.5; 4],
                    [0.0; 4],
                    0.0,
                ));
            }
            NotchStyle::Teardrop { width, height } => {
                // Approximation: a wide pill hanging from the
                // top edge. Real teardrops taper to a point;
                // the rect shader doesn't do non-uniform
                // rounded shapes, so the pill is a fair
                // stand-in at simulator zoom.
                let tx = (vw - width) * 0.5;
                rects.push(rect_inst(
                    tx, 0.0, width, height,
                    [0.0, 0.0, 0.0, 1.0],
                    [0.0, 0.0, height * 0.5, height * 0.5],
                    [0.0; 4],
                    0.0,
                ));
            }
        }

        // Corner masking + the rounded device silhouette are
        // handled by the renderer's `device_frame` pipeline.
        // See `ios-sim` for the rationale.
        let _ = cfg.bezel;

        // --- Gesture nav line ---
        // Android Q+ shows a thin pill (10dp tall, ~108dp wide)
        // in the gesture bar.
        if insets.bottom > 0.0 {
            let line_w = 108.0;
            let line_h = 4.0;
            let line_x = (vw - line_w) * 0.5;
            let line_y = vh - insets.bottom * 0.5 - line_h * 0.5;
            let line_color = match cfg.status_bar_style {
                StatusBarStyle::Dark => M3_STATUS_FG,
                StatusBarStyle::Light => [1.0, 1.0, 1.0, 1.0],
            };
            rects.push(rect_inst(
                line_x, line_y, line_w, line_h,
                line_color,
                [line_h * 0.5; 4],
                [0.0; 4],
                0.0,
            ));
        }
    }
}

/// Measure a pre-laid glyphon buffer's natural bounding box.
/// Used by `paint_navigator_header` for left-aligned title
/// placement + clip-rect sizing.
fn measure_buffer(buffer: &Buffer) -> (f32, f32) {
    let mut max_w: f32 = 0.0;
    let mut total_h: f32 = 0.0;
    for run in buffer.layout_runs() {
        max_w = max_w.max(run.line_w);
        total_h += run.line_height;
    }
    (max_w, total_h)
}
