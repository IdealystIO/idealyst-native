//! The composition visuals that show lifted painted content
//! (`crate::layers`) above the graphics surfaces it covers.
//!
//! Each lifted layer is a visual inserted directly above its graphics
//! node's visual, holding a premultiplied-alpha composition swapchain the
//! size of the layer's content and offset to where it sits. Both renderers
//! paint a layer into premultiplied BGRA pixels (lifted content is usually
//! small — a tooltip, a popover), which are copied into the swapchain's
//! back buffer and presented. The device is created on first use, so a
//! window with nothing lifted never makes one.

use windows::core::{Interface, Result};
use windows::Win32::Foundation::E_FAIL;
use windows::Win32::Graphics::Direct2D::Common::{D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_PIXEL_FORMAT};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1Bitmap1, ID2D1DeviceContext, ID2D1Factory1, D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
    D2D1_BITMAP_OPTIONS_TARGET, D2D1_BITMAP_PROPERTIES1, D2D1_DEVICE_CONTEXT_OPTIONS_NONE,
    D2D1_FACTORY_TYPE_SINGLE_THREADED,
};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION,
};
use windows::Win32::Graphics::DirectComposition::IDCompositionVisual;
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_UNKNOWN, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    IDXGIDevice, IDXGIFactory2, IDXGISurface, IDXGISwapChain1, DXGI_PRESENT, DXGI_SCALING_STRETCH,
    DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_CHAIN_FLAG, DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
    DXGI_USAGE_RENDER_TARGET_OUTPUT,
};

use crate::dcomp::CompositionTree;

/// One lifted layer's frame: the graphics node it sits above, its device
/// box `(x, y, w, h)`, and its premultiplied BGRA pixels (`w * 4` per row,
/// top-down).
pub(crate) struct LayerPixels {
    pub above: u64,
    pub bounds: (i32, i32, u32, u32),
    pub pixels: Vec<u8>,
}

#[derive(Default)]
pub(crate) struct Overlays {
    gpu: Option<Gpu>,
    layers: Vec<Layer>,
}

struct Gpu {
    d3d: ID3D11Device,
    ctx: ID2D1DeviceContext,
}

struct Layer {
    above: u64,
    visual: IDCompositionVisual,
    swap: IDXGISwapChain1,
    size: (u32, u32),
    offset: (i32, i32),
    back: Option<ID2D1Bitmap1>,
}

impl Overlays {
    /// Show exactly `wanted` (in layer order), reusing visuals where the
    /// layer above the same graphics node already exists. `container_of`
    /// gives a graphics node's top-level visual, the one a layer is placed
    /// above.
    pub(crate) fn present(
        &mut self,
        tree: &CompositionTree,
        container_of: impl Fn(u64) -> Option<IDCompositionVisual>,
        wanted: Vec<LayerPixels>,
    ) {
        if wanted.is_empty() && self.layers.is_empty() {
            return;
        }
        if let Err(e) = self.try_present(tree, &container_of, wanted) {
            eprintln!("[backend-windows] overlay layer failed ({e}); dropping overlays");
            for l in self.layers.drain(..) {
                tree.remove_child(&l.visual);
            }
            self.gpu = None;
            tree.commit();
        }
    }

    fn try_present(
        &mut self,
        tree: &CompositionTree,
        container_of: &impl Fn(u64) -> Option<IDCompositionVisual>,
        wanted: Vec<LayerPixels>,
    ) -> Result<()> {
        let mut changed = false;
        while self.layers.len() > wanted.len() {
            let l = self.layers.pop().expect("non-empty");
            tree.remove_child(&l.visual);
            changed = true;
        }
        for (i, lp) in wanted.into_iter().enumerate() {
            let (x, y, w, h) = lp.bounds;
            if i >= self.layers.len() || self.layers[i].above != lp.above {
                let container = container_of(lp.above).ok_or_else(|| windows::core::Error::from_hresult(E_FAIL))?;
                let layer = self.new_layer(tree, &container, lp.above, (w, h))?;
                if i < self.layers.len() {
                    let old = std::mem::replace(&mut self.layers[i], layer);
                    tree.remove_child(&old.visual);
                } else {
                    self.layers.push(layer);
                }
                changed = true;
            }
            let layer = &mut self.layers[i];
            unsafe {
                if layer.size != (w, h) {
                    layer.back = None;
                    layer.swap.ResizeBuffers(0, w, h, DXGI_FORMAT_UNKNOWN, DXGI_SWAP_CHAIN_FLAG(0))?;
                    layer.size = (w, h);
                }
                if layer.offset != (x, y) {
                    layer.visual.SetOffsetX2(x as f32)?;
                    layer.visual.SetOffsetY2(y as f32)?;
                    layer.offset = (x, y);
                    changed = true;
                }
                if layer.back.is_none() {
                    let ctx = &self.gpu.as_ref().expect("a layer implies the device").ctx;
                    let surface: IDXGISurface = layer.swap.GetBuffer(0)?;
                    layer.back = Some(ctx.CreateBitmapFromDxgiSurface(
                        &surface,
                        Some(&bitmap_props()),
                    )?);
                }
                let back = layer.back.as_ref().expect("just created");
                back.CopyFromMemory(None, lp.pixels.as_ptr() as *const _, w * 4)?;
                layer.swap.Present(0, DXGI_PRESENT(0)).ok()?;
            }
        }
        if changed {
            tree.commit();
        }
        Ok(())
    }

    fn new_layer(
        &mut self,
        tree: &CompositionTree,
        above: &IDCompositionVisual,
        node: u64,
        size: (u32, u32),
    ) -> Result<Layer> {
        if self.gpu.is_none() {
            self.gpu = Some(unsafe { create_gpu()? });
        }
        let gpu = self.gpu.as_ref().expect("just created");
        unsafe {
            let factory: IDXGIFactory2 = gpu.d3d.cast::<IDXGIDevice>()?.GetAdapter()?.GetParent()?;
            let desc = DXGI_SWAP_CHAIN_DESC1 {
                Width: size.0,
                Height: size.1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 2,
                Scaling: DXGI_SCALING_STRETCH,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
                AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
                ..Default::default()
            };
            let swap = factory.CreateSwapChainForComposition(&gpu.d3d, &desc, None)?;
            let visual = tree.add_visual_above(above).ok_or_else(|| windows::core::Error::from_hresult(E_FAIL))?;
            visual.SetContent(&swap)?;
            Ok(Layer { above: node, visual, swap, size, offset: (i32::MIN, i32::MIN), back: None })
        }
    }
}

fn bitmap_props() -> D2D1_BITMAP_PROPERTIES1 {
    D2D1_BITMAP_PROPERTIES1 {
        pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
        dpiX: 96.0,
        dpiY: 96.0,
        bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
        colorContext: std::mem::ManuallyDrop::new(None),
    }
}

unsafe fn create_gpu() -> Result<Gpu> {
    let mut d3d: Option<ID3D11Device> = None;
    let mut created = Err(windows::core::Error::from_hresult(E_FAIL));
    for driver in [D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP] {
        created = D3D11CreateDevice(
            None,
            driver,
            None,
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut d3d),
            None,
            None,
        );
        if created.is_ok() {
            break;
        }
    }
    created?;
    let d3d = d3d.ok_or_else(|| windows::core::Error::from_hresult(E_FAIL))?;
    let factory: ID2D1Factory1 = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
    let device = factory.CreateDevice(&d3d.cast::<IDXGIDevice>()?)?;
    let ctx = device.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
    Ok(Gpu { d3d, ctx })
}
