//! **Device management** — Android device-owner policy and a small iOS
//! counterpart.
//!
//! An app that is the Android *device owner* can lock another app (a kiosk)
//! onto the screen, keep it there across reboots and crashes, and install,
//! update and roll back APKs silently, with no MDM and no app store. This
//! crate exposes those `DevicePolicyManager`, `PackageInstaller` and lock-task
//! calls to Rust.
//!
//! ```ignore
//! use device_admin::{LockTaskFeatures, restriction};
//!
//! // In the device-owner agent, once:
//! device_admin::set_lock_task_packages(&["com.example.kiosk", "com.example.agent"])?;
//! device_admin::set_lock_task_features(LockTaskFeatures::NONE)?;
//! device_admin::set_keyguard_disabled(true)?;
//! device_admin::set_home_activity("com.example.kiosk", ".MainActivity")?;
//! device_admin::add_user_restriction(restriction::DISALLOW_SAFE_BOOT)?;
//!
//! // Later, a kiosk update pushed by the server:
//! let result = device_admin::install_apk(&path, &cert_sha256).await?;
//!
//! // In the kiosk, on launch:
//! device_admin::start_lock_task().await?;
//! ```
//!
//! # Errors
//!
//! Every policy call returns [`Error::NotDeviceOwner`] unless the app is the
//! device owner (or, for the bundled-app calls, a profile owner), and
//! [`Error::Unsupported`] off Android or below the API
//! level the call needs (named per function). Nothing panics on a device
//! that lacks a feature.
//!
//! # iOS
//!
//! iOS has no device-owner app: management belongs to an MDM server and apps
//! install only through Apple. On iOS, [`start_lock_task`] /
//! [`stop_lock_task`] request Autonomous Single App Mode (a supervised iPad
//! whose MDM profile allows the app), [`lock_task_state`] reports Guided
//! Access, and [`managed_config`] / [`set_managed_feedback`] read and write
//! the MDM's app configuration. Everything else is [`Error::Unsupported`].
//!
//! # Events
//!
//! [`events`] registers a listener for provisioning, admin, lock-task,
//! install and boot events. Events the OS raises while no listener is
//! registered — a boot, the provisioning hand-off, before the app's Rust side
//! is running — are queued on the device and delivered when the first
//! listener registers. Listeners run on the UI thread; register them there.
//!
//! # Manifest
//!
//! The crate contributes its receiver, the two provisioning activities, a
//! boot receiver, its permissions and `res/xml/device_admin.xml` to every app
//! that depends on it (`[package.metadata.idealyst.android]` in its
//! `Cargo.toml`). Whether an app is a kiosk or a home screen is the app's
//! choice, in `[package.metadata.idealyst.app.android]`
//! (`lock_task_mode`, `home_launcher`).

#![deny(missing_docs)]

use std::future::Future;
use std::ops::BitOr;
use std::path::Path;
use std::pin::Pin;

use serde_json::{json, Value};

#[cfg(all(not(target_arch = "wasm32"), target_os = "android"))]
mod android;
#[cfg(all(not(target_arch = "wasm32"), target_os = "ios"))]
mod ios;
mod events;
mod packages;
mod reply;

pub use events::{events, Event, EventSubscription};

/// Compile-checked usage recipes (docs / MCP catalog). Present only under the
/// `catalog` feature.
#[cfg(feature = "catalog")]
pub mod recipes;

/// Why a device-admin call failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// The call needs the app to be the device owner, and it is not.
    #[error("the app is not the device owner")]
    NotDeviceOwner,
    /// Not available on this platform, or below the API level the call needs.
    #[error("unsupported: {0}")]
    Unsupported(String),
    /// [`install_apk`]: the APK is not signed with the expected certificate.
    /// Nothing was written to the installer.
    #[error("certificate mismatch: expected SHA-256 {expected}, the APK is signed with {actual}")]
    CertificateMismatch {
        /// The expected certificate SHA-256, lowercase hex.
        expected: String,
        /// The APK's certificate SHA-256, lowercase hex.
        actual: String,
    },
    /// The named package is not on the device (not even hidden, or removed
    /// for this user).
    #[error("{0} is not installed")]
    NotInstalled(String),
    /// Hiding or suspending this package would strand the device; refused
    /// before reaching the platform.
    #[error("refusing to hide or suspend {package}: it is {reason}")]
    ProtectedPackage {
        /// The package.
        package: String,
        /// Why it is protected.
        reason: ProtectedReason,
    },
    /// The platform refused or failed the call; the message is its own.
    #[error("{0}")]
    Platform(String),
}

