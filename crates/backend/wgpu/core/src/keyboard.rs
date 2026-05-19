//! On-screen keyboard for the simulator preview.
//!
//! Painted as an overlay at the bottom of the viewport whenever
//! a `TextInput` is focused. Hit-tested by the host on
//! `pointer_down`: a tap on a key synthesizes a
//! [`crate::input::KeyEvent`] that flows through the existing
//! `Host::key` path — so the same backspace / character-insert
//! / Escape logic that handles physical keys handles screen
//! taps too. No extra plumbing for the consumer.
//!
//! Glyph buffers for every key label are created once at
//! [`Host`](crate::Host) construction and live for the host's
//! lifetime — the renderer's read-only walk just borrows them.
//! Tiny memory cost (~3 dozen glyphon Buffers) and zero per-tap
//! allocation.

use std::collections::HashMap;

use glyphon::{Attrs, Buffer, Family, FontSystem, Metrics, Shaping, TextBounds};

use backend_wgpu_api::{Key, KeyEvent, KeyModifiers};
use crate::node::{
    KEYBOARD_HEIGHT, KEYBOARD_KEY_FONT_SIZE, KEYBOARD_KEY_GAP, KEYBOARD_KEY_RADIUS,
    KEYBOARD_ROW_GAP, KEYBOARD_SIDE_MARGIN, KEYBOARD_VERT_MARGIN,
};
use crate::pipeline::Instance as RectInstance;
use backend_wgpu_api::SimulatedPlatform;
use crate::style_convert::srgb_rgba_to_linear;
use crate::text::StagedText;

// ---------------------------------------------------------------------------
// Layout vocabulary
// ---------------------------------------------------------------------------

/// What a key press synthesizes. Translated to a [`KeyEvent`] by
/// [`action_to_key_event`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyAction {
    /// Insert a printable character.
    Character(char),
    Backspace,
    Enter,
    Space,
}

/// Translate an on-screen key into the same [`KeyEvent`] vocabulary
/// the platform shell hands us for physical keys. Goes straight
/// into `Host::key`.
pub fn action_to_key_event(action: KeyAction) -> KeyEvent {
    match action {
        KeyAction::Character(c) => KeyEvent {
            key: Key::Character,
            text: Some(c.to_string()),
            modifiers: KeyModifiers::default(),
            pressed: true,
        },
        KeyAction::Space => KeyEvent {
            key: Key::Character,
            text: Some(" ".to_string()),
            modifiers: KeyModifiers::default(),
            pressed: true,
        },
        KeyAction::Backspace => KeyEvent {
            key: Key::Backspace,
            text: None,
            modifiers: KeyModifiers::default(),
            pressed: true,
        },
        KeyAction::Enter => KeyEvent {
            key: Key::Enter,
            text: None,
            modifiers: KeyModifiers::default(),
            pressed: true,
        },
    }
}

/// A single keyboard key. `label` doubles as the glyph-buffer
/// cache key. `width_units` are relative widths within a row:
/// every letter key is 1.0, space is wider, backspace is ~1.5,
/// etc. The row layout normalizes these to actual pixels.
struct KeySpec {
    label: &'static str,
    action: KeyAction,
    width_units: f32,
}

fn letter(c: char, label: &'static str) -> KeySpec {
    KeySpec { label, action: KeyAction::Character(c), width_units: 1.0 }
}

/// iOS QWERTY portrait layout — three letter rows + a bottom
/// row with space and return. Skipping shift/numbers/symbols
/// for the MVP.
fn rows_ios() -> Vec<Vec<KeySpec>> {
    vec![
        vec![
            letter('q', "q"), letter('w', "w"), letter('e', "e"), letter('r', "r"),
            letter('t', "t"), letter('y', "y"), letter('u', "u"), letter('i', "i"),
            letter('o', "o"), letter('p', "p"),
        ],
        vec![
            letter('a', "a"), letter('s', "s"), letter('d', "d"), letter('f', "f"),
            letter('g', "g"), letter('h', "h"), letter('j', "j"), letter('k', "k"),
            letter('l', "l"),
        ],
        vec![
            letter('z', "z"), letter('x', "x"), letter('c', "c"), letter('v', "v"),
            letter('b', "b"), letter('n', "n"), letter('m', "m"),
            KeySpec { label: "⌫", action: KeyAction::Backspace, width_units: 1.5 },
        ],
        vec![
            KeySpec { label: "space", action: KeyAction::Space, width_units: 5.0 },
            KeySpec { label: "return", action: KeyAction::Enter, width_units: 2.0 },
        ],
    ]
}

