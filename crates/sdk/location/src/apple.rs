//! Apple geolocation backend (iOS / macOS / tvOS), via `CLLocationManager`.
//!
//! `CLLocationManager` reports fixes through its **delegate** — there's no
//! completion-block form — so both [`current_fix`] and [`start_watch`] install
//! a declared `CLLocationManagerDelegate` subclass and pin the manager +
//! delegate alive for as long as updates are wanted:
//!
//! - [`current_fix`] calls `requestLocation` (a single fix) and bridges the
//!   first `locationManager:didUpdateLocations:` (or `didFailWithError:`) into
//!   a [`oneshot`](crate::oneshot), then tears the manager down.
//! - [`start_watch`] calls `startUpdatingLocation` and forwards **every**
//!   `didUpdateLocations:` to the user callback; the returned `WatchHandle`
//!   calls `stopUpdatingLocation` and releases the manager on drop (the RAII
//!   contract — no `mem::forget`).
//!
//! The grant itself is NOT requested here — `crate::current` already awaited
//! `permissions::request(LocationWhenInUse)` before reaching this module, and
//! `permissions` owns the `CLLocationManager` authorization delegate. We only
//! read position data.
//!
//! VERIFICATION: compile-checked only — exercising it needs a real device /
//! simulator with the `NSLocationWhenInUseUsageDescription` (iOS) /
//! `NSLocationUsageDescription` (macOS) plist string. The delegate-class +
//! manager-lifetime structure mirrors the verified `permissions` /
//! `biometrics` SDKs; the lifetime invariants are documented inline.

use std::cell::Cell;
use std::ptr;
use std::sync::Arc;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObjectProtocol};
use objc2::{class, declare_class, msg_send, msg_send_id, mutability, ClassType, DeclaredClass};
use objc2_foundation::{MainThreadMarker, NSObject};

use crate::oneshot;
use crate::{BoxedCallback, LocationError, Position};

// Link CoreLocation so `CLLocationManager` registers for `class!()` lookup.
#[link(name = "CoreLocation", kind = "framework")]
extern "C" {}

// ---------------------------------------------------------------------------
// Reading a CLLocation* into our Position.
//
// `CLLocation` exposes `coordinate` (a `CLLocationCoordinate2D` struct of two
// `CLLocationDegrees` = f64), `horizontalAccuracy` / `altitude` (f64 metres),
// `course` / `speed` (f64; **negative means "invalid"** per CoreLocation), and
// `timestamp` (an `NSDate`). A negative accuracy means the fix is invalid.
// ---------------------------------------------------------------------------

/// `CLLocationCoordinate2D` — matches the C struct layout CoreLocation returns
/// by value from `-[CLLocation coordinate]`.
#[repr(C)]
#[derive(Clone, Copy)]
struct CLLocationCoordinate2D {
    latitude: f64,
    longitude: f64,
}

// Teach `msg_send!` to return this struct by value: it's a C struct of two
// `CLLocationDegrees` (f64). objc2 0.5 has no `derive(Encode)`, so we hand-roll
// the `Encoding::Struct` exactly as `objc2-foundation` does for `NSRange`.
unsafe impl objc2::encode::Encode for CLLocationCoordinate2D {
    const ENCODING: objc2::encode::Encoding = objc2::encode::Encoding::Struct(
        "CLLocationCoordinate2D",
        &[f64::ENCODING, f64::ENCODING],
    );
}

