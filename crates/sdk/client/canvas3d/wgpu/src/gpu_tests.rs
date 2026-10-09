//! GPU-backed renderer tests: real pipelines on a headless device (a real
//! adapter, else the software one), pixels read back. Each test skips with a
//! note when the host has no adapter at all.
//!
//! The frame target is sRGB-encoded and premultiplied, so expected values are
//! written in those terms: an opaque unlit sRGB colour comes back as itself.

use crate::mesh::{choose_sample_count, MeshRenderer, LINE_WGSL, MESH_WGSL};
use crate::overlay::OverlayPass;
use canvas3d_core::{
    AlphaMode, Camera, Color, Light, LineDepth, Lines, Mat4, Material, MeshData, Model, Scene3d, Texture,
    Vec3,
};
use gpu_surface::{headless_device, headless_device_with_limits, make_target, read_target_rgba, RenderedImage};

const W: u32 = 64;
const H: u32 = 64;

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
}

fn gpu() -> Option<Gpu> {
    let d = headless_device();
    if d.is_none() {
        eprintln!("skipping: no GPU adapter on this host");
    }
    d.map(|(device, queue)| Gpu { device, queue })
}

fn render_with(gpu: &Gpu, mesh: &mut MeshRenderer, scene: &Scene3d) -> RenderedImage {
    let (target, view) = make_target(&gpu.device, W, H, "test-target");
    let mut enc = gpu.device.create_command_encoder(&Default::default());
    mesh.render(&gpu.device, &gpu.queue, &mut enc, scene, &view, (W, H));
    gpu.queue.submit([enc.finish()]);
    read_target_rgba(&gpu.device, &gpu.queue, &target, W, H)
}

fn render(gpu: &Gpu, scene: &Scene3d, samples: u32) -> RenderedImage {
    let mut mesh = MeshRenderer::new(&gpu.device, &gpu.queue, samples);
    render_with(gpu, &mut mesh, scene)
}

fn scene() -> Scene3d {
    let mut s = Scene3d::with_size(W as f32, H as f32);
    s.camera(Camera::look_at(Vec3::new(0.0, 0.0, 6.0), Vec3::ZERO));
    s
}

fn unlit(color: Color) -> Material {
    Material::color(color).unlit()
}

fn cube(size: f32, material: Material) -> Model {
    Model::from_mesh(MeshData::cube(size), material)
}

fn near(a: [u8; 4], b: [u8; 4], tol: i32) -> bool {
    a.iter().zip(b).all(|(x, y)| (*x as i32 - y as i32).abs() <= tol)
}

const CENTRE: (u32, u32) = (W / 2, H / 2);

#[test]
fn unlit_cube_renders_in_the_middle_over_the_clear_colour() {
    let Some(g) = gpu() else { return };
    let mut s = scene();
    s.model(&cube(1.0, unlit(Color::from_rgba8(200, 40, 40, 255))), Mat4::IDENTITY);
    let img = render(&g, &s, 1);
    assert!(near(img.px(CENTRE.0, CENTRE.1), [200, 40, 40, 255], 1), "{:?}", img.px(CENTRE.0, CENTRE.1));
    assert_eq!(img.px(1, 1), [0, 0, 0, 0], "default clear is transparent");

    s.clear(Color::from_rgba8(0, 0, 255, 255));
    let img = render(&g, &s, 1);
    assert_eq!(img.px(1, 1), [0, 0, 255, 255]);
}

#[test]
fn clear_colour_is_stored_premultiplied() {
    let Some(g) = gpu() else { return };
    let mut s = scene();
    s.clear(Color::from_rgba8(255, 0, 0, 128));
    let img = render(&g, &s, 1);
    assert!(near(img.px(1, 1), [128, 0, 0, 128], 1), "{:?}", img.px(1, 1));
}

