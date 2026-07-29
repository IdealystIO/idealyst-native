//! The new-core (idea-lite) implementation: the SAME authored surface
//! as [`oldcore`](crate) — `SwapNavigator::new(&route).screen(…)
//! .layout(|nav| …).mount_policy(…).bind(nav)` — re-expressed over the
//! vocabulary's [`swap_navigator`](runtime_vocabulary::builders::
//! swap_navigator) builder. Construction lowers to the vocabulary
//! builder, the layout closure adapts through the [`SwapNav`] world
//! context the navigator mount provides, `.bind` rides `.on_handle`
//! (the unified [`NavHandle`]), and the outlet is the plain
//! `navigator_outlet()` element handed back as `nav.outlet`.
//!
//! What has NO new-core body (and why that's correct, not a stub):
//!
//! - **Handler registration** ([`register`] / [`register_generic`] /
//!   the per-backend inventory registrars): the vocabulary's
//!   `register_builtins` installs the swap handler on every new-core
//!   host — there is no per-backend navigator registry to feed. The
//!   fns stay as no-ops so app bootstrap code compiles unchanged.
//! - **`recording` (runtime-server)**: the wire-recorder sidecar is an
//!   old-core dev-mode surface; a sidecar build selects the old core.
//! - **URL sync**: the old handler opted in via
//!   `control.enable_url_sync()`; on the new core every navigator
//!   registers with the host-installed
//!   `handlers::nav_url_sync::UrlSyncService` automatically (backend-web
//!   installs it at boot) — no author or SDK call needed.

use std::any::Any;
use std::collections::HashMap;
use std::rc::Rc;

use runtime_vocabulary::builders::{self, navigator_outlet};
use runtime_vocabulary::glue::{inject, ChildList, Element, IntoElement, Ref, Signal};
use runtime_vocabulary::prims::{NavHandle, SwapNav};

pub use runtime_core::primitives::navigator::{Route, RouteParams};
pub use runtime_vocabulary::prims::{MountPolicy, Screen};

/// Presentation-label marker (parity name for [`oldcore`](crate)'s
/// typed payload; the new core carries no payload — the label string
/// below is what introspection serves as `NavSnapshot::type_name`).
pub struct SwapPresentation;

/// The wire-parity presentation label (`std::any::type_name` of the old
/// payload struct at its old path).
const SWAP_LABEL: &str = "swap_navigator::SwapPresentation";

/// Per-swap-screen options. Empty today (swap screens draw no navigator
/// chrome of their own), kept as a named type so per-screen metadata
/// can be added without an API break. Mirror of the old-core type.
#[derive(Default, Clone)]
pub struct SwapScreenOptions {}

impl SwapScreenOptions {
    /// Empty options (`Default`).
    pub fn new() -> Self {
        Self::default()
    }
}

/// The value a swap navigator hands its `.layout(|nav| …)` closure —
/// same field names and roles as the old-core `SwapContext`, with
/// new-core signal handles. Splat [`outlet`](Self::outlet) exactly once
/// where the active screen renders. Not `Clone` (the outlet is a
/// one-shot element).
pub struct SwapContext {
    /// Splat into the layout (`{nav.outlet}`) where the active screen
    /// mounts.
    pub outlet: Element,
    /// Currently active route key — highlight the live tab/nav item.
    pub active_route: Signal<&'static str>,
    /// Full resolved path of the active screen.
    pub active_path: Signal<String>,
    /// Switch to a sibling screen by route name (`Select`).
    pub on_select: Rc<dyn Fn(&'static str)>,
}

/// Typed runtime handle to a live swap navigator, filled into the
/// [`Ref`] passed to [`SwapBuilder::bind`]. Wraps the vocabulary's
/// unified [`NavHandle`]. Cheap to clone.
#[derive(Clone)]
pub struct SwapHandle {
    inner: NavHandle,
}

impl SwapHandle {
    /// Wrap the vocabulary handle (called by the `.bind` glue; authors
    /// get one from [`SwapBuilder::bind`]).
    pub fn from_inner(inner: NavHandle) -> Self {
        Self { inner }
    }

    /// Switch to `route`, building its URL from typed `params`.
    /// Selecting the already-active screen is a no-op at the driver.
    pub fn select<P: RouteParams + Clone>(&self, route: &Route<P>, params: P) {
        self.inner.select(route, params);
    }

