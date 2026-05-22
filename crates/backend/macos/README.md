# backend-macos

macOS backend: builds AppKit views via [`objc2`](https://crates.io/crates/objc2)
under `target_os = "macos"`. Provides a type-checking stub on other hosts so
cross-compile + workspace-wide `cargo check` works from any platform.

## Status

The backend is a **structural skeleton**: `create_view`, `create_text`,
`create_button`, scheduler integration, animated property writes, theme
plumbing. Most of the primitive surface (`Image`, `TextInput`, `ScrollView`,
`Slider`, `Toggle`, `Icon`, `ActivityIndicator`, `Video`, `Virtualizer`,
`Graphics`) is **not yet implemented** — the trait defaults will
`unimplemented!()` if your app reaches for them.

If you want a polished cross-platform desktop story today, the
[`render-wgpu`](../../render/wgpu) renderer hosted on `appkit` / `winit` is
a more complete path. The macOS-native AppKit path is being built up
incrementally as the framework's primitive vocabulary stabilises.

Design notes live in [`docs/macos-backend-plan.md`](../../../docs/macos-backend-plan.md).

## Bootstrap

```rust
backend_macos::install_scheduler();
let backend = MacosBackend::new(...);
backend_macos::install_global_self(&backend); // for AnimatedValue::bind
```

The scheduler is NSTimer-backed; on macOS it forwards to the shared
`backend_apple_core::scheduler::install_scheduler` — the same code the iOS
backend uses.

## AppKit ≠ UIKit gotchas

The two frameworks look alike but aren't — patterns that work in
[`../ios/mobile`](../ios/mobile) need adjustment here. From experience so
far (also captured in `project_macos_appkit_uikit_diffs` in memory):

- **`setMasksToBounds` is UIView-only** — `NSView` has no such method.
  Use `layer.setMasksToBounds` after enabling layer backing.
- **`NSView` is layer-optional.** You must call `setWantsLayer:true` before
  touching `layer.*` properties; otherwise the layer is nil and writes
  silently no-op.
- **`CGColor` needs the `objc2-foundation` `Encode` wrapper** to be passed
  through an objc2-typed Objective-C method.
- **`objc2-foundation` feature gates.** `MainThreadMarker::new`,
  `NSWindow::initWithContentRect:...`, `NSApplication::setActivationPolicy:`
  all live behind features that must be enabled in `Cargo.toml`.

These aren't bugs the backend goes out of its way to work around — they're
the *first* place a primitive port from iOS will trip if you assume the
APIs match.

## Window-shell layering

A macOS app is more than a tree of views — it's a window, a menu bar, a
delegate. The framework intentionally keeps those concerns *out* of the
core `Backend` trait (see `feedback_mobile_first_philosophy` in memory).
Window / menu / multi-window plumbing lives in [`../../host/appkit`](../../host/appkit),
which composes the backend with the AppKit shell. Author code does not
participate.
