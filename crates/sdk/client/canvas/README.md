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

## The canvas's size

`s.size()` in the `draw` closure is the canvas's laid-out size `(width,
height)`, in the same logical units the scene draws in. Reading it makes the
painter re-run when the canvas is resized, so content can scale to its box with
no measuring code:

```rust
draw: canvas::draw(|s| {
    let (w, h) = s.size();
    s.fill_path(Path::rect(0.0, 0.0, w, h), background);
})
```

It is `(0, 0)` on the first paint (which can run before layout) and for a
scene built outside a canvas. Scenes built by `layer` / `layer_cached` see the
same size as their parent.

**For renderer authors.** A renderer paints through `CanvasPrim::paint()`
(instead of `paint_scene`) inside its repaint effect, and reports the size from
wherever it already learns it, through `CanvasPrim::size_reporter()`: a resize
observer (web), `drawRect:` (iOS/macOS), the backend's layout notification
(Android), the widget snapshot (Linux), the surface's ready/resize events
(vello). Report LOGICAL units (divide physical pixels by the scale). Everything
else lives in `SizeReporter` once: duplicate reports are ignored, the write is
committed through a 0 ms scheduler timer (every backend flushes after scheduler
callbacks, and resize notifications arrive outside the framework's dispatch),
and a report that lands after the canvas unmounted is dropped.

Renderers draw texture layers only where the scene has a texture op and never
add the missing ones at the end themselves; `paint_scene` / `CanvasPrim::paint`
already did that. So a scene handed straight to `canvas_native::make_2d_rasterizer`
must come from one of those, or be run through
`canvas_core::place_textures(scene, layers.len())` first. Otherwise any layer it
never placed is not drawn.

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
glyph is outlined (skrifa, via `canvas_core::expand_glyph_run`) and filled,
producing identical geometry. The font parser is linked through
`FontResource::new`, so an app that never makes a font — charts only — doesn't
ship it, and an app whose glyphs come from a lazy component ships it in that
component's module. This is the
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

## Textures (live video and images) in the draw order

`CanvasProps::layers` lists texture sources: a live `MediaStream` (a camera, a
screen share) or a static image, each with a reactive `rect`, a `fit`, an
optional `src_crop`, corner radius, opacity and border. The scene decides where
each one is drawn. `s.texture(i)` composites `layers[i]` at that point in the
draw order, so anything drawn after it lands on top:

```rust
canvas(CanvasProps {
    draw: canvas::draw(move |s: &mut Scene| {
        s.texture(0);                                   // the camera frame
        let m = camera.source_to_canvas(frame_w, frame_h);
        let (x, y) = m.apply(corner.x, corner.y);       // frame pixels → canvas
        // ... stroke an outline over the frame
    }),
    layers: vec![camera.clone()],
    ..Default::default()
})
```

The rules, applied once in `canvas_core::paint_scene` (`place_textures`) so every
renderer behaves the same:

- A texture is drawn in the canvas's logical coordinates. The scene's current
  transform and clip don't apply to it, and they carry on unchanged after it.
- A layer the scene never places is drawn after the whole scene, in `layers`
  order. A canvas that never calls `texture` therefore looks exactly as before.
- `texture` only counts at the top level of a scene. Inside a raster `layer`,
  a cached layer or a mask group it is ignored, as is an index with no layer.

`TextureLayer::source_rects` gives the crop + fit rectangles every renderer
composites with, and `TextureLayer::source_to_canvas` maps the source's pixel
coordinates into canvas coordinates, which is what you need to register vector
content to the picture (an outline around something detected in the frame).

A live layer only shows new frames when the canvas repaints, so keep a
`raf_loop` bumping a signal the painter reads while the stream is on.

## Renderers

Pick **one** at the boot entry's registry seam (the scene registry is
`TypeId`-keyed, last-registration-wins):

| Crate | Engine | Where it runs |
| ----- | ------ | ------------- |
| [`canvas-native`](native/) | each platform's native 2D API — web Canvas2D, iOS/macOS CoreGraphics, Android `android.graphics`, Linux Cairo | everywhere with a native 2D API |
| [`canvas-vello`](vello/) | GPU compute 2D via [`vello`](https://github.com/linebender/vello) on `wgpu` (Metal / Vulkan / DX12) | every native backend with a capable GPU |

`canvas-vello` gets its surface setup, frame target and present step from
[`gpu-surface`](../gpu-surface/), which the `canvas3d` renderer shares. The
present step turns vello's straight-alpha output into the premultiplied colour
the surface expects, so semi-transparent edges look right over dark UI. Two
entry points exist for drawing canvas scenes inside another GPU renderer's frame
(the `canvas3d` overlay uses both):

- `canvas_vello::SceneRenderer` renders with vello on a device you already own.
- `canvas_vello::rasterize_cpu` (crate feature `cpu`) uses vello_cpu, for GPUs
  that can't run vello's compute shaders.

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

## Testing checklist

Manual verification per backend — an unchecked **native** box means the code
compiles for that target but isn't confirmed on real hardware yet. Tick each
item as you exercise it. The same scene must render identically under both
renderers (CLAUDE.md §7), so verify the **GPU (`canvas-vello`)** and **CPU
(`canvas-native`)** paths produce the same pixels — only speed should differ.

**Automated**
- [ ] `cargo test -p canvas` — scene-model logic (paths, paint, `ShapeInstance` batches, glyph runs, blend/mask ops)
- [ ] `cargo build -p canvas --target wasm32-unknown-unknown` — web target
- [x] `cargo test -p canvas-native --target wasm32-unknown-unknown` (headless Chrome through the workspace runner) — `native/tests/web_canvas.rs`: the web Canvas2D replay (on web-glue) read back pixel by pixel — solid / gradient / image / even-odd fills, persistent layer, transform + clip — image and live-stream texture layers, the `captureStream` self-capture's native source (a `web_glue::dom::MediaStream`), and (`--features web-sys-canvas`) `make_2d_rasterizer` on a `web_sys` canvas (canvas-vello's fallback entry, the one HYBRID-BRIDGE: wgpu seam — behind that feature so a canvas app without canvas-vello links no wasm-bindgen and builds in own mode; `native/tests/no_wasm_bindgen.rs` pins it)

- [x] `cargo test -p canvas-core` — `tests/size.rs`: a renderer's size report reaches the painter and repaints, same-size reports don't repaint, a report after unmount is ignored; and the texture-op rules (`place_textures`: placement, appending unplaced layers, base state at a texture + state resumed after it, nested/out-of-range removal) and `TextureLayer::source_rects` / `source_to_canvas`
- [x] `cargo test -p canvas-native --target wasm32-unknown-unknown --test web_canvas` — `the_painter_sees_the_laid_out_canvas_size` (the resize observer's report reaches `Scene::size`); `texture_ops_order_vector_content_around_layers`: a fill after `texture(0)` is on top, the texture ignores the author transform, an unplaced layer composites over the scene, `src_crop` is honored
- [x] `cargo test -p canvas-native` (macOS host) — headless CoreGraphics tests in `native/src/macos.rs` drive the shared Apple painter (iOS + macOS): texture-op ordering, unplaced layers, transform reset, upright image, `src_crop`, Contain letterbox, a translucent texture composited as straight alpha; `native/tests/pixels.rs` — RGBA premultiply / Cairo ARGB32 conversions; `native/tests/layer_geometry.rs` — the Android layer math (fractional source rect → drawn rect mapping, border color faded by opacity)
- [x] Linux (Cairo) — `cargo test -p canvas-native` inside a Linux environment with gtk4/cairo (e.g. a devcontainer: `CARGO_TARGET_DIR=/tmp/cn-linux-target cargo test --locked -p canvas-native`): texture-op ordering, unplaced layers, `src_crop`, transform/clip reset, rounded/faded/framed layers
- [x] `cargo test -p canvas-vello` (macOS Metal) — texture-op ordering on the GPU (`vello/tests/headless_compositor.rs`), the overlay-reuse submit-ordering model (`texture_runs.rs`), segmentation incl. keeping the Cached/Hybrid/Shapes fast paths with layers (`plan.rs`), shared layer crop/fit/clip geometry (`layer_blit.rs`)
- [ ] Android `android.graphics` texture compositing — compile-checked (`cargo check -p canvas-native --target aarch64-linux-android`) and its pure math host-tested (`native/tests/layer_geometry.rs`); there is no host `android.graphics.Canvas` to read pixels back from, so these need a device/emulator run:
  - a layer drawn at `texture(i)` sits under the vector content recorded after it, and an unplaced layer over everything;
  - a `src_crop` / Cover crop that starts mid-pixel frames the same as on web (the `Matrix` `setScale` + `postTranslate` draw, not the integer `Rect` overload);
  - a square-cornered layer (`corner_radius` 0) does not spill outside its drawn rect — the `clipRect` bounds the whole-bitmap matrix draw, most visible with `src_crop` or `Fit::Cover`;
  - a layer's border frame fades with `opacity` (try 0.5 with a white border);
  - a translucent image layer (a logo's soft edge) is not too bright (the straight → premultiplied upload).

**Behavior**
- [ ] **Web** — register `canvas-native`; a `draw` scene renders via Canvas2D; reactive `Signal` reads re-render on change; self-capture records via `captureStream()`.
- [ ] **iOS** — on a **device**, `canvas-vello` (GPU) renders the scene and self-capture uses GPU→CPU read-back. On the **Simulator**, vello self-gates off (Metal lacks `INDIRECT_EXECUTION`) → `canvas-native` (CoreGraphics) renders the *same* pixels; recording uses the offscreen re-rasterize fallback and logs the one-time slow-path warning (`NSLog`). ⚠️ device GPU path needs a physical device to confirm.
- [ ] **Android** — on a **device**, `canvas-vello` (GPU) renders + self-capture via GPU→CPU read-back. On the **emulator**, vello self-gates off (Vulkan lacks `SHADER_F16`) → `android.graphics` CPU renderer, bitmap read-back, one-time `Log.w("canvas", …)` warning. ⚠️ device GPU path needs real Adreno/Mali hardware to confirm.
- [ ] **macOS** — GPU-verified: `canvas-vello` renders the scene and self-capture is zero-copy IOSurface → encoder (no read-back); confirm bulk `shapes` batches draw in one instanced-SDF pass and glyph runs render via vello's glyph pipeline.

No OS permission of its own — drawing and self-capture stay within the app's own surface.
