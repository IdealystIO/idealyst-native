//! Simulator — live preview demo.
//!
//! Hand-built (not via the `docs!` macro) so the page can drop the
//! `Simulator` component inline alongside the prose. The `docs!`
//! macro only emits text-flavored blocks (paragraphs, lists, code);
//! a live preview is a custom `Element`, so it goes here.
//!
//! What the preview actually does:
//!
//! - Mounts a tiny "Hello, from inside the simulator!" Idealyst app
//!   into a `Graphics` surface allocated by the framework's web
//!   backend (a `<canvas>` underneath).
//! - The `render_wgpu::Host` + `Renderer` driving the surface is
//!   exactly the same code path that the native `host-winit` shell
//!   uses on desktop. The skin is `IosSim` so the chrome looks
//!   like the iPhone preview window.
//!
//! Input events aren't plumbed yet (the embedded preview only
//! renders; pointer/keyboard events stop at the canvas). That's a
//! follow-up — the `EventSink` API on the host is ready for it,
//! the missing piece is translating browser `pointerdown` /
//! `keydown` events through to it.

use std::rc::Rc;

use runtime_core::{ui, Element};

use crate::components::simulator::{Simulator, SimulatorProps};
use crate::shell::{PageBody, PageHeader, PageTypographyProps, PageHeaderProps};

/// The "app" mounted INSIDE the simulator. Re-uses the docs site's
/// own `crate::app()` so the preview shows the same tree you're
/// looking at — drawer chrome, sidebar, the currently-routed page.
///
/// Notes / gotchas:
///
/// - `app()` is invoked here a SECOND time inside its own host. Each
///   call is independent reactive state — the outer's `Owner` tracks
///   the outer scopes, the inner's tracks the inner ones; signals,
///   refs, and theme installation are all per-tree.
/// - `install_idea_theme` is idempotent (first installer wins), so
///   the inner mount is a no-op for the theme.
/// - The wgpu render backend's navigator is still WIP — the initial
///   drawer screen mounts and the sidebar paints, but push/pop /
///   tab-select / drawer-select log "not yet wired". The preview
///   shows the static initial overview page; the live site
///   navigates normally.
/// - No infinite recursion: the inner app's Simulator route only
///   activates if you navigate to it inside the preview, and
///   navigation is one of the unwired paths above.
fn embedded_app() -> Element {
    crate::app()
}

pub fn page() -> Element {
    // `Rc<dyn Fn>` so the Simulator can clone it into the Graphics
    // primitive's `on_ready` closure. Invoked once when the
    // embedded host mounts.
    let build_ui: Rc<dyn Fn() -> Element> = Rc::new(embedded_app);

    ui! {
        PageBody {
            PageHeader(
                title = "Simulator".to_string(),
                description = "An embedded live preview that runs an Idealyst app through the \
                               wgpu render backend, hosted on a `Graphics` surface inside the \
                               docs page.".to_string(),
            )

            Simulator(
                build_ui = build_ui,
            )
        }
    }
}
