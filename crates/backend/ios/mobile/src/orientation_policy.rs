//! Which orientations the robot's `set_orientation` asks UIKit for
//! (`imp/orientation.rs`). Un-gated so the choice is tested on any host.

use runtime_vocabulary::caps::ScreenOrientation;

/// `UIInterfaceOrientationMask` bits.
const MASK_PORTRAIT: u64 = 1 << 1;
const MASK_LANDSCAPE_LEFT: u64 = 1 << 4;
const MASK_LANDSCAPE_RIGHT: u64 = 1 << 3;

/// The mask a request for `orientation` asks for. Portrait is upright only:
/// an iPhone's controllers don't allow upside-down by default, and a mask
/// with nothing allowed is refused. Landscape is either side, UIKit's pick.
pub(crate) fn mask(orientation: ScreenOrientation) -> u64 {
    match orientation {
        ScreenOrientation::Portrait => MASK_PORTRAIT,
        ScreenOrientation::Landscape => MASK_LANDSCAPE_LEFT | MASK_LANDSCAPE_RIGHT,
    }
}

/// Whether the app can turn its own interface. An iPad app that supports
/// multitasking (Split View, Stage Manager: every app without
/// `UIRequiresFullScreen`) follows the device's orientation, and iPadOS
/// ignores its geometry requests without an error; only a full-screen iPad
/// app, and any iPhone app, can choose. Where it can't, the backend reports
/// `set_orientation` unsupported and `idealyst test` turns the simulator.
pub(crate) fn app_can_turn(is_pad: bool, requires_full_screen: bool) -> bool {
    !is_pad || requires_full_screen
}

#[cfg(test)]
mod tests {
    use super::*;

    /// REGRESSION (robot-e2e demo, iPad Air row): the app asked iPadOS for
    /// landscape, nothing turned and no error came back, and the test
    /// waited out its timeout. A multitasking iPad app can't turn itself.
    #[test]
    fn regression_a_multitasking_ipad_app_cannot_turn_itself() {
        assert!(!app_can_turn(true, false));
        assert!(app_can_turn(true, true), "a full-screen iPad app chooses");
        assert!(app_can_turn(false, false), "an iPhone app chooses");
    }

    /// The masks are UIKit's: `UIInterfaceOrientationMaskPortrait` (2) and
    /// `UIInterfaceOrientationMaskLandscape` (24).
    #[test]
    fn the_requested_masks_are_uikits() {
        assert_eq!(mask(ScreenOrientation::Portrait), 2);
        assert_eq!(mask(ScreenOrientation::Landscape), 24);
    }
}
