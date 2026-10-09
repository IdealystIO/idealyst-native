//! GPU-backed renderer tests: real pipelines on a headless device (a real
//! adapter, else the software one), pixels read back. Each test skips with a
//! note when the host has no adapter at all.
//!
//! The frame target is sRGB-encoded and premultiplied, so expected values are
//! written in those terms: an opaque unlit sRGB colour comes back as itself.

use crate::mesh::{choose_sample_count, MeshRenderer, LINE_WGSL, MESH_WGSL};
use crate::overlay::OverlayPass;
use canvas3d_core::{
    AlphaMode, Camera, Color, Light, LineDepth, Lines, Mat4, Material, MeshData, Model, ModelDesc, Node, Part,
    Pose, Quat, Scene3d, Skin, SkinWeights, Texture, Transform, Vec3,
};
use std::sync::Arc;
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
    let (arm, elbow) = arm(2);
    s.model(&arm, Mat4::from_translation(Vec3::new(-1.0, -1.0, 1.0))).pose(bent(&arm, elbow));
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
    use naga::ShaderStage::{Fragment, Vertex};
    let modules = [
        ("mesh", MESH_WGSL, &[(Vertex, "vs_main"), (Vertex, "vs_skinned"), (Fragment, "fs_main")][..]),
        ("line", LINE_WGSL, &[(Vertex, "vs_main"), (Fragment, "fs_main")][..]),
    ];
    for (name, src, entries) in modules {
        let module = naga::front::wgsl::parse_str(src).unwrap_or_else(|e| panic!("{name}: {}", e.emit_to_string(src)));
        let info = naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::empty())
            .validate(&module)
            .unwrap_or_else(|e| panic!("{name}: {e:?}"));
        for &(stage, entry) in entries {
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

/// Draw `overlay` through `pass` over a transparent frame and read it back.
fn overlay_only(g: &Gpu, pass: &mut OverlayPass, overlay: &canvas_core::Scene) -> RenderedImage {
    let (target, view) = make_target(&g.device, W, H, "test-target");
    let drew = pass.prepare(&g.device, &g.queue, overlay, (W, H), 1.0);
    let mut enc = g.device.create_command_encoder(&Default::default());
    if drew {
        pass.composite(&g.device, &mut enc, &view);
    }
    g.queue.submit([enc.finish()]);
    read_target_rgba(&g.device, &g.queue, &target, W, H)
}

fn dot_at(x: f32, y: f32) -> canvas_core::Scene {
    use canvas_core::{Color as C2, Paint, Path};
    let mut s = canvas_core::Scene::with_size(W as f32, H as f32);
    s.path().add_path(Path::circle(x, y, 4.0));
    s.fill(Paint::solid(C2::new(255, 255, 255, 255)));
    s
}

/// Regression: the CPU overlay rasterized and uploaded the WHOLE frame every
/// repaint — at phone resolution in an unoptimized build ~100 ms per frame for
/// a 10 px marker, which an animating view pays every display frame (Android
/// "not responding"). It now uploads the marker's footprint only, so a marker
/// that moves must not leave its old pixels behind.
#[test]
fn regression_a_moved_cpu_overlay_leaves_nothing_where_it_was() {
    let Some(g) = gpu() else { return };
    let mut pass = OverlayPass::new_cpu(&g.device);
    let first = overlay_only(&g, &mut pass, &dot_at(12.0, 12.0));
    assert_eq!(first.px(12, 12)[3], 255, "drawn at the first place");
    let second = overlay_only(&g, &mut pass, &dot_at(50.0, 50.0));
    assert_eq!(second.px(50, 50)[3], 255, "drawn at the second place");
    assert_eq!(second.px(12, 12), [0, 0, 0, 0], "and gone from the first");
    let uploaded = pass.drawn_region().expect("drawn").area();
    assert!(uploaded * 8 < (W * H) as u64, "uploaded the marker's footprint, not the frame: {uploaded} px");
    // Gone entirely, then back: still nothing stale.
    assert_eq!(overlay_only(&g, &mut pass, &canvas_core::Scene::with_size(W as f32, H as f32)).px(50, 50), [0, 0, 0, 0]);
    let back = overlay_only(&g, &mut pass, &dot_at(12.0, 50.0));
    assert_eq!(back.px(50, 50), [0, 0, 0, 0], "the second place is clear after an empty frame");
    assert_eq!(back.px(12, 50)[3], 255);
}

#[test]
fn an_unchanged_overlay_is_not_rasterized_again() {
    let Some(g) = gpu() else { return };
    let mut pass = OverlayPass::new_cpu(&g.device);
    let a = overlay_only(&g, &mut pass, &dot_at(20.0, 20.0));
    let b = overlay_only(&g, &mut pass, &dot_at(20.0, 20.0));
    assert_eq!(pass.rasterized, 1, "the second, identical frame reused the texture");
    assert_eq!(a.data, b.data, "and composited the same pixels");
    overlay_only(&g, &mut pass, &dot_at(21.0, 20.0));
    assert_eq!(pass.rasterized, 2, "a changed overlay is drawn again");
}

// --- skinning ---------------------------------------------------------------

const ARM_RED: [u8; 4] = [220, 30, 30, 255];

/// A 0.3-thick bar from y = 0 to y = 2 in two segments: the lower bound
/// wholly to joint 0 (the root), the upper wholly to the LAST joint (the
/// elbow, one unit up). `joint_count` > 2 pads the skin with idle joints in
/// between, so the elbow's matrix lands at index `joint_count − 1`.
fn arm(joint_count: usize) -> (Model, usize) {
    let segment = |y0: f32| {
        let c = MeshData::cube(1.0);
        let xf = Mat4::from_translation(Vec3::new(0.0, y0 + 0.5, 0.0)) * Mat4::from_scale(Vec3::new(0.3, 1.0, 0.3));
        let p: Vec<[f32; 3]> = c.positions.iter().map(|p| xf.transform_point3(Vec3::from(*p)).to_array()).collect();
        (p, c.normals.clone(), c.indices.clone())
    };
    let (lp, ln, li) = segment(0.0);
    let (up, un, ui) = segment(1.0);
    let n = lp.len() as u32;
    let elbow_joint = (joint_count - 1) as u16;
    let joints = [vec![[0u16, 0, 0, 0]; lp.len()], vec![[elbow_joint, 0, 0, 0]; up.len()]].concat();
    let mesh = MeshData::new(
        [lp, up].concat(),
        Some([ln, un].concat()),
        None,
        Some(li.into_iter().chain(ui.into_iter().map(|i| i + n)).collect()),
    )
    .with_skin(SkinWeights { weights: vec![[1.0, 0.0, 0.0, 0.0]; joints.len()], joints });

    // Nodes: 0 root, 1 elbow (one up), 2.. idle pads, last the mesh node.
    let mut nodes = vec![
        Node { name: None, parent: None, rest: Transform::IDENTITY },
        Node { name: Some("elbow".into()), parent: Some(0), rest: Transform::from_translation(Vec3::Y) },
    ];
    let pads = joint_count - 2;
    for _ in 0..pads {
        nodes.push(Node { name: None, parent: Some(0), rest: Transform::IDENTITY });
    }
    nodes.push(Node { name: None, parent: None, rest: Transform::IDENTITY });
    let mesh_node = nodes.len() - 1;
    let skin_joints: Vec<usize> = std::iter::once(0).chain(2..2 + pads).chain(std::iter::once(1)).collect();
    let mut inverse_bind = vec![Mat4::IDENTITY; joint_count];
    inverse_bind[joint_count - 1] = Mat4::from_translation(-Vec3::Y);
    let model = Model::from_desc(ModelDesc {
        parts: vec![Part::new(Arc::new(mesh), 0, Mat4::IDENTITY).on_node(mesh_node).skinned(0)],
        materials: vec![unlit(Color::from_rgba8(ARM_RED[0], ARM_RED[1], ARM_RED[2], 255)).double_sided()],
        nodes,
        skins: vec![Skin { joints: skin_joints, inverse_bind }],
        animations: Vec::new(),
    });
    (model, 1)
}

/// The elbow bent −90° about Z: the upper segment points along +X.
fn bent(model: &Model, elbow: usize) -> Pose {
    let mut pose = Pose::rest(model);
    pose.set(elbow, Transform::new(Vec3::Y, Quat::from_rotation_z(-std::f32::consts::FRAC_PI_2), Vec3::ONE));
    pose
}

fn arm_scene() -> Scene3d {
    let mut s = Scene3d::with_size(W as f32, H as f32);
    s.camera(Camera::look_at(Vec3::new(0.0, 1.0, 6.0), Vec3::new(0.0, 1.0, 0.0)));
    s
}

fn px_at(s: &Scene3d, img: &RenderedImage, world: Vec3) -> [u8; 4] {
    let p = s.project(world).expect("in view");
    img.px(p.x as u32, p.y as u32)
}

#[test]
fn skinned_arm_bends_with_its_joint_and_rests_like_the_unskinned_mesh() {
    let Some(g) = gpu() else { return };
    let (arm, elbow) = arm(2);

    let mut rest = arm_scene();
    rest.model(&arm, Mat4::IDENTITY);
    let rest_img = render(&g, &rest, 1);
    assert!(near(px_at(&rest, &rest_img, Vec3::new(0.0, 1.6, 0.0)), ARM_RED, 1), "rest: upper segment straight up");
    assert_eq!(px_at(&rest, &rest_img, Vec3::new(0.6, 1.0, 0.0))[3], 0, "rest: nothing to the right");

    // The same triangles with no skin draw the same pixels.
    let plain = Model::from_mesh(
        MeshData::new(
            arm.parts()[0].mesh.positions.clone(),
            Some(arm.parts()[0].mesh.normals.clone()),
            None,
            Some(arm.parts()[0].mesh.indices.clone()),
        ),
        arm.materials()[0].clone(),
    );
    let mut unskinned = arm_scene();
    unskinned.model(&plain, Mat4::IDENTITY);
    assert_eq!(render(&g, &unskinned, 1).data, rest_img.data, "skinned at rest == unskinned");

    let mut posed = arm_scene();
    posed.model(&arm, Mat4::IDENTITY).pose(bent(&arm, elbow));
    let img = render(&g, &posed, 1);
    assert!(near(px_at(&posed, &img, Vec3::new(0.6, 1.0, 0.0)), ARM_RED, 1), "bent: upper segment along +X");
    assert_eq!(px_at(&posed, &img, Vec3::new(0.0, 1.7, 0.0))[3], 0, "bent: nothing straight up any more");
    assert!(near(px_at(&posed, &img, Vec3::new(0.0, 0.5, 0.0)), ARM_RED, 1), "the root segment didn't move");
}

/// 70 joints: the elbow's matrix is joint 69, on the joint texture's second
/// row — the row/column addressing in `vs_skinned` has to agree with the
/// upload.
#[test]
fn joints_past_the_first_texture_row_are_addressed_correctly() {
    let Some(g) = gpu() else { return };
    let (arm, elbow) = arm(70);
    let mut s = arm_scene();
    s.model(&arm, Mat4::IDENTITY).pose(bent(&arm, elbow));
    let mut mesh = MeshRenderer::new(&g.device, &g.queue, 1);
    let img = render_with(&g, &mut mesh, &s);
    assert!(mesh.joint_rows() >= 2, "70 joints need two rows of {}", crate::mesh::JOINTS_PER_ROW);
    assert!(near(px_at(&s, &img, Vec3::new(0.6, 1.0, 0.0)), ARM_RED, 1), "joint 69 bent the arm");
}

#[test]
fn one_model_in_two_poses_in_one_frame() {
    let Some(g) = gpu() else { return };
    let (arm, elbow) = arm(2);
    let left = Mat4::from_translation(Vec3::new(-1.2, 0.0, 0.0));
    let right = Mat4::from_translation(Vec3::new(1.0, 0.0, 0.0));
    let mut s = arm_scene();
    s.model(&arm, left);
    s.model(&arm, right).pose(bent(&arm, elbow));
    let img = render(&g, &s, 1);
    assert!(near(px_at(&s, &img, Vec3::new(-1.2, 1.6, 0.0)), ARM_RED, 1), "left copy at rest");
    assert!(near(px_at(&s, &img, Vec3::new(1.6, 1.0, 0.0)), ARM_RED, 1), "right copy bent");
    assert_eq!(px_at(&s, &img, Vec3::new(1.0, 1.7, 0.0))[3], 0, "right copy isn't also at rest");
}

#[test]
fn a_node_animated_rigid_part_follows_its_node() {
    let Some(g) = gpu() else { return };
    // A cube on a child node, one unit right of its parent; rotating the
    // parent 90° about Z carries it to one unit up.
    let model = Model::from_desc(ModelDesc {
        parts: vec![Part::new(Arc::new(MeshData::cube(0.4)), 0, Mat4::IDENTITY).on_node(1)],
        materials: vec![unlit(Color::from_rgba8(ARM_RED[0], ARM_RED[1], ARM_RED[2], 255))],
        nodes: vec![
            Node { name: None, parent: None, rest: Transform::IDENTITY },
            Node { name: None, parent: Some(0), rest: Transform::from_translation(Vec3::X) },
        ],
        ..Default::default()
    });
    let mut pose = Pose::rest(&model);
    pose.set(0, Transform::from_rotation(Quat::from_rotation_z(std::f32::consts::FRAC_PI_2)));
    let mut s = arm_scene();
    s.model(&model, Mat4::IDENTITY).pose(pose);
    let img = render(&g, &s, 1);
    assert!(near(px_at(&s, &img, Vec3::new(0.0, 1.0, 0.0)), ARM_RED, 1), "carried up by its parent");
    assert_eq!(px_at(&s, &img, Vec3::new(1.0, 0.0, 0.0))[3], 0, "no longer at its rest place");
}
