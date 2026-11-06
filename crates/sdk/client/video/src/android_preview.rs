//! Where an Android camera preview lands in its `TextureView`, as pure math.
//!
//! The Kotlin shim (`RustVideoFrameSink.kt`) asks for it through
//! `nativePreviewTransform` when the view's size or the display's rotation changes;
//! there is no JVM test setup in this repo, so the math lives in Rust and the shim
//! applies the answer: `setScale(scale_x, scale_y)` then
//! `postRotate(rotation_degrees)`, both about the view's center.
//!
//! ## What the view starts with
//!
//! Camera2 tags the buffers it sends a `SurfaceTexture` with a transform, and a
//! `TextureView` applies it and then stretches the result over the whole view:
//!
//! 1. **Rotated by the sensor mounting**, so the picture is upright for a device held
//!    in its natural orientation, whatever the display's rotation is now.
//! 2. **Mirrored if the lens faces front** (`OutputConfiguration.MIRROR_MODE_AUTO`,
//!    the only behaviour below API 33).
//! 3. **Stretched to the view**, aspect ratio lost.
//!
//! ## What the frames look like
//!
//! The framework's camera frames are upright for the current display and not
//! mirrored, on every backend (the `camera` README's "Orientation and mirroring", and
//! the formula in its `android_rotation`: back `sensor - display`, front
//! `sensor + display`). The preview has to show that same picture, so a check-in photo
//! holds what the person lined up. So the transform:
//!
//! - **Scales** each axis back to the picture's aspect ratio, for the requested fit.
//! - **Flips** a front camera's picture back. One mechanism on every API level, where
//!   `MIRROR_MODE_NONE` would be a second path, for API 33 and up only, that this
//!   would still have to carry for the rest.
//! - **Turns** it for the display: by `-display` for a back camera, `+display` for a
//!   front one, the two terms the frame formula adds to the sensor mounting.
//!
//! A cover fit scales past the view on one axis, and `TextureView` clips to its own
//! bounds, which is the crop. A contain fit leaves the rest transparent.

/// How to place a preview: see the module docs. `scale_x` is negative for a flip.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PreviewTransform {
    pub(crate) scale_x: f32,
    pub(crate) scale_y: f32,
    pub(crate) rotation_degrees: f32,
}

/// What a preview needs to know about its buffers, the display and the view.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PreviewPlacement {
    /// Buffer size, in the sensor's orientation.
    pub(crate) buffer: (f32, f32),
    /// `SENSOR_ORIENTATION`, degrees.
    pub(crate) sensor_orientation: i32,
    /// `LENS_FACING == FRONT`.
    pub(crate) front: bool,
    /// `Display.getRotation()`, a `Surface.ROTATION_*` constant (0..=3).
    pub(crate) display_rotation: i32,
    /// The view's size, in pixels.
    pub(crate) view: (f32, f32),
    /// Fill the view and crop, rather than fit inside it and letterbox.
    pub(crate) cover: bool,
}

