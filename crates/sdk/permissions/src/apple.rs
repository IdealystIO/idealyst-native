//! Apple permission backend (iOS / macOS / tvOS), via the Obj-C runtime.
//!
//! Two frameworks, two callback shapes — both bridged to our `async fn`
//! through the crate's [`oneshot`](crate::oneshot) channel:
//!
//! - **Notifications** → `UNUserNotificationCenter`.
//!   `getNotificationSettingsWithCompletionHandler:` (status) and
//!   `requestAuthorizationWithOptions:completionHandler:` (request) each take
//!   a `void(^)(...)` block that fires on a private queue. We bridge the
//!   block to a oneshot, exactly like the `biometrics` SDK bridges
//!   `LAContext.evaluatePolicy:`.
//! - **Location** → `CLLocationManager`. `authorizationStatus` is a
//!   synchronous class read (no prompt). The *request* methods
//!   (`requestWhenInUseAuthorization` / `requestAlwaysAuthorization`) return
//!   `void` and deliver the result through the manager's **delegate**
//!   (`locationManagerDidChangeAuthorization:`), not a completion block. So
//!   we install a tiny declared delegate class that forwards the one
//!   delegate callback into a oneshot, and pin the manager + delegate alive
//!   until it fires (the manager owns no strong ref to a delegate).
//! - **Camera / Microphone** → `AVCaptureDevice`. `authorizationStatusForMediaType:`
//!   is a synchronous class read (no prompt); `requestAccessForMediaType:`
//!   surfaces the prompt and fires a `void(^)(BOOL)` completion block, bridged
//!   to the oneshot like the notifications block. Video uses the `"vide"` media
//!   type, audio the `"soun"` one. This grant code was relocated faithfully
//!   from the `camera` / `microphone` SDKs (which now delegate here); see
//!   `request_av_media`.
//!
//! VERIFICATION: compile-checked only — these paths need a real device /
//! simulator with the matching `Info.plist` usage strings to exercise. The
//! structure mirrors the verified `biometrics` / `camera` SDKs; the block
//! and delegate lifetime invariants are documented inline. The camera path
//! was macOS-hardware-verified in its prior `camera`-SDK home and should be
//! re-confirmed through this crate on a host/device run.

use std::cell::Cell;
use std::ptr;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, NSObjectProtocol};
use objc2::{class, declare_class, msg_send, msg_send_id, mutability, ClassType, DeclaredClass};
use objc2_foundation::{MainThreadMarker, NSObject, NSString};

use crate::oneshot;
use crate::{Permission, PermissionStatus};

// Link the frameworks so their classes register for `class!()` lookup. We
// use them only through the Obj-C runtime, so empty extern blocks suffice.
#[link(name = "UserNotifications", kind = "framework")]
extern "C" {}
#[link(name = "CoreLocation", kind = "framework")]
extern "C" {}
// AVFoundation hosts `AVCaptureDevice`, whose `authorizationStatusForMediaType:`
// / `requestAccessForMediaType:` drive the camera + microphone grants below.
#[link(name = "AVFoundation", kind = "framework")]
extern "C" {}

// UNAuthorizationStatus (UNNotificationSettings.h).
const UN_AUTH_NOT_DETERMINED: isize = 0;
const UN_AUTH_DENIED: isize = 1;
const UN_AUTH_AUTHORIZED: isize = 2;
const UN_AUTH_PROVISIONAL: isize = 3; // quiet notifications — treat as granted
const UN_AUTH_EPHEMERAL: isize = 4; // App Clip — treat as granted

// UNAuthorizationOptions bit flags (UNUserNotificationCenter.h).
const UN_OPT_BADGE: u64 = 1 << 0;
const UN_OPT_SOUND: u64 = 1 << 1;
const UN_OPT_ALERT: u64 = 1 << 2;

// CLAuthorizationStatus (CLLocationManager.h). NOTE: unlike
// `UNAuthorizationStatus` (an `NSInteger` → `isize`), `CLAuthorizationStatus`
// is a plain `int32_t`, so `authorizationStatus` MUST be read as `i32`.
// Reading it as `isize` makes objc2's runtime encoding check abort
// ("expected 'i', found 'q'") on the real `CLLocationManager`.
const CL_AUTH_NOT_DETERMINED: i32 = 0;
const CL_AUTH_RESTRICTED: i32 = 1;
const CL_AUTH_DENIED: i32 = 2;
const CL_AUTH_ALWAYS: i32 = 3; // authorizedAlways
const CL_AUTH_WHEN_IN_USE: i32 = 4; // authorizedWhenInUse

