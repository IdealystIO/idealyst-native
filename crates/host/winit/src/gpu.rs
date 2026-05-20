//! wgpu device / surface init wrapping a winit `Window`.

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
    /// `Arc<Window>` keeps the underlying handle alive as long as
    /// the surface — wgpu's `'static` surface lifetime requires it.
    pub window: Arc<Window>,
}

impl Gpu {
    pub fn new(window: Arc<Window>) -> Result<Self, String> {
        // wgpu 29: `InstanceDescriptor` no longer implements `Default`,
        // `Instance::new` takes `&InstanceDescriptor`, and the descriptor
        // gained `flags` / `memory_budget_thresholds` / `backend_options`.
        // Pass explicit defaults; we don't need any of the new knobs.
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::PRIMARY,
            flags: wgpu::InstanceFlags::default(),
            memory_budget_thresholds: Default::default(),
            backend_options: wgpu::BackendOptions::default(),
            // wgpu 29 added an explicit display-handle slot to the
            // instance descriptor (GLES/Wayland need it; on Vulkan
            // / Metal / DX12 it's ignored). `None` lets wgpu fall
            // back to the per-surface handle.
            display: None,
        });

        let surface = instance
            .create_surface(window.clone())
            .map_err(|e| format!("create_surface failed: {e}"))?;

        // wgpu 29: `request_adapter` returns `Result<Adapter, RequestAdapterError>`
        // (it used to return `Option<Adapter>` and we'd `.ok_or_else`).
        let adapter = pollster::block_on(instance.request_adapter(
            &wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::default(),
                force_fallback_adapter: false,
                compatible_surface: Some(&surface),
            },
        ))
        .map_err(|e| format!("no suitable GPU adapter: {e}"))?;

        // wgpu 29: `request_device` takes one arg (no trace path), and
        // `DeviceDescriptor` gained `experimental_features` + `trace`.
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("idealyst-wgpu-device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::downlevel_defaults()
                    .using_resolution(adapter.limits()),
                memory_hints: wgpu::MemoryHints::default(),
                experimental_features: wgpu::ExperimentalFeatures::default(),
                trace: wgpu::Trace::Off,
            },
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
