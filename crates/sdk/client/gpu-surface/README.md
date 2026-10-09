# gpu-surface

The shared wgpu plumbing behind the framework's GPU renderers (`canvas-vello`,
`canvas3d-wgpu`). It turns a `graphics` primitive into a working device and a
frame to draw into, and puts that frame on screen. Apps don't use it directly; a
renderer crate does.

## What it gives a renderer

```rust
// native: blocking bring-up on the deferred on_ready tick
gpu_surface::mount(
    backend,
    "my-renderer",                 // log / console label
    Requirements::NONE,            // adapter capabilities the pipeline needs
    FrameAlpha::Premultiplied,     // how the renderer's output stores alpha
    move || prim.paint(),          // runs in a reactive effect
    move |w, h| sizing.report(w, h), // logical size, before each render
    move |gpu| MyState::new(gpu),  // build per-surface state on the device
)
```

`MyState` implements `SurfaceState`: it owns the `GpuSurface`, draws each frame
into `gpu.target_view()`, and presents with `begin_present` (acquire + blit,
recorded into the frame's encoder) and `finish_present` (after `submit`).

On web, `mount` takes an async builder instead. The builder decides how to
claim the `<canvas>`:

- `WebGpuProbe::run(..)` gets a WebGPU device without touching the canvas.
  Build your pipelines on it, then call `probe.claim(ev, alpha)`. If anything
  fails before the claim, the canvas is still unused and can fall back to
  Canvas2D (this is what canvas-vello does).
- `claim_webgl(ev, alpha, label)` (crate feature `webgl`) is the WebGL2 path.
  WebGL has no headless adapter, so it claims the canvas first. Use it only as
  the last option.
- `drive(state)` wraps a `SurfaceState` as the per-frame render function. Each
  frame it re-reads the device pixel ratio and the canvas size, then renders.

Rendering on web is paced to `requestAnimationFrame`. The other platforms render
on each reactive change.

## The frame target

Every renderer draws into one `Rgba8Unorm` texture (`TARGET_FORMAT`) holding
**sRGB-encoded** bytes. The surface is always configured with a non-sRGB format,
so the present blit copies the bytes as they are. One format means shared passes
(`OverlayCompositor`) work for every renderer, and self-capture records the same
bytes the screen shows.

The renderer declares its alpha convention once as a `FrameAlpha`:

| Renderer      | Frame alpha     | Why                                           |
| ------------- | --------------- | --------------------------------------------- |
| canvas-vello  | `Straight`      | vello's fine shader divides by alpha on store |
| canvas3d-wgpu | `Premultiplied` | blended 3D output is premultiplied naturally  |

The present blit converts the frame to what the surface expects. Surfaces are
`PreMultiplied` wherever the platform supports it, because the view is drawn
over the UI behind it. The GTK4 GL framebuffer is premultiplied too, and is
flipped vertically.

## Capability gates, not platform checks

`Requirements` names what a pipeline needs (`downlevel` flags, and f16 on
Vulkan). `adapter_meets_default` is the gate a renderer checks when it
registers. vello needs `INDIRECT_EXECUTION` and Vulkan f16, so it steps aside
on the iOS Simulator and the Android emulator. A plain raster pipeline asks for
`Requirements::NONE` and runs on both.

## Testing

- `cargo test -p gpu-surface` runs:
  - the pure unit tests (adapter rules, format and alpha choice, shader
    validation for every blit and compositor variant);
  - `tests/gpu.rs`, which uses a headless device to check the actual bytes:
    premultiply and unpremultiply, the V-flip with no re-encode, and overlay
    compositing over transparent and opaque frames.
- `headless_device_with_limits(Some(wgpu::Limits::downlevel_webgl2_defaults()))`
  creates a device that rejects anything WebGL2 can't do. Renderers use it to
  prove their pipelines are WebGL2-safe without a browser.
- The web bring-up needs a real browser. The `navigator.gpu` check is covered by
  a `wasm-bindgen-test`; probe-then-claim is covered by the Playwright E2E runs
  of the demos.