/// A device-admin result.
pub type Result<T> = std::result::Result<T, Error>;

/// A boxed future, for the calls that complete asynchronously.
pub type BoxFuture<T> = Pin<Box<dyn Future<Output = Result<T>>>>;

/// System UI that stays usable while an app is locked in lock task mode
/// (`DevicePolicyManager.LOCK_TASK_FEATURE_*`). Combine with `|`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct LockTaskFeatures(pub u32);

impl LockTaskFeatures {
    /// Nothing: no Home, Overview, notifications, status-bar info or power
    /// menu — the kiosk default.
    pub const NONE: Self = Self(0);
    /// The status bar's system info (clock, battery, connectivity).
    pub const SYSTEM_INFO: Self = Self(1);
    /// Notifications and the notification shade.
    pub const NOTIFICATIONS: Self = Self(2);
    /// The Home button.
    pub const HOME: Self = Self(4);
    /// The Overview (recents) button. Requires [`Self::HOME`].
    pub const OVERVIEW: Self = Self(8);
    /// The power-button menu.
    pub const GLOBAL_ACTIONS: Self = Self(16);
    /// The keyguard (lock screen).
    pub const KEYGUARD: Self = Self(32);
    /// Block starting activities from other apps into the locked task
    /// (API 30+).
    pub const BLOCK_ACTIVITY_START_IN_TASK: Self = Self(64);
}

impl BitOr for LockTaskFeatures {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

/// Whether an app is held on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LockTaskState {
    /// Not locked.
    None,
    /// Screen pinning: the user pinned the app and can unpin it.
    Pinned,
    /// Lock task mode (Android) or Single App Mode / Guided Access (iOS).
    Locked,
}

/// When the OS may install system updates
/// (`DevicePolicyManager.setSystemUpdatePolicy`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SystemUpdatePolicy {
    /// Install as soon as an update is available.
    Automatic,
    /// Install only inside a daily window, in minutes since local midnight
    /// (`0..1440`). A window may wrap past midnight (`start > end`).
    Windowed {
        /// Window start, minutes since midnight.
        start_minutes: u16,
        /// Window end, minutes since midnight.
        end_minutes: u16,
    },
    /// Postpone updates for up to 30 days.
    Postpone,
    /// Remove any policy; the device's default behaviour applies.
    Clear,
}

/// An installed package's version.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PackageVersion {
    /// `versionName`, when the package declares one.
    pub name: Option<String>,
    /// `versionCode` (the long version code on API 28+).
    pub code: i64,
}

/// Why [`set_application_hidden`] / [`set_packages_suspended`] refused a
/// package with [`Error::ProtectedPackage`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProtectedReason {
    /// The calling app itself.
    OwnPackage,
    /// On the lock-task allow-list ([`set_lock_task_packages`]).
    LockTask,
    /// The current home screen (what HOME resolves to).
    Home,
}

impl std::fmt::Display for ProtectedReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            ProtectedReason::OwnPackage => "the calling app",
            ProtectedReason::LockTask => "a lock-task package",
            ProtectedReason::Home => "the current home screen",
        })
    }
}

/// One package on the device, from [`installed_packages`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageInfo {
    /// The package name.
    pub package: String,
    /// The app's label as the launcher shows it.
    pub label: String,
    /// Its version.
    pub version: PackageVersion,
    /// Shipped on the system partition (bundled by the manufacturer), or an
    /// update to such an app. [`uninstall`] cannot remove these; hide,
    /// suspend or disable them instead.
    pub is_system: bool,
    /// Installed for this user. `false` for a system app that device-owner
    /// provisioning removed; [`enable_system_app`] brings it back.
    pub is_installed: bool,
    /// Hidden by [`set_application_hidden`].
    pub is_hidden: bool,
    /// Suspended by [`set_packages_suspended`].
    pub is_suspended: bool,
    /// Not disabled (Settings → Disable, or `pm disable`).
    pub is_enabled: bool,
}

/// What an install or uninstall did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PackageOperation {
    /// [`install_apk`].
    Install,
    /// [`uninstall`].
    Uninstall,
}

