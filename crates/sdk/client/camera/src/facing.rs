//! Does the camera a backend opened satisfy the requested [`CameraFacing`]?
//!
//! The contract on [`CameraFacing`] is that a `Front`/`Back` the device
//! can't satisfy is [`CameraError::NoCamera`](crate::CameraError::NoCamera),
//! never a silent open of the other camera. iOS/macOS and Android resolve
//! facing up front from the OS's lens metadata. Web and desktop Linux can't:
//! the browser treats `facingMode` as a preference and falls back to
//! whatever camera exists, and V4L2 reports no facing at all. Those backends
//! check what they actually opened against this rule instead.
//!
//! A camera that reports NO facing counts as user-facing. That is what a
//! desktop webcam is (a laptop's built-in camera, a monitor or USB webcam
//! pointed at the user), and it matches AVFoundation, where a Mac has no
//! back-position camera. So `Back` on a laptop is `NoCamera` on every
//! backend, and `Front` opens the webcam. Kept free of platform code so it
//! is host-tested.

use crate::CameraFacing;

/// What a backend could learn about the facing of the camera it opened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ObservedFacing {
    /// Reported as user-facing (web `facingMode: "user"`, code 1).
    User,
    /// Reported as world-facing (web `facingMode: "environment"`, code 2).
    Environment,
    /// Reported as something else (web `"left"` / `"right"` side cameras).
    Other,
    /// The device reports no facing (desktop webcams, V4L2).
    Unreported,
}

impl ObservedFacing {
    /// Map the web backend's `facingMode` code: 0 = none reported, 1 =
    /// `"user"`, 2 = `"environment"`, anything else = another value
    /// (`"left"`/`"right"`). The JS side (`web.rs`'s `js_track_facing`)
    /// produces it; a code because the JS boundary returns numbers.
    #[cfg_attr(not(any(test, target_arch = "wasm32")), allow(dead_code))]
    pub(crate) fn from_web_code(code: u32) -> Self {
        match code {
            0 => ObservedFacing::Unreported,
            1 => ObservedFacing::User,
            2 => ObservedFacing::Environment,
            _ => ObservedFacing::Other,
        }
    }
}

/// Whether a camera observed as `observed` satisfies `requested`.
/// `Default` takes whatever the platform opened.
pub(crate) fn satisfies(requested: CameraFacing, observed: ObservedFacing) -> bool {
    match requested {
        CameraFacing::Default => true,
        CameraFacing::Front => {
            matches!(observed, ObservedFacing::User | ObservedFacing::Unreported)
        }
        CameraFacing::Back => matches!(observed, ObservedFacing::Environment),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// REGRESSION: on a laptop with one
    /// camera, `open(CameraConfig::default().back())` on web silently opened
    /// the user-facing webcam, so a front/back switch did nothing and the app
    /// couldn't tell. A webcam that reports no facing must not satisfy `Back`.
    #[test]
    fn regression_back_is_not_satisfied_by_a_desktop_webcam() {
        assert!(!satisfies(CameraFacing::Back, ObservedFacing::Unreported));
        assert!(!satisfies(CameraFacing::Back, ObservedFacing::User));
        assert!(!satisfies(CameraFacing::Back, ObservedFacing::from_web_code(0)));
    }

    /// The same webcam DOES satisfy `Front` (it faces the user) and
    /// `Default`, so a laptop's camera keeps opening for both.
    #[test]
    fn desktop_webcam_satisfies_front_and_default() {
        assert!(satisfies(CameraFacing::Front, ObservedFacing::Unreported));
        assert!(satisfies(CameraFacing::Default, ObservedFacing::Unreported));
    }

    /// Reported facings must match exactly; a side camera matches neither.
    #[test]
    fn reported_facing_must_match() {
        let env = ObservedFacing::from_web_code(2);
        let user = ObservedFacing::from_web_code(1);
        let side = ObservedFacing::from_web_code(3);
        assert!(satisfies(CameraFacing::Back, env));
        assert!(!satisfies(CameraFacing::Front, env));
        assert!(satisfies(CameraFacing::Front, user));
        assert!(!satisfies(CameraFacing::Front, side));
        assert!(!satisfies(CameraFacing::Back, side));
        assert!(satisfies(CameraFacing::Default, side));
    }
}
