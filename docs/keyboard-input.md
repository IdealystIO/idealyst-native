# Keyboard input

App-level keyboard input: every key **press and release** anywhere in the
app, regardless of which view has focus. Use it for game controls (WASD,
arrows, held keys) and app-wide shortcuts. For a focused text field's own
keys, use `text_input` / `text_area`'s `on_key_down` instead. For the
on-screen keyboard, see [`keyboard.md`](keyboard.md).

```rust
use runtime_core::{on_key, key_state, raf_loop_scoped, signal, ui, Element, KeyOutcome};

#[component]
fn Game() -> Element {
    let x = signal(0.0_f32);
    let jumps = signal(0u32);

    // Events: react to a press.
    on_key(move |e| {
        if e.is_press() && e.code == "Space" {
            jumps.update(|n| n + 1);
            return KeyOutcome::PreventDefault; // no page scroll on web
        }
        KeyOutcome::Default
    });

    // Polling: read held keys each frame.
    let keys = key_state();
    raf_loop_scoped(move || {
        if keys.any_down(&["KeyA", "ArrowLeft"]) { x.update(|v| v - 2.0) }
        if keys.any_down(&["KeyD", "ArrowRight"]) { x.update(|v| v + 2.0) }
    });

    ui! { /* … */ }
}
```

## The API

| Item | What it does |
| --- | --- |
| `on_key(f)` | Listen to every key down and up while the calling component (or effect run) lives. |
| `key_state()` | A `KeyState` handle: `is_down(code)`, `any_down(&[codes])`, `keys_down()`. Tracks held keys while the component lives. |
| `add_key_listener(f) -> KeyListener` | Same as `on_key` with an explicit lifetime: the listener lives until the guard drops. Use it outside a component (e.g. from a button's `on_click`); `on_key` there registers nothing and logs a warning. |
| `set_app_key_handler(Option<handler>)` | The older single-slot hook: key-downs only, one handler app-wide. Kept for existing code. |

Every listener sees every event. A key's default is prevented if any
listener returns `KeyOutcome::PreventDefault`. Listeners also see keys typed
into a focused text input, so act only on the keys you care about.

## `AppKeyEvent`

| Field | Meaning |
| --- | --- |
| `phase` | `KeyPhase::Down` or `KeyPhase::Up`. |
| `key` | What the key means under the user's layout and modifiers: Web [`KeyboardEvent.key`](https://developer.mozilla.org/en-US/docs/Web/API/UI_Events/Keyboard_event_key_values) (`"a"`, `"A"`, `" "`, `"ArrowUp"`, `"Shift"`). Use it for shortcuts. |
| `code` | Which physical key it is: Web [`KeyboardEvent.code`](https://developer.mozilla.org/en-US/docs/Web/API/UI_Events/Keyboard_event_code_values) (`"KeyW"`, `"Digit1"`, `"Space"`, `"ShiftLeft"`). Use it for game controls. WASD stays in place on AZERTY, and a key pressed with Shift and released without it keeps the same `code`. Empty when the platform can't identify the key. |
| `repeat` | `true` for an auto-repeat down while the key is held. `e.is_press()` is "down and not a repeat". |
| `shift` / `ctrl` / `alt` / `meta` | Modifier state. |

## Guarantees on every backend

The dispatcher (`runtime_shared::key_input`) owns these, so no backend
behaves differently:

- **Downs and ups.** Every key, including the modifier keys themselves.
- **Repeats are flagged.** A down for a key that is already held is a
  repeat, even on platforms that don't mark repeats (GTK, iOS).
- **No stuck keys.** When the window or app loses focus, platforms don't
  deliver the releases for keys still held. The dispatcher sends an `Up`
  for each held key (with modifiers cleared).
- **Immediate.** A listener works as soon as it's added, including one
  added from an event handler. The older handler waited for the next style
  flush and could silently never install.
- **Nothing installed until asked.** The platform key source only exists
  while at least one listener does. On iOS and Android the source has to
  take focus to receive hardware keys, so apps that never listen don't pay
  for that.

## Per-backend mechanism

| Backend | Key source | Focus loss |
| --- | --- | --- |
| Web | `keydown` / `keyup` on `document`; `key`, `code`, `repeat` come straight from the DOM | window `blur` |
| macOS | `NSEvent` local monitor for key down, key up and `flagsChanged` (modifiers); `code` from the virtual key code | app resign-active / window resign-key |
| iOS | invisible first-responder view: `pressesBegan` / `pressesEnded` / `pressesCancelled`; `code` from the HID usage | app will-resign-active, or the key responder resigning first responder (a text field took focus) |
| Android | root `View.OnKeyListener`, `ACTION_DOWN` + `ACTION_UP` (`repeatCount` → `repeat`); `code` from the evdev scan code (layouts remap keycodes — AZERTY's physical Q reports `KEYCODE_A`), falling back to `KEYCODE_*` when there is none | window focus loss, or focus moving off the root (a text input, modal, or D-pad move takes it) |
| Linux (GTK4) | capture-phase `EventControllerKey` on the window; `code` from the hardware (evdev) keycode | window `is-active` |
| Windows | `WM_KEYDOWN` / `WM_KEYUP` (+ `SYS` variants), read in the host's message pump before dispatch so keys aimed at a focused child control are seen too; `code` from the scancode | `WM_ACTIVATE(WA_INACTIVE)` |
| wgpu (winit) | winit `KeyboardInput`; `code` from `physical_key` | `WindowEvent::Focused(false)` |
| Terminal | crossterm key events. Releases need the kitty keyboard protocol (the Windows console reports them natively); on terminals without it, each press is followed immediately by a release, so held keys read as taps. `code` is derived from the key, not the physical key | crossterm `FocusLost` |

iOS and Android only receive **hardware** keyboard events here. The
on-screen keyboard types into text fields and does not produce app-level
key events. On both, a focused text field takes the keys for itself: while
it has focus, app-level listeners get nothing (held keys are released when
it takes focus, so nothing sticks), and app-level keys resume once focus
returns to the app's root view.

## Writing a backend

Implement `AppEnvOps::set_keyboard_sink(Option<KeyboardSink>)`. While a sink
is installed:

- Call `sink.key(&AppKeyEvent)` for every down and up, and swallow the native
  event when it returns `PreventDefault`.
- Call `sink.focus_lost()` when the app stops receiving keys.
- Never hold a borrow of backend state while calling the sink. A listener
  may add or remove listeners, which re-enters `set_keyboard_sink`.

Put the physical-code table in a pure function and unit-test it. The
contract is documented on the trait method in
`crates/runtime/vocabulary/src/caps/app.rs`.
