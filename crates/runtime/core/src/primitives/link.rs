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

use crate::primitives::navigator::{
    ambient_navigator, DefaultLinkKind, NavCommand, NavigatorControl, Route, RouteParams,
};
use crate::{Bound, Primitive, Ref, RefFill};
use std::any::Any;
use std::rc::Rc;

// ---------------------------------------------------------------------------
// NavKind — which nav command the link dispatches on activation
// ---------------------------------------------------------------------------

/// How activation maps to a `NavCommand`.
///
/// The constructor picks a default based on the ambient navigator:
/// `Push` inside a stack navigator, `Select` inside a tab or drawer
/// navigator. Authors can override per-link via `.kind(...)`.
///
/// `Pop` is intentionally not a link kind — a hyperlink that
/// navigates backward isn't really a hyperlink, it's a back
/// button. Use a regular `Button` + `nav.pop()` for that.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum NavKind {
    /// Push the route onto the stack. Equivalent to
    /// `NavigatorHandle::push`. Default inside a stack navigator.
    Push,
    /// Replace the top of the stack with the new route. Equivalent
    /// to `NavigatorHandle::replace`.
    Replace,
    /// Clear the stack and mount the new route as the root.
    /// Equivalent to `NavigatorHandle::reset`. Useful for
    /// post-login redirects.
    Reset,
    /// Switch the active screen to the route's id without changing
    /// stack depth. Default inside tabs and drawer navigators.
    Select,
}

impl Default for NavKind {
    fn default() -> Self {
        NavKind::Push
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
    /// (e.g. "Link to home").
    pub route: &'static str,
    /// Concrete URL produced by `params.to_path(route.path)` at
    /// link construction time. Useful on web for the `<a href>`
    /// attribute and right-click affordances; ignored on native.
    pub url: String,
    /// Fire when the user activates the link. The framework wraps
    /// push/replace/reset dispatch in here, so the backend doesn't
    /// need to know which one this link is — just "the user
    /// activated it."
    pub on_activate: Rc<dyn Fn()>,
}

// ---------------------------------------------------------------------------
// Constructor
// ---------------------------------------------------------------------------

/// Build a Link.
///
/// `params: P` must match the route's `P` — the type system
/// enforces this the same way `NavigatorHandle::push` does.
///
/// `P: Clone` is required because the link's underlying primitive
/// may be activated multiple times (every click reproduces a fresh
/// boxed param payload for the dispatcher). For most apps `P` is
/// either `()` or a small `#[derive(Clone)]` struct, so the bound
/// is trivially met.
pub fn link<P: RouteParams + Clone>(
    route: &Route<P>,
    params: P,
    children: Vec<Primitive>,
) -> Bound<LinkHandle> {
    // Pre-compute the URL once. Web uses it for the `<a href>` and
    // right-click "copy link"; native backends ignore it but the
    // cost is one stringify, so unconditional is fine.
    let url = params.to_path(route.path());
    let route_name: &'static str = route.name();

    // Capture the ambient navigator at construction time. A link
    // built outside any screen captures `None` and no-ops on
    // activation.
    let ambient: Option<Rc<NavigatorControl>> = ambient_navigator();

    // Pick the default activation shape from the ambient navigator's
    // hint. Stack navigators expose `Push`; tabs and drawer expose
    // `Select`. A link outside any nav keeps `Push` (it no-ops on
    // activate anyway). Authors can override via `.kind(...)`.
    let kind = match ambient.as_ref().map(|c| c.default_link_kind()) {
        Some(DefaultLinkKind::Select) => NavKind::Select,
        _ => NavKind::Push,
    };

    // Type-erased params source. Each activation needs a fresh
    // `Box<dyn Any>` because `NavCommand::Push`/etc. own their
    // params. `P: Clone` is what lets us reproduce on demand.
    let params_rc: Rc<P> = Rc::new(params);
    let make_params: Rc<dyn Fn() -> Box<dyn Any>> = {
        let params_rc = params_rc.clone();
        Rc::new(move || Box::new((*params_rc).clone()) as Box<dyn Any>)
    };

    Bound::new(Primitive::Link {
        children,
        route: route_name,
        url,
        make_params,
        kind,
        target: ambient,
        style: None,
        ref_fill: None,
        accessibility: crate::accessibility::AccessibilityProps::default(),
    })
}

// ---------------------------------------------------------------------------
// Bound<LinkHandle> — builder methods
// ---------------------------------------------------------------------------

impl Bound<LinkHandle> {
    /// Switch dispatch shape. Default is `NavKind::Push`.
    pub fn kind(mut self, k: NavKind) -> Self {
        if let Primitive::Link { kind, .. } = &mut self.primitive {
            *kind = k;
        }
        self
    }

    /// Bind to a `Ref<LinkHandle>` for imperative `activate()`.
    pub fn bind(mut self, r: Ref<LinkHandle>) -> Self {
        if let Primitive::Link { ref_fill, .. } = &mut self.primitive {
            *ref_fill = Some(RefFill::Link(Box::new(move |h| r.fill(h))));
        }
        self
    }
}

// ---------------------------------------------------------------------------
// Internals shared with the walker
// ---------------------------------------------------------------------------

/// Build the activation closure for a link primitive. The walker
/// hands this to the backend as `LinkConfig::on_activate`.
///
/// `target` is the ambient navigator the link captured at
/// construction. `None` ⇒ no-op (no nav was active when the link
/// was built; activation is silently dropped).
pub(crate) fn make_on_activate(
    target: Option<Rc<NavigatorControl>>,
    route: &'static str,
    url: String,
    kind: NavKind,
    make_params: Rc<dyn Fn() -> Box<dyn Any>>,
) -> Rc<dyn Fn()> {
    Rc::new(move || {
        let Some(control) = target.as_ref() else { return };
        let url = url.clone();
        let params = make_params();
        let cmd = match kind {
            NavKind::Push => NavCommand::Push { name: route, url, params },
            NavKind::Replace => NavCommand::Replace { name: route, url, params },
            NavKind::Reset => NavCommand::Reset { name: route, url, params },
            NavKind::Select => NavCommand::Select { name: route, url, params },
        };
        control.dispatch(cmd);
    })
}
