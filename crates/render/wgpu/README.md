# render-wgpu

Custom rendering: a [`wgpu`](https://wgpu.rs/)-backed `framework_core::Backend`
implementation that paints the entire UI through a GPU pipeline.

**No `winit`. No browser deps.** Any native shell that translates its
platform events into the [`render-api`](../api) event vocabulary and provides
a `wgpu::Surface` can drive this backend. The shells in
[`../../host/`](../../host) (`appkit`, `winit`, `web`) are reference
implementations; new platforms slot in next to them.

## Architecture

- **`backend_impl::WgpuBackend`**: the `framework_core::Backend` trait
  impl. Builds and mutates the node tree + Taffy layout tree. Owns the
  animator and the shared text + font-system stores.
- **`Host`**: interaction state (focus, press, drag, momentum, keyboard
  slide) + the `EventSink` impl. The native shell talks to the render side
  only through this trait.
- **`Renderer`**: wgpu pipeline + tree walker. Renders one frame into a
  `wgpu::TextureView`.
- **`animation::Animator`**: tween engine used by both widget animations
  (toggle thumb) and style-driven transitions (theme crossfade).
- **`Skin`**: the pluggable platform skin contract. Concrete skins
  ([`ios-sim`](../../skin/ios-sim), [`android-sim`](../../skin/android-sim))
  live in their own crates; the renderer holds an `Rc<dyn Skin>` and
  dispatches every widget + keyboard paint call through it.
- **`scheduler::install_redraw_hook`**: the shell installs its redraw
  closure here; render-side state changes call `request_redraw()` to wake
  it.

## What this lets you do

- **Look-alike rendering across desktops.** With the `ios-sim` or
  `android-sim` skin, a macOS / Windows / Linux build paints a UI that
  looks like the native mobile platform. Useful for designing on desktop
  without the simulator overhead.
- **Embed UI inside something else.** Because nothing here depends on a
  particular OS shell, the renderer can be embedded as a wgpu pass inside
  a larger application (a game engine, a CAD tool, an Electron alternative).
- **Drive a window-less render target.** Any `wgpu::TextureView` works.
  Useful for tests, snapshot diffing, server-side prerender.

## Per-frame pipeline (rough)

1. Render walker has populated the node tree + Taffy layout tree via the
   `Backend` impl on `WgpuBackend`.
2. Animator advances tween values for the current frame's timestamp.
3. Tree walker classifies each node, hands paint off to the active `Skin`,
   builds vertex buffers per pipeline (rect, image, text, gradient,
   shadow, device-frame, …).
4. wgpu encodes commands and submits. The shell presents.

The `host::Host` receives platform events between frames and updates
interaction state. `request_redraw` is what shells listen for to know a
frame is needed.

## Animated property writes

`WgpuViewOps` / `WgpuTextOps` **must override** `set_animated_f32` /
`set_animated_color`. The trait defaults are silent no-ops; without the
overrides, every `AnimatedValue::bind` write is dropped. See
`project_wgpu_viewops_animated` in memory.

## Debug-stats

The crate participates in the framework's `debug-stats` feature
(`framework-core/debug-stats`). With it enabled, `PhaseTimer` calls report
microsecond durations into a thread-local map; `framework_core::debug::take_phase_counters()`
drains them. See project CLAUDE.md §6 for the full timing-instrumentation
guide, including the **`install_time_source()` requirement** without which
durations come back as 0.

## Status

Listed as "In progress" in the root README's roadmap. The `Backend` trait
implementation covers all the primitives the trait can name, but the GPU
side (skins, pipelines, text shaping) is still under active development.
Expect rough edges on advanced primitives until the rendering side
catches up to the structural side.
