//! Z0 spike (gating): prove the zero-copy seam end to end on a headless Metal
//! device — create a BGRA IOSurface, wrap it as an `MTLTexture` on wgpu's OWN
//! Metal device (`as_hal` → `raw_device` → `newTextureWithDescriptor:iosurface:
//! plane:`), import that texture into wgpu (`create_texture_from_hal`), GPU
//! clear-render a known color into it, then read the IOSurface's CPU bytes back
//! and assert the GPU write landed in the shared surface. If this passes, vello
//! can render straight into an IOSurface the encoder consumes — no readback, no
//! swizzle.

#![cfg(target_os = "macos")]

use objc2::runtime::ProtocolObject;
use objc2_core_foundation::{CFDictionary, CFNumber, CFString};
use objc2_io_surface::{
    kIOSurfaceBytesPerElement, kIOSurfaceHeight, kIOSurfacePixelFormat, kIOSurfaceWidth,
    IOSurfaceLockOptions, IOSurfaceRef,
};
use objc2_metal::{
    MTLDevice, MTLPixelFormat, MTLStorageMode, MTLTextureDescriptor, MTLTextureType,
    MTLTextureUsage,
};

const W: u32 = 64;
const H: u32 = 48;
const PIXEL_FORMAT_BGRA: i32 = 0x4247_5241; // 'BGRA'

fn create_bgra_iosurface(w: u32, h: u32) -> objc2_core_foundation::CFRetained<IOSurfaceRef> {
    let width = CFNumber::new_i32(w as i32);
    let height = CFNumber::new_i32(h as i32);
    let bpe = CFNumber::new_i32(4);
    let pix = CFNumber::new_i32(PIXEL_FORMAT_BGRA);
    // SAFETY: the kIOSurface* keys are valid CFString statics from the linked
    // IOSurface framework; IOSurfaceCreate over a well-formed properties dict
    // returns a +1 retained surface (or null, handled).
    unsafe {
        let keys: [&CFString; 4] = [
            kIOSurfaceWidth,
            kIOSurfaceHeight,
            kIOSurfaceBytesPerElement,
            kIOSurfacePixelFormat,
        ];
        let values: [&CFNumber; 4] = [&width, &height, &bpe, &pix];
        let props = CFDictionary::from_slices(&keys, &values);
        // `from_slices` yields a typed `CFDictionary<CFString, CFNumber>`;
        // IOSurface wants the untyped `CFDictionary<Opaque, Opaque>`. The generic
        // params are PhantomData over the same CF object, so the cast is sound.
        let props_opaque: &CFDictionary =
            &*(&*props as *const CFDictionary<CFString, CFNumber> as *const CFDictionary);
        IOSurfaceRef::new(props_opaque).expect("IOSurfaceCreate returned null")
    }
}

#[test]
fn iosurface_backed_texture_receives_gpu_render() {
    // 1. Headless wgpu Metal device (no surface needed for the interop proof).
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::METAL,
        flags: wgpu::InstanceFlags::default(),
        memory_budget_thresholds: Default::default(),
        backend_options: wgpu::BackendOptions::default(),
        display: None,
    });
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        force_fallback_adapter: false,
        compatible_surface: None,
    }))
    .expect("request_adapter");
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("spike-device"),
        ..Default::default()
    }))
    .expect("request_device");

    // 2. A BGRA IOSurface — shared CPU/GPU memory.
    let surface = create_bgra_iosurface(W, H);

    // 3. Make an MTLTexture from the IOSurface on wgpu's OWN Metal device.
    let mtl_texture = {
        let hal_device = unsafe { device.as_hal::<wgpu::hal::api::Metal>() }
            .expect("device is not a Metal device");
        let mtl_device: &ProtocolObject<dyn MTLDevice> = hal_device.raw_device();

        // SAFETY: standard MTLTextureDescriptor configuration + IOSurface-backed
        // texture creation on wgpu's own MTLDevice; BGRA8Unorm matches the
        // IOSurface's pixel format and W×H, plane 0.
        unsafe {
            let desc = MTLTextureDescriptor::new();
            desc.setPixelFormat(MTLPixelFormat::BGRA8Unorm);
            desc.setWidth(W as usize);
            desc.setHeight(H as usize);
            desc.setUsage(MTLTextureUsage::RenderTarget | MTLTextureUsage::ShaderRead);
            desc.setStorageMode(MTLStorageMode::Shared);

            mtl_device
                .newTextureWithDescriptor_iosurface_plane(&desc, &surface, 0)
                .expect("newTextureWithDescriptor:iosurface:plane: returned null")
        }
    };

    // 4. Import the MTLTexture into wgpu as a normal wgpu::Texture.
    let hal_tex = unsafe {
        wgpu::hal::metal::Device::texture_from_raw(
            mtl_texture,
            wgpu::TextureFormat::Bgra8Unorm,
            MTLTextureType::Type2D,
            1,
            1,
            wgpu::hal::CopyExtent { width: W, height: H, depth: 1 },
        )
    };
    let wgpu_tex = unsafe {
        device.create_texture_from_hal::<wgpu::hal::api::Metal>(
            hal_tex,
            &wgpu::TextureDescriptor {
                label: Some("iosurface-target"),
                size: wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Bgra8Unorm,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            },
        )
    };

    // 5. GPU clear-render the imported texture to BLUE (r=0,g=0,b=1).
    let view = wgpu_tex.create_view(&wgpu::TextureViewDescriptor::default());
    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    {
        let _rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("clear-blue"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.0, g: 0.0, b: 1.0, a: 1.0 }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    }
    queue.submit([enc.finish()]);
    let _ = device.poll(wgpu::PollType::wait_indefinitely());

    // 6. Read the IOSurface's CPU bytes directly — the GPU render must be visible
    //    in the SHARED surface memory (that's the whole point). BGRA blue =
    //    [B=255, G=0, R=0, A=255].
    unsafe {
        let _ = surface.lock(IOSurfaceLockOptions::ReadOnly, std::ptr::null_mut());
    }
    let base = surface.base_address().as_ptr() as *const u8;
    let px = unsafe { std::slice::from_raw_parts(base, 4) };
    let pixel = [px[0], px[1], px[2], px[3]];
    eprintln!("[spike] IOSurface pixel[0] (BGRA) = {pixel:?}");
    unsafe {
        let _ = surface.unlock(IOSurfaceLockOptions::ReadOnly, std::ptr::null_mut());
    }

    assert!(
        pixel[0] > 200 && pixel[1] < 50 && pixel[2] < 50 && pixel[3] > 200,
        "GPU render did not land in the IOSurface as BGRA blue: {pixel:?}"
    );
}
