//! `charts-demo` — an exercise of the `charts` SDK.
//!
//! Every chart kind the v1 SDK supports, over one dataset that can be
//! mutated live, so the reactive path (spec signal → memo → canvas repaint
//! + label rebuild) is visible rather than merely asserted in a test.
//!
//! Ships to web and macOS. The app registers `canvas_native::register`,
//! which is the real Canvas2D handler on web and the real CoreGraphics one
//! on macOS — the SAME app code and the SAME mark IR on both, which is the
//! property worth checking by eye.

mod app;

pub use app::app;

/// Register the scene handlers the demo needs.
///
/// A chart paints through a `Canvas`, and `charts` deliberately installs no
/// renderer of its own — picking one is the app's call, exactly as for any
/// other canvas author.
///
/// # Two renderers, one payload
///
/// Both `canvas-native` and `canvas-vello` install a handler for the SAME
/// `CanvasPrim`, and the scene registry is `TypeId`-keyed with last write
/// winning. So registering native first and vello second means: use the GPU
/// renderer when it is available, otherwise keep the native one.
///
/// That ordering is safe rather than merely hopeful. `canvas_vello::register`
/// no-ops when the GPU cannot run it — `gpu_can_run_vello()` on native (the
/// Android emulator's Vulkan lacks the f16 support vello needs), a missing
/// `navigator.gpu` on web — leaving the native registration in place. No
/// `cfg`, no simulator predicate, no per-environment branch.
///
/// vello is behind the `vello` feature and OFF by default. The chart code is
/// identical either way; this seam is the only thing that changes.
///
/// Registration is MANDATORY: an unregistered payload panics at realize, so
/// a missing call here fails loudly instead of drawing a blank box.
pub fn register_scene_extensions<H>(registry: &mut runtime_scene::Registry<H>)
where
    H: runtime_vocabulary::caps::ExternalOps
        + runtime_vocabulary::caps::GraphicsOps
        + runtime_vocabulary::style_attach::StyleServices
        + 'static,
{
    // Baseline: real on every target this demo ships to.
    canvas_native::register(registry);
    // Upgrade, when asked for AND the GPU can run it. Must come second —
    // last registration wins.
    #[cfg(feature = "vello")]
    canvas_vello::register(registry);
}

/// Android entry shim. Kept for parity with the other examples even though
/// this demo does not target Android.
pub fn scene_app() -> runtime_core::Element {
    app()
}
