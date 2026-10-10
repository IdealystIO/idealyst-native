# Soft keyboard

> For hardware key presses and releases (game controls, shortcuts), see
> [`keyboard-input.md`](keyboard-input.md).

Two pieces:

- **`keyboard_avoiding_view`** is a core primitive. Content inside it stays
  clear of the on-screen keyboard, moving **in step with the platform's own
  keyboard animation**.
- **`keyboard_inset()`** is a reactive value for app code that reacts to the
  keyboard itself.

Nothing avoids the keyboard unless you wrap it. The app viewport never
changes for the keyboard. `examples/keyboard-avoid` is the runnable demo.

```rust
use runtime_core::{keyboard_avoiding_view, ui, Element, KeyboardAvoidBehavior, SafeAreaSides};

pub fn app() -> Element {
    ui! {
        // App-wide: wrap the root.
        keyboard_avoiding_view(style = Screen()) {
            scroll_view { Messages() }
            view(safe_area = SafeAreaSides::BOTTOM) { Composer() }
        }
    }
}

// A form that should lift without re-laying out, and jump instead of animate:
ui! {
    keyboard_avoiding_view(behavior = KeyboardAvoidBehavior::Translate, animated = false) {
        LoginForm()
    }
}
```

## `keyboard_avoiding_view`

A `view` that also accepts `behavior` and `animated`. It takes the same
`style`, `safe_area`, `test_id`, a11y and `bind` props as `view`.

| Prop | Default | Meaning |
| --- | --- | --- |
| `behavior` | `Padding` | `Padding`: the view's bottom padding grows by the covered height, so its content area ends at the keyboard's top (a scroll area gets shorter and a composer sits on the keyboard). `Translate`: the whole view lifts by the covered height and nothing re-lays out. This is the cheapest mode, for forms, modals and sheets. |
| `animated` | `true` | `true`: move with the keyboard's own animation. `false`: jump to the final position when the keyboard starts to move. |

- **It measures its own box.** It moves only by how much of *itself* the
  keyboard covers. A view that ends above a tab bar moves less than one
  that reaches the bottom of the screen, and a centered card only moves if
  the keyboard reaches it.
- **Bottom safe area collapses in `Padding` mode.** Inside a `Padding`
  avoider, `.safe_area(BOTTOM)` insets on the view and its descendants
  shrink by the covered height (`runtime_shared::bottom_inset_under_keyboard`).
  The home indicator they reserve room for is behind the keyboard, so a
  composer sits flush on it with no gap. `Translate` doesn't re-lay out,
  so a bottom safe-area inset inside it stays, leaving that inset's height
  between the content and the keyboard. Use `Padding` for bottom-anchored
  composers.
- **Portals need their own avoider.** A portal (modal, sheet, overlay)
  mounts outside the app root, so a root avoider doesn't reach it. Wrap
  the portal's content. idea-ui's `Modal` already wraps its card in a
  `Translate` avoider. Anchored overlays (popovers, menus, autocomplete)
  follow their anchor and need nothing.
