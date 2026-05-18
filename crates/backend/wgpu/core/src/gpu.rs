//! GPU adapter/device/surface init.
//!
//! Spun up once per `App::resumed` event (winit 0.30 requires the
//! window to exist before the surface, and on some platforms
//! `Surface` is invalidated on suspend). The render loop pulls
//! `&Gpu` out of `Option<Gpu>` — if it's None, painting is skipped
//! and the next `resumed` will rebuild it.

use std::sync::Arc;
use winit::window::Window;

pub struct Gpu {
    // Held so the surface stays valid (wgpu's surface borrows
    // internally from the instance) and so we can re-query limits
    // when reconfiguring after a device-lost event.
    #[allow(dead_code)]
    pub instance: wgpu::Instance,
    #[allow(dead_code)]
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub surface: wgpu::Surface<'static>,
    pub config: wgpu::SurfaceConfiguration,
    /// We keep the `Arc<Window>` alive for as long as the surface
    /// it backs — wgpu's `'static` surface lifetime requires the
    /// underlying handle to outlive every use.
    pub window: Arc<Window>,
}

impl Gpu {
    pub fn new(window: Arc<Window>) -> Result<Self, String> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::PRIMARY,
            ..Default::default()
        });

        let surface = instance
            .create_surface(window.clone())
            .map_err(|e| format!("create_surface failed: {e}"))?;

        let adapter = pollster::block_on(instance.request_adapter(
            &wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::default(),
                force_fallback_adapter: false,
                compatible_surface: Some(&surface),
            },
        ))
        .ok_or_else(|| "no suitable GPU adapter".to_string())?;

        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("idealyst-wgpu-device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::downlevel_defaults()
                    .using_resolution(adapter.limits()),
                memory_hints: wgpu::MemoryHints::default(),
            },
            None,
        ))
        .map_err(|e| format!("request_device failed: {e}"))?;

        let size = window.inner_size();
        let caps = surface.get_capabilities(&adapter);
        // Prefer an sRGB-encoded format so colors from CSS-style
        // sRGB hex values render correctly without manual encoding.
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(caps.formats[0]);
        log::debug!(
            "wgpu: adapter={:?} format={:?} (sRGB={}) size={}x{}",
            adapter.get_info().name,
            format,
            format.is_srgb(),
            size.width,
            size.height,
        );
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::AutoVsync,
            desired_maximum_frame_latency: 2,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
        };
        surface.configure(&device, &config);

        Ok(Self {
            instance,
            adapter,
            device,
            queue,
            surface,
            config,
            window,
        })
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
    }
}