// AVAuthorizationStatus (AVCaptureDevice.h). IMPORTANT: unlike
// `CLAuthorizationStatus` (a plain `int32_t`), `AVAuthorizationStatus` IS an
// `NSInteger`, so `authorizationStatusForMediaType:` MUST be read as `i64`
// (== `isize` on 64-bit Apple targets). This is the width the relocated
// `camera` code used (`const AUTH_*: i64`) and it's correct — reading it as a
// narrower type would mis-decode objc2's runtime type check. NotDetermined is
// the implicit `0` (no named const; it's the fall-through arm below).
const AUTH_RESTRICTED: i64 = 1;
const AUTH_DENIED: i64 = 2;
const AUTH_AUTHORIZED: i64 = 3;

// `AVMediaTypeVideo` / `AVMediaTypeAudio` string values. The constants equal
// these literals, so we build them directly rather than linking the extern
// symbols — the same trick `camera` ("vide") and `microphone` ("soun") use.
const AV_MEDIA_TYPE_VIDEO: &str = "vide";
const AV_MEDIA_TYPE_AUDIO: &str = "soun";

pub(super) async fn status(permission: Permission) -> PermissionStatus {
    match permission {
        Permission::Notifications => notification_status().await,
        Permission::LocationWhenInUse | Permission::LocationAlways => location_status(),
        // Camera / Microphone authorization is `AVCaptureDevice`'s passive
        // `authorizationStatusForMediaType:` read — no prompt. Relocated here
        // from the `camera` / `microphone` SDKs so the grant lives in one
        // place; those SDKs now delegate to this crate.
        Permission::Camera => av_media_status(AV_MEDIA_TYPE_VIDEO),
        Permission::Microphone => av_media_status(AV_MEDIA_TYPE_AUDIO),
    }
}

pub(super) async fn request(permission: Permission) -> PermissionStatus {
    match permission {
        Permission::Notifications => request_notifications().await,
        Permission::LocationWhenInUse => request_location(false).await,
        Permission::LocationAlways => request_location(true).await,
        // `AVCaptureDevice requestAccessForMediaType:` surfaces the OS prompt
        // when undetermined (video for Camera, audio for Microphone) and
        // resolves through a completion block. This is the unified Apple grant
        // mechanism `camera` already proved on macOS hardware; `microphone`'s
        // iOS `AVAudioSession` *activation* stays in its capture path (it's
        // needed to make the input unit produce sound), but the record-grant
        // itself rides this same AVCaptureDevice audio request.
        Permission::Camera => request_av_media(AV_MEDIA_TYPE_VIDEO).await,
        Permission::Microphone => request_av_media(AV_MEDIA_TYPE_AUDIO).await,
    }
}

fn map_av_status(raw: i64) -> PermissionStatus {
    match raw {
        AUTH_AUTHORIZED => PermissionStatus::Granted,
        AUTH_DENIED => PermissionStatus::Denied,
        AUTH_RESTRICTED => PermissionStatus::Restricted,
        // NotDetermined (0) and any future value: a request may still prompt.
        _ => PermissionStatus::Undetermined,
    }
}

/// `+[AVCaptureDevice authorizationStatusForMediaType:]` — the passive status
/// read (no prompt, no device open). `media_type` is `"vide"` or `"soun"`.
fn av_media_status(media_type: &str) -> PermissionStatus {
    let media_type = NSString::from_str(media_type);
    let raw: i64 = unsafe {
        msg_send![class!(AVCaptureDevice), authorizationStatusForMediaType: &*media_type]
    };
    map_av_status(raw)
}

/// `+[AVCaptureDevice requestAccessForMediaType:completionHandler:]` — checks
/// the current status first (so a granted/denied state never re-prompts), then
/// surfaces the OS prompt for an undetermined one and bridges the completion
/// block to our oneshot. A faithful relocation of `camera`'s
/// `request_permission` video path, generalized over the media type.
async fn request_av_media(media_type: &str) -> PermissionStatus {
    // Already settled? Don't re-prompt.
    let current = av_media_status(media_type);
    if current != PermissionStatus::Undetermined {
        return current;
    }

    let media_type = NSString::from_str(media_type);
    let (tx, rx) = oneshot::channel::<bool>(false);
    let tx_cell = Cell::new(Some(tx));
    let block = RcBlock::new(move |granted: Bool| {
        // The system fires this on a private queue; catch any panic so an
        // unwind can't cross the Obj-C frame (UB) — log + abort instead.
        let result = std::panic::catch_unwind(|| granted.as_bool());
        match result {
            Ok(g) => {
                if let Some(tx) = tx_cell.take() {
                    tx.send(g);
                }
            }
            Err(_) => {
                eprintln!("permissions: panic in AVCaptureDevice access block; aborting");
                std::process::abort();
            }
        }
    });
    unsafe {
        let _: () = msg_send![
            class!(AVCaptureDevice),
            requestAccessForMediaType: &*media_type,
            completionHandler: &*block,
        ];
    }
    if rx.await {
        PermissionStatus::Granted
    } else {
        // The block fired `false`, or the sender dropped without firing —
        // either way the grant wasn't given.
        PermissionStatus::Denied
    }
}

