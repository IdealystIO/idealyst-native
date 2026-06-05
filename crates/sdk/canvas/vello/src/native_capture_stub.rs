//! Non-macOS stub for [`NativeCapture`]. canvas-vello also builds on wgpu
//! desktop (Linux/Windows), where there's no IOSurface zero-copy path — those
//! targets record through the CPU read-back fallback in `render.rs`. Same API as
//! the macOS module, all no-ops, so `render.rs` stays free of `cfg`.

use media_stream::FrameWriter;

pub(crate) struct NativeCapture;

impl NativeCapture {
    pub(crate) fn new(_writer: FrameWriter) -> Self {
        NativeCapture
    }

    pub(crate) fn wants(&self) -> bool {
        false
    }

    pub(crate) fn blit_into(
        &mut self,
        _device: &wgpu::Device,
        _encoder: &mut wgpu::CommandEncoder,
        _src_view: &wgpu::TextureView,
        _w: u32,
        _h: u32,
    ) -> Option<usize> {
        None
    }

    pub(crate) fn publish(&self, _idx: usize) {}
}

/// Non-macOS stub: no GPU layer compositor (those targets show the camera via
/// an overlay `video` widget instead).
pub(crate) struct LayerCompositor;

impl LayerCompositor {
    pub(crate) fn new(_device: &wgpu::Device) -> Self {
        LayerCompositor
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn composite_layers(
        &mut self,
        _device: &wgpu::Device,
        _queue: &wgpu::Queue,
        _encoder: &mut wgpu::CommandEncoder,
        _layers: &[canvas_core::TextureLayer],
        _target_view: &wgpu::TextureView,
        _scale: f32,
        _target_w: u32,
        _target_h: u32,
    ) {
    }
}