/// The transform for `p`, or the identity for a view with no size yet.
pub(crate) fn preview_transform(p: PreviewPlacement) -> PreviewTransform {
    let (view_w, view_h) = p.view;
    let (buffer_w, buffer_h) = p.buffer;
    if view_w <= 0.0 || view_h <= 0.0 || buffer_w <= 0.0 || buffer_h <= 0.0 {
        return PreviewTransform { scale_x: 1.0, scale_y: 1.0, rotation_degrees: 0.0 };
    }
    // The picture upright in the device's natural orientation.
    let sideways = |degrees: i32| degrees.rem_euclid(180) == 90;
    let (natural_w, natural_h) = if sideways(p.sensor_orientation) {
        (buffer_h, buffer_w)
    } else {
        (buffer_w, buffer_h)
    };
    let display = p.display_rotation.rem_euclid(4) * 90;
    let rotation = if p.front { display } else { -display }.rem_euclid(360);
    // The picture as it finally stands in the view.
    let (shown_w, shown_h) = if sideways(rotation) {
        (natural_h, natural_w)
    } else {
        (natural_w, natural_h)
    };
    let scale = if p.cover {
        (view_w / shown_w).max(view_h / shown_h)
    } else {
        (view_w / shown_w).min(view_h / shown_h)
    };
    // The view stretched the picture's axes onto its own, and the rotation happens
    // after this scale, so these work in the picture's axes.
    let scale_x = scale * natural_w / view_w;
    let scale_y = scale * natural_h / view_h;
    PreviewTransform {
        scale_x: if p.front { -scale_x } else { scale_x },
        scale_y,
        rotation_degrees: rotation as f32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPS: f32 = 0.01;

    /// Android's `Matrix`: `setScale` about the view's center, then `postRotate` about
    /// it. Positive degrees turn clockwise, with y pointing down.
    fn apply(t: PreviewTransform, view: (f32, f32), point: (f32, f32)) -> (f32, f32) {
        let (cx, cy) = (view.0 / 2.0, view.1 / 2.0);
        let x = (point.0 - cx) * t.scale_x;
        let y = (point.1 - cy) * t.scale_y;
        let (sin, cos) = t.rotation_degrees.to_radians().sin_cos();
        (cx + x * cos - y * sin, cy + x * sin + y * cos)
    }

    /// A clockwise quarter-turn multiple of a unit-square point, as `android_yuv` does
    /// to the pixels of a CPU frame.
    fn turn(degrees: i32, (x, y): (f32, f32)) -> (f32, f32) {
        match degrees.rem_euclid(360) {
            0 => (x, y),
            90 => (1.0 - y, x),
            180 => (1.0 - x, 1.0 - y),
            270 => (y, 1.0 - x),
            other => panic!("not a right angle: {other}"),
        }
    }

    /// `camera::android_rotation::frame_rotation`, restated: the crate is private and
    /// this test's job is to hold the preview to the same picture.
    fn frame_rotation(sensor: i32, display: i32, front: bool) -> i32 {
        let display = display.rem_euclid(4) * 90;
        if front { sensor + display } else { sensor - display }.rem_euclid(360)
    }

    /// Where the picture's bounding box lands: (left, top, right, bottom).
    fn bounds(p: PreviewPlacement) -> (f32, f32, f32, f32) {
        let t = preview_transform(p);
        let corners = [(0.0, 0.0), (p.view.0, 0.0), (0.0, p.view.1), p.view];
        let mapped = corners.map(|c| apply(t, p.view, c));
        let left = mapped.iter().map(|c| c.0).fold(f32::MAX, f32::min);
        let right = mapped.iter().map(|c| c.0).fold(f32::MIN, f32::max);
        let top = mapped.iter().map(|c| c.1).fold(f32::MAX, f32::min);
        let bottom = mapped.iter().map(|c| c.1).fold(f32::MIN, f32::max);
        (left, top, right, bottom)
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < EPS
    }

    fn placement(sensor: i32, front: bool, display: i32, view: (f32, f32), cover: bool) -> PreviewPlacement {
        PreviewPlacement {
            buffer: (1920.0, 1080.0),
            sensor_orientation: sensor,
            front,
            display_rotation: display,
            view,
            cover,
        }
    }

    fn every_placement() -> Vec<PreviewPlacement> {
        let mut all = Vec::new();
        for sensor in [0, 90, 180, 270] {
            for front in [false, true] {
                for display in 0..4 {
                    for view in [(1920.0, 1200.0), (1200.0, 1920.0), (500.0, 500.0), (640.0, 360.0)] {
                        for cover in [false, true] {
                            all.push(placement(sensor, front, display, view, cover));
                        }
                    }
                }
            }
        }
        all
    }

    /// The kiosk's bug report would be "the person is upside down" or "text reads
    /// backwards" in the preview but not in the photo. Follow a point of the sensor's
    /// buffer down both paths: through what Camera2 and the view do, then this
    /// transform; and through the rotation baked into a CPU frame.
    #[test]
    fn regression_preview_shows_the_same_picture_as_the_cpu_frame() {
        for p in every_placement() {
            let t = preview_transform(p);
            let total = frame_rotation(p.sensor_orientation, p.display_rotation, p.front);
            // The CPU frame, placed centered in the view at the scale the fit chose.
            let frame = if total % 180 == 90 { (1080.0, 1920.0) } else { (1920.0, 1080.0) };
            let scale = if p.cover {
                (p.view.0 / frame.0).max(p.view.1 / frame.1)
            } else {
                (p.view.0 / frame.0).min(p.view.1 / frame.1)
            };
            for buffer_point in [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (0.3, 0.8)] {
                let (fx, fy) = turn(total, buffer_point);
                let expected = (
                    p.view.0 / 2.0 + (fx - 0.5) * frame.0 * scale,
                    p.view.1 / 2.0 + (fy - 0.5) * frame.1 * scale,
                );
                // Camera2: rotate by the sensor, mirror a front camera. The view:
                // stretch over its whole rect.
                let (nx, ny) = turn(p.sensor_orientation, buffer_point);
                let nx = if p.front { 1.0 - nx } else { nx };
                let shown = apply(t, p.view, (nx * p.view.0, ny * p.view.1));
                assert!(
                    close(shown.0, expected.0) && close(shown.1, expected.1),
                    "{p:?}\n{buffer_point:?}: preview {shown:?}, frame {expected:?}"
                );
            }
        }
    }

    #[test]
    fn cover_always_fills_the_view_and_contain_always_fits_inside_it() {
        for p in every_placement() {
            let (left, top, right, bottom) = bounds(p);
            if p.cover {
                assert!(left < EPS && top < EPS, "{p:?} {left} {top}");
                assert!(right > p.view.0 - EPS && bottom > p.view.1 - EPS, "{p:?} {right} {bottom}");
            } else {
                assert!(left > -EPS && top > -EPS, "{p:?} {left} {top}");
                assert!(right < p.view.0 + EPS && bottom < p.view.1 + EPS, "{p:?} {right} {bottom}");
            }
        }
    }

    #[test]
    fn the_fitted_edge_lands_exactly_on_the_view() {
        for p in every_placement() {
            let (left, top, right, bottom) = bounds(p);
            let along_x = close(right - left, p.view.0);
            let along_y = close(bottom - top, p.view.1);
            assert!(along_x || along_y, "{p:?}: one axis must match the view exactly");
        }
    }

    #[test]
    fn the_picture_stays_centered() {
        for p in every_placement() {
            let (left, top, right, bottom) = bounds(p);
            assert!(close((left + right) / 2.0, p.view.0 / 2.0), "{p:?}");
            assert!(close((top + bottom) / 2.0, p.view.1 / 2.0), "{p:?}");
        }
    }

    #[test]
    fn the_aspect_ratio_is_the_cameras() {
        for p in every_placement() {
            let (left, top, right, bottom) = bounds(p);
            let (w, h) = (right - left, bottom - top);
            let ratio = w.max(h) / w.min(h);
            assert!((ratio - 1920.0 / 1080.0).abs() < 0.001, "{p:?}: {ratio}");
        }
    }

    /// The textbook case, by hand: a back camera on a portrait phone turned to
    /// landscape, in a view the buffer's exact shape.
    #[test]
    fn matches_the_known_answer_for_a_phone_in_landscape() {
        let t = preview_transform(placement(90, false, 1, (1920.0, 1080.0), true));
        assert!((t.rotation_degrees - 270.0).abs() < EPS);
        assert!((t.scale_x - 1080.0 / 1920.0).abs() < EPS);
        assert!((t.scale_y - 1920.0 / 1080.0).abs() < EPS);
    }

    #[test]
    fn a_landscape_tablet_held_naturally_only_crops() {
        // Sensor mounted at 0, buffer already the view's way up: no turn.
        let t = preview_transform(placement(0, false, 0, (1920.0, 1200.0), true));
        assert!(close(t.rotation_degrees, 0.0));
        assert!((t.scale_x - 1200.0 / 1080.0).abs() < EPS, "{t:?}");
        assert!((t.scale_y - 1.0).abs() < EPS, "{t:?}");
    }

    #[test]
    fn only_a_front_camera_is_flipped_back() {
        for p in every_placement() {
            let flipped = preview_transform(p).scale_x < 0.0;
            assert_eq!(flipped, p.front, "{p:?}");
        }
    }

    #[test]
    fn an_empty_view_or_buffer_is_the_identity() {
        let none = PreviewTransform { scale_x: 1.0, scale_y: 1.0, rotation_degrees: 0.0 };
        assert_eq!(preview_transform(placement(90, true, 1, (0.0, 0.0), true)), none);
        let mut p = placement(90, true, 1, (100.0, 100.0), true);
        p.buffer = (0.0, 0.0);
        assert_eq!(preview_transform(p), none);
    }

    #[test]
    fn an_out_of_range_rotation_wraps() {
        let a = preview_transform(placement(90, false, 5, (500.0, 500.0), true));
        let b = preview_transform(placement(90, false, 1, (500.0, 500.0), true));
        assert_eq!(a, b);
    }
}
