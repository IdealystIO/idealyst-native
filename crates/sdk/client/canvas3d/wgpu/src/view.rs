//! One mounted `Canvas3d`: the surface, the 3D pass and the overlay pass, and
//! the per-platform bring-up.

use crate::mesh::{choose_sample_count, MeshRenderer};
use crate::overlay::OverlayPass;
use crate::LABEL;
use canvas3d_core::{Canvas3dPrim, Frame3d};
use gpu_surface::{GpuSurface, SurfaceState};
use runtime_vocabulary::caps::GraphicsOps;
use std::rc::Rc;

/// Per-surface renderer state. `gpu` is the FIRST field: on the adopted-GL
/// path its `Drop` makes the context current before the passes' GPU objects
/// below drop.
pub(crate) struct Canvas3dState {
    gpu: GpuSurface,
    mesh: MeshRenderer,
    /// Built on the first frame that has overlay content.
    overlay: Option<OverlayPass>,
}

impl Canvas3dState {
    pub(crate) fn new(gpu: GpuSurface) -> Canvas3dState {
        let samples = choose_sample_count(gpu.adapter());
        let mesh = MeshRenderer::new(&gpu.device, &gpu.queue, samples);
        Canvas3dState { gpu, mesh, overlay: None }
    }

}

impl SurfaceState for Canvas3dState {
    type Frame = Frame3d;

    fn gpu(&self) -> &GpuSurface {
        &self.gpu
    }

    fn gpu_mut(&mut self) -> &mut GpuSurface {
        &mut self.gpu
    }

    fn render(&mut self, frame: &Frame3d) -> bool {
        self.gpu.make_current();
        let size = self.gpu.size();
        let scale = self.gpu.scale();

        // Overlay first: the GPU rasterizer submits its own work, which must
        // precede the encoder that composites it.
        let overlay_ready = match &frame.overlay {
            Some(scene) if !scene.ops().is_empty() => {
                let single = self.gpu.is_gl();
                let pass = self
                    .overlay
                    .get_or_insert_with(|| OverlayPass::new(&self.gpu.device, self.gpu.adapter(), single));
                pass.prepare(&self.gpu.device, &self.gpu.queue, scene, size, scale)
            }
            _ => false,
        };

        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("canvas3d-frame") });
        self.mesh.render(&self.gpu.device, &self.gpu.queue, &mut encoder, &frame.scene, self.gpu.target_view(), size);
        if overlay_ready {
            if let Some(pass) = &self.overlay {
                pass.composite(&self.gpu.device, &mut encoder, self.gpu.target_view());
            }
        }
        let Some(present) = self.gpu.begin_present(&mut encoder) else {
            return false;
        };
        self.gpu.queue.submit([encoder.finish()]);
        self.gpu.finish_present(present);
        true
    }

    fn resized(&mut self) {
        self.mesh.resized();
        if let Some(o) = &mut self.overlay {
            o.resized();
        }
    }
}

/// The GPU-path description reported to the author's `Canvas3dHandle`.
#[cfg(not(target_arch = "wasm32"))]
fn describe(gpu: &GpuSurface) -> String {
    let info = gpu.adapter().get_info();
    format!("{:?}", info.backend)
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn build<H: GraphicsOps>(prim: &Rc<Canvas3dPrim>, backend: &mut H) -> H::Node {
    let sizing = prim.size_reporter();
    let paint_prim = prim.clone();
    let handle = prim.props.handle.clone();
    gpu_surface::mount(
        backend,
        LABEL,
        gpu_surface::Requirements::NONE,
        gpu_surface::FrameAlpha::Premultiplied,
        move || paint_prim.paint(),
        move |w, h| sizing.report(w, h),
        move |gpu| {
            if let Some(h) = &handle {
                h.report_renderer(describe(&gpu));
            }
            Some(Canvas3dState::new(gpu))
        },
    )
}

/// Web bring-up: WebGPU when the browser has a working adapter, else WebGL2
/// (WebGL has no headless adapter, so the WebGPU probe runs first and WebGL2
/// claims the canvas only once WebGPU is ruled out). Neither available → the
/// view stays blank with a console error naming why.
#[cfg(target_arch = "wasm32")]
pub(crate) fn build<H: GraphicsOps>(prim: &Rc<Canvas3dPrim>, backend: &mut H) -> H::Node {
    use gpu_surface::{claim_webgl, debug_flag, drive, marker, FrameAlpha, Requirements, WebGpuProbe};
    let sizing = prim.size_reporter();
    let paint_prim = prim.clone();
    let handle = prim.props.handle.clone();
    gpu_surface::mount(
        backend,
        move || paint_prim.paint(),
        move |w, h| sizing.report(w, h),
        move |ev| {
            let handle = handle.clone();
            Box::pin(async move {
                // Debug-only: `window.__IDEALYST_FORCE_WEBGL = true` skips WebGPU
                // so the WebGL2 path can be exercised on a WebGPU browser.
                let probe = if debug_flag("__IDEALYST_FORCE_WEBGL") {
                    marker("canvas3d: WebGL2 forced (__IDEALYST_FORCE_WEBGL set)");
                    None
                } else {
                    WebGpuProbe::run(Requirements::NONE, LABEL).await
                };
                let gpu = match probe {
                    Some(probe) => probe.claim(ev, FrameAlpha::Premultiplied),
                    None => claim_webgl(ev, FrameAlpha::Premultiplied, LABEL).await,
                };
                match gpu {
                    Some(gpu) => {
                        if let Some(h) = &handle {
                            h.report_renderer(gpu.backend().name());
                        }
                        Some(drive(Canvas3dState::new(gpu)))
                    }
                    None => {
                        web_sys_error("canvas3d: neither WebGPU nor WebGL2 is available — the 3D view stays blank");
                        None
                    }
                }
            })
        },
    )
}

#[cfg(target_arch = "wasm32")]
fn web_sys_error(msg: &str) {
    gpu_surface::marker(msg);
    log::error!("{msg}");
}
