# `keep-awake`

Keep the **display on**, at its normal brightness, while some part of the app
needs it: a kiosk, a recipe, a navigation screen, a dashboard.

```rust
// For as long as the guard lives.
let awake = keep_awake::keep_screen_on();

// In a component: while it is mounted and the condition holds.
keep_awake::use_keep_screen_on(rx!(shift_open.get()));
```

## What you get

- `keep_screen_on() -> KeepAwake`: a hold, released on drop. Holds are
  counted; the display's normal timeout returns with the last one.
- `use_keep_screen_on(on)`: a hold owned by the calling component's scope.
  `on` is a `bool`, a signal, a memo or an `rx!`.
- `is_supported() -> bool`: `false` where every hold does nothing.
- `install_keep_awake_labels(KeepAwakeLabels)`: the reason a desktop may show
  beside the app ("Keeping the screen on").

Two rules hold on every platform:

- **Only while the app is on screen.** A hold has no effect while the app is
  in the background, minimized or in a hidden tab, and applies again when it
  returns. A desktop window that is visible but not focused still counts.
- **Not a lock.** It stops the idle timeout. The power button, the lid and the
  lock shortcut work as ever.

Call it on the UI thread; the guard is not `Send`.

## Per-platform mechanism

| Target | Mechanism |
| --- | --- |
| Android | `FLAG_KEEP_SCREEN_ON` on the app's Activity windows, reapplied to an Activity the system recreates. Also stops the dimming before the timeout |
| iOS | `UIApplication.isIdleTimerDisabled` |
| macOS | IOKit `PreventUserIdleDisplaySleep` assertion (`pmset -g assertions`) |
| Windows | `PowerRequestDisplayRequired` power request (`powercfg /requests`) |
| Linux | `org.freedesktop.portal.Inhibit` (idle), else `org.freedesktop.ScreenSaver`. Neither (a bare window manager): `is_supported()` is `false` |
| web | `navigator.wakeLock.request("screen")`, asked again when the page is shown. Needs a secure context. A refusal (low battery, a permissions policy, before the first gesture in Safari) is logged |
| other, and a server rendering for the web | nothing; `is_supported()` is `false` |

## Permissions

None, on any platform.

## Scope

The display only. Keeping the CPU or network alive, the brightness, and
device-wide timeouts (device owner) are out of scope.

## Testing checklist

**Automated**
- [x] `cargo test -p keep-awake`: counting, visibility, scope teardown, server render; on macOS the real assertion through `pmset`
- [x] `cargo test -p keep-awake --target wasm32-unknown-unknown`: the wake lock against a fake `navigator.wakeLock`
- [x] `cargo test -p keep-awake --target x86_64-pc-windows-gnu --lib`: the power request (Wine)

**Behavior** (`examples/keep-awake-demo`)
- [x] **Android** emulators (API 34 and 37, 30 s timeout): stays bright while held, on a charger and with the battery unplugged; sleeps 30 s after the last release; one of two guards keeps it; an unmounted component releases; a hold survives an Activity recreation (an app-locale change) and a rotation; released in the background and held again on return; holds under device-owner lock task; the power button still turns it off
- [ ] **Android** hardware: the dimming itself on a real panel
- [ ] **iOS**: with auto-lock at 30 s, on a device
- [ ] **Linux**: GNOME and KDE sessions
- [ ] **Windows**: `powercfg /requests` on a PC
- [ ] **Web**: the real Wake Lock API in Chrome and Safari
