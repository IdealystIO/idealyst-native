# `accessibility-service`

The app's own Android accessibility service, for seeing and operating a
device with nobody at it: screenshots of the whole screen, and taps,
swipes, system keys and text entry anywhere on it. Built for remote support
of unattended devices, such as an MDM agent streaming a locked kiosk
tablet to an admin console and replaying the admin's clicks as taps.

```rust,ignore
use accessibility_service::{GlobalAction, ImageFormat};

let shot = accessibility_service::screenshot(ImageFormat::Jpeg { quality: 70 }).await?;
send_frame(shot.width, shot.height, &shot.data);

// A click on that frame, in its pixels:
accessibility_service::tap(812.0, 440.0).await?;
accessibility_service::swipe((1200.0, 900.0), (1200.0, 300.0), 300).await?;
accessibility_service::global_action(GlobalAction::Back)?;
```

Live video with a person on site to approve it is `screen-recorder`
(MediaProjection). Android asks for consent on every MediaProjection
session, even from a device owner; an accessibility service, once switched
on, needs nobody.

## API

| Call | Android | Min API |
|---|---|---|
| `is_enabled()` | the service is bound | 24 |
| `on_enabled_change(f)` | `onServiceConnected` / `onUnbind` | 24 |
| `open_settings()` | `Settings.ACTION_ACCESSIBILITY_SETTINGS` | 24 |
| `screenshot(ImageFormat::Rgba \| Jpeg { quality })` | `takeScreenshot` (+ `takeScreenshotOfWindow` on 34+) | 30 |
| `tap(x, y)` / `long_press(x, y, ms)` / `swipe(from, to, ms)` | `dispatchGesture` | 24 |
| `global_action(a)` | `performGlobalAction` (`LockScreen`: 28) | 24 |
| `set_focused_text(text)` | `ACTION_SET_TEXT` on the input-focused node | 24 |

Off Android every call is `Error::Unsupported` and `is_enabled()` is
`false`. Every call is `Error::NotEnabled` until the service is switched on.

- **Coordinates.** A screenshot is the display as shown, in its current
  rotation (a landscape tablet gives 2560 × 1600, turned to portrait
  1600 × 2560), and gestures take points in that same space. A point
  outside the screen is `InvalidArgument`.
- **Rate.** Android allows one screenshot about every third of a second.
  Sooner fails with `TooSoon { retry_after_ms }`, without reaching Android.
  JPEG encoding runs on a background thread on the device.
- **Secure windows.** A `FLAG_SECURE` window comes out black in a
  whole-screen capture and Android reports success. On API 34+ the SDK
  also asks about the active window and returns `SecureWindow` instead.
  The answer comes from the front app's UI thread, so the SDK waits at most
  500 ms for it; a frame taken while a busy app is still closing a secure
  window can go out unchecked. Below 34 a secure window always arrives
  black.
- **Gestures** resolve when Android reports them done
  (`GestureCancelled` if a touch on the device interrupts one), so a
  caller can send the next in order.
- **`global_action`** reports whether Android accepted the action, not
  whether it happened: in lock task mode Home is blocked silently and the
  call still succeeds.

## Switching the service on

Android lets only a person (Settings > Accessibility) or `adb` switch an
accessibility service on. No app can, not even a device owner. Once per
device, after the app is installed:

```sh
adb shell settings put secure enabled_accessibility_services \
    <applicationId>/io.idealyst.accessibilityservice.RustAccessibilityService
```

Run it after `adb install` has returned. A component Android does not know
yet is dropped from the setting. On Android 13+ Settings greys the switch
out for an app installed outside an app store until someone allows
"restricted settings" in the app's info page; the `adb` route is not
affected. A device owner can limit which services may be switched on with
`device_admin::set_permitted_accessibility_services`.

Android starts the app's process to bind the service, but that does not
start the app's Rust side: something must still launch the app once after
boot, and a backgrounded app needs `keep_alive` (or
`device_admin::start_keep_alive`) to keep running.

## What the crate adds to the app

Through `[package.metadata.idealyst.android]` in this crate's
`Cargo.toml`, every app that depends on it gets the
`io.idealyst.accessibilityservice.RustAccessibilityService` `<service>`,
its configuration (`res/xml/accessibility_service.xml`: screenshots,
gestures, window content, interactive windows) and two strings Settings
shows. The service is inert until switched on. The app replaces the
strings:

```toml
[package.metadata.idealyst.app.android.strings]
accessibility_service_label = "Example remote support"
accessibility_service_description = "Lets your administrator see and operate this tablet remotely."
```

## Status

Verified on an Android 14 (API 34) tablet emulator with
`examples/remote-control`, built by `idealyst build --android`:

- The service binds with the app's label after the `adb` switch-on;
  `is_enabled()` follows it, and `on_enabled_change` delivers `true` and
  `false` to a running app.
- `screenshot` returns 2560 × 1600 (`Deg0`) in landscape and an upright
  1600 × 2560 (`Deg90`) after rotating; RGBA is exactly
  `width × height × 4` bytes, and a JPEG at 70 is about 45 KB for a plain
  screen. Back-to-back JPEG screenshots run at 2.1–2.7 a second. A second
  request inside the interval is `TooSoon` with the remaining wait.
- A tap at a button's position in the screenshot presses it, in both
  orientations; a swipe scrolls a Settings list; an 800 ms long press opens
  the launcher's long-press menu; `set_focused_text` fills the focused
  search field, and with no field focused it is `NoFocusedInput`.
- Back, Home and Notifications act. In screen pinning (lock task)
  Home leaves the app in front and the call returns `Ok`.
- A Chrome incognito tab is `SecureWindow`; with it closed, screenshots
  succeed again.
- Every call is `NotEnabled` once the service is switched off.

Not verified yet:

- The target CrewForge tablet (Lenovo TB336FU, Android 16) under
  device-owner lock task. The kiosk's lock blocked launching the test app
  there; the service itself installed and bound on it.
- `Recents`, `LockScreen` and `QuickSettings` (the emulator's launcher
  draws Recents inside itself, so the check was inconclusive).