/// The depth buffer: a near red cube hides a larger far green one, whichever
/// is drawn first.
#[test]
fn near_geometry_hides_far_geometry_in_either_draw_order() {
    let Some(g) = gpu() else { return };
    let red = cube(1.0, unlit(Color::rgb(1.0, 0.0, 0.0)));
    let green = cube(3.0, unlit(Color::rgb(0.0, 1.0, 0.0)));
    for near_first in [true, false] {
        let mut s = scene();
        let (near_xf, far_xf) =
            (Mat4::from_translation(Vec3::new(0.0, 0.0, 1.5)), Mat4::from_translation(Vec3::new(0.0, 0.0, -3.0)));
        if near_first {
            s.model(&red, near_xf);
            s.model(&green, far_xf);
        } else {
            s.model(&green, far_xf);
            s.model(&red, near_xf);
        }
        let img = render(&g, &s, 1);
        assert_eq!(img.px(CENTRE.0, CENTRE.1), [255, 0, 0, 255], "near_first={near_first}");
        // Off the red cube's silhouette but inside the green one.
        assert_eq!(img.px(CENTRE.0 + 14, CENTRE.1), [0, 255, 0, 255], "near_first={near_first}");
    }
}

/// A directional light from +X lights the sphere's right side and leaves the
/// left side to the ambient term.
#[test]
fn directional_light_lights_the_facing_side() {
    let Some(g) = gpu() else { return };
    let mut s = scene();
    s.ambient(Color::WHITE, 0.05).light(Light::directional(Vec3::new(-1.0, 0.0, 0.0), Color::WHITE, 3.0));
    s.model(&Model::from_mesh(MeshData::uv_sphere(1.5, 32, 16), Material::color(Color::GRAY)), Mat4::IDENTITY);
    let img = render(&g, &s, 1);
    let right = img.px(CENTRE.0 + 12, CENTRE.1);
    let left = img.px(CENTRE.0 - 12, CENTRE.1);
    assert!(right[0] > left[0] + 60, "lit {right:?} vs unlit {left:?}");
    assert_eq!(right[3], 255);
    assert!(left[0] > 0, "ambient keeps the far side visible");
}

#[test]
fn blended_materials_mix_over_what_is_behind() {
    let Some(g) = gpu() else { return };
    let mut s = scene();
    s.model(&cube(3.0, unlit(Color::rgb(0.0, 0.0, 1.0))), Mat4::from_translation(Vec3::new(0.0, 0.0, -3.0)));
    let glass = unlit(Color::rgba(1.0, 0.0, 0.0, 0.5));
    assert_eq!(glass.alpha_mode, AlphaMode::Blend);
    s.model(&cube(1.0, glass), Mat4::from_translation(Vec3::new(0.0, 0.0, 1.0)));
    let img = render(&g, &s, 1);
    // Premultiplied over: 0.5·red + 0.5·blue, alpha 1 (encoded space).
    assert!(near(img.px(CENTRE.0, CENTRE.1), [128, 0, 128, 255], 2), "{:?}", img.px(CENTRE.0, CENTRE.1));
}

/// Blended parts draw after opaque ones and sort back to front, so drawing
/// the glass first must not let it hide (or be hidden by) the opaque cube
/// behind it.
#[test]
fn blended_parts_draw_after_opaque_regardless_of_order() {
    let Some(g) = gpu() else { return };
    let mut s = scene();
    s.model(&cube(1.0, unlit(Color::rgba(1.0, 0.0, 0.0, 0.5))), Mat4::from_translation(Vec3::new(0.0, 0.0, 1.0)));
    s.model(&cube(3.0, unlit(Color::rgb(0.0, 0.0, 1.0))), Mat4::from_translation(Vec3::new(0.0, 0.0, -3.0)));
    let img = render(&g, &s, 1);
    assert!(near(img.px(CENTRE.0, CENTRE.1), [128, 0, 128, 255], 2), "{:?}", img.px(CENTRE.0, CENTRE.1));
}

