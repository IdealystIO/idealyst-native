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

    /// Stamp a navigator outlet with the structural hydration marker
    /// (`runtime_shared::primitives::navigator::NAV_OUTLET_HYDRATION_ATTR`
    /// = the navigator's base path). SSR implements this so the emitted
    /// document lets a hydrating client locate the outlet — and thereby
    /// the server-rendered position of the initial screen. Default no-op:
    /// live backends have nothing to annotate.
    ///
    /// Called by the navigator handlers right after the author-layout
    /// build captures the outlet node.
    fn annotate_nav_outlet(&mut self, _outlet: &Self::Node, _base: &str) {}

    /// HYDRATION cursor steering, part 1 of 2. The navigator handlers
    /// realize the initial screen BEFORE the author layout builds the
    /// outlet (the mount-order contract: screens must resolve the launch
    /// URL before chrome), but the server document nests the screen
    /// INSIDE the outlet. Adoption walks in `create_*` order, so without
    /// intervention the screen build would consume the outlet's node and
    /// shift the whole subtree one level (the `[hydrate] diverge` remount
    /// cascade). A hydrating backend implements this to (a) save the
    /// adoption cursor, (b) move it to the server-rendered screen
    /// position — the first element child of the outlet matching
    /// `[NAV_OUTLET_HYDRATION_ATTR = base]` under `root` — and (c) mark
    /// that outlet so the later layout-build adoption skips its
    /// already-consumed subtree. Default no-op (non-hydrating backends,
    /// and any backend outside its adoption pass).
    ///
    /// Must be paired with [`Self::hydrate_nav_screen_end`]; calls nest
    /// (a navigator mounted inside a screen brackets its own pair).
    fn hydrate_nav_screen_begin(&mut self, _root: &Self::Node, _base: &str) {}

    /// HYDRATION cursor steering, part 2 of 2: restore the adoption
    /// cursor saved by the matching [`Self::hydrate_nav_screen_begin`],
    /// so the author-layout build that follows adopts from where the
    /// navigator's first layout node actually sits. Default no-op.
    fn hydrate_nav_screen_end(&mut self) {}
}