- **Don't put a transform on the avoider itself in `Translate` mode.** On
  Android and web its lift owns the view's `translationY` / `translate`.
  Put the transform on a child instead. (iOS composes the lift with the
  view's own transform.)
- **Backends without a soft keyboard** (macOS, desktop GPU hosts,
  terminal, SSR) render it as a plain `view`.

## `keyboard_inset()`

`keyboard_inset() -> ReadSignal<KeyboardInset>`

| Field | Meaning |
| --- | --- |
| `height` | How much of the app root's bottom edge the keyboard covers once its current animation settles. `0` when it's hidden. |
| `transition` | The keyboard animation's timing as a `Transition`. iOS and Android report the real duration with the platform keyboard curve as a cubic-bezier; on web it's an estimate. |
| `is_visible()` | `height > 0`. |

It updates **once per keyboard move**, with the value the keyboard is
heading to, never per frame. Use it to hide a tab bar while typing, or
pass `transition` to your own style transition to move something in step.
It stays `KeyboardInset::HIDDEN` on platforms without a soft keyboard.
`viewport_size()` is unaffected by the keyboard.

## Per-backend mechanism

The motion comes from the platform's keyboard animation. Rust never
interpolates it.

### iOS: inside UIKit's keyboard animation block

`KeyboardObserver` (`imp/callbacks.rs`) observes
`UIKeyboardWillChangeFrameNotification` and reads the end frame, duration
and curve. The curve is `7`, a private curve UIKit honours when shifted into
the options bitfield. On a change (`IosBackend::keyboard_moved`):

1. The new keyboard inset is reported to `keyboard_inset()`.
2. `animated = false` avoiders are applied first, outside any animation.
3. `animated` avoiders are applied **inside**
   `+[UIView animateWithDuration:delay:options:animations:completion:]`
   with the keyboard's duration and curve:
   - `Padding` sets the layout tree's keyboard padding on the node, then
     runs a layout pass in the same block.
   - `Translate` writes the lift into the view's transform. It's a separate
     `keyboard_ty` term, so it composes with author transforms and
     animations.

   Core Animation interpolates the new frames with the keyboard's exact
   timing, with no per-frame work. The notification arrives just before
   UIKit starts the keyboard animation, so both start together.

Invariants (`keyboard_frame_policy.rs`):

- **Never drop a frame.** The close notification often arrives while the
  backend is borrowed: a flush unmounting the focused field makes UIKit
  resign first responder synchronously. Frames go through a latest-wins
  mailbox, and the scheduled layout pass drains it and runs the move.
- **The animation block can't outlive its pointer.**
  `run_synchronously_within` hands the block a raw-pointer slot that is
  cleared as soon as `animateWithDuration:` returns.
- **First frames are not animated.** Inside the block, a never-framed view
  gets its first frame with `UIView.animationsEnabled = NO`, so it doesn't
  grow out of a zero rect.
- **Overlap is measured against the untransformed frame**, the last frame
  the layout pass wrote. A `Translate` lift therefore never feeds back into
  its own measurement. After every layout pass, avoiders whose frame moved
  under a raised keyboard (first mount, rotation) are reconciled, without
  animation.

### Android: the system IME animation drives Kotlin

`RustKeyboardInsets.kt`, on the host root, lays the window out edge to
edge (`setDecorFitsSystemWindows(false)` + `adjustResize`). That's the only
mode in which the system hands IME movement to the app as per-frame
animation callbacks. It re-applies the system-bar insets as **margins on
the host root**, as `decorFitsSystemWindows(true)` did, so app layout is
unchanged. It forwards every phase of the IME animation to each
`RustKeyboardAvoider.kt`, and reports the target to `keyboard_inset()`.

Per-frame work never enters Rust:

- **`Translate`**: the avoider's `translationY` follows the live IME inset
  every frame.
- **`Padding`**: Rust lays out **once** per keyboard move
  (`imp::soft_keyboard::begin_padding`) and returns only the views that
  moved relative to their parent, each with its old and new top in device
  px. Every frame, Kotlin moves each view's *visual* top from old to new by
  the animation's `interpolatedFraction`, writing
  `translationY = desired − getTop()`.

  It measures against the view's *actual* top because a layout pass writes
  `LayoutParams`, which only take effect at the next traversal. On any
  frame the view may still be at its old position or already at its new
  one, and assuming either made the close jump for two frames. A layout
  listener re-places a view whenever its layout lands.

  A translation can move views but not resize them, so the size change
  happens where the keyboard hides it
  (`soft_keyboard_policy::padding_plan`):
  - **Opening:** keep the current, larger layout during the animation and
    apply the smaller one at the end.
  - **Closing:** apply the larger layout immediately.

Overlap is measured against the view's untranslated bottom edge. Leaving
full-screen (`RustSystemUi.setFullscreen(false)`) keeps the window edge to
edge. `WindowInsetsAnimationCompat` backports the callbacks to API 21;
below API 30 it animates with its own estimate of the IME curve.

### Web (mobile browsers): best estimate

`backend-web/src/keyboard_source.rs` gets the keyboard size from:

- **the VirtualKeyboard API** (Chromium on Android), with `overlaysContent`
  set so the browser stops resizing and panning for it; or
- **`visualViewport`** (Safari): `clientHeight - visualViewport.height ×
  scale`, counted only while a text field is focused.

Each avoider element measures its own untranslated box against the
keyboard's top:

- `Padding` sets `padding-bottom` to the author's padding plus the overlap,
  with `box-sizing: border-box`.
- `Translate` sets the individual `translate` property.

Both use a CSS transition with the estimated timing
(`WEB_KEYBOARD_ESTIMATE_CHROMIUM`, 285 ms on the Android IME curve, or
`WEB_KEYBOARD_ESTIMATE`, 250 ms on the iOS curve). Browsers expose no
keyboard animation, so this is the one platform where the motion isn't the
platform's own. The avoider's inline `transition` replaces any author
transition on that same element.

## Runtime-server dev (`idealyst dev` without `--local`)

The opt-in crosses the wire as
`Command::MarkKeyboardAvoiding { node, behavior, animated }`
(PROTOCOL_VERSION 22). The client backend observes its own keyboard and
moves the view locally. The keyboard value crosses back as
`AppToDev::KeyboardChanged`, so the sidecar's `keyboard_inset()` follows.
Remote components carry `keyboard_avoid` on `Node::View`
(`CODEC_VERSION` 3).

## Migrating from the automatic avoidance

Before `keyboard_avoiding_view`, iOS and Android shrank the whole app
viewport by the keyboard, and on Android the system resized the window.
That's gone. To get the old behavior, wrap the app root in a
`keyboard_avoiding_view`, and wrap the content of any portal that holds
text fields.

## Where things live

| Piece | Location |
| --- | --- |
| `KeyboardAvoid`, `KeyboardInset`, curves, overlap and safe-area math | `crates/runtime/shared/src/keyboard.rs` |
| Primitive: builder, glue, `ui!` tag, handler call | `vocabulary` `builders/view.rs`, `glue.rs`, `handlers/view.rs`; `macros/src/ui.rs` |
| Capability | `SafeAreaOps::mark_keyboard_avoiding` (`vocabulary/src/caps/scroll.rs`) |
| Per-view padding and safe-area collapse | `LayoutTree::set_keyboard_padding` (`runtime-layout`) |
| `keyboard_inset()` ctx | `crates/runtime/vocabulary/src/keyboard.rs` |
| iOS | `backend-ios-mobile`: `imp/mod.rs` (`keyboard_moved`, avoiders), `keyboard_frame_policy.rs`, `imp/animated.rs` (`keyboard_ty`) |
| Android | `backend-android-mobile`: `imp/soft_keyboard.rs`, `soft_keyboard_policy.rs`, `RustKeyboardInsets.kt`, `RustKeyboardAvoider.kt` |
| Web | `backend-web/src/keyboard_source.rs` |
| Wire | `Command::MarkKeyboardAvoiding`, `AppToDev::KeyboardChanged` |
