//! On-screen keyboard for the simulator preview.
//!
//! Painted as an overlay at the bottom of the viewport whenever
//! a `TextInput` is focused. Hit-tested by the host on
//! `pointer_down`: a tap on a key synthesizes a
//! [`render_api::KeyEvent`] that flows through the existing
//! `Host::key` path — so the same backspace / character-insert
//! / Escape logic that handles physical keys handles screen
//! taps too. No extra plumbing for the consumer.
//!
//! Glyph buffers for every key label are created once at
//! [`Host`](crate::Host) construction and live for the host's
//! lifetime — the renderer's read-only walk just borrows them.
//! Tiny memory cost (~3 dozen glyphon Buffers) and zero per-tap
//! allocation.
//!
//! This module owns the **shared** layout engine. Per-platform
//! row content + visual skin lives in the active
//! [`Painter`](crate::Painter) implementation; the layout engine
//! consumes `skin.keyboard_rows()` + `skin.keyboard_layout_metrics()`,
//! and the paint pass calls `skin.paint_keyboard(...)`.

use std::collections::HashMap;

use glyphon::{Attrs, Buffer, Family, FontSystem, Metrics, Shaping};

use render_api::{Key, KeyEvent, KeyModifiers};

use crate::node::{KEYBOARD_HEIGHT, KEYBOARD_KEY_FONT_SIZE};
use crate::painter::Painter;

// ---------------------------------------------------------------------------
// Key vocabulary — shared across all skins
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
/// the host shell hands us for physical keys. Goes straight
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
/// etc. The layout engine normalizes these to actual pixels.
pub struct KeySpec {
    pub label: &'static str,
    pub action: KeyAction,
    pub width_units: f32,
}

/// Convenience constructor for a letter key (width 1.0, action
/// = insert `c`).
pub fn letter(c: char, label: &'static str) -> KeySpec {
    KeySpec { label, action: KeyAction::Character(c), width_units: 1.0 }
}

/// Per-skin inter-key spacing knobs. The shared layout engine
/// uses these instead of hard-coding gaps so iOS and Android
/// (and any future skin) can each pack keys at its native
/// density.
pub struct LayoutMetrics {
    pub key_gap: f32,
    pub row_gap: f32,
    pub side_margin: f32,
    pub vert_margin: f32,
}

/// Construct glyphon buffers for every label the given `skin`
/// can show. Caller stores the result on the host and re-uses
/// the buffers across frames. If the skin changes at runtime
/// (e.g. iOS↔Android flip), the host rebuilds.
pub fn build_glyph_cache(
    font_system: &mut FontSystem,
    skin: &dyn Painter,
) -> HashMap<&'static str, Buffer> {
    let mut cache = HashMap::new();
    let rows = skin.keyboard_rows();
    for row in rows {
        for k in row {
            if cache.contains_key(k.label) {
                continue;
            }
            let mut buf = Buffer::new(
                font_system,
                Metrics::new(KEYBOARD_KEY_FONT_SIZE, KEYBOARD_KEY_FONT_SIZE * 1.2),
            );
            buf.set_size(font_system, None, None);
            buf.set_text(
                font_system,
                k.label,
                &Attrs::new().family(Family::SansSerif),
                Shaping::Advanced,
                None,
            );
            buf.shape_until_scroll(font_system, false);
            cache.insert(k.label, buf);
        }
    }
    cache
}

// ---------------------------------------------------------------------------
// Geometry — shared layout engine
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
    let y = viewport.1 - h + (1.0 - s) * h;
    Some((0.0, y, viewport.0, h))
}

/// A laid-out key with its absolute screen rect. Used by both
/// the paint path and the hit-tester so they agree exactly on
/// where each key sits.
pub struct LaidKey {
    pub label: &'static str,
    pub action: KeyAction,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// Lay out the rows from `skin` into absolute rects within the
/// viewport. Each row of total `Σ width_units` is stretched to
/// fit the keyboard's content width. `slide` shifts the whole
/// keyboard down — anywhere in `[0.0, 1.0]`.
pub fn layout(
    skin: &dyn Painter,
    viewport: (f32, f32),
    slide: f32,
) -> (Option<(f32, f32, f32, f32)>, Vec<LaidKey>) {
    let Some(kb_rect) = keyboard_rect(viewport, slide) else {
        return (None, Vec::new());
    };
    let (kb_x, kb_y, kb_w, kb_h) = kb_rect;
    let metrics = skin.keyboard_layout_metrics();
    let content_w = (kb_w - metrics.side_margin * 2.0).max(0.0);

    let rows = skin.keyboard_rows();
    let row_count = rows.len() as f32;
    if row_count == 0.0 {
        return (Some(kb_rect), Vec::new());
    }
    let total_row_gap = metrics.row_gap * (row_count - 1.0).max(0.0);
    let avail = (kb_h - metrics.vert_margin * 2.0 - total_row_gap).max(0.0);
    let row_h = (avail / row_count).max(0.0);

    let mut out = Vec::with_capacity(rows.iter().map(|r| r.len()).sum());
    let mut y = kb_y + metrics.vert_margin;
    for row in rows {
        let total_units: f32 = row.iter().map(|k| k.width_units).sum();
        let key_count = row.len() as f32;
        let total_gap = metrics.key_gap * (key_count - 1.0).max(0.0);
        let unit_w = if total_units > 0.0 {
            (content_w - total_gap) / total_units
        } else {
            0.0
        };
        let mut x = kb_x + metrics.side_margin;
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
            x += w + metrics.key_gap;
        }
        y += row_h + metrics.row_gap;
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
    skin: &dyn Painter,
    viewport: (f32, f32),
    slide: f32,
    point: (f32, f32),
) -> (Option<(f32, f32, f32, f32)>, Option<KeyAction>) {
    let (kb_rect, keys) = layout(skin, viewport, slide);
    let Some(rect) = kb_rect else {
        return (None, None);
    };
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
// Paint — dispatches to the skin
// ---------------------------------------------------------------------------

/// Paint the keyboard overlay through `skin`. The layout engine
/// produces the rect + laid keys; the skin owns all the chrome
/// + label drawing. `pressed_label` is the label of a key
/// currently shown as pressed (for the brief tap-feedback
/// highlight); `None` if no key is held.
pub fn paint<'a>(
    skin: &dyn Painter,
    viewport: (f32, f32),
    slide: f32,
    pressed_label: Option<&'static str>,
    glyphs: &'a HashMap<&'static str, Buffer>,
    rects: &mut Vec<crate::pipeline::Instance>,
    texts: &mut Vec<crate::text::StagedText<'a>>,
) {
    let (kb_rect, keys) = layout(skin, viewport, slide);
    let Some(rect) = kb_rect else { return };
    skin.paint_keyboard(rect, &keys, pressed_label, glyphs, rects, texts);
}
