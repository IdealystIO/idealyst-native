//! Pure bring-up decisions — what an adapter must offer, which surface format
//! and alpha mode to configure. Kept free of live GPU objects so every rule is
//! unit-testable against synthetic capabilities.

/// What a renderer needs from an adapter beyond "it exists".
///
/// These are **capability** checks, never platform checks (CLAUDE.md §7): a
/// renderer whose pipeline can't run on an adapter declares the missing
/// capability here and steps aside on any GPU that lacks it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Requirements {
    /// Downlevel flags the pipeline uses. vello is GPU-driven and needs
    /// `INDIRECT_EXECUTION` (the iOS Simulator's Metal lacks it).
    pub downlevel: wgpu::DownlevelFlags,
    /// The pipeline uses f16 in shaders, which naga enforces as the explicit
    /// `SHADER_F16` feature on Vulkan only (Metal/DX12 don't require it). vello's
    /// `flatten` shader is the case; the Android emulator's Vulkan lacks it.
    pub f16_on_vulkan: bool,
    /// The pipeline needs a hardware GPU: a software adapter (WARP on
    /// Windows, llvmpipe, SwiftShader — wgpu's `DeviceType::Cpu`) is
    /// refused. vello is the case: its GPU-driven compute pipeline on a
    /// software rasterizer is slower than the platform's own CPU painter
    /// (`canvas-native`), and the WARP that ships with Windows 11 (10.0.26100)
    /// crashes running it — an access violation inside `d3d10warp.dll` that
    /// takes the app down (the redistributable WARP 1.0.20 renders it fine,
    /// but an app cannot count on having it). A plain raster pipeline
    /// (canvas3d) runs fine on WARP and does not set this.
    pub hardware: bool,
}

impl Requirements {
    /// No capability beyond a working adapter (a plain raster pipeline).
    pub const NONE: Requirements =
        Requirements { downlevel: wgpu::DownlevelFlags::empty(), f16_on_vulkan: false, hardware: false };
}

impl Default for Requirements {
    fn default() -> Self {
        Requirements::NONE
    }
}

/// The adapter facts [`check_adapter`] decides on — split out of
/// `wgpu::Adapter` so the rule can be tested with any combination.
#[derive(Clone, Copy, Debug)]
pub struct AdapterFacts {
    pub backend: wgpu::Backend,
    pub features: wgpu::Features,
    pub downlevel: wgpu::DownlevelFlags,
    pub device_type: wgpu::DeviceType,
}

impl AdapterFacts {
    pub fn of(adapter: &wgpu::Adapter) -> Self {
        let info = adapter.get_info();
        AdapterFacts {
            backend: info.backend,
            features: adapter.features(),
            downlevel: adapter.get_downlevel_capabilities().flags,
            device_type: info.device_type,
        }
    }
}

/// Why an adapter can't run a renderer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unsupported {
    /// Missing downlevel flags (the subset of `Requirements::downlevel` absent).
    Downlevel(wgpu::DownlevelFlags),
    /// Vulkan without `SHADER_F16` while the pipeline needs f16.
    VulkanWithoutF16,
    /// A software adapter while the pipeline needs a hardware GPU.
    Software,
}

impl std::fmt::Display for Unsupported {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Unsupported::Downlevel(missing) => write!(f, "lacks downlevel {missing:?}"),
            Unsupported::VulkanWithoutF16 => f.write_str("is Vulkan without SHADER_F16"),
            Unsupported::Software => f.write_str("is a software rasterizer, not a GPU"),
        }
    }
}

/// Can an adapter with `facts` run a pipeline needing `req`?
pub fn check_adapter(facts: AdapterFacts, req: Requirements) -> Result<(), Unsupported> {
    if req.hardware && facts.device_type == wgpu::DeviceType::Cpu {
        return Err(Unsupported::Software);
    }
    let missing = req.downlevel - facts.downlevel;
    if !missing.is_empty() {
        return Err(Unsupported::Downlevel(missing));
    }
    if req.f16_on_vulkan
        && facts.backend == wgpu::Backend::Vulkan
        && !facts.features.contains(wgpu::Features::SHADER_F16)
    {
        return Err(Unsupported::VulkanWithoutF16);
    }
    Ok(())
}

/// Pick the surface format: the first NON-sRGB format offered, else the
/// first. The frame target holds already-sRGB-encoded bytes and the present
/// blit copies them through, so an sRGB surface (often `formats[0]` on macOS)
/// would gamma-encode them a second time and wash every colour out.
pub fn choose_surface_format(formats: &[wgpu::TextureFormat]) -> Option<wgpu::TextureFormat> {
    formats.iter().copied().find(|f| !f.is_srgb()).or_else(|| formats.first().copied())
}

/// Pick the surface alpha mode. The view composites over the UI behind it and
/// is transparent wherever nothing was drawn, so `Opaque` is wrong (unpainted
/// pixels would show as black). Prefer `PreMultiplied`, then
/// `PostMultiplied`, then whatever is offered.
///
/// `force_premultiplied`: wgpu's WebGPU backend reports only `Opaque`, but
/// every `GPUCanvasContext` supports `premultiplied` per the WebGPU spec — the
/// list under-reports, so web configures `PreMultiplied` directly.
pub fn choose_alpha_mode(
    modes: &[wgpu::CompositeAlphaMode],
    force_premultiplied: bool,
) -> wgpu::CompositeAlphaMode {
    use wgpu::CompositeAlphaMode::{PostMultiplied, PreMultiplied};
    if force_premultiplied || modes.contains(&PreMultiplied) {
        PreMultiplied
    } else if modes.contains(&PostMultiplied) {
        PostMultiplied
    } else {
        modes.first().copied().unwrap_or(wgpu::CompositeAlphaMode::Auto)
    }
}

