# App state and mouse input

Two things a game or a full-screen app needs beyond views:

- **The app's situation, as signals**: whether it has focus, whether it's
  on screen, the user's reduce-motion setting, whether the device is
  touch-only, whether the user has interacted yet (browsers allow sound
  only after that), and whether the pointer is locked.
- **App-wide mouse input**: buttons, movement and wheel anywhere in the
  app, plus **pointer lock** (the cursor is hidden and held in place and
  only movement amounts are reported, which first-person cameras need).

For keys, see [`keyboard-input.md`](keyboard-input.md). This is the same
design for the mouse. To know whether the person is using the app at all
(idle clocks, session timeouts), see [`user-activity.md`](user-activity.md).

```rust
use runtime_core::{
    app_focused, on_pointer, pointer_locked, pointer_state, raf_loop_scoped,
    request_pointer_lock, MouseButton, PointerOutcome, PointerPhase,
};

let locked = pointer_locked();   // ReadSignal<bool>
let focused = app_focused();     // ReadSignal<bool>: auto-pause on false

// Click to play: must be called from a click/key handler.
// button(label = "Play", on_click = move || request_pointer_lock())

let mouse = pointer_state();
raf_loop_scoped(move || {
    let (dx, dy) = mouse.take_movement(); // mouse motion since last frame
    // turn the camera by dx, dy
});

on_pointer(move |e| match (e.phase, e.button) {
    (PointerPhase::Down, Some(MouseButton::Primary)) => { /* fire */ PointerOutcome::PreventDefault }
    (PointerPhase::Down, Some(MouseButton::Secondary)) => PointerOutcome::PreventDefault, // no context menu
    (PointerPhase::Wheel, _) => { /* e.wheel_dy */ PointerOutcome::PreventDefault }      // no page scroll
    _ => PointerOutcome::Default,
});
```

The `pointer_lock_game_controls` recipe in the MCP catalog is a complete,
compile-checked version of this.

## App state signals

All are `ReadSignal`s; effects and `ui!` re-run when they change.

| Signal | Meaning |
| --- | --- |
| `app_focused()` | The app's window is the active window or tab. `false` after alt-tab, switching tabs, or clicking another app. |
| `app_visible()` | The app is on screen. `false` for a background tab, a minimized window, or a backgrounded mobile app. |
| `reduce_motion()` | The OS "reduce motion" accessibility setting. Follows changes live. |
| `primary_pointer()` | `PointerKind::Fine` (mouse, trackpad), `Coarse` (finger: a touch-only device), or `None` (no pointer: a TV). |
| `user_activated()` | The user has pressed, clicked or typed. Browsers only allow audio after this. Always `true` on native. Never goes back to `false`. |
| `pointer_locked()` | The pointer is locked. Goes `false` when the platform drops the lock (Esc, alt-tab), not only when the app releases it. |

## Mouse API

| Item | What it does |
| --- | --- |
| `on_pointer(f)` | Every mouse, trackpad or pen event anywhere in the app while the calling component lives: `Down`/`Up` (with `button`), `Move` (`x`, `y`, and `dx`, `dy`), `Wheel` (`wheel_dx`, `wheel_dy`). Finger touches are not included; they go to the view-level touch and gesture APIs. |
| `pointer_state()` | A `PointerState` handle to poll from a frame loop: `is_down(button)`, `take_movement()`, `take_wheel()`, `position()`, `is_locked()`, `clear()`. Each handle has its own movement and wheel accumulators. |
| `add_pointer_listener(f)` | `on_pointer` with an explicit lifetime: the listener lives until the returned guard drops. |
| `request_pointer_lock()` / `exit_pointer_lock()` | Ask for or release the lock. Call `request_pointer_lock` from a click or key handler. The result arrives as `pointer_locked()`. |
| `set_cursor_hidden(bool)` | Hide or show the cursor over the app. A locked pointer is always hidden. |

Units are logical pixels relative to the app's root. While locked the
position doesn't change; `dx`/`dy` (and `take_movement()`) carry the
motion, unaccelerated where the platform offers raw input. A positive
`wheel_dy` means the wheel turned toward the user (content scrolls down),
the same sign as the web's `WheelEvent.deltaY`.

Returning `PointerOutcome::PreventDefault` suppresses the platform's
default for that event: focus change and text selection on a press, the
context menu on a right-button press, page scroll on a wheel. Every
listener sees every event; the default is prevented if any listener asks.

## Guarantees on every backend

- **No stuck buttons.** When the window loses focus, the dispatcher sends
  an `Up` for every held button.
- **Lock loss is reported.** A lock the platform drops flips
  `pointer_locked()` to `false`, and nothing re-locks behind the user's
  back. The app decides whether to ask again.
