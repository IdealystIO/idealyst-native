//! The fixed half of the fiddle app. The server writes the submitted code
//! next to this file as `app.rs` (plus `app/<name>.rs` for any module it
//! declares), so `mod app;` here is the learner's code and `app()` is the
//! function their entry file must define.

mod app;

/// The root `idealyst::entry!` mounts. A wrapper rather than
/// `pub use app::app`: when the submitted `app` is a `#[component]`, the
/// macro also emits a type named `app`, and re-exporting it would collide
/// with `mod app` in the type namespace. A fn and a module never collide.
pub fn app() -> runtime_core::Element {
    app::app()
}

/// The scene registration seam `idealyst::entry!` calls. The fiddle
/// registers no extension SDKs, so a submitted file can render every
/// builtin primitive and idea-ui component, and nothing that needs a
/// registered handler.
pub fn register_scene_extensions<H: runtime_scene::Host>(
    _registry: &mut runtime_scene::Registry<H>,
) {
}
