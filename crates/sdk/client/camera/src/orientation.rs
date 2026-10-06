//! Interface orientation + camera position → capture-connection rotation.
//!
//! The pure half of the iOS backend's orientation handling (`apple.rs` owns
//! the AVFoundation/UIKit wiring). Kept free of Obj-C so every orientation ×
//! camera combination is unit-tested on the host.
//!
//! **Why a rotation at all.** An iPhone/iPad camera sensor is mounted
//! landscape, so an un-rotated capture connection hands out buffers that are
//! only upright in one landscape orientation. The connection has to follow
//! the *interface* orientation (what the user sees as "up" on screen), or
//! frames come out 90° / 180° off. Pinning Portrait once at open — what the
//! backend did up to 1.6.0 — was only right for a portrait-held phone; on an
//! iPad in landscape both the preview and the CPU frames came out rotated
//! 90°. On web the browser does this for us (`getUserMedia` frames are
//! always upright), so following the interface orientation is what makes the
//! two backends converge.
//!
//! **Two AVFoundation APIs.**
//! - iOS 16: `AVCaptureConnection.videoOrientation` — an
//!   `AVCaptureVideoOrientation`. Its raw values are numbered exactly like
//!   `UIInterfaceOrientation` (Portrait 1, PortraitUpsideDown 2,
//!   LandscapeRight 3, LandscapeLeft 4 — both name the landscape cases by
//!   the side the home button sits on), and AVFoundation applies the camera
//!   position itself, so the mapping is the identity for both cameras.
//! - iOS 17+: `videoRotationAngle` — degrees clockwise relative to the
//!   *sensor's* native orientation. That makes it camera-dependent:
//!   * back camera: Portrait 90, LandscapeRight 0, UpsideDown 270,
//!     LandscapeLeft 180;
//!   * front camera: the device rotation reads the opposite way round from
//!     the sensor's side, so the landscape cases swap — LandscapeRight 180,
//!     LandscapeLeft 0 (Portrait / UpsideDown unchanged). These match
//!     `AVCaptureDevice.RotationCoordinator` on iPhone 16 and earlier.
//!   * Some sensors are mounted differently (the iPhone 17 Pro front camera
//!     reports Portrait = 0). So the table is expressed relative to the
//!     *portrait angle*, which `apple.rs` calibrates per connection by asking
//!     AVFoundation (set `videoOrientation = Portrait`, read back
//!     `videoRotationAngle`); [`STANDARD_PORTRAIT_ANGLE`] is the fallback.
//!
//! **Mirroring.** The web backend publishes the raw `getUserMedia` stream:
//! the front camera is *not* mirrored (text held up to it reads correctly),
//! for both the `<video>` display and the canvas RGBA readback. iOS matches
//! that: the connection's automatic mirroring is turned off and
//! `videoMirrored = NO` for both cameras, so `apple.rs` and `web.rs` hand out
//! the same image. A mirror about the output's vertical axis keeps an upright
//! image upright, so the rotation table does not depend on it.

/// `UIInterfaceOrientation` — the on-screen "up" the user sees. Unknown (0)
/// has no variant: there is nothing to follow, so the caller keeps the last
/// rotation it applied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InterfaceOrientation {
    Portrait,
    PortraitUpsideDown,
    /// Home button (or the bottom edge) on the left.
    LandscapeLeft,
    /// Home button (or the bottom edge) on the right.
    LandscapeRight,
}

impl InterfaceOrientation {
    /// From a raw `UIInterfaceOrientation` (`NSInteger`). `None` for
    /// `UIInterfaceOrientationUnknown` (0) or anything out of range.
    pub(crate) fn from_raw(raw: isize) -> Option<Self> {
        match raw {
            1 => Some(Self::Portrait),
            2 => Some(Self::PortraitUpsideDown),
            3 => Some(Self::LandscapeRight),
            4 => Some(Self::LandscapeLeft),
            _ => None,
        }
    }
}

/// The capturing camera's `AVCaptureDevicePosition`, as far as rotation is
/// concerned. `Unspecified` (an external camera) rotates like the back one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CameraPosition {
    Back,
    Front,
}

impl CameraPosition {
    /// From a raw `AVCaptureDevicePosition` (`NSInteger`): 2 is Front;
    /// Back (1) and Unspecified (0) rotate as the back camera.
    pub(crate) fn from_raw(raw: isize) -> Self {
        if raw == 2 {
            Self::Front
        } else {
            Self::Back
        }
    }
}