/// Alpha mask: texels under the cutoff are discarded (the background shows),
/// the rest are opaque.
#[test]
fn masked_texels_below_the_cutoff_are_discarded() {
    let Some(g) = gpu() else { return };
    // Left half alpha 0, right half alpha 255.
    let mut px = Vec::new();
    for _y in 0..2 {
        for x in 0..2 {
            px.extend_from_slice(&[255, 255, 255, if x == 0 { 0 } else { 255 }]);
        }
    }
    let mut m = unlit(Color::rgb(0.0, 1.0, 0.0));
    m.base_color_texture = Some(Texture::from_rgba8(2, 2, px));
    m.alpha_mode = AlphaMode::Mask { cutoff: 0.5 };
    let mut s = scene();
    s.clear(Color::rgb(1.0, 1.0, 1.0));
    // A plane facing the camera (+Z): rotate the +Y-facing plane onto it.
    let quad = Model::from_mesh(MeshData::plane(3.0, 3.0), m);
    s.model(&quad, Mat4::from_rotation_x(std::f32::consts::FRAC_PI_2));
    let mut mesh = MeshRenderer::new(&g.device, &g.queue, 1);
    let img = render_with(&g, &mut mesh, &s);
    assert_eq!(img.px(CENTRE.0 - 8, CENTRE.1), [255, 255, 255, 255], "discarded → clear colour");
    assert_eq!(img.px(CENTRE.0 + 8, CENTRE.1), [0, 255, 0, 255], "kept, opaque");
}

/// sRGB textures decode to linear when sampled and re-encode on output, so an
/// unlit textured surface reproduces its texel bytes.
#[test]
fn srgb_base_colour_texture_round_trips() {
    let Some(g) = gpu() else { return };
    let mut m = unlit(Color::WHITE);
    m.base_color_texture = Some(Texture::from_rgba8(1, 1, vec![180, 90, 30, 255]));
    let mut s = scene();
    s.model(&cube(2.0, m), Mat4::IDENTITY);
    let img = render(&g, &s, 1);
    assert!(near(img.px(CENTRE.0, CENTRE.1), [180, 90, 30, 255], 2), "{:?}", img.px(CENTRE.0, CENTRE.1));
}

#[test]
fn single_sided_back_faces_are_culled_double_sided_are_not() {
    let Some(g) = gpu() else { return };
    // The +Y plane seen from below shows only its back face.
    let mut s = Scene3d::with_size(W as f32, H as f32);
    s.camera(Camera::look_at(Vec3::new(0.0, -5.0, 0.01), Vec3::ZERO));
    s.model(&Model::from_mesh(MeshData::plane(4.0, 4.0), unlit(Color::rgb(1.0, 0.0, 0.0))), Mat4::IDENTITY);
    assert_eq!(render(&g, &s, 1).px(CENTRE.0, CENTRE.1)[3], 0, "culled");

    let mut s2 = Scene3d::with_size(W as f32, H as f32);
    s2.camera(*s.get_camera());
    s2.model(
        &Model::from_mesh(MeshData::plane(4.0, 4.0), unlit(Color::rgb(1.0, 0.0, 0.0)).double_sided()),
        Mat4::IDENTITY,
    );
    assert_eq!(render(&g, &s2, 1).px(CENTRE.0, CENTRE.1), [255, 0, 0, 255]);
}

/// A mirroring transform flips winding; the renderer flips the front face so
/// the outside of the mirrored cube still draws (and its inside still culls).
#[test]
fn mirrored_transform_still_draws_outward_faces() {
    let Some(g) = gpu() else { return };
    let mut s = scene();
    s.model(&cube(1.0, unlit(Color::rgb(1.0, 1.0, 0.0))), Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0)));
    assert_eq!(render(&g, &s, 1).px(CENTRE.0, CENTRE.1), [255, 255, 0, 255]);
}

#[test]
fn tested_lines_hide_behind_geometry_on_top_lines_do_not() {
    let Some(g) = gpu() else { return };
    let line = Lines::new(vec![[Vec3::new(-3.0, 0.0, -1.0), Vec3::new(3.0, 0.0, -1.0)]]);
    let occluder = cube(1.0, unlit(Color::rgb(0.0, 0.0, 1.0)));
    let row = |img: &RenderedImage, x: u32| -> Vec<[u8; 4]> { (CENTRE.1 - 1..=CENTRE.1 + 1).map(|y| img.px(x, y)).collect() };

    let mut s = scene();
    s.model(&occluder, Mat4::IDENTITY);
    s.lines(&line, Color::rgb(1.0, 1.0, 1.0), LineDepth::Tested);
    let img = render(&g, &s, 1);
    assert!(row(&img, CENTRE.0).iter().all(|p| *p == [0, 0, 255, 255]), "hidden behind the cube: {:?}", row(&img, CENTRE.0));
    assert!(row(&img, 4).iter().any(|p| *p == [255, 255, 255, 255]), "visible where nothing is in front");

    let mut s = scene();
    s.model(&occluder, Mat4::IDENTITY);
    s.lines(&line, Color::rgb(1.0, 1.0, 1.0), LineDepth::OnTop);
    let img = render(&g, &s, 1);
    assert!(row(&img, CENTRE.0).iter().any(|p| *p == [255, 255, 255, 255]), "drawn over the cube");
}

