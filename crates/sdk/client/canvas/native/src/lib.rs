//! `canvas-native` — the native-2D-engine renderer for the `canvas` SDK.
//!
//! Registers a scene handler for [`canvas_core::CanvasPrim`] that replays
//! the author's [`Scene`] with the platform's native 2D engine. The app
//! selects this renderer (over `canvas-vello`) by passing [`register`] to
//! the boot entry's registry seam.
//!
//! Per-target impls live in cfg-gated modules; only one compiles per
//! build. Hosts with no native 2D engine (desktop Linux/Windows, the
//! terminal, the wgpu host, the test harness) get the
//! External-placeholder handler — use `canvas-vello` there.
//!
//! [`Scene`]: canvas_core::Scene
#![deny(missing_docs)]

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use canvas_core::CanvasPrim;
use runtime_scene::{Element, MountCx, Registry};
use runtime_vocabulary::caps::ExternalOps;
use runtime_vocabulary::style_attach::{attach_style, on_teardown, StyleServices};

// Shared glyph-outline expansion for `DrawOp::Glyphs`, used by every CPU
// backend (web / apple / android). Gated to those targets so the
// placeholder build (no native 2D engine) doesn't carry an unused skrifa
// dependency.
#[cfg(any(
    target_arch = "wasm32",
    all(
        any(target_os = "ios", target_os = "macos", target_os = "android"),
        not(target_arch = "wasm32")
    )
))]
mod glyphs;

// Web: the core-free Canvas2D rasterizer (`web`) + the
// `WebBackend`-concrete mount handler that drives it (`web_scene`).
#[cfg(target_arch = "wasm32")]
mod web;
#[cfg(target_arch = "wasm32")]
mod web_scene;
// Reusable Canvas2D rasterizer + capture helper — `canvas-vello`'s web renderer
// calls these as its WebGPU-unavailable fallback (renders into the graphics
// primitive's own `<canvas>`, same output as this crate's standalone handler)
// and for self-capture on its GPU path (captureStream works on any canvas).
#[cfg(target_arch = "wasm32")]
pub use web::{make_2d_rasterizer, publish_capture_stream};

// Shared CoreGraphics painter for the Apple platforms (iOS + macOS).
// The Scene→CGContext op-replay is platform-identical; only context
// acquisition + the bezier/color vtable differ per backend.
#[cfg(all(any(target_os = "ios", target_os = "macos"), not(target_arch = "wasm32")))]
mod apple;

#[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
mod ios;
#[cfg(all(target_os = "macos", not(target_arch = "wasm32")))]
mod macos;
#[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
mod android;

/// Shared mount tail for a concrete-backend canvas handler: author style
/// onto the node the platform builder returned, then the scope-tied
/// `release_external` teardown (every external mount releases at unmount,
/// handler-backed or not).
pub(crate) fn finish_mount<H>(backend: &Rc<RefCell<H>>, node: &H::Node, prim: &CanvasPrim)
where
    H: ExternalOps + StyleServices,
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

/// Placeholder handler for hosts with no native 2D engine — the External
/// degradation path, so a canvas renders the host's labeled "unsupported"
/// box instead of panicking at realize (an unregistered payload panics on
/// the scene registry). The fill default still attaches, so the box is
/// visible.
fn mount_placeholder<H>(
    cx: &mut MountCx<'_, H>,
    prim: &Rc<CanvasPrim>,
    _children: Vec<Element>,
) -> H::Node
where
    H: ExternalOps + StyleServices,
{
    let backend = cx.backend().clone();
    let payload: Rc<dyn Any> = prim.props.clone();
    let node = backend.borrow_mut().create_external(
        std::any::TypeId::of::<canvas_core::CanvasProps>(),
        std::any::type_name::<canvas_core::CanvasProps>(),
        &payload,
        &runtime_shared::accessibility::AccessibilityProps::default(),
    );
    finish_mount(&backend, &node, prim);
    node
}

/// Install the native-2D canvas renderer on a scene registry. Pass as
/// (part of) the boot registration seam —
/// `backend_web::newcore::start_in("#app", canvas_native::register, app)`,
/// `host_appkit::newcore::run_with(build, opts, |r| canvas_native::register(r))`,
/// the mobile `run_in_view`, …
///
/// # One `register`, resolved at registration time
///
/// Every real renderer here is backend-CONCRETE (a `<canvas>` 2D context,
/// a `UIView`/`NSView` subclass, an Android `ImageView` — none of it has a
/// caps-trait expression), but a native build must ALSO serve
/// `Registry<HostMock>` for the test harness. A cfg-split pair of
/// same-named `register` fns cannot express that, so `register` stays
/// generic on every target and type-dispatches ONCE at registration: it
/// downcasts `&mut Registry<H>` to the platform's concrete registry
/// (`H: 'static` makes the registry `Any`) and installs the native
/// handler on hit; every other `H` gets the placeholder. Mount-path cost:
/// zero.
pub fn register<H>(registry: &mut Registry<H>)
where
    H: ExternalOps + StyleServices + 'static,
{
    #[cfg(target_arch = "wasm32")]
    {
        let any: &mut dyn Any = registry;
        if let Some(reg) = any.downcast_mut::<Registry<backend_web::WebBackend>>() {
            reg.register::<CanvasPrim, _>(web_scene::mount_canvas);
            return;
        }
    }
    #[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
    {
        let any: &mut dyn Any = registry;
        if let Some(reg) = any.downcast_mut::<Registry<backend_ios::IosBackend>>() {
            reg.register::<CanvasPrim, _>(ios::mount_canvas);
            return;
        }
    }
    #[cfg(all(target_os = "macos", not(target_arch = "wasm32")))]
    {
        let any: &mut dyn Any = registry;
        if let Some(reg) = any.downcast_mut::<Registry<backend_macos::MacosBackend>>() {
            reg.register::<CanvasPrim, _>(macos::mount_canvas);
            return;
        }
    }
    #[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
    {
        let any: &mut dyn Any = registry;
        if let Some(reg) = any.downcast_mut::<Registry<backend_android::AndroidBackend>>() {
            reg.register::<CanvasPrim, _>(android::mount_canvas);
            return;
        }
    }
    registry.register::<CanvasPrim, _>(mount_placeholder::<H>);
}
