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
//!
//! # Not redrawing what didn't change
//!
//! An animating 3D view repaints every display frame, and its overlay
//! (a marker, a label) usually doesn't move. Rasterizing a full-screen
//! overlay on the CPU and uploading it costs ~100 ms per frame in an
//! unoptimized build at phone resolution — enough to starve the main thread
//! into Android's "not responding" dialog. So, for a scene `cpu_footprint`
//! can measure (and that therefore carries no state across frames):
//!
//! - an overlay equal to last frame's is not rasterized again — the texture
//!   still holds it;
//! - the CPU path rasterizes and uploads only the scene's footprint, and
//!   clears what the previous frame drew outside it.
//!
//! Scenes it can't measure (text, retained layers, masks) take the full
//! path every frame, as before.

use canvas_vello::{Footprint, PixelRect};
use gpu_surface::{make_target, FrameAlpha, OverlayCompositor};

enum Raster {
    Gpu(canvas_vello::SceneRenderer),
    Cpu,
}

pub(crate) struct OverlayPass {
    raster: Raster,
    compositor: OverlayCompositor,
    target: Option<(wgpu::Texture, wgpu::TextureView, (u32, u32))>,
    /// The stateless scene the texture holds, with the size and scale it was
    /// drawn at. `None` when the texture's content is unknown or stateful.
    last: Option<(Vec<canvas_core::DrawOp>, (u32, u32), f64)>,
    /// Where the texture may hold non-transparent pixels (CPU path).
    drawn: Option<PixelRect>,
    /// Rasterizations performed (tests check that unchanged frames skip).
    #[cfg(test)]
    pub(crate) rasterized: usize,
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
            last: None,
            drawn: None,
            #[cfg(test)]
            rasterized: 0,
        }
    }

    /// The CPU rasterizer regardless of the adapter (tests compare the two).
    #[cfg(test)]
    pub(crate) fn new_cpu(device: &wgpu::Device) -> OverlayPass {
        OverlayPass {
            raster: Raster::Cpu,
            compositor: OverlayCompositor::new(device, FrameAlpha::Premultiplied, FrameAlpha::Premultiplied),
            target: None,
            last: None,
            drawn: None,
            rasterized: 0,
        }
    }

    /// Where the texture may hold pixels (CPU path).
    #[cfg(test)]
    pub(crate) fn drawn_region(&self) -> Option<PixelRect> {
        self.drawn
    }

    pub(crate) fn resized(&mut self) {
        self.target = None;
        self.last = None;
        self.drawn = None;
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
        if size.0 == 0 || size.1 == 0 {
            return false;
        }
        let footprint = canvas_vello::cpu_footprint(scene, size.0, size.1, scale);
        if footprint == Footprint::Nothing {
            // Nothing to composite; what the texture holds is cleared before
            // anything is drawn into it again.
            self.last = None;
            return false;
        }
        let stateless = footprint != Footprint::Unknown;
        if stateless
            && self.target.is_some()
            && self.last.as_ref().is_some_and(|(s, sz, sc)| *sz == size && *sc == scale && s.as_slice() == scene.ops())
        {
            return true;
        }
        if self.target.as_ref().is_none_or(|t| t.2 != size) {
            let (t, v) = make_target(device, size.0, size.1, "canvas3d-overlay");
            self.target = Some((t, v, size));
            self.drawn = None;
        }
        #[cfg(test)]
        {
            self.rasterized += 1;
        }
        self.last = stateless.then(|| (scene.ops().to_vec(), size, scale));
        let (texture, view, _) = self.target.as_ref().expect("built");
        match &mut self.raster {
            Raster::Gpu(r) => r.render(device, queue, scene, view, size.0, size.1, scale),
            Raster::Cpu => {
                let full = PixelRect { x: 0, y: 0, w: size.0, h: size.1 };
                let region = match footprint {
                    // A footprint covering most of the frame gains nothing
                    // from cropping.
                    Footprint::Region(r) if r.area() * 2 < full.area() => r,
                    _ => full,
                };
                if let Some(prev) = self.drawn.take() {
                    if prev != region && region != full {
                        write_rect(queue, texture, prev, &vec![0u8; prev.area() as usize * 4]);
                    }
                }
                let pixels = if region == full {
                    canvas_vello::rasterize_cpu(scene, size.0, size.1, scale)
                } else {
                    canvas_vello::rasterize_cpu_region(scene, region, scale)
                };
                write_rect(queue, texture, region, &pixels);
                self.drawn = Some(region);
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

/// Upload tightly packed RGBA8 `pixels` into `rect` of `texture`.
fn write_rect(queue: &wgpu::Queue, texture: &wgpu::Texture, rect: PixelRect, pixels: &[u8]) {
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d { x: rect.x, y: rect.y, z: 0 },
            aspect: wgpu::TextureAspect::All,
        },
        pixels,
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(rect.w * 4), rows_per_image: Some(rect.h) },
        wgpu::Extent3d { width: rect.w, height: rect.h, depth_or_array_layers: 1 },
    );
}
