# `accessibility-service`

The app's own Android accessibility service, for seeing and operating a
device with nobody at it: screenshots of the whole screen; taps, swipes,
system keys and text entry anywhere on it; finding an element on screen by
id, text, package or class and pressing it; and the windows on screen,
with notice when they change. Built for remote support of unattended
devices, such as an MDM agent streaming a locked kiosk tablet to an admin
console, replaying the admin's clicks as taps, and pressing a system
prompt's button when nobody is at the tablet to.

```rust,ignore
use accessibility_service::{GlobalAction, ImageFormat};

let shot = accessibility_service::screenshot(ImageFormat::Jpeg { quality: 70 }).await?;
send_frame(shot.width, shot.height, &shot.data);

// A click on that frame, in its pixels:
accessibility_service::tap(812.0, 440.0).await?;
accessibility_service::swipe((1200.0, 900.0), (1200.0, 300.0), 300).await?;
accessibility_service::global_action(GlobalAction::Back)?;

// Press a button by what it is, in any app or system dialog:
let q = Query::new().package("com.android.systemui").view_id("android:id/button1");
let found = accessibility_service::wait_for(&q, Duration::from_secs(5)).await?;
accessibility_service::click(&found[0]).await?;

// Hear when windows change, and read them:
let sub = accessibility_service::on_window_change(|e| log::info!("{e:?}"));
for w in accessibility_service::windows().await? {
    log::info!("{:?} {:?} {:?}", w.kind, w.package, w.bounds);
}
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
| `type_text(text)` | the service's input method (`flagInputMethodEditor`): `commitText` at the cursor | 33 |
| `press_key(key)` | the same input connection: a key event, or the field's editor action for `Enter` | 33 |
| `find(&query)` / `wait_for(&query, timeout)` | a walk of every interactive window's node tree | 24 |
| `click(&node)` | `ACTION_CLICK`, else a tap at the centre of its bounds | 24 |
| `windows()` | `getWindows` (picture-in-picture: 26, display: 30) | 24 |
| `on_window_change(f)` | `TYPE_WINDOWS_CHANGED` / `TYPE_WINDOW_STATE_CHANGED` (what changed: 28) | 24 |

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
- **Finding elements.** A `Query` matches on any of view id, text,
  content description, package and class; every criterion set must hold.
  Text and description compare ignoring case, exactly or as a substring.
  A view id without a `:` (`"button1"`) matches that id in any package, a
  class without a `.` (`"Button"`) its simple name. The search covers every
  interactive window (the app in front, a system dialog, the status bar,
  the keyboard) and runs on a background thread on the device: each
  element is a call into the app that draws it. `wait_for` searches again
  every 250 ms until something matches or the timeout passes
  (`Timeout`). Only elements an app reports to accessibility are found.
- **Clicking.** `click` asks Android to click the element itself, which
  works whatever is drawn over it. An element that takes no click (a label
  inside a clickable row) is tapped at its centre instead, read again at
  the time of the click. An element that has left the screen is
  `StaleNode`; so is one from more than the last 512 found, so find again.
- **Windows** are listed top of the stack first with their kind
  (application, system, keyboard, split-screen divider, …), layer,
  bounds, package, title and focus. `on_window_change` reports windows
  appearing, going away, moving or changing focus, and activities, dialogs
  or menus opening; Android delivers the last of a burst at most every
  100 ms.

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
gestures, window content, interactive windows, view ids, and the window
events) and two strings Settings shows. The service is inert until switched on. The app replaces the
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
- `type_text` and `press_key` (API 34 emulator, Contacts' new-contact
  form): text lands at the cursor; Backspace, Delete, Home, End and the
  arrows edit and move as a keyboard does; Tab moves to the next field and
  typing follows it; Enter in a single-line field runs its action (Next).
  Typing within ~50 ms of the tap that focuses a field is
  `NoFocusedInput`: Android binds the input connection a moment later.
- Back, Home and Notifications act. In screen pinning (lock task)
  Home leaves the app in front and the call returns `Ok`.
- A Chrome incognito tab is `SecureWindow`; with it closed, screenshots
  succeed again.
- Every call is `NotEnabled` once the service is switched off.
- `find` / `wait_for` / `click` / `windows` / `on_window_change`
  (API 34 tablet emulator, 2026-11-13): SystemUI's own "Start
  Recording?" dialog (`ScreenRecordDialog`, opened from the Screen record
  tile) lists every element with its view id
  (`com.android.systemui:id/button_start`). `wait_for(id = "button_start")`
  finds it and `click` starts the recording, every time across repeated
  runs. A `Stop` button in the notification shade is clicked the same
  way. A label that takes no click (a Settings row's `android:id/title`)
  is tapped at its centre and opens its page. A node from a closed shade
  is `StaleNode`. A 1.5 s `wait_for` with no match is `Timeout` after
  1.6 s, and a zero timeout answers at once. `windows()` lists the
  taskbar, status bar and app with kind, layer, bounds, package and
  title. Events report windows added, removed, moved and refocused, and
  each activity or dialog opening with its package and class.

Not verified yet:

- Android's MediaProjection prompt for another app ("Start recording or
  casting with …?"). No Android app in this repo requests MediaProjection,
  so the check above used SystemUI's own screen-record dialog. On a
  device, `find(Query::new().package("com.android.systemui"))` while the
  prompt is up shows its buttons and their ids.

- The target CrewForge tablet (Lenovo TB336FU, Android 16) under
  device-owner lock task. The kiosk's lock blocked launching the test app
  there; the service itself installed and bound on it.
- `Recents`, `LockScreen` and `QuickSettings` (the emulator's launcher
  draws Recents inside itself, so the check was inconclusive).