/// `videoRotationAngle` for Portrait on a standard landscape-mounted sensor
/// (every iPad, and iPhones through the 16 for both cameras).
pub(crate) const STANDARD_PORTRAIT_ANGLE: f64 = 90.0;

/// What to set on an `AVCaptureConnection` so its buffers come out upright.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CaptureRotation {
    /// `videoRotationAngle` (iOS 17+), degrees clockwise in `[0, 360)`.
    pub(crate) rotation_angle: f64,
    /// `AVCaptureVideoOrientation` raw value (iOS 16 fallback).
    pub(crate) video_orientation: isize,
    /// `videoMirrored`. Always `false` — matches the web backend.
    pub(crate) mirrored: bool,
}

/// Map the interface orientation + camera position onto the connection's
/// rotation. `portrait_angle` is the connection's calibrated Portrait angle
/// ([`STANDARD_PORTRAIT_ANGLE`] when it couldn't be calibrated).
pub(crate) fn capture_rotation(
    orientation: InterfaceOrientation,
    position: CameraPosition,
    portrait_angle: f64,
) -> CaptureRotation {
    use InterfaceOrientation::*;
    // Clockwise turn from Portrait. Rotating the interface to LandscapeRight
    // (device turned counter-clockwise) un-rotates the back camera by 90°;
    // the front camera faces the other way, so it turns the other way.
    let from_portrait = match (orientation, position) {
        (Portrait, _) => 0.0,
        (PortraitUpsideDown, _) => 180.0,
        (LandscapeRight, CameraPosition::Back) | (LandscapeLeft, CameraPosition::Front) => -90.0,
        (LandscapeLeft, CameraPosition::Back) | (LandscapeRight, CameraPosition::Front) => 90.0,
    };
    let video_orientation = match orientation {
        Portrait => 1,
        PortraitUpsideDown => 2,
        LandscapeRight => 3,
        LandscapeLeft => 4,
    };
    CaptureRotation {
        rotation_angle: normalize_degrees(portrait_angle + from_portrait),
        video_orientation,
        mirrored: false,
    }
}

/// Accept a calibrated Portrait angle read back from AVFoundation only when
/// it is a right angle (what every real sensor mounting is); anything else
/// means the readback didn't work and the caller falls back to
/// [`STANDARD_PORTRAIT_ANGLE`].
pub(crate) fn calibrated_portrait_angle(read_back: f64) -> Option<f64> {
    if !read_back.is_finite() {
        return None;
    }
    let normalized = normalize_degrees(read_back);
    let quarter_turns = (normalized / 90.0).round();
    // A CGFloat readback of a right angle is exact; the tolerance only
    // absorbs float noise, it never rounds a real non-right angle.
    if (normalized - quarter_turns * 90.0).abs() < 1e-6 {
        Some(normalize_degrees(quarter_turns * 90.0))
    } else {
        None
    }
}

fn normalize_degrees(angle: f64) -> f64 {
    let a = angle.rem_euclid(360.0);
    // rem_euclid can return 360.0 itself for tiny negative inputs.
    if a >= 360.0 {
        0.0
    } else {
        a
    }
}

#[cfg(test)]
mod tests {
    //! Regression tests for "iPad in landscape: preview + CPU frames rotated
    //! 90°" (camera 1.6.0 pinned `AVCaptureVideoOrientationPortrait` once at
    //! open). The AVFoundation/UIKit wiring in `apple.rs` can't run in CI —
    //! there is no capture hardware on a host or the iOS Simulator (which
    //! uses `sim_camera.rs`), and interface rotation needs a live
    //! `UIWindowScene` — so the mapping it applies is tested here instead.
    use super::*;
    use CameraPosition::*;
    use InterfaceOrientation::*;

    const ALL: [InterfaceOrientation; 4] = [Portrait, PortraitUpsideDown, LandscapeLeft, LandscapeRight];

    fn angle(o: InterfaceOrientation, p: CameraPosition) -> f64 {
        capture_rotation(o, p, STANDARD_PORTRAIT_ANGLE).rotation_angle
    }