/// 4× MSAA resolves partial coverage on silhouette edges; 1× is binary.
#[test]
fn msaa_antialiases_silhouette_edges() {
    let Some(g) = gpu() else { return };
    let mut s = scene();
    s.model(
        &cube(2.0, unlit(Color::rgb(1.0, 1.0, 1.0))),
        Mat4::from_rotation_z(0.3),
    );
    let partial = |img: &RenderedImage| img.data.chunks_exact(4).filter(|p| p[3] > 10 && p[3] < 245).count();
    assert_eq!(partial(&render(&g, &s, 1)), 0, "1× has no partial coverage");
    let adapter_samples = {
        let instance = wgpu::Instance::default();
        pollster::block_on(instance.request_adapter(&Default::default())).ok().map(|a| choose_sample_count(&a))
    };
    if adapter_samples != Some(4) {
        eprintln!("skipping the 4× half: adapter can't multisample these formats");
        return;
    }
    assert!(partial(&render(&g, &s, 4)) > 20, "4× smooths the rotated edges");
}

/// Resources the author drops are released: after a frame without the model
/// (and with its last `Arc` gone) the GPU copy is gone too.
#[test]
fn dropped_models_release_their_gpu_meshes() {
    let Some(g) = gpu() else { return };
    let mut mesh = MeshRenderer::new(&g.device, &g.queue, 1);
    let mut s = scene();
    let model = cube(1.0, unlit(Color::WHITE));
    s.model(&model, Mat4::IDENTITY);
    render_with(&g, &mut mesh, &s);
    assert_eq!(mesh.cached_meshes(), 1);
    drop(s);
    drop(model);
    render_with(&g, &mut mesh, &scene());
    assert_eq!(mesh.cached_meshes(), 0);
}

/// The whole renderer under WebGL2's limits: a device that rejects storage
/// buffers/textures and compute must accept every pipeline and draw (lit,
/// textured, normal-mapped, blended, lines) without a validation error.
#[test]
fn renders_under_webgl2_limits() {
    let Some((device, queue)) = headless_device_with_limits(Some(wgpu::Limits::downlevel_webgl2_defaults())) else {
        eprintln!("skipping: no GPU adapter on this host");
        return;
    };
    let g = Gpu { device, queue };
    let scope = g.device.push_error_scope(wgpu::ErrorFilter::Validation);
    let mut m = Material::color(Color::rgb(0.8, 0.6, 0.4)).metallic(0.3).roughness(0.4);
    m.base_color_texture = Some(Texture::from_rgba8(4, 4, vec![200; 64]));
    m.normal_texture = Some(Texture::from_rgba8(2, 2, [128, 128, 255, 255].repeat(4)));
    m.metallic_roughness_texture = Some(Texture::from_rgba8(1, 1, vec![0, 200, 200, 255]));
    let mut s = scene();
    s.light(Light::directional(Vec3::new(-1.0, -1.0, -1.0), Color::WHITE, 2.0));
    s.model(&Model::from_mesh(MeshData::uv_sphere(1.5, 24, 12), m), Mat4::IDENTITY);
    s.model(&cube(0.5, unlit(Color::rgba(0.0, 1.0, 0.0, 0.5))), Mat4::from_translation(Vec3::new(1.0, 0.0, 2.0)));
    s.lines(&Lines::grid(2.0, 0.5), Color::GRAY, LineDepth::Tested);
    s.lines(&Lines::grid(1.0, 1.0), Color::WHITE, LineDepth::OnTop);
    for samples in [1, 4] {
        let img = render(&g, &s, samples);
        assert_eq!(img.px(CENTRE.0, CENTRE.1)[3], 255, "{samples}×: the sphere drew");
    }
    let err = pollster::block_on(scope.pop());
    assert!(err.is_none(), "validation error under WebGL2 limits: {err:?}");
}