/// Map a non-null `CLLocation*` to a [`Position`], or `None` if the fix is
/// invalid (negative horizontal accuracy). `course`/`speed` are `< 0` when
/// CoreLocation has no valid value, mapped to `None`.
///
/// # Safety
/// `location` must be a valid `CLLocation*` (or null).
unsafe fn location_to_position(location: *mut AnyObject) -> Option<Position> {
    if location.is_null() {
        return None;
    }
    let coord: CLLocationCoordinate2D = msg_send![location, coordinate];
    let accuracy_m: f64 = msg_send![location, horizontalAccuracy];
    // A negative horizontalAccuracy means the coordinate is invalid.
    if accuracy_m < 0.0 {
        return None;
    }
    let altitude: f64 = msg_send![location, altitude];
    let vertical_accuracy: f64 = msg_send![location, verticalAccuracy];
    let course: f64 = msg_send![location, course];
    let speed: f64 = msg_send![location, speed];

    // `[timestamp timeIntervalSince1970]` → seconds since the Unix epoch (f64).
    let timestamp: *mut AnyObject = msg_send![location, timestamp];
    let secs_since_epoch: f64 = if timestamp.is_null() {
        0.0
    } else {
        msg_send![timestamp, timeIntervalSince1970]
    };

    Some(Position {
        latitude: coord.latitude,
        longitude: coord.longitude,
        accuracy_m,
        // verticalAccuracy < 0 marks altitude invalid.
        altitude: (vertical_accuracy >= 0.0).then_some(altitude),
        heading: (course >= 0.0).then_some(course),
        speed: (speed >= 0.0).then_some(speed),
        timestamp_ms: secs_since_epoch * 1000.0,
    })
}

/// The last `CLLocation*` from a `didUpdateLocations:` `NSArray` — CoreLocation
/// delivers newest-last, so we read the final element.
///
/// # Safety
/// `locations` must be a valid `NSArray<CLLocation*>*` (or null).
unsafe fn newest_location(locations: *mut AnyObject) -> *mut AnyObject {
    if locations.is_null() {
        return ptr::null_mut();
    }
    let count: usize = msg_send![locations, count];
    if count == 0 {
        return ptr::null_mut();
    }
    msg_send![locations, lastObject]
}

// ===========================================================================
// The delegate. One declared class drives both `current` (one-shot) and
// `watch` (continuous) — its ivar holds a `Mode` enum deciding what to do with
// each fix.
// ===========================================================================

enum DelegateMode {
    /// One-shot: send the first fix (or error) through the oneshot, once.
    Once(Cell<Option<oneshot::Sender<Result<Position, LocationError>>>>),
    /// Continuous: forward every fix to the user callback. `Arc` so the
    /// `Send`-bounded callback survives the delegate's `&self` methods.
    Watch(Arc<BoxedCallback>),
}

struct LocationDelegateIvars {
    mode: DelegateMode,
}

declare_class!(
    struct LocationDataDelegate;

    unsafe impl ClassType for LocationDataDelegate {
        type Super = NSObject;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "IdealystLocationDataDelegate";
    }

    impl DeclaredClass for LocationDataDelegate {
        type Ivars = LocationDelegateIvars;
    }

    unsafe impl NSObjectProtocol for LocationDataDelegate {}

    unsafe impl LocationDataDelegate {
        #[method(locationManager:didUpdateLocations:)]
        fn did_update_locations(&self, _manager: *mut AnyObject, locations: *mut AnyObject) {
            // The delegate fires on the run loop; a panic must NOT unwind
            // across the Obj-C frame — catch, log, abort.
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let pos = unsafe { location_to_position(newest_location(locations)) };
                if let Some(pos) = pos {
                    match &self.ivars().mode {
                        DelegateMode::Once(tx) => {
                            if let Some(tx) = tx.take() {
                                tx.send(Ok(pos));
                            }
                        }
                        DelegateMode::Watch(cb) => cb(pos),
                    }
                }
            }));
            if result.is_err() {
                eprintln!("location: panic in didUpdateLocations; aborting");
                std::process::abort();
            }
        }

        #[method(locationManager:didFailWithError:)]
        fn did_fail_with_error(&self, _manager: *mut AnyObject, error: *mut AnyObject) {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                // For a one-shot request, a failure resolves the oneshot. For
                // a watch, a transient error is ignored — CoreLocation keeps
                // trying and the next success forwards normally.
                if let DelegateMode::Once(tx) = &self.ivars().mode {
                    if let Some(tx) = tx.take() {
                        let msg = unsafe { error_description(error) };
                        tx.send(Err(LocationError::Unavailable(msg)));
                    }
                }
            }));
            if result.is_err() {
                eprintln!("location: panic in didFailWithError; aborting");
                std::process::abort();
            }
        }
    }
);

impl LocationDataDelegate {
    fn new(mtm: MainThreadMarker, mode: DelegateMode) -> Retained<Self> {
        let this = mtm
            .alloc::<Self>()
            .set_ivars(LocationDelegateIvars { mode });
        unsafe { msg_send_id![super(this), init] }
    }
}

