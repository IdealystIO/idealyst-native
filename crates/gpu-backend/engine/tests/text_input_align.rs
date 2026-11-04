//! `text_align` on `text_input` and `text_area`, through a real `Renderer`.
//!
//! An input's text starts where the renderer works it out from the field's
//! laid-out width and the shaped line, and the skin draws there. A text area aligns
//! each of its lines inside the field, in the shaped buffer the skin is handed. A
//! laid-out frame goes through with a skin recording what it's handed; the sim
//! skins' own tests pin how they use it. A signal flips the alignment, so a style
//! with no alignment going back to natural runs the real reactive path.
//!
//! Skips without a wgpu adapter, like `headless_screenshot.rs`.
//! Run with: `cargo test -p render-wgpu --features headless --test text_input_align`

#![cfg(feature = "headless")]

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use glyphon::Buffer;
use render_wgpu::headless::Screenshotter;
use render_wgpu::keyboard::{KeySpec, LaidKey, LayoutMetrics};
use render_wgpu::pipeline::Instance as RectInstance;
use render_wgpu::text::StagedText;
use render_wgpu::widgets::TEXT_INPUT_HPAD;
use render_wgpu::{NavigatorHeaderChrome, NavigatorHeaderHit, Painter};
use runtime_shared::{Color, ColorScheme, Length, StyleRules, TextAlign, Tokenized};

/// One text input or text area as the renderer handed it to the skin.
#[derive(Clone, Debug)]
struct Painted {
    offset: f32,
    caret_x_local: f32,
    /// Where each shaped line's glyphs start and end, relative to the buffer.
    lines: Vec<(f32, f32)>,
}

