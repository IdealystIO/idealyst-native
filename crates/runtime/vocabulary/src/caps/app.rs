//! App-environment and mount-lifecycle capabilities.

use std::rc::Rc;

use runtime_shared::primitives;
use runtime_shared::{Color, ColorScheme, PageMetadata, Platform, Tokenized};
use runtime_scene::Host;

/// Mount-time app environment: platform identity, appearance, the
/// self-contained closures `mount(...)` stashes in thread-locals
/// (`open_url` / `set_fullscreen`), and app-level chrome (page metadata,
/// host-surface background, scrollbar theme, app key handler).
///
/// Serves `mount(...)` plus the app-level drains in `walker/style.rs`
/// and `walker/view.rs` (`set_app_background`, `set_scrollbar_theme`,
/// `set_app_key_handler`) and `walker.rs` (`set_page_metadata`).
pub trait AppEnvOps: Host {
    /// The platform's current color scheme (default theme selection).
    fn color_scheme(&self) -> ColorScheme {
        ColorScheme::Auto
    }

    /// Host platform identity. `Custom("")` = no identity declared.
    fn platform(&self) -> Platform {
        Platform::Custom("")
    }

    /// Self-contained external-URL opener, or `None` when the backend
    /// has no external-open capability.
    fn url_opener(&self) -> Option<Rc<dyn Fn(&str)>> {
        None
    }

    /// Self-contained full-screen/immersive toggle, or `None` when the
    /// backend has no full-screen concept.
    fn fullscreen_setter(&self) -> Option<Rc<dyn Fn(bool)>> {
        None
    }

    /// Declare page-level metadata (title / description / OG) for the
    /// screen just built.
    #[allow(unused_variables)]
    fn set_page_metadata(&mut self, meta: &PageMetadata) {
        // default: no-op
    }

    /// Theme the host surface behind the rendered tree.
    #[allow(unused_variables)]
    fn set_app_background(&mut self, color: &Tokenized<Color>) {
        // default: no-op
    }

    /// Theme the platform scrollbar where the backend can.
    #[allow(unused_variables)]
    fn set_scrollbar_theme(&mut self, thumb: &Tokenized<Color>, track: &Tokenized<Color>) {
        // default: no-op
    }

    /// Install (or remove, with `None`) the app-level keyboard handler
    /// that fires regardless of focus.
    #[allow(unused_variables)]
    fn set_app_key_handler(&mut self, handler: Option<primitives::key::KeyDownHandler>) {
        // default: no-op
    }
}

/// Build/flush lifecycle: the end-of-build commit, layout scheduling,
/// and the hydration / lazy-chunk policy flags the structural walkers
/// consult.
///
/// Serves `walker.rs` (`finish`), `walker/navigator.rs` (layout
/// scheduling via `NavigatorControl`), `walker/dynamic.rs` +
/// `walker/when_switch.rs` (`is_hydrating` anchored-mode gate), and
/// `walker/lazy.rs` (`renders_lazy_chunks`).
pub trait LifecycleOps: Host {
    /// Commit the build: the framework hands over the root node once
    /// the tree is fully constructed.
    fn finish(&mut self, root: Self::Node);

    /// Drive a fresh layout pass synchronously (runtime-server shells).
    fn run_layout(&mut self) {}

    /// Schedule a coalesced layout pass for the next main-loop turn.
    /// No `self` — registered as `|| B::schedule_layout_pass()` by the
    /// navigator walker, so the hook holds no backend reference.
    fn schedule_layout_pass() {}

    /// Whether the backend is mid SSR-hydration adoption.
    fn is_hydrating(&self) -> bool {
        false
    }

    /// Whether `Element::Lazy` should resolve its chunk (`true`) or
    /// stop at the placeholder (SSR returns `false`).
    fn renders_lazy_chunks(&self) -> bool {
        true
    }
}
