//! Which Android permissions a [`Permission`] stands for, and how they are
//! asked for. Platform-free, so host tests cover it; `android.rs` asks.

use crate::Permission;

/// Any one of these granted is the stage granted; all are asked for together.
pub(crate) type Stage = &'static [&'static str];

const FOREGROUND_LOCATION: Stage = &[
    "android.permission.ACCESS_FINE_LOCATION",
    "android.permission.ACCESS_COARSE_LOCATION",
];

/// The grants `permission` needs on a device at API level `sdk`, asked for one
/// after the other: all granted is the permission granted.
///
/// - **Location while in use** is precise or approximate, the user's choice.
///   Both are asked for at once: from Android 12 a request for
///   `ACCESS_FINE_LOCATION` alone is ignored, without a dialog.
/// - **Location always** is that first and the background grant second. From
///   Android 11 a request naming both is ignored, and the second one opens the
///   app's location settings. Below API 29 there is no background grant.
pub(crate) fn stages(permission: Permission, sdk: i32) -> &'static [Stage] {
    match permission {
        Permission::Notifications => &[&["android.permission.POST_NOTIFICATIONS"]],
        Permission::LocationWhenInUse => &[FOREGROUND_LOCATION],
        Permission::LocationAlways if sdk < 29 => &[FOREGROUND_LOCATION],
        Permission::LocationAlways => {
            &[FOREGROUND_LOCATION, &["android.permission.ACCESS_BACKGROUND_LOCATION"]]
        }
        Permission::Camera => &[&["android.permission.CAMERA"]],
        Permission::Microphone => &[&["android.permission.RECORD_AUDIO"]],
    }
}

/// The first stage that is not granted, if any.
pub(crate) fn first_missing(
    stages: &'static [Stage],
    granted: impl Fn(&str) -> bool,
) -> Option<Stage> {
    stages.iter().copied().find(|stage| !stage.iter().any(|p| granted(p)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FINE: &str = "android.permission.ACCESS_FINE_LOCATION";
    const COARSE: &str = "android.permission.ACCESS_COARSE_LOCATION";
    const BACKGROUND: &str = "android.permission.ACCESS_BACKGROUND_LOCATION";

    /// REGRESSION: fine location was asked for alone, which Android 12+
    /// ignores: no dialog, and the answer is a denial.
    #[test]
    fn regression_location_is_asked_for_fine_and_coarse_together() {
        let stages = stages(Permission::LocationWhenInUse, 34);
        assert_eq!(stages.len(), 1);
        assert_eq!(stages[0], [FINE, COARSE]);
    }

    /// REGRESSION: a user who chose "approximate" was reported as not having
    /// granted location.
    #[test]
    fn regression_approximate_location_is_a_grant() {
        let stages = stages(Permission::LocationWhenInUse, 34);
        assert_eq!(first_missing(stages, |p| p == COARSE), None);
        assert_eq!(first_missing(stages, |p| p == FINE), None);
        assert_eq!(first_missing(stages, |_| false), Some(stages[0]));
    }

    /// The background grant is a request of its own, after the foreground one.
    #[test]
    fn always_asks_for_the_foreground_grant_first() {
        let s = stages(Permission::LocationAlways, 34);
        assert_eq!(first_missing(s, |_| false), Some(FOREGROUND_LOCATION));
        assert_eq!(first_missing(s, |p| p == COARSE), Some([BACKGROUND].as_slice()));
        // Granted by policy without the foreground one: still not usable.
        assert_eq!(first_missing(s, |p| p == BACKGROUND), Some(FOREGROUND_LOCATION));
        assert_eq!(first_missing(s, |p| p == FINE || p == BACKGROUND), None);
    }

    /// REGRESSION: below Android 10 the background permission does not exist,
    /// so "always" could never be granted there.
    #[test]
    fn regression_always_is_the_foreground_grant_below_api_29() {
        assert_eq!(first_missing(stages(Permission::LocationAlways, 28), |p| p == FINE), None);
    }
}
