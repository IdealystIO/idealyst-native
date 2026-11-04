//! A `text_input`'s `text_align` reaches the skin's paint: the renderer works out
//! where the text starts from the field's laid-out width and the shaped line, and
//! the skin draws there. A laid-out frame goes through a real `Renderer` with a
//! skin recording what it's handed; the sim skins' own tests pin how they use it.
//!
//! Skips without a wgpu adapter, like `headless_screenshot.rs`.
//! Run with: `cargo test -p render-wgpu --features headless --test text_input_align`

#![cfg(feature = "headless")]

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use glyphon::Buffer;
use render_wgpu::headless::Screenshotter;
use render_wgpu::keyboard::{KeySpec, LaidKey, LayoutMetrics};
use render_wgpu::pipeline::Instance as RectInstance;
use render_wgpu::text::StagedText;
use render_wgpu::widgets::TEXT_INPUT_HPAD;
use render_wgpu::{NavigatorHeaderChrome, NavigatorHeaderHit, Painter};
use runtime_shared::{ColorScheme, Length, StyleRules, TextAlign, Tokenized};

/// Records the offset of every text input it's asked to paint.
struct Recording {
    offsets: Rc<RefCell<Vec<f32>>>,
}

impl Painter for Recording {
    // Native widgets — stubbed. The layout box still exists (the
    // framework laid it out and the renderer drew its background/border
    // if styled); only the skin-drawn knob/track/caret is absent.
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
        text_offset_x: f32,
        _text_color: [f32; 4],
        _field_bg: Option<[f32; 4]>,
        _rects: &mut Vec<RectInstance>,
        _texts: &mut Vec<StagedText<'a>>,
    ) {
        self.offsets.borrow_mut().push(text_offset_x);
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

    // On-screen keyboard — never shown headlessly (no rows → the
    // layout engine produces nothing to paint).
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

    // Navigator header — stubbed. The screen body still renders; only
    // the skin-drawn title bar is absent.
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

const FIELD_WIDTH: f32 = 300.0;

/// The offset the renderer hands a skin for an input aligned `align`, or `None`
/// without a wgpu adapter.
fn offset_for(align: Option<TextAlign>) -> Option<f32> {
    let offsets = Rc::new(RefCell::new(Vec::new()));
    let skin = Rc::new(Recording { offsets: offsets.clone() });
    let mut shot = match Screenshotter::with_color_scheme_and_skin(400, 100, ColorScheme::Light, skin) {
        Ok(shot) => shot,
        Err(e) => {
            eprintln!("[text_input_align] skipping, no usable wgpu adapter: {e}");
            return None;
        }
    };
    let _app = render_wgpu::newcore::start(shot.backend(), |_| {}, move || {
        runtime_vocabulary::builders::text_input()
            .value("12".to_string())
            .style(StyleRules {
                width: Some(Tokenized::Literal(Length::Px(FIELD_WIDTH))),
                text_align: align,
                ..Default::default()
            })
            .build()
    });
    shot.capture_rgba();
    let recorded = offsets.borrow();
    Some(*recorded.last().expect("the input was painted"))
}

#[test]
fn regression_gpu_text_input_ignored_text_align() {
    let Some(left) = offset_for(None) else { return };
    assert_eq!(left, 0.0, "no alignment starts at the padding");
    assert_eq!(offset_for(Some(TextAlign::Left)), Some(0.0));

    let right = offset_for(Some(TextAlign::Right)).unwrap();
    let center = offset_for(Some(TextAlign::Center)).unwrap();
    assert!(right > 0.0, "right moves the text, got {right}");
    assert!((center - right / 2.0).abs() < 0.5, "centre is half of right: {center} and {right}");
    assert!(right < FIELD_WIDTH - TEXT_INPUT_HPAD * 2.0, "by the room the text leaves, got {right}");
}
