//! `sdk-gallery` — one panel per client SDK, for checking each on a
//! platform by hand: an SVG, a clipboard round trip, live connectivity,
//! a notification (now and scheduled), an embedded web page, a video, a sound,
//! a location, the share sheet, the camera and a screen recording (with a
//! private "REC" badge the recording leaves out).

mod app;

pub use app::app;

/// Register the scene handlers the gallery's SDK primitives need. An
/// unregistered payload panics at realize, so a missing call fails loudly.
pub fn register_scene_extensions<H>(registry: &mut runtime_scene::Registry<H>)
where
    H: runtime_vocabulary::caps::ExternalOps + runtime_vocabulary::style_attach::StyleServices + 'static,
{
    // On web `svg::register` is `WebBackend`-concrete (its real renderer
    // is), while this seam is generic over the backend; the web boot always
    // passes the web registry.
    #[cfg(target_arch = "wasm32")]
    {
        let any: &mut dyn std::any::Any = registry;
        if let Some(web) = any.downcast_mut::<runtime_scene::Registry<backend_web::WebBackend>>() {
            svg::register(web);
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    svg::register(registry);
    webview::register(registry);
    video::register(registry);
    screen_recorder::register(registry);
}

/// Android entry shim, for parity with the other examples.
pub fn scene_app() -> runtime_core::Element {
    app()
}