fn rows_for(platform: SimulatedPlatform) -> Vec<Vec<KeySpec>> {
    match platform {
        // TODO(android): a Material 3 keyboard layout. For now
        // fall through to iOS so apps remain typable.
        SimulatedPlatform::Ios | SimulatedPlatform::Android => rows_ios(),
    }
}

/// Every label any platform's keyboard can show. Used by
/// [`Host`](crate::Host) at construction to pre-create glyph
/// buffers — once the host is up, the renderer's walk can hand
/// out `&Buffer` references without any further allocation.
pub fn all_labels() -> Vec<&'static str> {
    // Union of every platform's row labels.
    let mut out: Vec<&'static str> = Vec::new();
    for row in rows_ios() {
        for k in row {
            if !out.contains(&k.label) {
                out.push(k.label);
            }
        }
    }
    out
}

/// Construct glyphon buffers for every keyboard label using
/// `font_system`. Caller stores the result on the host and
/// re-uses the buffers across frames.
pub fn build_glyph_cache(
    font_system: &mut FontSystem,
) -> HashMap<&'static str, Buffer> {
    let mut cache = HashMap::new();
    for label in all_labels() {
        // Show the user-facing text. "space" / "return" stand
        // in for their semantic action — render the same word.
        let mut buf = Buffer::new(
            font_system,
            Metrics::new(KEYBOARD_KEY_FONT_SIZE, KEYBOARD_KEY_FONT_SIZE * 1.2),
        );
        buf.set_size(font_system, None, None);
        buf.set_text(
            font_system,
            label,
            Attrs::new().family(Family::SansSerif),
            Shaping::Advanced,
        );
        buf.shape_until_scroll(font_system, false);
        cache.insert(label, buf);
    }
    cache
}

// ---------------------------------------------------------------------------
// Geometry
// ---------------------------------------------------------------------------

/// Outer rect of the keyboard at the given `slide` value, in
/// logical px. `slide` in `[0.0, 1.0]`: `1.0` = fully on-screen
/// at the bottom edge; `0.0` = fully off-screen below the
/// viewport. Returns `None` for a degenerate viewport.
pub fn keyboard_rect(
    viewport: (f32, f32),
    slide: f32,
) -> Option<(f32, f32, f32, f32)> {
    if viewport.0 <= 0.0 || viewport.1 <= 0.0 {
        return None;
    }
    let h = KEYBOARD_HEIGHT.min(viewport.1);
    let s = slide.clamp(0.0, 1.0);
    // At rest, the keyboard's top is `viewport_h - h`. While
    // sliding in, push it further down by `(1 - s) * h`.
    let y = viewport.1 - h + (1.0 - s) * h;
    Some((0.0, y, viewport.0, h))
}

