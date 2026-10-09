# canvas3d

3D views for idealyst apps. You build a `Scene3d` (glTF models, lights, a camera,
world-space lines) inside a `draw` closure, and a renderer draws it on every
backend, including the web through WebGPU or, where WebGPU isn't available,
WebGL2.

```rust
use canvas3d::prelude::*;

let model = Model::from_gltf(include_bytes!("../assets/bottle.glb"))?;
let orbit = OrbitCamera::new(OrbitConfig::default());
let view3d = Canvas3dHandle::new();

ui! {
    view(on_touch = orbit.touch_handler(&view3d), on_wheel = orbit.wheel_handler()) {
        { Canvas3d(Canvas3dProps {
            draw: canvas3d::draw(move |s: &mut Scene3d| {
                s.camera(orbit.camera())
                 .light(Light::directional(Vec3::new(-1.0, -2.0, -1.0), Color::WHITE, 3.0))
                 .lines(&Lines::grid(5.0, 0.5), Color::GRAY, LineDepth::Tested);
                s.model(&model, Mat4::IDENTITY).pick_id(1);
            }),
            handle: Some(view3d.clone()),
            ..Default::default()
        }) }
    }
}
```

At boot, register the renderer: `canvas3d_wgpu::register(registry)`. A
`Canvas3d` with no renderer registered panics when it mounts. SSR builds
register `canvas3d::register_ssr` instead, which emits a plain `<canvas>`.

`crates/sdk/client/canvas3d/examples/canvas3d-demo` is a working example.
Fetch its models first with `./scripts/fetch-canvas3d-demo-model.sh`.

## Three layers

Game engines build a frame the same way:

1. **`draw`: the 3D scene.** Anything that belongs to the world goes here,
   including grids, selection boxes and gizmos, drawn as `Lines` that are
   depth-tested (`LineDepth::Tested`) or always on top (`LineDepth::OnTop`).
2. **`overlay`: 2D on top, in the same frame.** This is an ordinary
   `canvas::draw` closure painting a `canvas::Scene` (labels, markers, a HUD).
   The renderer draws it over the 3D image before presenting, so the two can
   never be a frame apart. To pin 2D content to a 3D point, use
   `camera.project(world_point, s.size())` or `Scene3d::project`.
3. **Ordinary views stacked over the canvas**, for interactive UI such as
   buttons and panels.

The painters run inside the renderer's reactive effect, so any signal they read
(an `OrbitCamera`, a selection) re-renders the view. On web the render is
paced to `requestAnimationFrame`.

## Models

- `Model::from_gltf(bytes)` loads binary `.glb`, or `.gltf` with embedded
  `data:` buffers. It takes bytes the app already has (embedded, fetched or
  picked), because file and URL loading doesn't work the same on every target.
  A `.gltf` that references external files returns `ModelError::ExternalUri`.
