//! The 2D overlay pass: a `canvas::Scene` drawn into its own frame-sized
//! texture, then composited over the 3D result in the same frame.
//!
//! Two interchangeable rasterizers, chosen once per surface by capability:
//!
//! - **vello (GPU)** where vello's compute pipeline runs — on the surface's own
//!   device, writing straight alpha.
//! - **vello_cpu** everywhere else (WebGL2, the iOS Simulator, the Android
//!   emulator) — rasterized on the CPU and uploaded, premultiplied.
//!
//! Both draw the same op list with the same conversions
//! (`canvas_vello::rasterize_cpu` is tested against GPU vello), and the
//! compositor is told each one's alpha convention, so the overlay looks the
//! same either way (CLAUDE.md §7).

use gpu_surface::{make_target, FrameAlpha, OverlayCompositor};

enum Raster {
    Gpu(canvas_vello::SceneRenderer),
    Cpu,
}

pub(crate) struct OverlayPass {
    raster: Raster,
    compositor: OverlayCompositor,
    target: Option<(wgpu::Texture, wgpu::TextureView, (u32, u32))>,
}

impl OverlayPass {
    /// `single_threaded_init`: the device is an adopted GL context (see
    /// `canvas_vello::SceneRenderer::new`).
    pub(crate) fn new(device: &wgpu::Device, adapter: &wgpu::Adapter, single_threaded_init: bool) -> OverlayPass {
        let raster = if canvas_vello::adapter_can_run_vello(adapter) {
            match canvas_vello::SceneRenderer::new(device, single_threaded_init) {
                Some(r) => Raster::Gpu(r),
                None => Raster::Cpu,
            }
        } else {
            Raster::Cpu
        };
        let src = match raster {
            Raster::Gpu(_) => FrameAlpha::Straight,
            Raster::Cpu => FrameAlpha::Premultiplied,
        };
        OverlayPass {
            raster,
            // The 3D frame target is premultiplied.
            compositor: OverlayCompositor::new(device, src, FrameAlpha::Premultiplied),
            target: None,
        }
    }

    /// The CPU rasterizer regardless of the adapter (tests compare the two).
    #[cfg(test)]
    pub(crate) fn new_cpu(device: &wgpu::Device) -> OverlayPass {
        OverlayPass {
            raster: Raster::Cpu,
            compositor: OverlayCompositor::new(device, FrameAlpha::Premultiplied, FrameAlpha::Premultiplied),
            target: None,
        }
    }

    pub(crate) fn resized(&mut self) {
        self.target = None;
    }

    /// Rasterize `scene` (logical units at `scale`) into the overlay texture.
    /// The GPU path submits its own work immediately; the CPU path writes
    /// through `queue`. Either way it lands before an encoder submitted
    /// afterwards. `false` when there's nothing to composite.
    pub(crate) fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scene: &canvas_core::Scene,
        size: (u32, u32),
        scale: f64,
    ) -> bool {
        if scene.ops().is_empty() || size.0 == 0 || size.1 == 0 {
            return false;
        }
        if self.target.as_ref().is_none_or(|t| t.2 != size) {
            let (t, v) = make_target(device, size.0, size.1, "canvas3d-overlay");
            self.target = Some((t, v, size));
        }
        let (texture, view, _) = self.target.as_ref().expect("built");
        match &mut self.raster {
            Raster::Gpu(r) => r.render(device, queue, scene, view, size.0, size.1, scale),
            Raster::Cpu => {
                let pixels = canvas_vello::rasterize_cpu(scene, size.0, size.1, scale);
                queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    &pixels,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(size.0 * 4),
                        rows_per_image: Some(size.1),
                    },
                    wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
                );
                true
            }
        }
    }

    /// Composite the prepared overlay over `dst` (the 3D frame target).
    pub(crate) fn composite(&self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder, dst: &wgpu::TextureView) {
        if let Some((_, view, _)) = &self.target {
            self.compositor.composite(device, encoder, view, dst);
        }
    }
}