/// A laid-out key with its absolute screen rect. Used by both
/// the renderer and the hit-tester so they agree exactly on
/// where each key sits.
struct LaidKey {
    label: &'static str,
    action: KeyAction,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

/// Lay out the rows into absolute rects within the viewport.
/// Each row of total `Σ width_units` is stretched to fit the
/// keyboard's content width. `slide` shifts the whole keyboard
/// down — anywhere in `[0.0, 1.0]`.
fn layout(
    platform: SimulatedPlatform,
    viewport: (f32, f32),
    slide: f32,
) -> (Option<(f32, f32, f32, f32)>, Vec<LaidKey>) {
    let Some(kb_rect) = keyboard_rect(viewport, slide) else {
        return (None, Vec::new());
    };
    let (kb_x, kb_y, kb_w, kb_h) = kb_rect;
    let content_w = (kb_w - KEYBOARD_SIDE_MARGIN * 2.0).max(0.0);

    let rows = rows_for(platform);
    let row_count = rows.len() as f32;
    if row_count == 0.0 {
        return (Some(kb_rect), Vec::new());
    }
    // Even row heights minus the top/bottom margins and inter-row gaps.
    let total_row_gap = KEYBOARD_ROW_GAP * (row_count - 1.0).max(0.0);
    let avail = (kb_h - KEYBOARD_VERT_MARGIN * 2.0 - total_row_gap).max(0.0);
    let row_h = (avail / row_count).max(0.0);

    let mut out = Vec::with_capacity(rows.iter().map(|r| r.len()).sum());
    let mut y = kb_y + KEYBOARD_VERT_MARGIN;
    for row in rows {
        let total_units: f32 = row.iter().map(|k| k.width_units).sum();
        let key_count = row.len() as f32;
        let total_gap = KEYBOARD_KEY_GAP * (key_count - 1.0).max(0.0);
        let unit_w = if total_units > 0.0 {
            (content_w - total_gap) / total_units
        } else {
            0.0
        };
        let mut x = kb_x + KEYBOARD_SIDE_MARGIN;
        for k in row {
            let w = unit_w * k.width_units;
            out.push(LaidKey {
                label: k.label,
                action: k.action,
                x,
                y,
                w,
                h: row_h,
            });
            x += w + KEYBOARD_KEY_GAP;
        }
        y += row_h + KEYBOARD_ROW_GAP;
    }
    (Some(kb_rect), out)
}

// ---------------------------------------------------------------------------
// Hit-test
// ---------------------------------------------------------------------------

/// Returns `(keyboard_rect, Option<key_action_at_point>)`. The
/// `keyboard_rect` is `None` if the keyboard isn't visible (no
/// viewport / degenerate size); otherwise the rect is always
/// returned so the host can know whether a press fell inside
/// the keyboard area at all (vs. landing on the app content
/// above it).
pub fn hit_test(
    platform: SimulatedPlatform,
    viewport: (f32, f32),
    slide: f32,
    point: (f32, f32),
) -> (Option<(f32, f32, f32, f32)>, Option<KeyAction>) {
    let (kb_rect, keys) = layout(platform, viewport, slide);
    let Some(rect) = kb_rect else {
        return (None, None);
    };
    // Point inside the keyboard's overall rect?
    let in_kb = point.0 >= rect.0
        && point.0 <= rect.0 + rect.2
        && point.1 >= rect.1
        && point.1 <= rect.1 + rect.3;
    if !in_kb {
        return (Some(rect), None);
    }
    for k in keys {
        if point.0 >= k.x
            && point.0 <= k.x + k.w
            && point.1 >= k.y
            && point.1 <= k.y + k.h
        {
            return (Some(rect), Some(k.action));
        }
    }
    // Inside keyboard frame but in a gutter between keys — swallow
    // the event (don't let it reach the content beneath).
    (Some(rect), None)
}

// ---------------------------------------------------------------------------
// Paint
// ---------------------------------------------------------------------------

/// Paint the keyboard overlay. iOS skin: light-gray background
/// with white rounded keys. Labels rendered from the host's
/// pre-built glyph cache.
pub fn paint<'a>(
    platform: SimulatedPlatform,
    viewport: (f32, f32),
    slide: f32,
    glyphs: &'a HashMap<&'static str, Buffer>,
    rects: &mut Vec<RectInstance>,
    texts: &mut Vec<StagedText<'a>>,
) {
    match platform {
        SimulatedPlatform::Ios | SimulatedPlatform::Android => {
            paint_ios(viewport, slide, glyphs, rects, texts);
        }
    }
}

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

fn paint_ios<'a>(
    viewport: (f32, f32),
    slide: f32,
    glyphs: &'a HashMap<&'static str, Buffer>,
    rects: &mut Vec<RectInstance>,
    texts: &mut Vec<StagedText<'a>>,
) {
    let (kb_rect, keys) = layout(SimulatedPlatform::Ios, viewport, slide);
    let Some((kb_x, kb_y, kb_w, kb_h)) = kb_rect else { return };

    // Background panel.
    rects.push(RectInstance {
        rect: [kb_x, kb_y, kb_w, kb_h],
        bg: srgb_rgba_to_linear(IOS_KB_BG),
        corner_radius: [0.0; 4],
        border_color: [0.0; 4],
        border_width: 0.0,
        _pad: [0.0; 3],
    });

    // Keys.
    for k in keys {
        // Special keys (return, space, backspace) get the darker
        // shade — iOS visual distinction between letter keys and
        // modifier keys.
        let is_special = !matches!(k.action, KeyAction::Character(_));
        let bg = if is_special { IOS_KEY_BG_DARK } else { IOS_KEY_BG };
        rects.push(RectInstance {
            rect: [k.x, k.y, k.w, k.h],
            bg: srgb_rgba_to_linear(bg),
            corner_radius: [KEYBOARD_KEY_RADIUS; 4],
            border_color: [0.0; 4],
            border_width: 0.0,
            _pad: [0.0; 3],
        });

        // Center the label glyph within the key's rect.
        if let Some(buf) = glyphs.get(k.label) {
            // Glyphon doesn't expose a measured-width API; the
            // first layout run's `line_w` is the rendered width
            // (close enough for centered labels).
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