    /// Borrow the underlying kind-agnostic [`NavHandle`].
    pub fn inner(&self) -> &NavHandle {
        &self.inner
    }
}

/// The swap-navigator builder. [`SwapNavigator::new`] starts one; the
/// fluent [`SwapBuilder`] methods register screens, set the author
/// layout, and bind the `Ref`. The result drops into a `ui!` tree
/// (it coerces to an `Element`).
pub struct SwapNavigator {
    b: builders::SwapNavigatorBuilder,
}

impl SwapNavigator {
    /// Start a swap navigator whose initial (selected) screen is
    /// `initial`.
    pub fn new(initial: &Route<()>) -> Self {
        Self {
            b: builders::swap_navigator(initial).nav_label(SWAP_LABEL),
        }
    }
}

/// Fluent builder methods for the swap navigator — the same trait shape
/// as the old core (a trait, so the surface stays swappable per core).
pub trait SwapBuilder: Sized {
    /// Register a screen: its route and the closure that builds the
    /// screen from typed params.
    fn screen<P, R, F>(self, route: Route<P>, render: F) -> Self
    where
        P: RouteParams + 'static,
        R: Into<Screen> + 'static,
        F: Fn(P) -> R + 'static;
    /// Set the author layout — the closure owns the chrome tree and
    /// splats `{nav.outlet}` where the active screen renders.
    fn layout<F>(self, f: F) -> Self
    where
        F: Fn(SwapContext) -> Element + 'static;
    /// Set the screen mount lifecycle — see [`MountPolicy`].
    fn mount_policy(self, policy: MountPolicy) -> Self;
    /// Bind a [`Ref<SwapHandle>`] so the app can switch screens
    /// imperatively.
    fn bind(self, r: Ref<SwapHandle>) -> Self;
}

impl SwapBuilder for SwapNavigator {
    fn screen<P, R, F>(mut self, route: Route<P>, render: F) -> Self
    where
        P: RouteParams + 'static,
        R: Into<Screen> + 'static,
        F: Fn(P) -> R + 'static,
    {
        self.b = self.b.screen(route, render);
        self
    }

    fn layout<F>(mut self, f: F) -> Self
    where
        F: Fn(SwapContext) -> Element + 'static,
    {
        // The vocabulary layout closure takes no argument; the mount
        // provides `SwapNav` for the build window — adapt it back into
        // the old context-parameter shape.
        self.b = self.b.layout(move || {
            let nav = inject::<SwapNav>()
                .expect("swap-navigator: SwapNav provided by the navigator mount");
            f(SwapContext {
                outlet: navigator_outlet().build(),
                active_route: nav.active_route,
                active_path: nav.active_path,
                on_select: nav.on_select,
            })
        });
        self
    }

    fn mount_policy(mut self, policy: MountPolicy) -> Self {
        self.b = self.b.mount_policy(policy);
        self
    }

    fn bind(mut self, r: Ref<SwapHandle>) -> Self {
        self.b = self
            .b
            .on_handle(move |handle| r.fill(SwapHandle::from_inner(handle)));
        self
    }
}

impl IntoElement for SwapNavigator {
    fn into_element(self) -> Element {
        self.b.build()
    }
}

impl ChildList for SwapNavigator {
    fn append_to(self, out: &mut Vec<Element>) {
        out.push(self.into_element());
    }
}

/// No-op on the new core: the vocabulary's `register_builtins` installs
/// the swap handler on every host — there is no per-backend navigator
/// registry. Kept so app bootstrap (`swap_navigator::register(&mut
/// backend)`) compiles unchanged.
pub fn register<B>(_backend: &mut B) {}

/// No-op on the new core — see [`register`]. The generic-registry
/// (SSR/test) hosts render navigators through `register_builtins` too.
pub fn register_generic<B>(_backend: &mut B) {}

/// Convenience re-exports — glob-import to bring the builder, handle,
/// screen options, and value types into scope. Superset of the old
/// prelude: also exports the shared data surface (`Route`, `Screen`,
/// `SwapContext`) so a same-source app imports everything from here.
pub mod prelude {
    pub use super::{
        register, MountPolicy, Route, Screen, SwapBuilder, SwapContext, SwapHandle,
        SwapNavigator, SwapPresentation, SwapScreenOptions,
    };
}
