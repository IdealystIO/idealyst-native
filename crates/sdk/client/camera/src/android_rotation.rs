//! Android: camera sensor mounting + display rotation → how far to rotate
//! each frame so it comes out upright and unmirrored.
//!
//! The pure half of the Android backend's orientation handling. The Kotlin
//! shim (`RustCamera2Helper.kt`) reads `SENSOR_ORIENTATION`, `LENS_FACING`
//! and the display's `Surface.ROTATION_*`, asks this function (through the
//! `nativeFrameRotation` JNI export in `android.rs`) for the angle, and bakes
//! that rotation into its YUV→RGBA conversion. It asks again whenever the
//! display rotates. The math lives here, in Rust, so it is unit-tested on the
//! host; there is no JVM test setup in this repo.
//!
//! **The formula** is the standard Camera2 one for an UNMIRRORED image (the
//! `JPEG_ORIENTATION` computation in the Camera2 docs):
//!
//! - back camera:  `(sensor - display + 360) % 360`
//! - front camera: `(sensor + display) % 360`
//!
//! `display` is the display rotation in degrees (`Surface.ROTATION_90` = 90:
//! the drawn content is turned 90° counter-clockwise, i.e. the device was
//! turned counter-clockwise). The front camera takes the opposite sign
//! because it faces the user: turning the device one way turns the scene the
//! other way in its sensor. (The older Camera1 `setDisplayOrientation`
//! recipe adds a `(360 - result)` step for the front camera — that step
//! un-mirrors a *mirrored* preview. Camera2 `ImageReader` frames are never
//! mirrored, and the web and iOS backends hand out unmirrored frames, so the
//! step is deliberately absent.)
//!
//! Up to camera 1.6.0 the shim rotated by `SENSOR_ORIENTATION` alone — the
//! `display = 0` case — so frames were upright only while the device was
//! held in its natural (portrait) orientation and came out 90° / 180° off
//! in every other one.

/// Clockwise rotation in degrees (0, 90, 180 or 270) to apply to a camera
/// frame so it is upright on screen.
///
/// - `sensor_orientation`: `CameraCharacteristics.SENSOR_ORIENTATION`
///   (degrees; a multiple of 90, normalized here defensively).
/// - `display_rotation`: `Display.getRotation()` — a `Surface.ROTATION_*`
///   constant (0..=3). Out-of-range values are taken modulo 4.
/// - `front`: `LENS_FACING == LENS_FACING_FRONT`. External cameras rotate
///   like the back one.
pub(crate) fn frame_rotation(sensor_orientation: i32, display_rotation: i32, front: bool) -> i32 {
    let sensor = sensor_orientation.rem_euclid(360);
    let display = display_rotation.rem_euclid(4) * 90;
    if front {
        (sensor + display).rem_euclid(360)
    } else {
        (sensor - display).rem_euclid(360)
    }
}

#[cfg(test)]
mod tests {
    //! Regression tests for "frames rotated 90° in landscape" on Android
    //! (camera 1.6.0 rotated by SENSOR_ORIENTATION only). The Camera2 /
    //! DisplayListener wiring in `RustCamera2Helper.kt` needs a device with a
    //! camera and a rotating display — CI has neither (and the repo has no
    //! JVM test harness) — so the rotation it applies is tested here.
    use super::frame_rotation;

    // `Surface.ROTATION_*`.
    const R0: i32 = 0;
    const R90: i32 = 1;
    const R180: i32 = 2;
    const R270: i32 = 3;

    // Typical phone mounting: back sensor 90, front sensor 270.
    const BACK_SENSOR: i32 = 90;
    const FRONT_SENSOR: i32 = 270;

    #[test]
    fn regression_landscape_not_rotated_by_sensor_alone() {
        // The reported bug: landscape got the portrait (sensor-only) angle.
        assert_ne!(frame_rotation(BACK_SENSOR, R90, false), BACK_SENSOR);
        assert_ne!(frame_rotation(BACK_SENSOR, R270, false), BACK_SENSOR);
        assert_ne!(frame_rotation(FRONT_SENSOR, R90, true), FRONT_SENSOR);
        assert_ne!(frame_rotation(FRONT_SENSOR, R270, true), FRONT_SENSOR);
    }

    #[test]
    fn back_camera_all_display_rotations() {
        assert_eq!(frame_rotation(BACK_SENSOR, R0, false), 90);
        assert_eq!(frame_rotation(BACK_SENSOR, R90, false), 0);
        assert_eq!(frame_rotation(BACK_SENSOR, R180, false), 270);
        assert_eq!(frame_rotation(BACK_SENSOR, R270, false), 180);
    }

    #[test]
    fn front_camera_all_display_rotations() {
        assert_eq!(frame_rotation(FRONT_SENSOR, R0, true), 270);
        assert_eq!(frame_rotation(FRONT_SENSOR, R90, true), 0);
        assert_eq!(frame_rotation(FRONT_SENSOR, R180, true), 90);
        assert_eq!(frame_rotation(FRONT_SENSOR, R270, true), 180);
    }

    #[test]
    fn front_and_back_turn_opposite_ways() {
        // Same sensor mounting, opposite facing: a display turn moves the two
        // angles in opposite directions (the front camera faces the user).
        for sensor in [0, 90, 180, 270] {
            for display in [R0, R90, R180, R270] {
                let back = frame_rotation(sensor, display, false);
                let front = frame_rotation(sensor, display, true);
                assert_eq!((back + front).rem_euclid(360), (2 * sensor).rem_euclid(360));
            }
        }
    }

    #[test]
    fn natural_orientation_is_sensor_only_for_both_cameras() {
        // The pre-fix behavior is still right while held naturally.
        for sensor in [0, 90, 180, 270] {
            assert_eq!(frame_rotation(sensor, R0, false), sensor);
            assert_eq!(frame_rotation(sensor, R0, true), sensor);
        }
    }

    #[test]
    fn landscape_native_device_sensor_zero() {
        // A landscape-natural tablet with a sensor mounted at 0.
        assert_eq!(frame_rotation(0, R0, false), 0);
        assert_eq!(frame_rotation(0, R90, false), 270);
        assert_eq!(frame_rotation(0, R90, true), 90);
    }

    #[test]
    fn always_a_right_angle_in_range() {
        for sensor in [-90, 0, 90, 180, 270, 360, 450] {
            for display in -1..=5 {
                for front in [false, true] {
                    let r = frame_rotation(sensor, display, front);
                    assert!(matches!(r, 0 | 90 | 180 | 270), "{sensor} {display} {front} -> {r}");
                }
            }
        }
    }
}
