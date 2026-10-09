//! Render a canvas [`Scene`](canvas_core::Scene) with vello into a texture on
//! a device the CALLER owns — for embedding 2D canvas content in another GPU
//! renderer's frame (the `canvas3d` overlay). The on-screen canvas renderers
//! use the same encoder and the same image-atlas discipline.

use crate::anim::AnimTextures;
use crate::encode::encode_scene;
use canvas_core::{DrawOp, Scene as CanvasScene};
use vello::kurbo::Affine;
use vello::peniko::Color;
use vello::{AaConfig, AaSupport, Renderer, RendererOptions, RenderParams, Scene as VelloScene};

/// Build a vello `Renderer` with the canvas's standard options (area AA, GPU).
///
/// `single_threaded_init` MUST be `true` on an adopted GL context (GTK4).
/// vello's default shader init compiles pipelines on spawned WORKER threads —
/// but an adopted GL context is current only on the thread that made it
/// current, so those workers create GL programs with no context and panic
/// (`wgpu-hal gles/device.rs`). `num_init_threads: Some(1)` makes vello compile
/// on THIS thread. Swapchain (Metal/Vulkan/WebGPU) contexts keep the faster
/// parallel init.
pub(crate) fn new_vello_renderer(device: &wgpu::Device, single_threaded_init: bool) -> Option<Renderer> {
    Renderer::new(
        device,
        RendererOptions {
            use_cpu: false,
            antialiasing_support: AaSupport::area_only(),
            num_init_threads: if single_threaded_init { std::num::NonZeroUsize::new(1) } else { None },
            pipeline_cache: None,
        },
    )
    .ok()
}

/// Can vello's GPU pipeline run on `adapter`? ([`crate::VELLO_REQUIREMENTS`]:
/// compute + indirect dispatch, f16 on Vulkan.) `false` on WebGL2, the iOS
/// Simulator and the Android emulator.
pub fn adapter_can_run_vello(adapter: &wgpu::Adapter) -> bool {
    let compute = adapter
        .get_downlevel_capabilities()
        .flags
        .contains(wgpu::DownlevelFlags::COMPUTE_SHADERS);
    compute && gpu_surface::check_adapter(gpu_surface::AdapterFacts::of(adapter), crate::VELLO_REQUIREMENTS).is_ok()
}

/// A vello renderer for canvas scenes on a borrowed device.
///
/// Output is a [`gpu_surface::TARGET_FORMAT`] texture holding sRGB-encoded,
/// **straight-alpha** pixels (vello's storage format) over a transparent base.
pub struct SceneRenderer {
    renderer: Renderer,
    /// Image content renders on its own renderer so an image-less frame can't
    /// shrink the image atlas out from under a cached upload (see the
    /// `image_renderer` field of the on-screen renderers). Built on first use.
    image_renderer: Option<Renderer>,
    single_threaded_init: bool,
    anim: AnimTextures,
    scene: VelloScene,
}

impl SceneRenderer {
    /// `None` when vello's pipeline can't be built on `device`.
    pub fn new(device: &wgpu::Device, single_threaded_init: bool) -> Option<SceneRenderer> {
        Some(SceneRenderer {
            renderer: new_vello_renderer(device, single_threaded_init)?,
            image_renderer: None,
            single_threaded_init,
            anim: AnimTextures::new(),
            scene: VelloScene::new(),
        })
    }

    /// Render `scene` (logical units) at `scale` physical px per unit into
    /// `view` (`width`×`height`, a storage-capable `TARGET_FORMAT` texture),
    /// replacing its contents. Submits its own command buffer immediately, so
    /// anything that samples `view` afterwards (a compositor recorded into a
    /// later-submitted encoder) sees the result. `false` if vello failed.
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scene: &CanvasScene,
        view: &wgpu::TextureView,
        width: u32,
        height: u32,
        scale: f64,
    ) -> bool {
        self.scene.reset();
        encode_scene(scene.ops(), &mut self.scene, Affine::scale(scale));
        let has_image = scene.ops().iter().any(|op| matches!(op, DrawOp::Image { .. }));
        if has_image && self.image_renderer.is_none() {
            self.image_renderer = new_vello_renderer(device, self.single_threaded_init);
        }
        self.anim.apply(device, queue, &mut self.renderer, self.image_renderer.as_mut());
        let renderer = match (has_image, self.image_renderer.as_mut()) {
            (true, Some(r)) => r,
            _ => &mut self.renderer,
        };
        let params = RenderParams {
            base_color: Color::from_rgba8(0, 0, 0, 0),
            width,
            height,
            antialiasing_method: AaConfig::Area,
        };
        renderer.render_to_texture(device, queue, &self.scene, view, &params).is_ok()
    }
}