/// `[[error localizedDescription] UTF8String]` → a `String`, best-effort.
///
/// # Safety
/// `error` must be a valid `NSError*` (or null).
unsafe fn error_description(error: *mut AnyObject) -> String {
    if error.is_null() {
        return "location error".into();
    }
    let desc: *mut AnyObject = msg_send![error, localizedDescription];
    if desc.is_null() {
        return "location error".into();
    }
    let cstr: *const std::os::raw::c_char = msg_send![desc, UTF8String];
    if cstr.is_null() {
        return "location error".into();
    }
    std::ffi::CStr::from_ptr(cstr)
        .to_string_lossy()
        .into_owned()
}

/// Allocate + init a `CLLocationManager`, returning a retained handle.
fn new_manager() -> Retained<NSObject> {
    unsafe {
        let raw: *mut AnyObject = msg_send![class!(CLLocationManager), alloc];
        let raw: *mut AnyObject = msg_send![raw, init];
        Retained::from_raw(raw.cast()).expect("CLLocationManager init returned nil")
    }
}

// ===========================================================================
// Public-to-the-crate entry points.
// ===========================================================================

pub(crate) async fn current_fix() -> Result<Position, LocationError> {
    // `requestLocation` and the delegate are main-thread-bound. Off the main
    // thread we can't safely build the manager; report unavailable rather than
    // risk a wrong-thread call. A host driving from its main run loop is fine.
    let Some(mtm) = MainThreadMarker::new() else {
        return Err(LocationError::Unavailable(
            "location must be requested from the main thread".into(),
        ));
    };

    let (tx, rx) = oneshot::channel::<Result<Position, LocationError>>(Err(
        LocationError::Unavailable("location request produced no fix".into()),
    ));

    let delegate = LocationDataDelegate::new(mtm, DelegateMode::Once(Cell::new(Some(tx))));
    let manager = new_manager();

    unsafe {
        let _: () = msg_send![&*manager, setDelegate: &*delegate];
        // `requestLocation` delivers exactly one fix (or one error) to the
        // delegate, then stops on its own.
        let _: () = msg_send![&*manager, requestLocation];
    }

    let result = rx.await;

    // Detach the delegate before dropping so no late callback targets a freed
    // delegate, then release manager + delegate (no leak, no `mem::forget`).
    unsafe {
        let _: () = msg_send![&*manager, setDelegate: ptr::null::<AnyObject>()];
    }
    drop(manager);
    drop(delegate);
    result
}

/// Holds the manager + delegate alive for the watch's lifetime; `Drop` stops
/// updates and releases them.
pub(crate) struct WatchHandle {
    // `Option` so `Drop` can take ownership and tear down in order.
    manager: Option<Retained<NSObject>>,
    delegate: Option<Retained<LocationDataDelegate>>,
}

impl Drop for WatchHandle {
    fn drop(&mut self) {
        if let Some(manager) = self.manager.take() {
            unsafe {
                let _: () = msg_send![&*manager, stopUpdatingLocation];
                // Detach before release so a late callback can't target the
                // freed delegate.
                let _: () = msg_send![&*manager, setDelegate: ptr::null::<AnyObject>()];
            }
            drop(manager);
        }
        self.delegate.take();
    }
}

pub(crate) fn start_watch(callback: BoxedCallback) -> WatchHandle {
    // Off the main thread we can't build the manager. Return an inert handle;
    // the callback simply never fires (consistent with the contract that a
    // grant + main-thread context are the caller's responsibility).
    let Some(mtm) = MainThreadMarker::new() else {
        return WatchHandle {
            manager: None,
            delegate: None,
        };
    };

    let delegate =
        LocationDataDelegate::new(mtm, DelegateMode::Watch(Arc::new(callback)));
    let manager = new_manager();

    unsafe {
        let _: () = msg_send![&*manager, setDelegate: &*delegate];
        let _: () = msg_send![&*manager, startUpdatingLocation];
    }

    WatchHandle {
        manager: Some(manager),
        delegate: Some(delegate),
    }
}