- **Requests are immediate.** `request_pointer_lock()` reaches the
  platform inside the calling event handler. Browsers only grant a lock
  requested during a user gesture.
- **Nothing installed until asked.** The platform mouse source exists only
  while a listener, a lock or a hidden cursor needs it.

## Per-backend mechanism

### App state

| | Focused | Visible | Reduce motion | Primary pointer |
| --- | --- | --- | --- | --- |
| Web | `document.hasFocus()`; window `focus`/`blur` | `visibilityState`; `visibilitychange` | `matchMedia('(prefers-reduced-motion: reduce)')` | `matchMedia('(pointer: fine/coarse)')`; neither → `None` |
| macOS | `NSApp.isActive`; become/resign active | not hidden and window not occluded or minimized | `accessibilityDisplayShouldReduceMotion` (live) | `Fine` |
| iOS | did become / will resign active | entered background / will enter foreground | `UIAccessibilityIsReduceMotionEnabled` (live) | `Coarse`; `Fine` while a mouse (`GCMouse`) is connected |
| Android | root window focus | Activity started/stopped | animator duration scale is 0 (live) | `Fine` with a mouse connected, `Coarse` with a touchscreen, else `None` |
| Linux (GTK4) | window `is-active` | mapped and not minimized | `gtk-enable-animations` off (live) | seat capabilities |
| Windows | `WM_ACTIVATE` | `WM_SIZE` minimized/restored | `SPI_GETCLIENTAREAANIMATION` off (live) | `SM_MOUSEPRESENT` → `Fine`, touch digitizer → `Coarse`, else `None` |
| wgpu (winit) | `WindowEvent::Focused` | `WindowEvent::Occluded` | not reported (winit has no API) | `Fine` |
| Terminal | terminal focus reporting | not reported | not reported | `Fine` |

`user_activated()` is only ever `false` on web: from page load until the
first `pointerdown`, `mousedown`, `keydown` or `touchend`. If the browser
remembers an earlier gesture (`navigator.userActivation.hasBeenActive`),
it starts `true`.

### Mouse

| | Source | Pointer lock | Hidden cursor |
| --- | --- | --- | --- |
| Web | `mousedown`/`mouseup`, `pointermove`, `wheel` on `document` (capture) | `requestPointerLock()` on the app root; `pointerlockchange`/`pointerlockerror`. Movement is the browser's (accelerated) `movementX/Y` | `cursor: none` on the app root and `<html>` |
| macOS | `NSEvent` local monitor | cursor detached from the mouse and hidden; only while the app is active; dropped on deactivation or Esc | `NSCursor hide` (applies while the app is active) |
| iOS | `GCMouse` (buttons, raw motion), hover recognizer (position) | `prefersPointerLocked` on the host view controller; iPad only, full screen, with a mouse | hidden pointer style on the root |
| Android | `Window.Callback` wrapper (mouse, pen, touchpad; not fingers) | `requestPointerCapture()` (Android 8+), raw counts | an invisible top view with the null pointer icon (Android 7+) |
| Linux (GTK4) | capture-phase `EventControllerLegacy` on the window | X11: confined pointer grab with warp-to-centre. **Wayland: unsupported** (GTK4 doesn't expose the protocols); a request reports `false` | cursor `none` |
| Windows | host message pump, before dispatch | `ClipCursor` to the window centre plus raw input (`WM_INPUT`), unaccelerated | `WM_SETCURSOR` → no cursor over the client area |
| wgpu (winit) | winit mouse events | cursor grab `Locked`, else `Confined` plus re-centring; raw `DeviceEvent::MouseMotion` | `set_cursor_visible(false)` |
| Terminal | crossterm mouse capture; positions in cells converted to layout px | unsupported | not applicable |

A notched wheel scrolls `WHEEL_NOTCH_PX` (40 logical px) per notch on every
native backend. Trackpads report pixels. Web line-mode deltas use 16 px
per line, and terminals scroll 3 cells per notch.

**Known limits.** On iOS, `PreventDefault` has no effect: GameController
input can observe the mouse but not cancel UIKit's handling. On Android,
a separate dialog window opened by an SDK isn't covered, and the mouse's
back/forward buttons also send back/forward key events.

## Writing a backend

- Report host facts with `runtime_shared::app_state::report(AppStateChange::…)`
  from platform callbacks (and once at boot), then schedule the backend's
  flush. Report only what the platform knows; the defaults describe a
  focused, visible desktop app with a mouse.
- Implement `AppEnvOps::set_pointer_sink`, `set_pointer_lock` and
  `set_cursor_hidden`. Their doc comments in
  `crates/runtime/vocabulary/src/caps/app.rs` are the contract.
- Never hold a borrow of backend state while calling the sink.
- A platform without pointer lock leaves `set_pointer_lock` doing nothing
  and never reports a lock.
