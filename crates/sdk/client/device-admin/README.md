# `device-admin`

Android device-owner policy for Rust, plus a small iOS counterpart. An app
that is the device owner can lock another app (a kiosk) onto the screen,
keep it there across reboots and crashes, and install, update and roll back
APKs silently, with no MDM and no app store.

```rust,ignore
use device_admin::{restriction, LockTaskFeatures};

// The device-owner app, once:
device_admin::set_lock_task_packages(&["com.example.kiosk", "com.example.agent"])?;
device_admin::set_lock_task_features(LockTaskFeatures::NONE)?;
device_admin::set_keyguard_disabled(true)?;
device_admin::set_home_activity("com.example.kiosk", ".MainActivity")?;
device_admin::add_user_restriction(restriction::DISALLOW_SAFE_BOOT)?;

// A kiosk update its server pushed:
let result = device_admin::install_apk(&apk_path, &expected_cert_sha256).await?;

// The kiosk, on launch:
device_admin::start_lock_task().await?;
```

## A kiosk tablet in two apps

The pattern this crate is built for:

- an **agent** that is the device owner, locks the kiosk in, and installs
  its updates;
- the **kiosk** that runs locked, using only `start_lock_task` /
  `stop_lock_task`.

Keep them separate. An app updating itself could strand the device with a
broken build, because only the device owner or a factory reset can undo
device-owner state. A small agent that rarely changes can always roll the
kiosk back. `examples/kiosk-agent` and `examples/kiosk-app` are a working
pair.

The kiosk app declares itself a lock-task app and a home screen in its own
manifest metadata:

```toml
[package.metadata.idealyst.app.android]
lock_task_mode = "if_whitelisted"
home_launcher  = true
```

## Becoming device owner

Only on a freshly reset device with no accounts:

```sh
adb shell dpm set-device-owner <agent package>/io.idealyst.deviceadmin.AdminReceiver
```

Or QR provisioning from the setup wizard: build a release APK, host it over
https, and run `idealyst android provisioning-qr --download-url <url>`. The
`--extras` JSON reaches the agent as `Event::ProvisioningComplete`. Release
signing and versionCode come from `[package.metadata.idealyst.app.android]`;
see `docs/android-packaging.md`. Silent updates need every build signed with
the same key.

## API

| Call | Android | Min API | iOS |
|---|---|---|---|
| `is_device_owner()` | `isDeviceOwnerApp` | 18 | `Unsupported` |
| `set_lock_task_packages(&[pkg])` | `setLockTaskPackages` | 21 | `Unsupported` |
| `set_lock_task_features(f)` | `setLockTaskFeatures` | 28 | `Unsupported` |
| `start_lock_task()` / `stop_lock_task()` | `Activity.startLockTask` / `stopLockTask` | 21 | Single App Mode (`UIAccessibilityRequestGuidedAccessSession`) |
| `lock_task_state()` | `getLockTaskModeState` | 23 (21: locked/none) | `UIAccessibilityIsGuidedAccessEnabled` |
| `set_keyguard_disabled(b)` | `setKeyguardDisabled` | 23 | `Unsupported` |
| `set_status_bar_disabled(b)` | `setStatusBarDisabled` | 23 | `Unsupported` |
| `set_home_activity(pkg, activity)` / `clear_home_activity(pkg)` | `addPersistentPreferredActivity` (HOME) | 21 | `Unsupported` |
| `add_user_restriction(r)` / `clear_user_restriction(r)` | `addUserRestriction` / `clearUserRestriction` | 21 | `Unsupported` |
| `set_global_setting(key, value)` | `setGlobalSetting` (an allow-listed set of keys; not the animation scales) | 21 | `Unsupported` |
| `set_application_hidden(pkg, b)` / `is_application_hidden(pkg)` | `setApplicationHidden` / `isApplicationHidden` | 21 | `Unsupported` |
| `set_packages_suspended(&[pkg], b)` / `is_package_suspended(pkg)` | `setPackagesSuspended` / `isPackageSuspended` | 24 | `Unsupported` |
| `enable_system_app(pkg)` | `enableSystemApp` | 21 | `Unsupported` |
| `installed_packages(include_system)` | `getInstalledPackages(MATCH_UNINSTALLED_PACKAGES)` | 21 | `Unsupported` |
| `start_keep_alive(title, text)` / `stop_keep_alive()` | a `systemExempted` foreground service | 26 | `Unsupported` |
| `install_apk(path, cert_sha256)` | `PackageInstaller` session, committed silently | 21 | `Unsupported` |
| `uninstall(pkg)` | `PackageInstaller.uninstall` | 21 | `Unsupported` |
| `installed_version(pkg)` | `getPackageInfo` | 1 | `Unsupported` |
| `launch(pkg)` | `getLaunchIntentForPackage` | 1 | `Unsupported` |
| `reboot()` | `DevicePolicyManager.reboot` | 24 | `Unsupported` |
| `set_system_update_policy(p)` | `setSystemUpdatePolicy` | 23 | `Unsupported` |
| `wipe(reason)` | `wipeDevice` (34+) / `wipeData` | 8 | `Unsupported` |
| `relinquish_device_owner()` | `clearDeviceOwnerApp` | 21 | `Unsupported` |
| `managed_config()` / `set_managed_feedback(json)` | `Unsupported` | — | `NSUserDefaults` `com.apple.configuration.managed` / `com.apple.feedback.managed` |

