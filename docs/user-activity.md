# User activity

`user_activity()` tells app code that the person is using the app. It
ticks on every touch, click, key press, scroll, wheel or keystroke in a
text field, anywhere in the app, including inside modals, sheets and other
portals. It is what an idle clock needs: a kiosk's "Still there?" prompt,
a session timeout, or auto-lock after a few minutes.

An app can't build this from its own views. On native platforms a touch
reaches a parent's `on_touch` only if the view under the finger passes it
up, and buttons, text fields and scroll views keep theirs. Portals mount
outside the app's root view, so nothing the app draws sees touches inside
them. Each backend therefore watches input at the window level instead.

```rust
use runtime_core::{after_ms_scoped, effect, signal, user_activity};

const IDLE_SECONDS: u32 = 60;

let activity = user_activity();          // ReadSignal<u64>
let remaining = signal(IDLE_SECONDS);

// Every tick restarts the clock.
effect!({
    activity.get();
    remaining.set(IDLE_SECONDS);
});
```

`examples/user-activity` is a complete idle clock with a button, a scroll
view, a text field and an overlay.

## API

| Item | What it does |
| --- | --- |
| `user_activity()` | A `ReadSignal<u64>` counting activity ticks (`0` before the first). Reading it subscribes, so an effect or `ui!` re-runs on each tick. Compare values only for change; the number itself has no meaning beyond "it moved". |
| `add_user_activity_listener(f)` | Runs `f` on every tick until the returned `UserActivityListener` guard drops. Use it from code that has no reactive scope, such as a session service owned by a struct. `f` runs inside the platform's event callback; a signal it sets commits on the backend's next flush. |

**What counts:** a pointer, finger or pen going down; a key press; a wheel
or scroll; a change to a text field's text from typing. **What doesn't:**
releases, pointer movement and hover. A mouse resting on a desk can
jitter, and hovering shouldn't hold off a timeout.

**Throttled.** At most one tick per frame (16.7 ms), so a scroll or drag
doesn't re-run subscribers on every event. An idle clock measured in
seconds can't tell the difference.

**Observe only.** Watching never consumes, delays or reorders an event.
Buttons still press, scroll views still scroll and text fields still take
focus exactly as they would without it.

## Per-backend mechanism

| Backend | Touch / click | Keys and typing | Scroll / wheel |
| --- | --- | --- | --- |
| Web | capture-phase passive `pointerdown` / `touchstart` on `document` | `keydown`, plus `input` for soft keyboards that send no usable `keydown` | `wheel`, and `scroll` from any element (captured at `document`) |
| iOS | a gesture recognizer on every app window: it reports in `touchesBegan:` and fails at once, so it never holds, delays or cancels a touch. New windows are picked up through `UIWindowDidBecomeVisibleNotification`; UIKit's private keyboard windows are skipped | text-field and text-view change notifications (soft-keyboard taps happen in another process); hardware keys through the app key source while one is installed | finger scrolls start with a touch; iPad mouse wheel through the app pointer source while one is installed |
| Android | `Window.Callback` wrapper on the Activity window that reports and then calls through: `ACTION_DOWN`, `ACTION_POINTER_DOWN`, `ACTION_BUTTON_PRESS` | `dispatchKeyEvent` key downs; soft-keyboard typing from the text inputs' `TextWatcher` (app-set text excluded) | generic-motion `ACTION_SCROLL` |
| macOS | `NSEvent` local monitor (left / right / other mouse down, trackpad gestures) that returns the event unchanged | key down through the same monitor (typing in a text field included) | scroll wheel through the same monitor |
| Linux (GTK4) | a capture-phase `EventControllerLegacy` on the host window that always returns `Proceed`: button press, touch begin, touchpad swipe / pinch, pad button. Re-added as the newest controller whenever the app key or pointer source installs, because GTK runs capture controllers newest-first and a source that claims an event hides it from older ones | key press through the same controller | scroll through the same controller |
| Windows | the host message pump, before dispatch, for every window on the thread: L/R/M/X button down and double-click, `WM_POINTERDOWN` | `WM_KEYDOWN`, `WM_SYSKEYDOWN` | `WM_MOUSEWHEEL`, `WM_MOUSEHWHEEL`, `WM_POINTERWHEEL`, `WM_POINTERHWHEEL` |
| wgpu | the engine's `Host::pointer_down`, which every GPU shell (winit, web canvas, mobile) reaches through `EventSink` | `Host::key` presses | `Host::scroll` |
| Terminal | mouse down in the backend's pointer dispatch | key presses, auto-repeat included | mouse wheel |

On every backend, key presses and mouse presses or wheel events that reach
the app-wide key and mouse sources (`on_key`, `on_pointer`) also count,
while those sources are installed.

Under runtime-server dev, author code runs in the dev server, not on the
device. The device's shell sends `AppToDev::UserActivity` (wire protocol
23) whenever its count moves, and the server ticks the session's
`user_activity()` once per message. This works the same as `--local`.

**Known limits.**
- macOS: events that an AppKit tracking loop takes (an open menu, a
  slider drag, a window drag) don't reach the monitor. The click that
  started the loop still counts.
- Android: a `Dialog` window opened by a third-party SDK has its own
  window and isn't covered. The framework's own portals are views in the
  Activity window, so they are covered.
- iOS: a hardware key pressed outside a text field counts only while an
  app key listener is installed. Typing into a field always counts.
- wgpu on winit: the shell ignores `WindowEvent::Touch`, so finger touches
  there reach neither the app nor this signal. Mouse, keys and wheel count.

## Writing a backend

- Install one observer at boot, unconditionally. Unlike the app key
  source, it has no side effects, so it is always on.
- Call `runtime_shared::user_activity::report()` from the raw platform
  callback. When it returns `true` (it ticked rather than being
  throttled), schedule the backend's flush so world signals commit.
- Never consume, delay or reorder the event. Report, then let it continue
  exactly as before.
- Cover every window the backend opens (portals, panels, a second scene),
  not only the main one.