/// Why an install or uninstall failed (`PackageInstaller.STATUS_*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InstallFailure {
    /// `STATUS_FAILURE`: unspecified.
    Generic,
    /// `STATUS_FAILURE_BLOCKED`: blocked by policy or another app.
    Blocked,
    /// `STATUS_FAILURE_ABORTED`: the session was abandoned.
    Aborted,
    /// `STATUS_FAILURE_INVALID`: not a valid APK, or the package is absent.
    Invalid,
    /// `STATUS_FAILURE_CONFLICT`: conflicts with an installed package (a
    /// different signer, or a lower versionCode).
    Conflict,
    /// `STATUS_FAILURE_STORAGE`: not enough storage.
    Storage,
    /// `STATUS_FAILURE_INCOMPATIBLE`: wrong ABI, SDK level or feature set.
    Incompatible,
    /// `STATUS_FAILURE_TIMEOUT` (API 34+).
    Timeout,
    /// `STATUS_PENDING_USER_ACTION`: the installer wanted confirmation,
    /// which a device owner never needs — the app is not the device owner.
    PendingUserAction,
    /// A status code this crate does not name.
    Other(i32),
}

/// The outcome of [`install_apk`] or [`uninstall`], also delivered as
/// [`Event::InstallResult`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallResult {
    /// It worked.
    Success {
        /// Install or uninstall.
        operation: PackageOperation,
        /// The package.
        package: Option<String>,
        /// The installed versionCode (none after an uninstall).
        version_code: Option<i64>,
    },
    /// The installer refused or failed.
    Failure {
        /// Install or uninstall.
        operation: PackageOperation,
        /// The package, when the installer names it.
        package: Option<String>,
        /// Why.
        reason: InstallFailure,
        /// The installer's own message.
        message: Option<String>,
    },
}

impl InstallResult {
    /// `true` for [`InstallResult::Success`].
    pub fn is_success(&self) -> bool {
        matches!(self, InstallResult::Success { .. })
    }
}

/// `UserManager.DISALLOW_*` restrictions commonly set on a kiosk, for
/// [`add_user_restriction`] / [`clear_user_restriction`]. Any other
/// restriction key string works too.
pub mod restriction {
    /// No booting into safe mode (which disables the kiosk).
    pub const DISALLOW_SAFE_BOOT: &str = "no_safe_boot";
    /// No factory reset from Settings.
    pub const DISALLOW_FACTORY_RESET: &str = "no_factory_reset";
    /// No adding users.
    pub const DISALLOW_ADD_USER: &str = "no_add_user";
    /// No developer options or USB debugging.
    pub const DISALLOW_DEBUGGING_FEATURES: &str = "no_debugging_features";
    /// No mounting USB storage or transferring files over USB.
    pub const DISALLOW_USB_FILE_TRANSFER: &str = "no_usb_file_transfer";
    /// No installing apps (other than through the device owner).
    pub const DISALLOW_INSTALL_APPS: &str = "no_install_apps";
    /// No uninstalling apps.
    pub const DISALLOW_UNINSTALL_APPS: &str = "no_uninstall_apps";
    /// No adjusting the volume.
    pub const DISALLOW_ADJUST_VOLUME: &str = "no_adjust_volume";
}

// ── Android policy calls ────────────────────────────────────────────────

/// Whether this app is the device owner. Everything marked "device owner"
/// below needs `true`.
pub fn is_device_owner() -> Result<bool> {
    as_bool(call("is_device_owner", json!({}))?)
}

/// Allow-list the packages that may enter lock task mode silently — the kiosk,
/// and usually the agent itself. Replaces the previous list. Device owner;
/// API 21.
pub fn set_lock_task_packages(packages: &[&str]) -> Result<()> {
    call("set_lock_task_packages", json!({ "packages": packages })).map(drop)
}

/// The system UI available while locked. Device owner; API 28.
pub fn set_lock_task_features(features: LockTaskFeatures) -> Result<()> {
    call("set_lock_task_features", json!({ "flags": features.0 })).map(drop)
}

/// Lock this app on screen. Resolves `Ok(true)` when it locked with no user
/// interaction — on Android, the app is allow-listed by
/// [`set_lock_task_packages`]; on iOS, a supervised device's MDM profile
/// allows Single App Mode — and `Ok(false)` otherwise (Android then asks
/// the user to pin the app; iOS does nothing). Called by the app being
/// locked, from the UI thread. API 21.
pub fn start_lock_task() -> BoxFuture<bool> {
    #[cfg(all(not(target_arch = "wasm32"), target_os = "ios"))]
    return ios::set_single_app_mode(true);
    #[cfg(not(all(not(target_arch = "wasm32"), target_os = "ios")))]
    Box::pin(map_future(call_async("start_lock_task", json!({})), as_bool))
}