/// Every shader entry point translates to GLSL ES 3.00 — what wgpu hands
/// WebGL2. A WGSL feature WebGL2 can't express fails here, not in a browser.
#[test]
fn shaders_translate_to_glsl_es_300() {
    use naga::back::glsl;
    for (name, src) in [("mesh", MESH_WGSL), ("line", LINE_WGSL)] {
        let module = naga::front::wgsl::parse_str(src).unwrap_or_else(|e| panic!("{name}: {}", e.emit_to_string(src)));
        let info = naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::empty())
            .validate(&module)
            .unwrap_or_else(|e| panic!("{name}: {e:?}"));
        for (stage, entry) in [(naga::ShaderStage::Vertex, "vs_main"), (naga::ShaderStage::Fragment, "fs_main")] {
            let options = glsl::Options {
                version: glsl::Version::Embedded { version: 300, is_webgl: true },
                ..Default::default()
            };
            let pipeline = glsl::PipelineOptions { shader_stage: stage, entry_point: entry.to_string(), multiview: None };
            let mut out = String::new();
            let mut writer = glsl::Writer::new(&mut out, &module, &info, &options, &pipeline, naga::proc::BoundsCheckPolicies::default())
                .unwrap_or_else(|e| panic!("{name}/{entry}: {e:?}"));
            writer.write().unwrap_or_else(|e| panic!("{name}/{entry}: {e:?}"));
            assert!(out.starts_with("#version 300 es"), "{name}/{entry}");
        }
    }
}

/// The 2D overlay composites over the 3D frame in both rasterizers, and the
/// two agree: a half-transparent white label over a blue cube.
#[test]
fn overlay_composites_over_the_3d_frame_with_either_rasterizer() {
    use canvas_core::{Color as C2, Paint, Path};
    let Some(g) = gpu() else { return };
    let mut s = scene();
    s.model(&cube(3.0, unlit(Color::rgb(0.0, 0.0, 1.0))), Mat4::IDENTITY);
    let mut overlay = canvas_core::Scene::with_size(W as f32, H as f32);
    overlay.path().add_path(Path::rect(24.0, 24.0, 16.0, 16.0));
    overlay.fill(Paint::solid(C2::new(255, 255, 255, 128)));

    let adapter = {
        let instance = wgpu::Instance::default();
        pollster::block_on(instance.request_adapter(&Default::default())).ok()
    };
    let mut passes = vec![("cpu", OverlayPass::new_cpu(&g.device))];
    if let Some(a) = &adapter {
        passes.push(("auto", OverlayPass::new(&g.device, a, false)));
    }
    for (name, mut pass) in passes {
        let (target, view) = make_target(&g.device, W, H, "test-target");
        let mut mesh = MeshRenderer::new(&g.device, &g.queue, 1);
        assert!(pass.prepare(&g.device, &g.queue, &overlay, (W, H), 1.0), "{name}");
        let mut enc = g.device.create_command_encoder(&Default::default());
        mesh.render(&g.device, &g.queue, &mut enc, &s, &view, (W, H));
        pass.composite(&g.device, &mut enc, &view);
        g.queue.submit([enc.finish()]);
        let img = read_target_rgba(&g.device, &g.queue, &target, W, H);
        // 50% white over opaque blue, premultiplied: (128, 128, 255).
        assert!(near(img.px(CENTRE.0, CENTRE.1), [128, 128, 255, 255], 2), "{name}: {:?}", img.px(CENTRE.0, CENTRE.1));
        assert_eq!(img.px(CENTRE.0 + 12, CENTRE.1), [0, 0, 255, 255], "{name}: outside the label");
    }
    // An empty overlay scene composites nothing.
    let mut pass = OverlayPass::new_cpu(&g.device);
    assert!(!pass.prepare(&g.device, &g.queue, &canvas_core::Scene::new(), (W, H), 1.0));
}
