# canvas

Retained-mode 2D drawing. An author writes a `draw` closure that fills a
[`Scene`](core/) (paths, paint, strokes, transforms, gradients); the framework
replays that scene through a renderer registered at bootstrap. The same scene
renders identically on every backend — only the registered renderer differs.

```rust
use canvas::prelude::*;
ui! {
    view {
        canvas(CanvasProps {
            draw: canvas::draw(move |s: &mut Scene| {
                s.path().move_to(10.0, 10.0).line_to(120.0, 10.0)
                 .cubic_to(140.0, 40.0, 90.0, 80.0, 10.0, 60.0).close();
                s.fill(Paint::solid(Color::new(40, 120, 255, 255)));
                s.stroke(Color::new(20, 20, 20, 255), Stroke::width(2.0));
            }),
            ..Default::default()
        })
    }
}
```

Any `Signal` read inside `draw` re-renders the canvas when it changes (the same
reactive convention as `video`/`svg`).

## Bulk shapes (instanced)

For a grid or scatter of **many** simple shapes, filling one `Path::circle` /
`Path::rounded_rect` per shape gets expensive — each path is flattened and binned
individually. `Scene::shapes` takes a batch of flat-colored
[`ShapeInstance`](core/)s instead:

```rust
s.shapes((0..10_000).map(|i| {
    let (x, y) = ((i % 100) as f32 * 8.0, (i / 100) as f32 * 8.0);
    ShapeInstance::circle(x, y, 3.0, Color::new(40, 120, 255, 255))
}));
```

A `ShapeInstance` is a **rounded box** — the constructors `circle`, `rect`,
`rounded_rect`, and `pill` cover the common shapes, and one SDF rasterizes them
all, so a single batch can mix shapes. On `canvas-vello`, a scene made entirely
of `shapes` batches (Normal blend) is drawn in **one GPU-instanced, analytic-SDF
pass** — thousands of shapes cost one draw call, not one tessellated fill each.
Any other scene (shapes mixed with paths/images, or a non-Normal blend) falls
back to expanding each shape to the equivalent per-shape fill, in order: the
pixels are identical (CLAUDE.md §7), so the batch only ever changes *how fast* it
draws, never *what*. The batch carries a solid color per shape; for gradient or
stroked shapes, use individual `fill`/`stroke` calls.

## Text (glyph runs)

`Scene::glyphs(font, glyphs, paint)` draws a run of glyphs from a font
([`FontResource`](core/) = raw sfnt/CFF bytes + a cache id) — each
[`PositionedGlyph`](core/) is a glyph id plus the affine placing its
**1000-units-per-em** outline in logical space. On `canvas-vello` the run drives
vello's GPU glyph pipeline with one cached font upload; on `canvas-native` each
glyph is outlined (skrifa) and filled, producing identical geometry. This is the
primitive the [`pdf`](../pdf/) SDK builds text from — a rendered PDF page is a
scene of glyph runs (text), fills/strokes (vectors), and image blits.

## Blend modes & soft masks

`Paint::blend(BlendMode)` covers the full W3C/PDF set — `Normal`, `Multiply`,
`Screen`, `Overlay`, `Darken`, `Lighten`, `ColorDodge`/`ColorBurn`,
`Hard`/`SoftLight`, `Difference`, `Exclusion`, `Hue`/`Saturation`/`Color`/
`Luminosity`, plus `DestinationOut` (the eraser). `Stroke::dash(pattern, offset)`
dashes a stroke. `DrawOp::MaskGroup` masks one op list by another's **luminance**
(soft masks / watermarks): on `canvas-vello` it uses vello's luminance-mask
layer; the [`pdf`](../pdf/) SDK builds these from PDF `/SMask`s.

## Renderers

Pick **one** at bootstrap (the `Element::External` registry is `TypeId`-keyed,
last-registration-wins):

| Crate | Engine | Where it runs |
| ----- | ------ | ------------- |
| [`canvas-native`](native/) | each platform's native 2D API — web Canvas2D, iOS/macOS CoreGraphics, Android `android.graphics` | everywhere with a native 2D API |
| [`canvas-vello`](vello/) | GPU compute 2D via [`vello`](https://github.com/linebender/vello) on `wgpu` (Metal / Vulkan / DX12) | every native backend with a capable GPU |

Registering both (native first, then vello) is the recommended setup on
GPU-capable platforms: `canvas-vello` **self-gates** — it wins on a real GPU and
steps aside for `canvas-native` when the GPU can't run vello's compute pipeline
(see below), so you always get the best renderer the device supports with no
app-side branching.

## Self-capture (recording the canvas's own output)

A canvas can record **its own rendered content** — strokes plus any composited
texture layers (e.g. a live camera) — into a `MediaStream`, WYSIWYG:

```rust
let (stream, writer) = media_stream::MediaStream::new();
canvas(CanvasProps { capture: Some(writer), ..Default::default() });
// hand `stream` to media-writer to encode to a file.
```

The renderer only reads frames back while a recorder is actually tapping the
stream (`FrameWriter::wants_cpu_frames`), so an un-recorded canvas pays nothing.

### Performance: GPU path vs. simulator/emulator CPU fallback

How recording captures frames depends on which renderer is active:

| Where | Renderer | Capture | Performance |
| ----- | -------- | ------- | ----------- |
| macOS | vello (GPU) | zero-copy IOSurface → encoder | fast (no readback) |
| iOS **device** | vello (GPU) | GPU→CPU read-back | good |
| Android **device** | vello (GPU) | GPU→CPU read-back | good |
| desktop Linux/Windows | vello (GPU) | GPU→CPU read-back | good |
| web | Canvas2D | `captureStream()` | native |
| **iOS Simulator** | CoreGraphics (CPU) | offscreen re-rasterize + read-back | **slow — fallback** |
| **Android emulator** | `android.graphics` (CPU) | bitmap read-back | **slow — fallback** |

> **⚠️ The iOS Simulator and Android emulator record on a CPU renderer and will
> show severe performance degradation while recording.** Their virtual GPUs
> can't run vello — the iOS Simulator's Metal lacks `INDIRECT_EXECUTION` and the
> Android emulator's Vulkan lacks `SHADER_F16`, both of which vello's GPU-driven
> pipeline requires. The framework detects this at startup, falls back to the
> native CPU renderer, and **logs a one-time warning** when you start recording
> (`NSLog` on iOS, `Log.w("canvas", …)` on Android). **Always validate recording
> performance on a physical device** — real Apple/Adreno/Mali GPUs run vello and
> capture is fast.

The CPU read-back fallback is **only compiled for the iOS Simulator**
(`cfg(target_abi = "sim")`); device iOS builds don't include it at all. On
Android, emulator and device share one build target, so the CPU path is compiled
but stays dormant on a device (vello wins; the CPU renderer is never invoked).
In neither case is there a runtime branch in the GPU path — the two capture
implementations live in separate renderer crates.

## Why a GPU renderer needs specific capabilities

`canvas-vello` is *GPU-driven*: it bins and rasterizes the scene in a chain of
compute passes where each stage's output sizes the next, issued via **indirect
dispatch** (the GPU reads workgroup counts out of a buffer). That requires
`INDIRECT_EXECUTION`, and its `flatten` shader requires `SHADER_F16` on Vulkan.
Emulated/virtualized GPUs (the iOS Simulator, the Android emulator) advertise
reduced feature sets that omit these, which is why vello can't run there. The
gate is a **capability check, not a platform check** (`canvas_vello::render`),
so any GPU lacking a required feature falls back uniformly.