/// Leave lock task mode (Android) or Single App Mode (iOS). Resolves
/// `Ok(true)` when the app is no longer locked by this call. API 21.
pub fn stop_lock_task() -> BoxFuture<bool> {
    #[cfg(all(not(target_arch = "wasm32"), target_os = "ios"))]
    return ios::set_single_app_mode(false);
    #[cfg(not(all(not(target_arch = "wasm32"), target_os = "ios")))]
    Box::pin(map_future(call_async("stop_lock_task", json!({})), as_bool))
}

/// Whether an app is held on screen. Android reports pinning separately
/// from lock task (API 23; `Locked` or `None` below); iOS reports Guided
/// Access / Single App Mode as `Locked`.
pub fn lock_task_state() -> Result<LockTaskState> {
    #[cfg(all(not(target_arch = "wasm32"), target_os = "ios"))]
    return Ok(ios::lock_task_state());
    #[cfg(not(all(not(target_arch = "wasm32"), target_os = "ios")))]
    match call("lock_task_state", json!({}))?.as_str() {
        Some("locked") => Ok(LockTaskState::Locked),
        Some("pinned") => Ok(LockTaskState::Pinned),
        Some("none") => Ok(LockTaskState::None),
        other => Err(Error::Platform(format!("unexpected lock task state {other:?}"))),
    }
}

/// Disable the keyguard, so waking the device returns straight to the
/// locked app. Fails when a screen-lock credential is set. Device owner;
/// API 23.
pub fn set_keyguard_disabled(disabled: bool) -> Result<()> {
    call("set_keyguard_disabled", json!({ "disabled": disabled })).map(drop)
}

/// Disable the status bar (no pull-down shade). Device owner; API 23.
pub fn set_status_bar_disabled(disabled: bool) -> Result<()> {
    call("set_status_bar_disabled", json!({ "disabled": disabled })).map(drop)
}

/// Make `package`'s `activity` the persistent home screen, so it relaunches
/// at boot and after a crash. `activity` may be relative (`.MainActivity`).
/// The activity must declare the HOME category — set `home_launcher = true`
/// in that app's `[package.metadata.idealyst.app.android]`. Device owner;
/// API 21.
pub fn set_home_activity(package: &str, activity: &str) -> Result<()> {
    call("set_home_activity", json!({ "package": package, "activity": activity })).map(drop)
}

/// Remove the persistent home screen set for `package`. Device owner;
/// API 21.
pub fn clear_home_activity(package: &str) -> Result<()> {
    call("clear_home_activity", json!({ "package": package })).map(drop)
}

/// Set a user restriction (see [`restriction`]). Device owner; API 21.
pub fn add_user_restriction(restriction: &str) -> Result<()> {
    call("add_user_restriction", json!({ "restriction": restriction })).map(drop)
}

/// Clear a user restriction. Device owner; API 21.
pub fn clear_user_restriction(restriction: &str) -> Result<()> {
    call("clear_user_restriction", json!({ "restriction": restriction })).map(drop)
}

/// Set a `Settings.Global` value, e.g. `("stay_on_while_plugged_in", "7")`
/// to stay awake on any charger. Android only allows a fixed set of keys
/// here (see `DevicePolicyManager.setGlobalSetting`); the animation scales
/// (`window_animation_scale`, `transition_animation_scale`,
/// `animator_duration_scale`) are not among them. Device owner; API 21.
pub fn set_global_setting(key: &str, value: &str) -> Result<()> {
    call("set_global_setting", json!({ "key": key, "value": value })).map(drop)
}

/// Install (or update) the APK at `path`, silently. The APK's signing
/// certificate must have SHA-256 `expected_cert_sha256` (hex, colons
/// allowed); otherwise this fails with [`Error::CertificateMismatch`] before
/// anything reaches the installer. Resolves with the installer's
/// [`InstallResult`], which is also delivered as [`Event::InstallResult`].
/// The APK must be readable by the app (its own files dir or cache). An
/// update must carry a higher versionCode and the same certificate as the
/// installed app. Device owner; API 21.
pub fn install_apk(path: impl AsRef<Path>, expected_cert_sha256: &str) -> BoxFuture<InstallResult> {
    let args = json!({
        "path": path.as_ref().to_string_lossy(),
        "expected_cert_sha256": expected_cert_sha256,
    });
    Box::pin(map_future(call_async("install_apk", args), reply::install_result))
}