The device-owner calls return `Error::NotDeviceOwner` unless the app is the
device owner. Below a call's API level the result is `Error::Unsupported`
naming the level. Off Android and iOS, everything is `Unsupported`.

`start_lock_task()` resolves `Ok(true)` when the app locked with no user
interaction. On Android that means it is allow-listed; otherwise Android
falls back to screen pinning, asks the user, and the call resolves
`Ok(false)`. On iOS `true` means the supervised device's MDM profile allows
Single App Mode for the app.

## Staying reachable in the background

Android 12+ freezes a backgrounded app's process (the cached-apps
freezer): it stays alive, but no thread runs, so timers, a server socket's
heartbeat and every reconnect stop until the app is in front again. On a
kiosk the agent is behind the kiosk all day. Call
`start_keep_alive("Device management", "Connected")` once at start: it runs
a foreground service, which keeps the process out of the freezer and shows
an ongoing notification. The service uses the `systemExempted` type Android
14+ reserves for device and profile owners, so it has no time limit.
`stop_keep_alive()` ends it.

## Bundled apps

`uninstall` cannot remove apps on the system partition, such as the
manufacturer's media hub on a consumer tablet. A device owner switches them
off instead:

```rust,ignore
// Gone from the launcher, cannot run, data kept. Returns whether it changed.
device_admin::set_application_hidden("com.lenovo.entertainment", true)?;
// Or: greyed out in the launcher, shows a dialog instead of running.
// Returns the packages Android could not suspend.
let refused = device_admin::set_packages_suspended(&["com.example.bloat"], true)?;
```

`installed_packages(true)` lists everything for a console, including
hidden apps and the system apps provisioning removed (`is_installed ==
false`), so each can be brought back: `set_application_hidden(pkg, false)`,
`set_packages_suspended(&[pkg], false)` or `enable_system_app(pkg)`.

Provisioning removes the bundled apps that have a launcher icon by default.
To keep them, generate the setup QR code with
`idealyst android provisioning-qr --leave-system-apps`
(`PROVISIONING_LEAVE_ALL_SYSTEM_APPS_ENABLED`).

These calls work for a device owner or a profile owner. Hiding or
suspending the app itself, a lock-task package or the current home screen
would strand the device, so it fails with `Error::ProtectedPackage` before
reaching Android. A package that is not on the device is
`Error::NotInstalled`.

`install_apk` checks the APK's signing certificate against
`expected_cert_sha256` (hex, colons allowed) before anything reaches the
installer, and fails with `Error::CertificateMismatch` otherwise. The APK
must be readable by the app, so download it into the app's own files or
cache dir. An update needs a higher versionCode and the installed app's
certificate. The result resolves the future and is also delivered as
`Event::InstallResult`.

## Events