/// Records every text input and text area it's asked to paint.
struct Recording {
    painted: Rc<RefCell<Vec<Painted>>>,
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
        buffer: &'a Buffer,
        caret_x_local: f32,
        text_offset_x: f32,
        _text_color: [f32; 4],
        _field_bg: Option<[f32; 4]>,
        _rects: &mut Vec<RectInstance>,
        _texts: &mut Vec<StagedText<'a>>,
    ) {
        let lines = buffer
            .layout_runs()
            .filter_map(|run| {
                let first = run.glyphs.first()?;
                let last = run.glyphs.last()?;
                Some((first.x, last.x + last.w))
            })
            .collect();
        self.painted.borrow_mut().push(Painted { offset: text_offset_x, caret_x_local, lines });
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

fn screenshotter(painted: Rc<RefCell<Vec<Painted>>>) -> Option<Screenshotter> {
    let skin = Rc::new(Recording { painted });
    match Screenshotter::with_color_scheme_and_skin(400, 100, ColorScheme::Light, skin) {
        Ok(shot) => Some(shot),
        Err(e) => {
            eprintln!("[text_input_align] skipping, no usable wgpu adapter: {e}");
            None
        }
    }
}

fn aligned(align: Option<TextAlign>) -> StyleRules {
    StyleRules {
        width: Some(Tokenized::Literal(Length::Px(FIELD_WIDTH))),
        color: Some(Tokenized::Literal(Color("#000000".into()))),
        text_align: align,
        ..Default::default()
    }
}

/// The offset the renderer hands a skin for an input aligned `align`, or `None`
/// without a wgpu adapter.
fn offset_for(align: Option<TextAlign>) -> Option<f32> {
    let painted = Rc::new(RefCell::new(Vec::new()));
    let mut shot = screenshotter(painted.clone())?;
    let _app = render_wgpu::newcore::start(shot.backend(), |_| {}, move || {
        runtime_vocabulary::builders::text_input()
            .value("12".to_string())
            .style(aligned(align))
            .build()
    });
    shot.capture_rgba();
    let recorded = painted.borrow();
    Some(recorded.last().expect("the input was painted").offset)
}

/// A reactive alignment, built by `build` and flipped through a signal. `step` runs
/// after each flip has been flushed and painted.
fn flip_through(
    flips: &[Option<TextAlign>],
    painted: Rc<RefCell<Vec<Painted>>>,
    build: impl Fn(Rc<dyn Fn() -> StyleRules>) -> runtime_scene::Element + 'static,
    mut step: impl FnMut(Option<TextAlign>),
) {
    let Some(mut shot) = screenshotter(painted) else { return };
    let slot: Rc<Cell<Option<runtime_world::Signal<Option<TextAlign>>>>> = Rc::new(Cell::new(None));
    let slot_in = slot.clone();
    let first = flips[0];
    let _app = render_wgpu::newcore::start(shot.backend(), |_| {}, move || {
        let align = runtime_world::signal(first);
        slot_in.set(Some(align));
        build(Rc::new(move || aligned(align.get())))
    });
    let align = slot.get().expect("the build ran");
    for flip in flips {
        align.set(*flip);
        render_wgpu::newcore::flush_sync();
        shot.capture_rgba();
        step(*flip);
    }
}

/// `RenderStyle` kept the last alignment when a style dropped it, so an input that
/// went back to `Natural` stayed where `Right` put it.
#[test]
fn regression_gpu_removed_text_align_keeps_alignment_on_an_input() {
    let painted = Rc::new(RefCell::new(Vec::new()));
    let seen = painted.clone();
    flip_through(
        &[Some(TextAlign::Right), None, Some(TextAlign::Center), None],
        painted,
        |style| {
            runtime_vocabulary::builders::text_input()
                .value("12".to_string())
                .style(move || style())
                .build()
        },
        |align| {
            let offset = seen.borrow().last().expect("the input was painted").offset;
            match align {
                None => assert_eq!(offset, 0.0, "no alignment starts at the padding"),
                Some(_) => assert!(offset > 0.0, "{align:?} moves the text, got {offset}"),
            }
        },
    );
}

/// A text area stayed at the start whatever its `text_align`. Each line aligns on
/// its own, and the caret follows the first.
#[test]
fn regression_gpu_text_area_ignored_text_align() {
    let inner = FIELD_WIDTH - TEXT_INPUT_HPAD * 2.0;
    let painted = Rc::new(RefCell::new(Vec::new()));
    let seen = painted.clone();
    flip_through(
        &[Some(TextAlign::Right), Some(TextAlign::Center), None, Some(TextAlign::Left)],
        painted,
        |style| {
            runtime_vocabulary::builders::text_area()
                .value("One\nThree long".to_string())
                .style(move || style())
                .build()
        },
        |align| {
            let recorded = seen.borrow();
            let area = recorded.last().expect("the text area was painted");
            assert_eq!(area.offset, 0.0, "the lines carry the alignment, not an offset");
            assert_eq!(area.lines.len(), 2, "two lines stay two lines: {:?}", area.lines);
            for &(start, end) in &area.lines {
                match align {
                    Some(TextAlign::Right) => assert!((end - inner).abs() < 1.0, "right ends at {inner}, got {start}..{end}"),
                    Some(TextAlign::Center) => {
                        assert!(((start + end) / 2.0 - inner / 2.0).abs() < 1.0, "centre is around {}, got {start}..{end}", inner / 2.0)
                    }
                    _ => assert!(start.abs() < 1.0, "{align:?} starts at the padding, got {start}..{end}"),
                }
            }
            let first_line_end = area.lines[0].1;
            assert!(
                (area.caret_x_local - first_line_end).abs() < 1.0,
                "{align:?}: the caret sits at the end of the first line {first_line_end}, got {}",
                area.caret_x_local
            );
        },
    );
}

/// A line wider than the field starts at the padding instead of spilling left, and
/// the field doesn't wrap it: soft wrap isn't part of the GPU text area.
#[test]
fn text_area_line_wider_than_the_field_starts_at_the_padding() {
    let painted = Rc::new(RefCell::new(Vec::new()));
    let seen = painted.clone();
    flip_through(
        &[Some(TextAlign::Right)],
        painted,
        |style| {
            runtime_vocabulary::builders::text_area()
                .value("a line of text far too long to fit inside the width of this text area field".to_string())
                .style(move || style())
                .build()
        },
        |_| {
            let recorded = seen.borrow();
            let area = recorded.last().expect("the text area was painted");
            assert_eq!(area.lines.len(), 1, "no soft wrap: {:?}", area.lines);
            assert!(area.lines[0].0.abs() < 1.0, "starts at the padding: {:?}", area.lines);
        },
    );
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