/// Uninstall `package`, silently. Device owner; API 21.
pub fn uninstall(package: &str) -> BoxFuture<InstallResult> {
    Box::pin(map_future(
        call_async("uninstall", json!({ "package": package })),
        reply::install_result,
    ))
}

// ── Bundled apps: hide, suspend, re-enable ──────────────────────────────
//
// For the apps a manufacturer ships on the system partition, which
// `uninstall` cannot remove. Each of these works for a device owner or a
// profile owner, and refuses (`Error::ProtectedPackage`) to hide or suspend
// the calling app, a lock-task package or the current home screen.

/// Hide or unhide `package`. A hidden app is gone from the launcher and
/// cannot run, but stays installed and keeps its data. Works on system and
/// privileged apps. Returns whether the hidden state changed (`false` when it
/// already was). [`Error::NotInstalled`] when the package is not on the
/// device; [`Error::Platform`] when Android refuses (it will not hide some
/// packages the system depends on). Device or profile owner; API 21.
pub fn set_application_hidden(package: &str, hidden: bool) -> Result<bool> {
    if hidden {
        refuse_protected(&[package])?;
    }
    as_bool(call("set_application_hidden", json!({ "package": package, "hidden": hidden }))?)
}

/// Whether `package` is hidden. [`Error::NotInstalled`] when it is not on
/// the device. Device or profile owner; API 21.
pub fn is_application_hidden(package: &str) -> Result<bool> {
    as_bool(call("is_application_hidden", json!({ "package": package }))?)
}

/// Suspend or unsuspend `packages`. A suspended app stays in the launcher,
/// greyed out, and shows a dialog instead of running. Returns the packages
/// Android could not change (not installed, or one it refuses such as an
/// active admin), as the platform reports them. Device or profile owner;
/// API 24.
pub fn set_packages_suspended(packages: &[&str], suspended: bool) -> Result<Vec<String>> {
    if suspended {
        refuse_protected(packages)?;
    }
    let failed = call(
        "set_packages_suspended",
        json!({ "packages": packages, "suspended": suspended }),
    )?;
    packages::strings(&failed)
}

/// Whether `package` is suspended. [`Error::NotInstalled`] when it is not
/// on the device. Device or profile owner; API 24.
pub fn is_package_suspended(package: &str) -> Result<bool> {
    as_bool(call("is_package_suspended", json!({ "package": package }))?)
}

/// Re-install for this user a system app that device-owner provisioning
/// removed (provisioning removes bundled apps unless the QR code sets
/// `PROVISIONING_LEAVE_ALL_SYSTEM_APPS_ENABLED`; see
/// `idealyst android provisioning-qr --leave-system-apps`). Does not unhide
/// or unsuspend. [`Error::Platform`] for a package that is not a system app.
/// Device or profile owner; API 21.
pub fn enable_system_app(package: &str) -> Result<()> {
    call("enable_system_app", json!({ "package": package })).map(drop)
}

/// Every package on the device, including hidden ones and system apps
/// removed for this user, so a console can show and restore them. With
/// `include_system == false`, only apps that are not on the system
/// partition. Loads every app's label, so it takes a moment on a device with
/// hundreds of packages. Device or profile owner; API 21.
pub fn installed_packages(include_system: bool) -> Result<Vec<PackageInfo>> {
    packages::package_infos(&call(
        "installed_packages",
        json!({ "include_system": include_system }),
    )?)
}

fn refuse_protected(packages: &[&str]) -> Result<()> {
    packages::Protected::parse(&call("protected_packages", json!({}))?)?.check(packages)
}

/// `package`'s installed version, or `None` when it is not installed.
/// API 1.
pub fn installed_version(package: &str) -> Result<Option<PackageVersion>> {
    let v = call("installed_version", json!({ "package": package }))?;
    if v.is_null() {
        return Ok(None);
    }
    Ok(Some(PackageVersion {
        name: v.get("name").and_then(Value::as_str).map(str::to_string),
        code: v
            .get("code")
            .and_then(Value::as_i64)
            .ok_or_else(|| Error::Platform(format!("malformed version {v}")))?,
    }))
}