- **Supported:** triangle meshes (positions, normals, `TEXCOORD_0`, indices),
  the node hierarchy, metallic-roughness materials with all five texture maps,
  opaque, mask and blend alpha modes, double-sided materials,
  `KHR_materials_unlit`, skins (up to four joints per vertex) and animation
  clips that move nodes (translation, rotation, scale; step, linear and
  cubic-spline). See [Animation](#animation).
- **Not supported yet:** morph targets (and the `weights` animation channels
  that drive them), a second set of skin influences (`JOINTS_1` /
  `WEIGHTS_1`), the file's own cameras and lights, and non-triangle
  primitives.
- `Model::from_mesh(MeshData::cube(1.0), Material::color(..))` builds a model
  in code. `MeshData` also has `plane` and `uv_sphere`.
- `from_gltf` parses and decodes every texture synchronously. The demo's 9 MB
  model takes about 160 ms in an optimized build and several seconds in an
  unoptimized debug build, plus mip generation on its first frame. Build large
  models once, outside any reactive path, and off the UI thread if you can.
- Cloning a `Model` is cheap and keeps its id. The renderer uploads each mesh
  and texture once and frees the GPU copy when the last `Model` holding it is
  dropped.

## Animation

A loaded model keeps its node hierarchy, skins and clips. Time is an input
you pass in, so playing, pausing, scrubbing and crossfading all work the same
way:

```rust
let fox = Model::from_gltf(FOX_GLB)?;
let walk = fox.animation("Walk").expect("clip").clone();
let clock = AnimationClock::new();
clock.play();

draw: canvas3d::draw(move |s| {
    let t = clock.time();                       // reactive: repaints every frame while playing
    s.model(&fox, xf).animation(&walk, walk.looped(t));
})
```

- **`AnimationClock`** is a reactive number of seconds with `play`, `pause`,
  `seek` and `set_speed`. It's `Copy`, like a signal. It advances on the
  framework's animation clock, the same per-frame tick that animated styles
  use, and is registered there only while playing. A paused clock, or a
  view that doesn't read one, costs nothing per frame. It stops when the
  component that created it goes away.
- **`Pose`** holds the local transform of every node of one model.
  `Pose::rest(&model).sampled(&clip, t)` samples a clip.
  `a.blend(&b, w)` crossfades between two poses. `pose.set(node, transform)`
  moves one node by hand (aiming a head, an IK result), and
  `model.node("Head")` finds a node's index. Draw the result with
  `s.model(&model, xf).pose(pose)`. `.animation(&clip, t)` is shorthand for
  one clip over the rest pose.
- Clips clamp at their ends. Wrap time with `clip.looped(t)` to repeat.
- **Picking follows the pose.** A pick tests the model where it's drawn,
  deforming skinned meshes on the CPU only for pickable models and only when
  you pick. `model.bounds()` is the rest pose; `model.posed_bounds(&pose)`
  measures a pose, for example to draw a selection box that moves with the
  model.
- **The renderer** deforms skinned meshes on the GPU. It puts every joint
  matrix of the frame in one float texture, because WebGL2 limits a uniform
  block to 16 KiB (256 matrices). So joint counts have no limit, and two
  copies of one model can be drawn in different poses in the same frame.
  Parts attached to animated nodes are placed on the CPU.

## Picking

`Canvas3dHandle::pick(x, y)` returns the nearest pickable model (one drawn with
`.pick_id(n)`) under a point in the view, as a `PickHit { pick_id, world_pos,
distance }`. It ray-casts on the CPU against the triangles the renderer draws,
so it's exact. Back faces of single-sided materials can't be picked, because
they aren't drawn.

## Orbit camera

`OrbitCamera` stores its state in a signal and provides input handlers for the
view that wraps the canvas:

- drag: orbit
- shift + drag, or two fingers: pan
- mouse wheel or trackpad pinch: dolly (zoom)

The pan math keeps the point under your finger fixed. The pitch is kept just
short of straight up or down, so the camera never flips.

## Renderer (`canvas3d-wgpu`)

| Target      | GPU path                                          |
| ----------- | ------------------------------------------------- |
| web         | WebGPU, else WebGL2 (decided per view at mount)   |
| macOS, iOS  | Metal                                             |
| Android     | Vulkan                                            |
| Windows     | DX12 / Vulkan                                     |
| Linux (GTK) | the GL context the GTK area lends                 |

- **Shaders:** every shader stays within WebGL2's limits (uniform buffers only,
  no compute, no storage buffers or textures), so one set of pipelines runs
  everywhere.
- **Lighting:** physically based (GGX), with up to 4 directional lights plus
  an ambient term. There's no image-based lighting.
- **Normal maps:** these use a tangent frame computed in the shader, so meshes
  don't need tangents.
- **Anti-aliasing:** 4× MSAA where the GPU supports it for the formats used.
- **Mipmaps:** built on the CPU when a texture uploads, averaging colour
  textures in linear light.
- **Web:** `window.__IDEALYST_FORCE_WEBGL = true` (debug builds only) skips
  WebGPU so you can test the WebGL2 path.
- **The 2D overlay** is drawn with GPU vello where vello's compute pipeline can
  run. Elsewhere (WebGL2, the iOS Simulator, the Android emulator) it uses
  vello_cpu and uploads the result. The two are tested to produce matching
  pixels. An overlay identical to the previous frame's isn't drawn again,
  and the CPU path rasterizes and uploads only the area the overlay covers.
  This matters for an animating view: it repaints every display frame, and a
  full-screen CPU overlay at phone resolution costs about 100 ms per frame in
  an unoptimized build. Text, retained layers and masks still take the
  full-frame path.
- **Which backend is running:** `Canvas3dHandle::renderer_info()` returns a
  reactive string such as "WebGPU", "WebGL2" or "Metal", for diagnostics and
  HUDs.

Surface setup, the frame target and the present step come from
[`gpu-surface`](../gpu-surface/), which canvas-vello also uses.

## Testing

- `cargo test -p canvas3d-core` covers:
  - camera math and project/unproject round trips;
  - orbit, dolly and pan, plus their limits;
  - glTF loading (a GLB built inside the test, and the demo's real model when
    it has been fetched);
  - picking;
  - the payload running through the scene registry, including size reporting
    and repaints driven by orbit input.
- `cargo test -p canvas3d-wgpu` runs real pipelines on a headless device and
  reads the pixels back. It covers:
  - depth ordering, lighting, blending and sorting, alpha mask, sRGB textures,
    culling, mirrored transforms, lines, and MSAA;
  - freeing resources when models are dropped;
  - the 2D overlay with both rasterizers;
  - a device restricted to `downlevel_webgl2_defaults()`, which stands in for
    WebGL2 without a browser, plus a check that every shader translates to
    GLSL ES 3.00.
- The web WebGPU/WebGL2 setup and real input need a browser: run the demo with
  `idealyst dev --web --local` and reload with the WebGL flag set.
