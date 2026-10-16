//! App-environment and mount-lifecycle capabilities.

use std::rc::Rc;

use runtime_shared::primitives;
use runtime_shared::{Color, ColorScheme, PageMetadata, Platform, Tokenized};
use runtime_scene::Host;

/// Mount-time app environment: platform identity, appearance, the
/// self-contained closures `mount(...)` stashes in thread-locals
/// (`open_url` / `set_fullscreen`), and app-level chrome (page metadata,
/// host-surface background, scrollbar theme, app keyboard sink).
///
/// `set_app_background` / `set_scrollbar_theme` / `set_page_metadata` are
/// app-level drains of the theme and page-metadata channels;
/// `set_keyboard_sink` is driven by
/// `runtime_shared::key_input` through the host wired in
/// `install_env_services`.
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

    /// Install (or remove, with `None`) the app-level keyboard sink.
    ///
    /// While a sink is installed the backend delivers EVERY key down and
    /// key up the app receives — regardless of focus, including keys typed
    /// into a focused text input where the platform routes them app-wide
    /// (web + desktop; on iOS/Android a focused field owns its keys, and the
    /// backend reports `focus_lost` when it takes them) — as
    /// [`AppKeyEvent`]s through
    /// [`KeyboardSink::key`], honoring a returned `PreventDefault` where the
    /// platform allows; and calls [`KeyboardSink::focus_lost`] when the
    /// app/window stops receiving keys (the platform won't deliver releases
    /// for keys held at that moment). Fill `code` with the physical key in
    /// Web `KeyboardEvent.code` vocabulary and `repeat` where the platform
    /// reports it — the dispatcher normalizes repeats either way.
    ///
    /// Installed by `runtime_shared::key_input` only while at least one
    /// listener is live, so a backend may take focus or install monitors
    /// here without affecting apps that never listen.
    ///
    /// [`AppKeyEvent`]: primitives::key::AppKeyEvent
    /// [`KeyboardSink::key`]: primitives::key::KeyboardSink::key
    /// [`KeyboardSink::focus_lost`]: primitives::key::KeyboardSink::focus_lost
    #[allow(unused_variables)]
    fn set_keyboard_sink(&mut self, sink: Option<primitives::key::KeyboardSink>) {
        // default: no-op
    }

    /// Install (or remove, with `None`) the app-level pointer sink
    /// (`runtime_shared::pointer_input`).
    ///
    /// While installed, deliver every mouse / trackpad / pen button down
    /// and up, movement and wheel anywhere in the app (NOT finger touches)
    /// through [`PointerSink::event`], in logical pixels relative to the
    /// app root. While the pointer is locked, `dx`/`dy` carry the motion
    /// (raw / unaccelerated where the platform offers it) and the position
    /// stays put. Honor `PreventDefault`: suppress focus change / text
    /// selection on a down, the context menu on a secondary down
    /// (including the platform's separate context-menu event that follows
    /// it), page scroll on a wheel. Call
    /// [`PointerSink::lock_changed`] whenever the lock is granted or lost
    /// — including losses the app didn't request — and
    /// [`PointerSink::focus_lost`] when the window stops receiving input.
    ///
    /// Never hold a backend borrow while calling the sink.
    ///
    /// [`PointerSink::event`]: runtime_shared::pointer_input::PointerSink::event
    /// [`PointerSink::lock_changed`]: runtime_shared::pointer_input::PointerSink::lock_changed
    /// [`PointerSink::focus_lost`]: runtime_shared::pointer_input::PointerSink::focus_lost
    #[allow(unused_variables)]
    fn set_pointer_sink(&mut self, sink: Option<runtime_shared::pointer_input::PointerSink>) {
        // default: no-op
    }

    /// Lock (`true`) or release the pointer: hide the cursor, keep it from
    /// moving or leaving the window, and report only movement amounts.
    /// Report the result (possibly asynchronous, possibly refused) through
    /// the installed sink's `lock_changed`. Default: unsupported — never
    /// reports a lock.
    #[allow(unused_variables)]
    fn set_pointer_lock(&mut self, locked: bool) {
        // default: no-op
    }

    /// Hide (`true`) or show the cursor while it is over the app.
    #[allow(unused_variables)]
    fn set_cursor_hidden(&mut self, hidden: bool) {
        // default: no-op
    }
}

/// Build/flush lifecycle: the end-of-build commit, layout scheduling,
/// and the hydration / lazy-chunk policy flags the structural handlers
/// consult (`schedule_layout_pass` after navigation, `is_hydrating`,
/// `renders_lazy_chunks` in the lazy handler).
pub trait LifecycleOps: Host {
    /// Commit the build: the framework hands over the root node once
    /// the tree is fully constructed.
    fn finish(&mut self, root: Self::Node);

    /// Drive a fresh layout pass synchronously (runtime-server shells).
    fn run_layout(&mut self) {}

    /// Schedule a coalesced layout pass for the next main-loop turn.
    /// No `self` — the navigator handlers call `H::schedule_layout_pass()`
    /// from their driver effect, which holds no backend reference.
    fn schedule_layout_pass() {}

    /// Whether the backend is mid SSR-hydration adoption.
    fn is_hydrating(&self) -> bool {
        false
    }

    /// Whether a `lazy` boundary should resolve its chunk (`true`) or
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