/// Start `package`'s launcher activity. API 1.
pub fn launch(package: &str) -> Result<()> {
    call("launch", json!({ "package": package })).map(drop)
}

/// Reboot the device. Device owner; API 24.
pub fn reboot() -> Result<()> {
    call("reboot", json!({})).map(drop)
}

/// When the OS installs system updates. Device owner; API 23.
pub fn set_system_update_policy(policy: SystemUpdatePolicy) -> Result<()> {
    let args = match policy {
        SystemUpdatePolicy::Automatic => json!({ "kind": "automatic" }),
        SystemUpdatePolicy::Windowed {
            start_minutes,
            end_minutes,
        } => {
            if start_minutes >= 1440 || end_minutes >= 1440 {
                return Err(Error::Platform(format!(
                    "update window {start_minutes}..{end_minutes} is outside 0..1440 minutes"
                )));
            }
            json!({ "kind": "windowed", "start_minutes": start_minutes, "end_minutes": end_minutes })
        }
        SystemUpdatePolicy::Postpone => json!({ "kind": "postpone" }),
        SystemUpdatePolicy::Clear => json!({ "kind": "clear" }),
    };
    call("set_system_update_policy", args).map(drop)
}

/// Factory-reset the device. Irreversible: callers should gate it behind
/// their own confirmation. `reason` is shown to the user on API 28–33.
/// Device owner; API 8.
pub fn wipe(reason: &str) -> Result<()> {
    call("wipe", json!({ "reason": reason })).map(drop)
}

/// Stop being the device owner, for decommissioning. Every policy this app
/// set stays until changed by someone else or a reset. (Deprecated by
/// Android in API 26 but still the only in-app way out.) Device owner;
/// API 21.
pub fn relinquish_device_owner() -> Result<()> {
    call("relinquish_device_owner", json!({})).map(drop)
}

// ── iOS managed app configuration ───────────────────────────────────────

/// The MDM-supplied app configuration (`com.apple.configuration.managed`),
/// or `None` when the MDM set none. iOS only.
pub fn managed_config() -> Result<Option<Value>> {
    #[cfg(all(not(target_arch = "wasm32"), target_os = "ios"))]
    return ios::managed_config();
    #[cfg(not(all(not(target_arch = "wasm32"), target_os = "ios")))]
    Err(unsupported("managed_config"))
}

/// Report status back to the MDM (`com.apple.feedback.managed`). `feedback`
/// must be a JSON object. iOS only.
pub fn set_managed_feedback(feedback: &Value) -> Result<()> {
    #[cfg(all(not(target_arch = "wasm32"), target_os = "ios"))]
    return ios::set_managed_feedback(feedback);
    #[cfg(not(all(not(target_arch = "wasm32"), target_os = "ios")))]
    {
        let _ = feedback;
        Err(unsupported("set_managed_feedback"))
    }
}

// ── Plumbing ────────────────────────────────────────────────────────────

fn as_bool(v: Value) -> Result<bool> {
    v.as_bool()
        .ok_or_else(|| Error::Platform(format!("expected a boolean, got {v}")))
}

/// A synchronous call into the platform half.
#[allow(unused_variables)]
fn call(method: &str, args: Value) -> Result<Value> {
    #[cfg(all(not(target_arch = "wasm32"), target_os = "android"))]
    return android::call(method, &args);
    #[cfg(not(all(not(target_arch = "wasm32"), target_os = "android")))]
    Err(unsupported(method))
}

/// A call whose result arrives later.
#[allow(unused_variables)]
fn call_async(method: &str, args: Value) -> BoxFuture<Value> {
    #[cfg(all(not(target_arch = "wasm32"), target_os = "android"))]
    return android::call_async(method, &args);
    #[cfg(not(all(not(target_arch = "wasm32"), target_os = "android")))]
    {
        let err = unsupported(method);
        Box::pin(async move { Err(err) })
    }
}

async fn map_future<T>(fut: BoxFuture<Value>, f: fn(Value) -> Result<T>) -> Result<T> {
    f(fut.await?)
}

#[allow(dead_code)]
fn unsupported(what: &str) -> Error {
    Error::Unsupported(format!("device_admin::{what} is not available on {}", std::env::consts::OS))
}
