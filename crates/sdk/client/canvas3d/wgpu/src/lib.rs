//! `canvas3d-wgpu` — the wgpu renderer for the `canvas3d` SDK.
//!
//! Draws a `canvas3d_core::Scene3d` (glTF metallic-roughness meshes, directional
//! + ambient light, world-space lines) and composites the view's 2D overlay
//! over it in the same frame, on the `graphics` primitive's surface:
//!
//! | Platform | GPU path                                              |
//! | -------- | ----------------------------------------------------- |
//! | web      | WebGPU, else WebGL2 (decided per view, see `view.rs`) |
//! | macOS / iOS | Metal                                              |
//! | Android  | Vulkan (GL where Vulkan is absent)                    |
//! | Windows  | DX12 / Vulkan                                         |
//! | Linux    | the GTK area's GL context                             |
//!
//! Every shader stays inside WebGL2's limits (uniform buffers only, no
//! compute, no storage), so the same pipelines run on all of them. The 2D
//! overlay uses GPU vello where its compute pipeline runs and vello_cpu
//! elsewhere (WebGL2, simulators).
//!
//! Register it at the app's boot seam: `canvas3d_wgpu::register(registry)`.

#![allow(missing_docs)]

mod mesh;
mod overlay;
mod textures;
mod view;

#[cfg(all(test, not(target_arch = "wasm32")))]
mod gpu_tests;

pub use mesh::{choose_sample_count, MeshRenderer, DEPTH_FORMAT};

use canvas3d_core::Canvas3dPrim;
use runtime_scene::{Element, MountCx, Registry};
use runtime_vocabulary::caps::GraphicsOps;
use runtime_vocabulary::style_attach::{attach_style, on_teardown, StyleServices};
use std::cell::RefCell;
use std::rc::Rc;

/// Name in this renderer's logs and console markers.
pub(crate) const LABEL: &str = "canvas3d";

/// Install the `Canvas3d` handler on a scene registry.
///
/// Native: a 3D view needs a GPU adapter (any — no compute or special
/// features). On a device with none at all, the handler is still installed but
/// panics when a `Canvas3d` is actually shown, naming why — a 3D view can't
/// fall back to anything meaningful, and a silent blank is worse.
#[cfg(not(target_arch = "wasm32"))]
pub fn register<H>(registry: &mut Registry<H>)
where
    H: GraphicsOps + StyleServices + 'static,
{
    if gpu_surface::adapter_meets_default(gpu_surface::Requirements::NONE, LABEL) {
        registry.register::<Canvas3dPrim, _>(mount::<H>);
    } else {
        registry.register::<Canvas3dPrim, _>(|_cx: &mut MountCx<'_, H>, _prim, _children| {
            panic!("canvas3d: this device has no GPU adapter, so a Canvas3d view cannot be shown")
        });
    }
}

/// Install the `Canvas3d` handler on a scene registry (web). Each view picks
/// WebGPU or WebGL2 when its surface is ready.
#[cfg(target_arch = "wasm32")]
pub fn register<H>(registry: &mut Registry<H>)
where
    H: GraphicsOps + StyleServices + 'static,
{
    registry.register::<Canvas3dPrim, _>(mount::<H>);
}

/// Queue the handler for registration from a lazily-loaded chunk, so wgpu and
/// this renderer stay out of the initial bundle. Requires the app to declare
/// `Registry::defer::<canvas3d_core::Canvas3dPrim>()` at boot. The caller pins
/// `H` to its backend: `register_from_chunk::<backend_web::WebBackend>()`.
#[cfg(target_arch = "wasm32")]
pub fn register_from_chunk<H>()
where
    H: runtime_scene::Host + GraphicsOps + StyleServices + 'static,
{
    runtime_scene::defer_registration::<H, _>(|registry| {
        registry.register_deferred::<Canvas3dPrim, _>(mount::<H>);
    });
}

fn mount<H>(cx: &mut MountCx<'_, H>, prim: &Rc<Canvas3dPrim>, _children: Vec<Element>) -> H::Node
where
    H: GraphicsOps + StyleServices,
{
    let backend = cx.backend().clone();
    let node = {
        let mut b = backend.borrow_mut();
        view::build(prim, &mut *b)
    };
    finish_mount(&backend, &node, prim);
    node
}

/// Author style onto the graphics node, then the scope-tied
/// `release_external` teardown every external mount installs.
fn finish_mount<H>(backend: &Rc<RefCell<H>>, node: &H::Node, prim: &Canvas3dPrim)
where
    H: GraphicsOps + StyleServices,
{
    if let Some(style) = prim.take_style() {
        attach_style(backend, node, style);
    }
    let backend = backend.clone();
    let node = node.clone();
    on_teardown(move || {
        backend.borrow_mut().release_external(&node);
    });
}