fn map_un_status(raw: isize) -> PermissionStatus {
    match raw {
        UN_AUTH_NOT_DETERMINED => PermissionStatus::Undetermined,
        UN_AUTH_DENIED => PermissionStatus::Denied,
        UN_AUTH_AUTHORIZED | UN_AUTH_PROVISIONAL | UN_AUTH_EPHEMERAL => PermissionStatus::Granted,
        _ => PermissionStatus::Undetermined,
    }
}

fn map_cl_status(raw: i32) -> PermissionStatus {
    match raw {
        CL_AUTH_NOT_DETERMINED => PermissionStatus::Undetermined,
        CL_AUTH_RESTRICTED => PermissionStatus::Restricted,
        CL_AUTH_DENIED => PermissionStatus::Denied,
        CL_AUTH_ALWAYS | CL_AUTH_WHEN_IN_USE => PermissionStatus::Granted,
        _ => PermissionStatus::Undetermined,
    }
}

/// `[UNUserNotificationCenter currentNotificationCenter]
/// getNotificationSettingsWithCompletionHandler:]` — the block receives a
/// `UNNotificationSettings*`; we read `authorizationStatus`.
async fn notification_status() -> PermissionStatus {
    let (tx, rx) = oneshot::channel(PermissionStatus::Undetermined);
    let tx_cell = Cell::new(Some(tx));

    let block = RcBlock::new(move |settings: *mut AnyObject| {
        // The system fires this on a private queue; catch any panic so an
        // unwind can't cross the Obj-C frame (UB) — log + abort instead.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            if settings.is_null() {
                return PermissionStatus::Undetermined;
            }
            let raw: isize = msg_send![settings, authorizationStatus];
            map_un_status(raw)
        }));
        match result {
            Ok(status) => {
                if let Some(tx) = tx_cell.take() {
                    tx.send(status);
                }
            }
            Err(_) => {
                eprintln!("permissions: panic in notification-settings block; aborting");
                std::process::abort();
            }
        }
    });

    unsafe {
        let center: *mut AnyObject =
            msg_send![class!(UNUserNotificationCenter), currentNotificationCenter];
        if center.is_null() {
            return PermissionStatus::Unsupported;
        }
        let _: () = msg_send![center, getNotificationSettingsWithCompletionHandler: &*block];
    }
    rx.await
}

/// `requestAuthorizationWithOptions:completionHandler:` — prompts when
/// undetermined; the block yields `(BOOL granted, NSError*)`. We re-read the
/// settled status afterward so a "denied" reads as `Denied`, not just
/// `!granted`.
async fn request_notifications() -> PermissionStatus {
    let (tx, rx) = oneshot::channel::<bool>(false);
    let tx_cell = Cell::new(Some(tx));

    let block = RcBlock::new(move |granted: Bool, _error: *mut AnyObject| {
        let result = std::panic::catch_unwind(|| granted.as_bool());
        match result {
            Ok(g) => {
                if let Some(tx) = tx_cell.take() {
                    tx.send(g);
                }
            }
            Err(_) => {
                eprintln!("permissions: panic in notification-auth block; aborting");
                std::process::abort();
            }
        }
    });

    let options = UN_OPT_ALERT | UN_OPT_SOUND | UN_OPT_BADGE;
    unsafe {
        let center: *mut AnyObject =
            msg_send![class!(UNUserNotificationCenter), currentNotificationCenter];
        if center.is_null() {
            return PermissionStatus::Unsupported;
        }
        let _: () = msg_send![
            center,
            requestAuthorizationWithOptions: options,
            completionHandler: &*block,
        ];
    }
    let _granted = rx.await;
    // Re-read the authoritative settled status (granted vs provisional vs
    // denied) rather than collapsing the bool.
    notification_status().await
}

/// `[CLLocationManager authorizationStatus]` — a synchronous class method,
/// no prompt, no manager instance required on modern SDKs (it's also an
/// instance method; the class method is the no-side-effect read).
fn location_status() -> PermissionStatus {
    unsafe {
        let raw: i32 = msg_send![class!(CLLocationManager), authorizationStatus];
        map_cl_status(raw)
    }
}

