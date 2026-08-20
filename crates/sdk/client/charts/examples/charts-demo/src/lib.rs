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
/// other canvas author. `canvas_native::register` type-dispatches to the
/// concrete backend registry internally, so one registry-generic seam
/// serves both targets.
///
/// Registration is MANDATORY: an unregistered payload panics at realize, so
/// a missing call here fails loudly instead of drawing a blank box.
pub fn register_scene_extensions<H>(registry: &mut runtime_scene::Registry<H>)
where
    H: runtime_vocabulary::caps::ExternalOps
        + runtime_vocabulary::style_attach::StyleServices
        + 'static,
{
    canvas_native::register(registry);
}

/// Android entry shim. Kept for parity with the other examples even though
/// this demo does not target Android.
pub fn scene_app() -> runtime_core::Element {
    app()
}
