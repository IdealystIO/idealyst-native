//! Link primitive — declarative navigation.
//!
//! `Link(route, params) { children }` is the declarative counterpart
//! to `NavigatorHandle::push`. It wraps content in a tappable
//! container; activation dispatches a nav command (`Push` by
//! default; `.kind(NavKind::Replace | Reset)` switches semantics)
//! against the **ambient navigator** — the nearest enclosing
//! `Navigator` whose `mount_screen` is currently building this
//! screen subtree.
//!
//! # Why a primitive, not just `Button` + `nav.push`?
//!
//! - **Web semantics.** Backends are free to emit a real `<a href>`
//!   so the browser's link contract works without re-implementation:
//!   hover URL preview, right-click "copy link," middle-click and
//!   cmd-click for new tab/window, keyboard activation, screen-reader
//!   "link" role, search-engine crawlability.
//! - **Static introspection.** A primitive lets future tooling
//!   extract the declared link graph; imperative dispatch can't be
//!   inspected.
//! - **No prop drilling.** The ambient navigator wiring means
//!   authors don't have to thread a `Ref<NavigatorHandle>` through
//!   every component crossing a screen boundary.
//!
//! # Ambient navigator
//!
//! The framework's `Navigator` pushes its `Rc<NavigatorControl>`
//! onto a thread-local stack while running each `mount_screen`
//! call. `link(...)` reads the top of that stack at construction
//! time and captures it; on activation it dispatches through that
//! captured control plane.
//!
//! A link constructed outside any screen has no ambient navigator
//! and silently no-ops on activation (matches the
//! handle-before-build posture of the rest of the navigator
//! system). Nested navigators target correctly — each one pushes
//! its own control plane while building its screens, so a `Link`
//! inside a child navigator's screen drives the child by default.

use std::any::Any;
use std::rc::Rc;

// ---------------------------------------------------------------------------
// NavKind — which nav command the link dispatches on activation
// ---------------------------------------------------------------------------

/// How activation maps to a `NavCommand`.
///
/// `Default` defers to the SDK-installed link activator on the
/// ambient `NavigatorControl` — stack SDKs typically don't install
/// one and the activator falls through to `Push`; tab/drawer SDKs
/// install one that returns `Select`. Authors can override per-link
/// with an explicit kind.
///
/// `Pop` isn't a link kind — a hyperlink that navigates backward
/// isn't a hyperlink, it's a back button. Use a regular `Button` +
/// `nav.pop()` for that.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum NavKind {
    /// Use the SDK-installed link activator on the ambient navigator,
    /// or fall back to `Push` when none is installed.
    Default,
    Push,
    Replace,
    Reset,
    Select,
}

impl Default for NavKind {
    fn default() -> Self {
        NavKind::Default
    }
}

// ---------------------------------------------------------------------------
// LinkHandle — imperative API for refs
// ---------------------------------------------------------------------------

/// Handle exposed via `Ref<LinkHandle>`. Lets a parent fire a
/// link's nav command programmatically — useful for "press enter
/// on a focused row triggers its link" patterns where there's no
/// synthesizable click event.
#[derive(Clone)]
pub struct LinkHandle {
    node: Rc<dyn Any>,
    ops: &'static dyn LinkOps,
}

impl LinkHandle {
    pub fn new(node: Rc<dyn Any>, ops: &'static dyn LinkOps) -> Self {
        Self { node, ops }
    }

    /// Fire the link's nav command. Same effect as a user tap /
    /// click on the rendered widget.
    pub fn activate(&self) {
        self.ops.activate(&*self.node);
    }
}

pub trait LinkOps {
    fn activate(&self, node: &dyn Any);
}

// ---------------------------------------------------------------------------
// LinkConfig — what `Backend::create_link` receives
// ---------------------------------------------------------------------------

/// Bundle the framework hands to `Backend::create_link`. The
/// backend wires the platform-native interaction widget (a real
/// `<a href>` on web, an accessibility-Link-roled tappable
/// container on native) and calls `on_activate` when the user
/// activates it.
pub struct LinkConfig {
    /// Route name (matches `Route::name()`). Stable; passed through
    /// to backends that want to expose it in accessibility metadata
    /// (e.g. "Link to home"). Empty (`""`) for external links — they
    /// have no in-app route; use [`url`](Self::url) for the label.
    pub route: &'static str,
    /// Concrete URL. For in-app links: `params.to_path(route.path)`,
    /// used on web for the `<a href>` and right-click affordances,
    /// ignored on native. For external links: the off-app destination
    /// (`https://…`, `mailto:`, `tel:`) the backend opens directly.
    pub url: String,
    /// `true` ⇒ this link points *outside* the app. Backends route it
    /// to the platform's external handler rather than the in-app
    /// navigator: web emits `<a target="_blank" rel="noopener">` and
    /// lets the browser navigate (no SPA `preventDefault`); native
    /// fires `on_activate`, which calls
    /// [`open_url`](crate::open_url). `false` ⇒ in-app navigation
    /// (the historical behavior).
    pub external: bool,
    /// Fire when the user activates the link. For in-app links the
    /// framework wraps push/replace/reset dispatch in here; for
    /// external links it wraps [`open_url`](crate::open_url). Either
    /// way the backend just fires it on activation. (Web skips this
    /// for external links — the native `<a target="_blank">` already
    /// navigates.)
    pub on_activate: Rc<dyn Fn()>,
}

// ---------------------------------------------------------------------------
// Constructor
// ---------------------------------------------------------------------------



// ---------------------------------------------------------------------------
// Bound<LinkHandle> — builder methods
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Internals shared with the walker
// ---------------------------------------------------------------------------