    #[test]
    fn regression_ipad_landscape_back_camera_not_pinned_portrait() {
        // The reported bug: landscape got Portrait's rotation.
        assert_eq!(angle(LandscapeLeft, Back), 180.0);
        assert_eq!(angle(LandscapeRight, Back), 0.0);
        assert_ne!(capture_rotation(LandscapeLeft, Back, 90.0).video_orientation, 1);
        assert_ne!(capture_rotation(LandscapeRight, Back, 90.0).video_orientation, 1);
    }

    #[test]
    fn back_camera_rotation_angles_all_orientations() {
        assert_eq!(angle(Portrait, Back), 90.0);
        assert_eq!(angle(LandscapeRight, Back), 0.0);
        assert_eq!(angle(PortraitUpsideDown, Back), 270.0);
        assert_eq!(angle(LandscapeLeft, Back), 180.0);
    }

    #[test]
    fn front_camera_rotation_angles_all_orientations() {
        assert_eq!(angle(Portrait, Front), 90.0);
        assert_eq!(angle(LandscapeRight, Front), 180.0);
        assert_eq!(angle(PortraitUpsideDown, Front), 270.0);
        assert_eq!(angle(LandscapeLeft, Front), 0.0);
    }

    #[test]
    fn video_orientation_fallback_matches_interface_numbering_for_both_cameras() {
        for position in [Back, Front] {
            assert_eq!(capture_rotation(Portrait, position, 90.0).video_orientation, 1);
            assert_eq!(capture_rotation(PortraitUpsideDown, position, 90.0).video_orientation, 2);
            assert_eq!(capture_rotation(LandscapeRight, position, 90.0).video_orientation, 3);
            assert_eq!(capture_rotation(LandscapeLeft, position, 90.0).video_orientation, 4);
        }
        // The fallback value is the raw UIInterfaceOrientation, round-trip.
        for raw in 1..=4 {
            let o = InterfaceOrientation::from_raw(raw).unwrap();
            assert_eq!(capture_rotation(o, Back, 90.0).video_orientation, raw);
        }
    }

    #[test]
    fn never_mirrored_matching_web_backend() {
        for o in ALL {
            for p in [Back, Front] {
                assert!(!capture_rotation(o, p, STANDARD_PORTRAIT_ANGLE).mirrored);
            }
        }
    }

    #[test]
    fn rotated_sensor_portrait_angle_shifts_every_orientation() {
        // iPhone 17 Pro front camera: Portrait = 0 (RotationCoordinator
        // reports 0 / 90 LandscapeLeft-device / 180 / 270).
        assert_eq!(capture_rotation(Portrait, Front, 0.0).rotation_angle, 0.0);
        assert_eq!(capture_rotation(LandscapeRight, Front, 0.0).rotation_angle, 90.0);
        assert_eq!(capture_rotation(PortraitUpsideDown, Front, 0.0).rotation_angle, 180.0);
        assert_eq!(capture_rotation(LandscapeLeft, Front, 0.0).rotation_angle, 270.0);
        // Angles always land in [0, 360).
        for o in ALL {
            for p in [Back, Front] {
                for portrait in [0.0, 90.0, 180.0, 270.0] {
                    let a = capture_rotation(o, p, portrait).rotation_angle;
                    assert!((0.0..360.0).contains(&a), "{o:?} {p:?} {portrait} -> {a}");
                }
            }
        }
    }

    #[test]
    fn raw_decoding() {
        assert_eq!(InterfaceOrientation::from_raw(0), None);
        assert_eq!(InterfaceOrientation::from_raw(5), None);
        assert_eq!(InterfaceOrientation::from_raw(3), Some(LandscapeRight));
        assert_eq!(InterfaceOrientation::from_raw(4), Some(LandscapeLeft));
        assert_eq!(CameraPosition::from_raw(2), Front);
        assert_eq!(CameraPosition::from_raw(1), Back);
        assert_eq!(CameraPosition::from_raw(0), Back);
    }

    #[test]
    fn calibration_accepts_only_right_angles() {
        assert_eq!(calibrated_portrait_angle(90.0), Some(90.0));
        assert_eq!(calibrated_portrait_angle(0.0), Some(0.0));
        assert_eq!(calibrated_portrait_angle(450.0), Some(90.0));
        assert_eq!(calibrated_portrait_angle(-90.0), Some(270.0));
        assert_eq!(calibrated_portrait_angle(360.0), Some(0.0));
        assert_eq!(calibrated_portrait_angle(45.0), None);
        assert_eq!(calibrated_portrait_angle(f64::NAN), None);
    }
}