```rust,ignore
let subscription = device_admin::events(|event| match event {
    device_admin::Event::ProvisioningComplete { extras } => { /* first check-in */ }
    device_admin::Event::InstallResult(result) => { /* report it */ }
    device_admin::Event::BootCompleted => { /* check in */ }
    _ => {}
});
// Tie it to a component: on_scope_drop(move || drop(subscription));
```

| Event | Android source |
|---|---|
| `ProvisioningComplete { extras }` | the policy-compliance activity / `onProfileProvisioningComplete`, delivered once |
| `AdminEnabled` / `AdminDisabled` | `DeviceAdminReceiver.onEnabled` / `onDisabled` |
| `LockTaskEntering { package }` / `LockTaskExiting` | `onLockTaskModeEntering` / `onLockTaskModeExiting` |
| `InstallResult(_)` | the `PackageInstaller` status intent |
| `PackageReplaced { package }` | `ACTION_PACKAGE_REPLACED`, only while a listener is registered |
| `BootCompleted` | `ACTION_BOOT_COMPLETED` |

Listeners run on the UI thread, so register them there. The OS raises most
of these events while the app's Rust side is not running: at boot,
during provisioning, or while another app is locked in front. The crate
queues them on the device and replays them in order when the first listener
registers, one per looper turn, so a listener that appends to a signal sees
each earlier event committed. A device that never runs the app keeps the
newest 64. Force-stopping an app (`am force-stop`, Settings → Force stop)
stops Android delivering it any broadcast until it is next launched.

## What the crate adds to the app

Through `[package.metadata.idealyst.android]` in this crate's
`Cargo.toml`, every app that depends on it gets the following, all inert
until the app becomes device owner:

- the `io.idealyst.deviceadmin.AdminReceiver` device-admin receiver and its
  `res/xml/device_admin.xml` policy list;
- the two activities QR provisioning requires on Android 10+;
- a boot receiver;
- `RECEIVE_BOOT_COMPLETED`, `REQUEST_INSTALL_PACKAGES`,
  `REQUEST_DELETE_PACKAGES` and `QUERY_ALL_PACKAGES`. The last is what
  `installed_version` and `launch` need to see other packages on Android
  11+; Google Play restricts it, which does not matter for a sideloaded
  device-owner app.

## Status

Verified on an Android 14 (API 34) emulator with the two example apps,
built by `idealyst build --android`:

- `dpm set-device-owner` succeeds against the declared receiver, and
  `is_device_owner()` is `true`.
- After lock-down the kiosk enters lock task with no prompt. Home, Overview,
  Back and the notification shade do nothing, and power → wake returns to
  the kiosk with no keyguard.
- A reboot comes back to the kiosk in lock task with no interaction, and the
  events raised while the agent was not running are replayed to it in order.
- `install_apk` of a higher versionCode with the same certificate installs
  with no prompt, the kiosk relaunches on the new version, and the result is
  `InstallResult::Success`. An APK signed by another key is refused with
  `CertificateMismatch` before commit.
- Bundled apps (a fresh API 34 emulator): `set_application_hidden` hides
  and unhides `com.android.camera2` and reports `false` when nothing
  changed; `installed_packages` still lists it while hidden; suspending
  `[camera2, a missing package]` suspends camera2 and returns the missing
  one; hiding or suspending the app itself, a lock-task package or the home
  launcher is `ProtectedPackage`; a missing package is `NotInstalled`;
  `enable_system_app` brings back a system app removed with
  `pm uninstall --user 0`; and every call before `dpm set-device-owner` is
  `NotDeviceOwner`.
- `stop_lock_task()` returns the tablet to normal use, and
  `relinquish_device_owner()` leaves `dpm list-owners` empty. After that,
  policy calls return `NotDeviceOwner` and an un-allow-listed
  `start_lock_task()` resolves `false`.
- Both provisioning activities launch and report `ProvisioningComplete`.

Not verified yet:

- The full QR flow on a wiped device: the setup wizard downloading the APK
  and handing over the extras bundle.
- The iOS calls on a supervised iPad. They are compile-checked for
  `aarch64-apple-ios` only.