/// The alpha mode for a swapchain created for a COMPOSITION visual
/// (Windows: `CreateSwapChainForComposition`). Such swapchains accept only
/// premultiplied or ignored alpha: wgpu's leading `Auto` maps to "unspecified"
/// and post-multiplied to "straight", both of which the call rejects, so the
/// swapchain failed at its first configure. Premultiplied when offered (a
/// transparent frame stays transparent over the scene), else `Opaque`.
pub fn composition_alpha_mode(modes: &[wgpu::CompositeAlphaMode]) -> wgpu::CompositeAlphaMode {
    if modes.contains(&wgpu::CompositeAlphaMode::PreMultiplied) {
        wgpu::CompositeAlphaMode::PreMultiplied
    } else {
        wgpu::CompositeAlphaMode::Opaque
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regression_composition_swapchains_never_get_auto_or_straight_alpha() {
        assert_eq!(composition_alpha_mode(&[A::Auto, A::Opaque, A::PreMultiplied]), A::PreMultiplied);
        assert_eq!(composition_alpha_mode(&[A::Auto, A::Opaque]), A::Opaque);
        assert_eq!(composition_alpha_mode(&[A::Auto, A::PostMultiplied]), A::Opaque);
        assert_eq!(composition_alpha_mode(&[]), A::Opaque);
    }
    use wgpu::{Backend, CompositeAlphaMode as A, DownlevelFlags as D, Features, TextureFormat as F};

    fn facts(backend: Backend, features: Features, downlevel: D) -> AdapterFacts {
        AdapterFacts { backend, features, downlevel, device_type: wgpu::DeviceType::DiscreteGpu }
    }

    const VELLO: Requirements =
        Requirements { downlevel: D::INDIRECT_EXECUTION, f16_on_vulkan: true, hardware: true };

    /// WARP (the Windows software rasterizer, `DeviceType::Cpu`) offers
    /// everything vello asks for, so vello took the canvas and WARP crashed
    /// inside its compute shaders, killing the app. A pipeline that needs
    /// hardware refuses it; one that doesn't (canvas3d) still runs there.
    #[test]
    fn regression_software_adapter_rejected_when_hardware_required() {
        let warp = AdapterFacts { device_type: wgpu::DeviceType::Cpu, ..facts(Backend::Dx12, Features::all(), D::all()) };
        assert_eq!(check_adapter(warp, VELLO), Err(Unsupported::Software));
        assert_eq!(check_adapter(warp, Requirements::default()), Ok(()));
        for hw in [wgpu::DeviceType::DiscreteGpu, wgpu::DeviceType::IntegratedGpu, wgpu::DeviceType::VirtualGpu, wgpu::DeviceType::Other] {
            let gpu = AdapterFacts { device_type: hw, ..warp };
            assert_eq!(check_adapter(gpu, VELLO), Ok(()), "{hw:?}");
        }
    }

    #[test]
    fn no_requirements_accepts_any_adapter() {
        let weak = facts(Backend::Gl, Features::empty(), D::empty());
        assert_eq!(check_adapter(weak, Requirements::default()), Ok(()));
    }

    #[test]
    fn regression_ios_simulator_without_indirect_rejected_for_vello() {
        // iOS Simulator Metal: f16 irrelevant on Metal, but no INDIRECT_EXECUTION.
        let sim = facts(Backend::Metal, Features::empty(), D::empty());
        assert_eq!(check_adapter(sim, VELLO), Err(Unsupported::Downlevel(D::INDIRECT_EXECUTION)));
        // ...while a renderer with no downlevel needs (3D) runs there.
        assert_eq!(check_adapter(sim, Requirements::default()), Ok(()));
    }

    #[test]
    fn regression_android_emulator_vulkan_without_f16_rejected_for_vello() {
        let emu = facts(Backend::Vulkan, Features::empty(), D::INDIRECT_EXECUTION);
        assert_eq!(check_adapter(emu, VELLO), Err(Unsupported::VulkanWithoutF16));
        // Metal without the explicit feature is fine: naga doesn't enforce it there.
        let metal = facts(Backend::Metal, Features::empty(), D::INDIRECT_EXECUTION);
        assert_eq!(check_adapter(metal, VELLO), Ok(()));
        let vk = facts(Backend::Vulkan, Features::SHADER_F16, D::INDIRECT_EXECUTION);
        assert_eq!(check_adapter(vk, VELLO), Ok(()));
    }

    #[test]
    fn surface_format_prefers_non_srgb() {
        assert_eq!(choose_surface_format(&[F::Bgra8UnormSrgb, F::Bgra8Unorm]), Some(F::Bgra8Unorm));
        assert_eq!(choose_surface_format(&[F::Rgba8UnormSrgb]), Some(F::Rgba8UnormSrgb));
        assert_eq!(choose_surface_format(&[]), None);
    }

    #[test]
    fn alpha_mode_never_picks_opaque_when_a_blending_mode_exists() {
        assert_eq!(choose_alpha_mode(&[A::Opaque, A::PreMultiplied], false), A::PreMultiplied);
        assert_eq!(choose_alpha_mode(&[A::Opaque, A::PostMultiplied], false), A::PostMultiplied);
        assert_eq!(choose_alpha_mode(&[A::Opaque], false), A::Opaque);
        // Web: the caps list under-reports; premultiplied is configured regardless.
        assert_eq!(choose_alpha_mode(&[A::Opaque], true), A::PreMultiplied);
    }
}
