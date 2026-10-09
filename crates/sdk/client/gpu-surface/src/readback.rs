//! Headless GPU access + texture read-back: what tests, offscreen export and
//! surface-less compositors use instead of a window.

/// Tightly-packed, top-down `RGBA8` pixels (`width * height * 4` bytes).
pub struct RenderedImage {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

impl RenderedImage {
    /// The `[r, g, b, a]` texel at `(x, y)`.
    pub fn px(&self, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * self.width + x) * 4) as usize;
        [self.data[i], self.data[i + 1], self.data[i + 2], self.data[i + 3]]
    }
}

/// A headless wgpu `(device, queue)`: a real GPU adapter first, then a
/// **software** adapter (Mesa lavapipe / DX WARP) so it runs where there's no
/// GPU (CI, servers). No surface. **Blocking** (drives wgpu via `pollster`) —
/// call off any async/request path. `None` when no adapter or device exists.
pub fn headless_device() -> Option<(wgpu::Device, wgpu::Queue)> {
    headless_device_with_limits(None)
}

/// [`headless_device`] with the device held to `limits` instead of the
/// default. Passing `wgpu::Limits::downlevel_webgl2_defaults()` gives a device
/// that rejects anything WebGL2 can't do (storage buffers/textures, compute
/// workgroups) — the way to prove a pipeline WebGL2-safe without a browser.
pub fn headless_device_with_limits(
    limits: Option<wgpu::Limits>,
) -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::all(),
        flags: wgpu::InstanceFlags::default(),
        memory_budget_thresholds: Default::default(),
        backend_options: wgpu::BackendOptions::default(),
        display: None,
    });
    let request = |fallback: bool| {
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: fallback,
            compatible_surface: None,
        }))
    };
    // Real GPU first; software adapter (lavapipe / WARP) if there's none.
    let adapter = request(false).or_else(|_| request(true)).ok()?;
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("gpu-surface-headless"),
        required_limits: limits.unwrap_or_default(),
        ..Default::default()
    }))
    .ok()
}

/// Copy a `TARGET_FORMAT` (4 bytes/texel) `target` back to the CPU as
/// tightly-packed, top-down RGBA8. `bytes_per_row` must be 256-aligned, so the
/// copy is padded and the padding stripped. **Blocking** (polls the device to
/// completion). The texture needs `COPY_SRC`.
pub fn read_target_rgba(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    target: &wgpu::Texture,
    w: u32,
    h: u32,
) -> RenderedImage {
    let unpadded = (w * 4) as usize;
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as usize;
    let padded = unpadded.div_ceil(align) * align;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("gpu-surface-readback"),
        size: (padded * h as usize) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: target,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded as u32),
                rows_per_image: Some(h),
            },
        },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
    queue.submit([enc.finish()]);
    buffer.slice(..).map_async(wgpu::MapMode::Read, |_| {});
    let _ = device.poll(wgpu::PollType::wait_indefinitely());

    let mapped = buffer.slice(..).get_mapped_range();
    let mut data = Vec::with_capacity(unpadded * h as usize);
    for row in 0..h as usize {
        let start = row * padded;
        data.extend_from_slice(&mapped[start..start + unpadded]);
    }
    RenderedImage { data, width: w, height: h }
}
