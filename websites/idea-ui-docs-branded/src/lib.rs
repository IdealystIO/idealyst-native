//! idea-ui-docs under the "Ink" design language.
//!
//! The whole difference from `websites/idea-ui-docs` is one boot hook:
//! [`app`] mounts the docs app through
//! [`idea_ui_docs::app_with`] with [`brand::install`], which restyles
//! idea-ui's component slots before the first render. No page, route or
//! component is forked, and nothing here wraps or re-implements a
//! component — a design system built on `component_styles!` is branded
//! from the outside. See [`brand`] for the restyle set.

pub mod brand;

use runtime_core::Element;

/// Root entry: the docs app with the Ink restyles installed at boot.
pub fn app() -> Element {
    idea_ui_docs::app_with(brand::install)
}

// The platform registration seams are idea-ui-docs's own: the branded
// build registers exactly the same scene extensions and backend hooks.
pub use idea_ui_docs::{register_scene_extensions, route_ids};

#[cfg(any(
    target_arch = "wasm32",
    all(target_os = "ios", not(target_arch = "wasm32")),
    all(target_os = "android", not(target_arch = "wasm32")),
    all(target_os = "macos", not(target_arch = "wasm32"), not(feature = "terminal")),
    feature = "terminal",
))]
pub use idea_ui_docs::register_extensions;

#[cfg(feature = "sidecar")]
pub use idea_ui_docs::register_scene_extensions_recorder;
