//! `sdk-gallery` — one panel per client SDK, for checking each on a
//! platform by hand: an SVG, a clipboard round trip, live connectivity,
//! a notification (now and scheduled), and an embedded web page.

mod app;

pub use app::app;

/// Register the scene handlers the gallery's SDK primitives need. An
/// unregistered payload panics at realize, so a missing call fails loudly.
pub fn register_scene_extensions<H>(registry: &mut runtime_scene::Registry<H>)
where
    H: runtime_vocabulary::caps::ExternalOps + runtime_vocabulary::style_attach::StyleServices + 'static,
{
    svg::register(registry);
    webview::register(registry);
}

/// Android entry shim, for parity with the other examples.
pub fn scene_app() -> runtime_core::Element {
    app()
}