// =========================================================================
// CLLocationManager delegate bridge.
//
// `requestWhenInUseAuthorization` / `requestAlwaysAuthorization` return void
// and report the outcome through `locationManagerDidChangeAuthorization:`.
// We install this minimal delegate, which forwards that single callback into
// the oneshot, then drops its retained manager+self (releasing the strong
// cycle) so nothing leaks. The manager is `MainThreadOnly`.
// =========================================================================

struct LocationDelegateIvars {
    // `Cell<Option<_>>` so the `&self` delegate method can take the sender
    // out on its single fire. The oneshot `Sender` is `Send`.
    tx: Cell<Option<oneshot::Sender<PermissionStatus>>>,
}

declare_class!(
    struct LocationAuthDelegate;

    unsafe impl ClassType for LocationAuthDelegate {
        type Super = NSObject;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "IdealystPermissionsLocationDelegate";
    }

    impl DeclaredClass for LocationAuthDelegate {
        type Ivars = LocationDelegateIvars;
    }

    unsafe impl NSObjectProtocol for LocationAuthDelegate {}

    unsafe impl LocationAuthDelegate {
        // Modern (iOS 14+ / macOS 11+) single-arg delegate callback.
        #[method(locationManagerDidChangeAuthorization:)]
        fn did_change_authorization(&self, manager: *mut AnyObject) {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let raw: i32 = if manager.is_null() {
                    CL_AUTH_NOT_DETERMINED
                } else {
                    unsafe { msg_send![manager, authorizationStatus] }
                };
                map_cl_status(raw)
            }));
            match result {
                Ok(status) => {
                    // Ignore the transient `Undetermined` the delegate may
                    // emit before the user has actually answered the prompt;
                    // wait for a settled state.
                    if status != PermissionStatus::Undetermined {
                        if let Some(tx) = self.ivars().tx.take() {
                            tx.send(status);
                        }
                    }
                }
                Err(_) => {
                    eprintln!("permissions: panic in location delegate; aborting");
                    std::process::abort();
                }
            }
        }
    }
);

impl LocationAuthDelegate {
    fn new(mtm: MainThreadMarker, tx: oneshot::Sender<PermissionStatus>) -> Retained<Self> {
        let this = mtm
            .alloc::<Self>()
            .set_ivars(LocationDelegateIvars { tx: Cell::new(Some(tx)) });
        unsafe { msg_send_id![super(this), init] }
    }
}

/// Allocate a `CLLocationManager`, attach the delegate, and call the
/// appropriate request method. The delegate + manager are retained inside
/// the spawned bridge and released after the oneshot resolves, so neither
/// leaks (no `mem::forget`).
async fn request_location(always: bool) -> PermissionStatus {
    // If already settled (granted/denied/restricted), don't re-prompt.
    let current = location_status();
    if current != PermissionStatus::Undetermined {
        return current;
    }

    // Location prompts must be driven from the main thread (the delegate is
    // `MainThreadOnly`). If we're off it, we can't safely build the manager
    // here; report the current status rather than risking a wrong-thread
    // call. A host driving this from its main run loop satisfies the marker.
    let Some(mtm) = MainThreadMarker::new() else {
        return current;
    };

    let (tx, rx) = oneshot::channel(PermissionStatus::Undetermined);

    // The delegate and manager must outlive the async request. We retain
    // both here and move them into the future so they're released exactly
    // when it resolves — see the lifetime note above.
    let delegate = LocationAuthDelegate::new(mtm, tx);

    let manager: Retained<NSObject> = unsafe {
        let raw: *mut AnyObject = msg_send![class!(CLLocationManager), alloc];
        let raw: *mut AnyObject = msg_send![raw, init];
        // Take ownership of the +1 from alloc/init.
        Retained::from_raw(raw.cast()).expect("CLLocationManager init returned nil")
    };

    unsafe {
        let _: () = msg_send![&*manager, setDelegate: &*delegate];
        if always {
            let _: () = msg_send![&*manager, requestAlwaysAuthorization];
        } else {
            let _: () = msg_send![&*manager, requestWhenInUseAuthorization];
        }
    }

    let status = rx.await;

    // Detach the delegate before dropping so no late callback targets a
    // freed delegate, then release manager + delegate.
    unsafe {
        let _: () = msg_send![&*manager, setDelegate: ptr::null::<AnyObject>()];
    }
    drop(manager);
    drop(delegate);
    status
}
