+++
title = "Backends and Cross-Platform Notes"
order = 60
tags = ["backends", "platform"]
+++

# Backends

Idealyst targets four backends, each via its own native toolkit:

| Backend | Toolkit | Notes |
|---------|---------|-------|
| iOS     | UIKit   | `UIView` tree; Taffy drives layout; per-touch event delivery |
| Android | View    | `FrameLayout` tree; Taffy drives layout via `MarginLayoutParams` |
| Web     | DOM     | `<div>` tree; CSS used for layout/style; signals drive via wasm |
| macOS   | wgpu    | GPU-rendered; aspect-locked windows; not all primitives supported (no [[Video]]) |

## The contract

**One author tree, identical observable behavior.** Backend implementations diverge in mechanism but converge in output. Per [[backend_owns_rendering]]:

- No `if platform == X { … }` workarounds in framework or backend code to make a feature work elsewhere.
- No `is_simulator()` predicate — sim vs. device is dev-only and inconsistent across backends.
- Author code branching on [[Platform]] is fine for *legitimate* variance (keyboard shortcuts, platform copy). Branching to paper over rendering differences is a smell — fix the upstream backend.

## When you need platform branching

Reach for the [[platform]] utility:

```rust
let p = runtime_core::platform();
let shortcut_text = match p {
    Platform::MacOs => "⌘S",
    Platform::Web => "Ctrl+S",
    _ => "",
};
```

For dev-only markers (debug ribbons, perf overlays), use `#[cfg(debug_assertions)]` — these should not survive into release.

## Known per-backend quirks

The framework absorbs many of these so authors don't see them; they show up in the codebase as backend-internal subtleties:

- **iOS** — `UIScrollView` bounds.origin preservation ([[ios_scrollview_bounds_origin]]); image tinting needs explicit `imageWithRenderingMode` ([[ios_uiimageview_tinting]]); `cornerRadius` clamping ([[ios_cornerradius_unclamped]]); intrinsic-size measurer for UIKit controls ([[ios_intrinsic_size_measurer]]).
- **Android** — translation values are in device px not dp ([[android_setTranslation_device_px]]); scheduler handles must be `mem::forget`ted ([[android_scheduler_handle_leak]]); gradient radius computed post-layout ([[android_gradient_radius_post_layout]]).
- **Web** — must call `install_scheduler()` + `install_time_source()` at startup ([[web_bootstrap_scheduler]]); animations need `install_global_self` ([[web_install_global_self_for_animation]]).
- **macOS / wgpu** — aspect-locked windows via NSWindowDelegate ([[wgpu_aspect_lock]]); AppKit/UIKit naming and feature-gate differences ([[macos_appkit_uikit_diffs]]).

These all live in the relevant backend crate; authors writing component code don't see them.
