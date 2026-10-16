//! Windows geolocation — WinRT `Windows.Devices.Geolocation`.
//!
//! - **Authorization** is Windows' location privacy setting (Settings ›
//!   Privacy › Location). `crate::current` asks the `permissions` SDK first,
//!   which shows Windows' "Let Windows and apps access your location?"
//!   prompt when undecided; `Geolocator::RequestAccessAsync` here confirms,
//!   and `Denied` maps to [`LocationError::NotAuthorized`].
//! - **[`current_fix`]** — `GetGeopositionAsync` on a high-accuracy
//!   `Geolocator` (Windows fuses GPS, Wi-Fi and IP sources), awaited
//!   directly (WinRT async operations are Rust futures) and bounded at 30 s
//!   like the Linux backend. No fix (location off, no source, the timeout)
//!   → [`LocationError::Unavailable`].
//! - **[`start_watch`]** — the `PositionChanged` event, delivered on a
//!   thread-pool thread (the crate contract: the callback runs on the
//!   location thread and is `Send`). The handle's `Drop` removes the
//!   handler.

use windows::Devices::Geolocation::{
    GeolocationAccessStatus, Geolocator, Geoposition, PositionAccuracy, PositionChangedEventArgs,
};
use windows::Foundation::{TimeSpan, TypedEventHandler};

use crate::{BoxedCallback, LocationError, Position};

/// How long [`current_fix`] waits for a fix: 30 s, in 100 ns ticks (the
/// Linux backend's `FIX_TIMEOUT`).
const FIX_TIMEOUT_TICKS: i64 = 30 * 10_000_000;

/// 1601-01-01 → 1970-01-01, in WinRT `DateTime` ticks (100 ns).
const UNIX_EPOCH_TICKS: i64 = 116_444_736_000_000_000;

fn unavailable(e: windows::core::Error) -> LocationError {
    LocationError::Unavailable(format!("{e}"))
}

pub(crate) async fn current_fix() -> Result<Position, LocationError> {
    let access = Geolocator::RequestAccessAsync().map_err(unavailable)?.await.map_err(unavailable)?;
    if access == GeolocationAccessStatus::Denied {
        return Err(LocationError::NotAuthorized);
    }
    let locator = Geolocator::new().map_err(unavailable)?;
    let _ = locator.SetDesiredAccuracy(PositionAccuracy::High);
    // A fresh fix (max age 0), bounded like the Linux backend's.
    let fix = locator
        .GetGeopositionAsyncWithAgeAndTimeout(TimeSpan { Duration: 0 }, TimeSpan { Duration: FIX_TIMEOUT_TICKS })
        .map_err(unavailable)?
        .await
        .map_err(unavailable)?;
    position(&fix).map_err(unavailable)
}

/// A WinRT fix as a [`Position`].
fn position(fix: &Geoposition) -> windows::core::Result<Position> {
    let c = fix.Coordinate()?;
    let p = c.Point()?.Position()?;
    let opt = |r: windows::core::Result<windows::Foundation::IReference<f64>>| {
        r.ok().and_then(|v| v.Value().ok()).filter(|v| v.is_finite())
    };
    Ok(Position {
        latitude: p.Latitude,
        longitude: p.Longitude,
        accuracy_m: c.Accuracy()?,
        // An altitude of exactly 0 with no altitude accuracy is "unknown".
        altitude: opt(c.AltitudeAccuracy()).map(|_| p.Altitude),
        heading: opt(c.Heading()),
        speed: opt(c.Speed()),
        timestamp_ms: ticks_to_unix_ms(c.Timestamp()?.UniversalTime),
    })
}

fn ticks_to_unix_ms(ticks: i64) -> f64 {
    (ticks - UNIX_EPOCH_TICKS) as f64 / 10_000.0
}

/// Removes the `PositionChanged` handler on drop. Inert (no locator) when
/// the locator could not be created — `watch` never panics.
pub(crate) struct WatchHandle {
    registration: Option<(Geolocator, i64)>,
}

impl Drop for WatchHandle {
    fn drop(&mut self) {
        if let Some((locator, token)) = self.registration.take() {
            let _ = locator.RemovePositionChanged(token);
        }
    }
}

pub(crate) fn start_watch(callback: BoxedCallback) -> WatchHandle {
    let register = || -> windows::core::Result<(Geolocator, i64)> {
        let locator = Geolocator::new()?;
        locator.SetDesiredAccuracy(PositionAccuracy::High)?;
        // Report movement of a metre or more, at most once a second.
        locator.SetMovementThreshold(1.0)?;
        locator.SetReportInterval(1000)?;
        let handler = TypedEventHandler::<Geolocator, PositionChangedEventArgs>::new(move |_, args| {
            if let Some(args) = args.as_ref() {
                if let Ok(pos) = args.Position().and_then(|p| position(&p)) {
                    callback(pos);
                }
            }
            Ok(())
        });
        let token = locator.PositionChanged(&handler)?;
        Ok((locator, token))
    };
    WatchHandle { registration: register().ok() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn winrt_ticks_convert_to_unix_ms() {
        assert_eq!(ticks_to_unix_ms(UNIX_EPOCH_TICKS), 0.0);
        assert_eq!(ticks_to_unix_ms(UNIX_EPOCH_TICKS + 10_000 * 1500), 1500.0);
    }

    /// Windows used the NotSupported stub. Now the request reaches WinRT:
    /// a fix, or the machine's honest answer (location off → NotAuthorized,
    /// no source → Unavailable) — never NotSupported. The test VM has no
    /// location source, so which of the three arrives depends on its
    /// settings.
    #[test]
    fn regression_windows_location_reaches_the_platform() {
        // A multithreaded apartment: completions arrive on a pool thread.
        // In an app the UI thread's single-threaded apartment gets them
        // through its message pump, which a blocked test thread never runs.
        unsafe {
            let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
        }
        let result = pollster::block_on(current_fix());
        assert_ne!(result, Err(LocationError::NotSupported), "{result:?}");
        if let Ok(p) = result {
            assert!(p.latitude.abs() <= 90.0 && p.longitude.abs() <= 180.0 && p.accuracy_m >= 0.0, "{p:?}");
        }
        let watch = start_watch(Box::new(|_| {}));
        assert!(watch.registration.is_some(), "PositionChanged handler registered");
    }
}
