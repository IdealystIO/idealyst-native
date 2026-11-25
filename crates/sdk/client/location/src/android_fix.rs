//! What the Android shim's answers mean. Platform-free, so host tests cover
//! it; `android.rs` owns the JNI on either side.

use crate::{LocationError, Position};

/// The immediate answer of `RustLocation.current` / `startWatch`: `Ok` when
/// the result follows through a callback. The codes are the shim's constants.
pub(crate) fn started(code: i32) -> Result<(), LocationError> {
    match code {
        0 => Ok(()),
        1 => Err(LocationError::NotAuthorized),
        2 => Err(LocationError::ServicesOff),
        3 => Err(LocationError::Unavailable("no location provider is on, and no fix is known".into())),
        other => Err(LocationError::Unavailable(format!("unknown answer {other} from RustLocation"))),
    }
}

/// A fix as the shim passes it: `NaN` where the `Location` has no value.
pub(crate) struct RawFix {
    pub latitude: f64,
    pub longitude: f64,
    pub accuracy: f64,
    pub altitude: f64,
    pub heading: f64,
    pub speed: f64,
    pub time_ms: i64,
}

impl From<RawFix> for Position {
    fn from(f: RawFix) -> Self {
        let opt = |v: f64| (!v.is_nan()).then_some(v);
        Position {
            latitude: f.latitude,
            longitude: f.longitude,
            // No accuracy: no confidence, rather than a perfect fix.
            accuracy_m: opt(f.accuracy).unwrap_or(f64::INFINITY),
            altitude: opt(f.altitude),
            heading: opt(f.heading),
            speed: opt(f.speed),
            timestamp_ms: f.time_ms as f64,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// "Location is off" and "no fix yet" are different answers.
    #[test]
    fn answers_map_to_distinct_errors() {
        assert_eq!(started(0), Ok(()));
        assert_eq!(started(1), Err(LocationError::NotAuthorized));
        assert_eq!(started(2), Err(LocationError::ServicesOff));
        assert!(matches!(started(3), Err(LocationError::Unavailable(_))));
        assert!(matches!(started(99), Err(LocationError::Unavailable(_))));
    }

    #[test]
    fn absent_fields_are_none_and_an_unknown_accuracy_is_not_zero() {
        let p: Position = RawFix {
            latitude: 1.0,
            longitude: 2.0,
            accuracy: f64::NAN,
            altitude: f64::NAN,
            heading: 0.0,
            speed: f64::NAN,
            time_ms: 1_700_000_000_000,
        }
        .into();
        assert_eq!((p.altitude, p.heading, p.speed), (None, Some(0.0), None));
        assert_eq!(p.accuracy_m, f64::INFINITY);
        assert_eq!(p.timestamp_ms, 1_700_000_000_000.0);
    }
}
