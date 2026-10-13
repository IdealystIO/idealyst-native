# backend-macos

macOS backend: builds AppKit views via [`objc2`](https://crates.io/crates/objc2)
under `target_os = "macos"`. Provides a type-checking stub on other hosts so
cross-compile + workspace-wide `cargo check` works from any platform.

## Status

`src/newcore.rs` implements `runtime_scene::Host` plus all 30
`runtime_vocabulary::caps` traits on `MacosBackend`, delegating to the
AppKit mechanism code in `src/imp/`. The
[GPU backend](../../gpu-backend) hosted on `appkit` / `winit` remains an
alternative desktop path — it paints its own pixels instead of driving
AppKit views.

Design notes live in [`docs/macos-backend-plan.md`](../../../docs/macos-backend-plan.md).

## Bootstrap

```rust
backend_macos::install_scheduler();
let backend = MacosBackend::new(...);
backend_macos::install_global_self(&backend); // for AnimatedValue::bind
```

The scheduler is NSTimer-backed; on macOS it forwards to the shared
`backend_apple_core::scheduler::install_scheduler`, the same code the iOS
backend uses.

## AppKit ≠ UIKit gotchas

The two frameworks look alike but aren't; patterns that work in
[`../ios/mobile`](../ios/mobile) need adjustment here. From experience so
far (also captured in `project_macos_appkit_uikit_diffs` in memory):

- **`setMasksToBounds` is UIView-only.** `NSView` has no such method.
  Use `layer.setMasksToBounds` after enabling layer backing.
- **`NSView` is layer-optional.** You must call `setWantsLayer:true` before
  touching `layer.*` properties; otherwise the layer is nil and writes
  silently no-op.
- **`CGColor` needs the `objc2-foundation` `Encode` wrapper** to be passed
  through an objc2-typed Objective-C method.
- **`objc2-foundation` feature gates.** `MainThreadMarker::new`,
  `NSWindow::initWithContentRect:...`, `NSApplication::setActivationPolicy:`
  all live behind features that must be enabled in `Cargo.toml`.

These aren't bugs the backend goes out of its way to work around. They're
the *first* place a primitive port from iOS will trip if you assume the
APIs match.

## Window-shell layering

A macOS app is more than a tree of views: it's a window, a menu bar, a
delegate. The framework intentionally keeps those concerns *out* of the
`Host` seam and the capability traits (see
`feedback_mobile_first_philosophy` in memory).
Window / menu / multi-window plumbing lives in [`host-appkit`](../../gpu-backend/host/appkit),
which composes the backend with the AppKit shell. Author code does not
participate.

## App situation, app-wide pointer, user activity

`AppEnvOps::set_pointer_sink` / `set_pointer_lock` / `set_cursor_hidden`
and the app-state reports (`runtime_shared::app_state`) — mechanism in
`src/imp/pointer.rs` and `src/imp/app_state.rs`, pure translation in
`src/pointer_policy.rs`:

- **Pointer source**: one `NSEvent` local monitor (left/right/other down,
  up, dragged; mouse-moved; scroll-wheel). Positions are host-root
  relative, top-left origin, points. Wheel deltas are negated to web's
  `deltaY` sign; a non-precise (notched) wheel's lines × 40 px. Returning
  `nil` honors `PreventDefault` on a down (no `mouseDown:` /
  `rightMouseDown:` → no focus change, selection, or `menuForEvent:`
  context menu) and on a wheel (no scroll). `acceptsMouseMovedEvents` is
  turned on for the host window while installed.
- **Eaten releases**: a control's tracking loop (NSButton, NSSlider, text
  selection, title-bar drag) consumes its mouse-up before any local
  monitor sees it; a 100 ms poll against `+[NSEvent pressedMouseButtons]`
  synthesizes the missing `Up` while a button is held.
- **Lock**: `CGAssociateMouseAndMouseCursorPosition(false)` + a balanced
  `+[NSCursor hide]`; motion keeps arriving as `deltaX`/`deltaY`. Granted
  synchronously and only while the app is active (disassociation is
  session-wide). Dropped and reported on app resign-active, window
  resign-key, and Esc (web parity). Released on teardown and
  `NewCoreApp::stop`.
- **Cursor hiding**: balanced `+[NSCursor hide]`/`unhide` (process-wide
  while the app is active).
- **App state**: `NSApp.isActive` (focus), `!isHidden && occlusionState &
  Visible` (visibility), `NSWorkspace.accessibilityDisplayShouldReduceMotion`
  (followed via the workspace's own notification center), primary pointer
  `Fine`.
- **User activity** (`runtime_shared::user_activity`, `src/imp/user_activity.rs`):
  one more `NSEvent` local monitor, installed at boot unconditionally, for
  left/right/other down, key down, scroll wheel and trackpad
  magnify/rotate/swipe. It reports and always returns the event unchanged,
  and sees every window of the app (portals, panels, sheets, a focused text
  field's field editor). Events eaten by a nested tracking loop (open menu,
  slider drag) are invisible, but the press that started the loop is
  counted. The flush is scheduled only when a report ticks (throttled to
  one tick per frame).

`tests/app_pointer.rs` drives all of it with real AppKit events
(`cargo test -p backend-macos --test app_pointer`); `tests/app_user_activity.rs`
covers the activity monitor.
