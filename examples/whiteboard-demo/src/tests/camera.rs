//! Camera-widget bounds: `clamp_cam` (the single in-bounds enforcer) and the
//! size/shape dimensions.

use crate::settings::camera_dims;
use crate::{clamp_cam, CameraShape, CameraSize, DRAG_MARGIN};

// Medium rounded-rect camera dims, for the bounds tests.
fn cam_wh() -> (f32, f32) {
    let (w, h, _r) = camera_dims(CameraShape::RoundedRect, CameraSize::Medium);
    (w, h)
}

// Regression for settings requirement #3 ("the camera should stay in the
// bounds"): `clamp_cam` is the single enforcer — every read site
// (`clamped_cam`) funnels through it, so the widget can never escape the
// stage regardless of a stale drag position or an aspect change shrinking
// the stage under it.
const STAGE_W: f32 = 400.0;
const STAGE_H: f32 = 700.0;

#[test]
fn camera_inside_bounds_is_unchanged() {
    let (cw, ch) = cam_wh();
    let (x, y) = clamp_cam(120.0, 300.0, STAGE_W, STAGE_H, cw, ch);
    assert_eq!((x, y), (120.0, 300.0));
}

#[test]
fn camera_past_right_and_bottom_clamps_inside() {
    // Way past the far corner → pinned to the max inset, fully inside.
    let (cw, ch) = cam_wh();
    let (x, y) = clamp_cam(9_999.0, 9_999.0, STAGE_W, STAGE_H, cw, ch);
    assert_eq!(x, STAGE_W - cw - DRAG_MARGIN);
    assert_eq!(y, STAGE_H - ch - DRAG_MARGIN);
    // The whole widget rect sits within the stage.
    assert!(x + cw + DRAG_MARGIN <= STAGE_W);
    assert!(y + ch + DRAG_MARGIN <= STAGE_H);
}

#[test]
fn camera_past_top_left_clamps_to_margin() {
    let (cw, ch) = cam_wh();
    let (x, y) = clamp_cam(-50.0, -50.0, STAGE_W, STAGE_H, cw, ch);
    assert_eq!((x, y), (DRAG_MARGIN, DRAG_MARGIN));
}

#[test]
fn stage_smaller_than_widget_pins_to_margin() {
    // An aspect change can shrink the stage below the widget size; the
    // `.max(m)` floor keeps the position valid (top-left margin) instead of
    // producing a negative clamp range that would invert.
    let (cw, ch) = cam_wh();
    let (x, y) = clamp_cam(200.0, 200.0, cw - 10.0, ch - 10.0, cw, ch);
    assert_eq!((x, y), (DRAG_MARGIN, DRAG_MARGIN));
}

// The camera scales with size and is square (full-radius) when circular.
#[test]
fn camera_dims_scale_and_shape() {
    let (mw, mh, _mr) = camera_dims(CameraShape::RoundedRect, CameraSize::Medium);
    let (sw, sh, _sr) = camera_dims(CameraShape::RoundedRect, CameraSize::Small);
    let (lw, lh, _lr) = camera_dims(CameraShape::RoundedRect, CameraSize::Large);
    assert!(sw < mw && mw < lw && sh < mh && mh < lh);
    let (cw, ch, cr) = camera_dims(CameraShape::Circle, CameraSize::Medium);
    assert_eq!(cw, ch); // square
    assert_eq!(cr, cw * 0.5); // full radius → circle
}
