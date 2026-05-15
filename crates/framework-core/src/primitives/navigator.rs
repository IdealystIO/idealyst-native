//! Navigator + Screen primitives.
//!
//! A `Navigator` is the framework's stack-based navigation container.
//! Authors declare a set of `Screen` routes up-front and an initial
//! route; an imperative `NavigatorHandle` (obtained via `.bind(ref)`)
//! drives push / pop / replace / reset at runtime.
//!
//! # Per-platform semantics
//!
//! - **iOS**: the navigator is a `UINavigationController`. Each pushed
//!   screen is a child `UIViewController` whose `view` is the screen
//!   subtree's root. Back-swipe + nav bar come for free.
//! - **Android**: the navigator is a `FrameLayout` driven by a
//!   `FragmentManager`. Each push commits a new `Fragment` whose view
//!   is the screen subtree's root and adds it to the back stack so the
//!   system back button pops correctly.
//! - **Web** (no-op): the navigator is a plain container that holds the
//!   active screen inline. push/pop swap the subtree atomically. URL
//!   pathing comes later.
//!
//! # Lifecycles
//!
//! Each *mounted* screen runs inside its own reactive `Scope`. Popping
//! drops that scope, freeing every signal/effect/ref scoped to the
//! screen. The pattern mirrors `Virtualizer`'s per-item scopes: backends
//! get `mount_screen(idx, params)` + `release_screen(scope_id)`
//! callbacks; the framework owns the scope registry.
//!
//! # Route params
//!
//! Routes are typed via the generic param `P`:
//!
//! ```ignore
//! let home = Route::<()>::new("home");
//! let detail = Route::<DetailParams>::new("detail");
//! ```
//!
//! `nav.push(&detail, DetailParams { id: 42 })` is a compile-time check
//! that the params match the route. Inside the framework the params get
//! boxed into `Box<dyn Any>` so the navigator's screen table stays
//! non-generic; each registered screen builder downcasts back to its
//! declared param type before calling the user's render closure. A
//! mismatch (e.g. user constructs a route at runtime with the wrong
//! param) panics in the renderer with a clear message.

use crate::{Bound, Primitive, Ref, RefFill};
use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::rc::Rc;

// ---------------------------------------------------------------------------
// RouteParams — URL <-> typed params conversion
// ---------------------------------------------------------------------------

/// Convert route params to/from URL path segments. Implemented on
/// every type used as a `Route<P>` payload. Built-in impl for `()` (the
/// no-params case). For custom types, authors implement this trait
/// directly — usually a few-line affair.
///
/// # Why this trait exists
///
/// The web backend (and any future SSR backend) needs to reconcile a
/// URL like `/detail/42` with the typed `DetailParams { id: 42 }` the
/// rest of the framework speaks. The trait moves that conversion into
/// the params type itself, keeping path-pattern handling (which is
/// pure-Rust string matching) reusable across backends and SSR.
///
/// Native backends (iOS, Android) don't touch URLs at all — the param
/// payload flows as `Box<dyn Any>` directly to the receiving screen.
/// The trait is still required because the framework needs *any* path
/// rendering to work uniformly when the user wires up a web view
/// alongside native (a future use case).
pub trait RouteParams: 'static + Sized {
    /// Render `self` into URL path segments for a route whose pattern
    /// is `pattern` (e.g. `/detail/:id`). Returns the concrete URL
    /// path (e.g. `/detail/42`).
    fn to_path(&self, pattern: &str) -> String {
        // Default impl: only works for `()`. Types with actual params
        // must override.
        let _ = self;
        // For the unit type and similar no-segment cases, return the
        // pattern as-is (no `:placeholder` segments to substitute).
        if pattern.contains(':') {
            panic!(
                "RouteParams::to_path default impl can't fill placeholder \
                 segments in pattern '{}'. Implement RouteParams for your \
                 params type to serialize each `:segment`.",
                pattern
            );
        }
        pattern.to_string()
    }

    /// Parse `self` from a `:placeholder` -> value map. The map is
    /// populated by the framework after matching the URL against the
    /// route's pattern. Returns `None` on parse failure (a path that
    /// matched the pattern but had unparseable values for the
    /// declared `P`).
    fn from_segments(_segments: &HashMap<String, String>) -> Option<Self> {
        // Default impl is for `()` — only matches if there are no
        // segments (the pattern was a literal path). Custom impls
        // override to parse their own fields.
        None
    }
}

impl RouteParams for () {
    fn to_path(&self, pattern: &str) -> String {
        pattern.to_string()
    }

    fn from_segments(_segments: &HashMap<String, String>) -> Option<Self> {
        Some(())
    }
}

// ---------------------------------------------------------------------------
// Route<P> — typed route name + phantom param type
// ---------------------------------------------------------------------------

/// A navigation route. The `name` is the in-stack key (used by native
/// backends + framework); the `path` is the URL pattern used by web
/// (and any future SSR / pathing backend). The phantom `P` is what
/// `Navigator::push` / `.screen` type-check against so the params
/// can't drift from the route.
///
/// # Path pattern syntax
///
/// Patterns are slash-delimited segments. A segment of the form
/// `:name` is a parameter placeholder filled at push time. Everything
/// else is matched literally.
///
///   `/`              — root
///   `/settings`      — literal
///   `/detail/:id`    — single param
///   `/u/:user/p/:post` — two params
///
/// Native backends (iOS, Android) ignore `path`. The framework's
/// renderer never inspects it either — it's data the web backend (and
/// future SSR backend) reads.
#[derive(Clone)]
pub struct Route<P: RouteParams = ()> {
    name: &'static str,
    path: &'static str,
    _params: PhantomData<P>,
}

impl<P: RouteParams> Route<P> {
    /// Declare a route. `name` must be unique across the navigator's
    /// screen table; `path` is the URL pattern (see [`Route`] doc).
    /// A param-less route uses `Route::<()>::new("home", "/")`; a
    /// route with a param payload uses
    /// `Route::<MyParams>::new("detail", "/detail/:id")`.
    pub const fn new(name: &'static str, path: &'static str) -> Self {
        Self { name, path, _params: PhantomData }
    }

    /// The route's stable name. Used as the navigator's screen table
    /// key, and (on native) passed as-is through commands.
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// The route's URL path pattern. Used by web for matching
    /// `window.location` and constructing `history.pushState` URLs.
    /// Native backends ignore this.
    pub fn path(&self) -> &'static str {
        self.path
    }
}

// ---------------------------------------------------------------------------
// ScreenBuilder + RouteEntry — type-erased renderer + path-match data
// ---------------------------------------------------------------------------

/// Per-route builder closure. Takes the boxed params and returns the
/// screen's `Primitive` subtree.
///
/// Param downcasting happens here. If the framework dispatches the
/// wrong concrete type for a route (only possible if user code
/// fabricates a `Route<X>` at runtime with the wrong `P`), the
/// downcast panics with a clear message — same posture as any other
/// type-erased registry in the framework.
pub(crate) type ScreenBuilder = Rc<dyn Fn(Box<dyn Any>) -> Primitive>;

/// Closure that parses a `:placeholder` segment map into the
/// route's typed param payload, then boxes it as `dyn Any`. Used by
/// path-matching backends (web, future SSR) to go from a matched URL
/// to the params the receiving screen expects.
pub(crate) type ParamsFromSegments = Rc<dyn Fn(&HashMap<String, String>) -> Option<Box<dyn Any>>>;

/// Per-route bookkeeping. Carries everything path-matching backends
/// need: the pattern, the typed builder, and the segment-parser. The
/// framework's screen table maps route names to these entries.
pub(crate) struct RouteEntry {
    pub(crate) path: &'static str,
    pub(crate) build: ScreenBuilder,
    pub(crate) from_segments: ParamsFromSegments,
}

// ---------------------------------------------------------------------------
// Path matching — pure-Rust, used by web + future SSR backends
// ---------------------------------------------------------------------------

/// Match `path` against `pattern`. Returns `Some(map)` if the segment
/// counts agree and every literal segment matches (case-sensitively);
/// `:placeholder` segments end up as entries in the returned map.
/// Returns `None` on a mismatch.
///
/// Trailing slashes are tolerated on both sides (treated as
/// equivalent). Empty path is treated as `/`.
///
/// Pure function — no DOM access, no JS APIs. Ports unchanged to a
/// future SSR backend.
pub fn match_pattern(path: &str, pattern: &str) -> Option<HashMap<String, String>> {
    let path_segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let pat_segs: Vec<&str> = pattern.split('/').filter(|s| !s.is_empty()).collect();
    if path_segs.len() != pat_segs.len() {
        return None;
    }
    let mut out = HashMap::new();
    for (p, pat) in path_segs.iter().zip(pat_segs.iter()) {
        if let Some(name) = pat.strip_prefix(':') {
            out.insert(name.to_string(), (*p).to_string());
        } else if *p != *pat {
            return None;
        }
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// NavigatorHandle — the imperative API exposed via .bind(...)
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct NavigatorHandle {
    node: Rc<dyn Any>,
    ops: &'static dyn NavigatorOps,
    /// Shared with the running navigator's state. Cloning the handle
    /// re-uses the same control plane — multiple owners drive the same
    /// stack. None when the handle is a no-op (the trait default).
    control: Option<Rc<NavigatorControl>>,
}

impl NavigatorHandle {
    pub fn new(node: Rc<dyn Any>, ops: &'static dyn NavigatorOps) -> Self {
        Self { node, ops, control: None }
    }

    /// Construct a handle wired to a control plane. Used by backends
    /// that actually drive the navigator (web, iOS, Android impls).
    pub fn with_control(
        node: Rc<dyn Any>,
        ops: &'static dyn NavigatorOps,
        control: Rc<NavigatorControl>,
    ) -> Self {
        Self { node, ops, control: Some(control) }
    }

    /// Push a new screen onto the stack. `route` is what was declared
    /// in `.screen(...)`; `params` must match the route's `P`. If the
    /// route is not registered, panics — declaring routes up-front is
    /// part of the contract.
    ///
    /// On web, this also calls `history.pushState` with the URL
    /// produced by `params.to_path(route.path())`. On native backends,
    /// the URL is computed but unused.
    pub fn push<P: RouteParams>(&self, route: &Route<P>, params: P) {
        if let Some(c) = &self.control {
            let url = params.to_path(route.path);
            c.dispatch(NavCommand::Push {
                name: route.name,
                url,
                params: Box::new(params),
            });
            self.ops.notify_pushed(&*self.node, route.name);
        }
    }

    /// Pop the top screen. No-op when the stack has only the root
    /// screen (matches platform behavior — iOS won't pop the root VC,
    /// Android's FragmentManager won't pop an empty back stack).
    ///
    /// On web, this calls `history.back()`, which fires `popstate`;
    /// the web backend's popstate handler then performs the stack
    /// pop. Native backends pop via their native API immediately.
    pub fn pop(&self) {
        if let Some(c) = &self.control {
            c.dispatch(NavCommand::Pop);
            self.ops.notify_popped(&*self.node);
        }
    }

    /// Replace the top screen without changing stack depth. Equivalent
    /// to pop + push for state but skips the platform's push/pop
    /// animation. On web, uses `history.replaceState`.
    pub fn replace<P: RouteParams>(&self, route: &Route<P>, params: P) {
        if let Some(c) = &self.control {
            let url = params.to_path(route.path);
            c.dispatch(NavCommand::Replace {
                name: route.name,
                url,
                params: Box::new(params),
            });
            self.ops.notify_replaced(&*self.node, route.name);
        }
    }

    /// Clear the entire stack and mount `route` as the new root.
    /// Useful for post-login redirects. On web, this is a single
    /// `history.replaceState` (we don't `pushState` because there's
    /// nothing above to navigate back to).
    pub fn reset<P: RouteParams>(&self, route: &Route<P>, params: P) {
        if let Some(c) = &self.control {
            let url = params.to_path(route.path);
            c.dispatch(NavCommand::Reset {
                name: route.name,
                url,
                params: Box::new(params),
            });
            self.ops.notify_reset(&*self.node, route.name);
        }
    }

    /// Current stack depth (1 = only root). Cheap.
    pub fn depth(&self) -> usize {
        self.control.as_ref().map(|c| c.depth()).unwrap_or(0)
    }
}

/// Optional per-backend operations on a navigator node. Backends that
/// only need the command stream (web, iOS, Android) leave most of
/// these as no-ops — the real work happens inside the control plane's
/// dispatch closure, which the backend installs when building the
/// navigator. The notifiers are here for backends that want to know
/// the operation happened without re-implementing the command queue.
pub trait NavigatorOps {
    fn notify_pushed(&self, _node: &dyn Any, _route: &str) {}
    fn notify_popped(&self, _node: &dyn Any) {}
    fn notify_replaced(&self, _node: &dyn Any, _route: &str) {}
    fn notify_reset(&self, _node: &dyn Any, _route: &str) {}
}

// ---------------------------------------------------------------------------
// Control plane — shared state between handle + framework + backend
// ---------------------------------------------------------------------------

/// The bridge between the user-facing handle and the framework's
/// per-navigator state. Carries:
/// - the command dispatcher the handle uses (set by the framework
///   during `build_navigator`),
/// - a read-only depth probe so handles can answer `.depth()` without
///   reaching into the backend.
///
/// Wrapped in `Rc` so handle clones share one control plane.
pub struct NavigatorControl {
    dispatch: RefCell<Option<Box<dyn Fn(NavCommand)>>>,
    depth: RefCell<usize>,
}

impl NavigatorControl {
    pub fn new() -> Self {
        Self {
            dispatch: RefCell::new(None),
            depth: RefCell::new(1),
        }
    }

    /// Install the dispatcher. Called once from `build_navigator` after
    /// the backend's command-execution closure is wired up.
    pub fn install(&self, dispatch: Box<dyn Fn(NavCommand)>) {
        *self.dispatch.borrow_mut() = Some(dispatch);
    }

    /// Update the cached depth. Called by the framework when commands
    /// commit so `handle.depth()` stays in sync.
    pub fn set_depth(&self, d: usize) {
        *self.depth.borrow_mut() = d;
    }

    pub fn depth(&self) -> usize {
        *self.depth.borrow()
    }

    fn dispatch(&self, cmd: NavCommand) {
        // If the handle is somehow called before the navigator has
        // been built, just drop the command — same posture as a `Ref`
        // that's been used before the matching primitive has mounted.
        if let Some(f) = self.dispatch.borrow().as_ref() {
            f(cmd);
        }
    }
}

impl Default for NavigatorControl {
    fn default() -> Self {
        Self::new()
    }
}

/// Commands that flow from the handle into the framework's dispatcher.
/// Boxed params survive the channel hop and are downcast at the
/// builder boundary. `url` is the concrete URL string produced by
/// `RouteParams::to_path` — web pushes it; native backends ignore it.
pub enum NavCommand {
    Push {
        name: &'static str,
        url: String,
        params: Box<dyn Any>,
    },
    Pop,
    Replace {
        name: &'static str,
        url: String,
        params: Box<dyn Any>,
    },
    Reset {
        name: &'static str,
        url: String,
        params: Box<dyn Any>,
    },
}

// ---------------------------------------------------------------------------
// NavigatorCallbacks — what the framework hands to the backend
// ---------------------------------------------------------------------------

/// Bundle the framework hands to `Backend::create_navigator`. Same
/// shape philosophy as `VirtualizerCallbacks`: typed where possible
/// (`mount_screen` returns the backend's actual `N`), `Rc`'d so
/// per-event handlers can clone freely.
pub struct NavigatorCallbacks<N: Clone + 'static> {
    /// The declared initial route (name + path). Backends that don't
    /// do path-matching (iOS, Android) mount this directly. The web
    /// backend (and any SSR backend) may instead use [`match_path`] to
    /// resolve the current URL against the registered routes; if the
    /// URL matches a non-initial route, the web backend mounts that
    /// route as the root and the initial route is bypassed.
    pub initial_route: &'static str,
    /// Initial route's path pattern (always param-less, see
    /// [`Navigator::new`]). Web uses this to determine whether the
    /// current URL "is" the initial route.
    pub initial_path: &'static str,
    /// Mount a screen for `name` with `params`. The framework builds
    /// the subtree inside a fresh per-screen `Scope` and returns the
    /// resulting native node plus the scope id so the backend can
    /// later release the same screen by id.
    pub mount_screen: Rc<dyn Fn(&'static str, Box<dyn Any>) -> (N, u64)>,
    /// Release a previously-mounted screen by scope id. Drops the
    /// screen's `Scope`, freeing every signal/effect/ref inside. The
    /// backend should *not* use the node after this and should also
    /// detach it from its parent.
    pub release_screen: Rc<dyn Fn(u64)>,
    /// Match a URL path against the registered routes. Returns
    /// `Some((name, boxed_params))` if `path` matches one of the
    /// declared routes' patterns AND the matched segments can be
    /// parsed into the route's typed params; `None` otherwise.
    ///
    /// Used by the web backend on mount (deep linking) and on
    /// `popstate` (forward/back button); future SSR backends use it
    /// to map an HTTP request path to a screen subtree without ever
    /// running a JS dispatcher.
    pub match_path: Rc<dyn Fn(&str) -> Option<(&'static str, Box<dyn Any>)>>,
    /// Subscribe to commands from the handle. The backend installs a
    /// dispatcher here (see `Backend::create_navigator` doc); when the
    /// user's code calls `handle.push(...)`, that dispatcher fires.
    /// The backend's dispatcher must:
    ///   1. Call `mount_screen(name, params)` to get the new node +
    ///      scope id (for push / replace / reset).
    ///   2. Insert the node into its native container (push child VC,
    ///      replace fragment, etc.).
    ///   3. Call `release_screen` for any screen it pops.
    ///   4. Notify `depth_changed(new_depth)` so the handle's depth
    ///      probe stays in sync.
    pub depth_changed: Rc<dyn Fn(usize)>,
}

// ---------------------------------------------------------------------------
// Navigator builder — author-facing
// ---------------------------------------------------------------------------

/// Author-facing navigator builder. Routes get declared via `.screen(...)`;
/// the framework wires the rest. See module-level docs for usage.
pub struct Navigator {
    pub(crate) initial: &'static str,
    pub(crate) initial_path: &'static str,
    pub(crate) screens: HashMap<&'static str, RouteEntry>,
    pub(crate) style: Option<crate::StyleSource>,
    pub(crate) ref_fill: Option<RefFill>,
}

impl Navigator {
    /// Construct a navigator with `initial` as the root screen. The
    /// route must be registered via `.screen(...)` before the
    /// navigator mounts; an unregistered initial route panics.
    ///
    /// The initial route's params are always `()` — the root screen
    /// is unparameterized by construction. Apps that need a
    /// parameterized "deep-link" root should rely on web's
    /// path-matching path: declare the screen normally, and the web
    /// backend will mount it as the root when the URL matches.
    pub fn new(initial: &Route<()>) -> Bound<NavigatorHandle> {
        Bound::new(Primitive::Navigator(Box::new(Navigator {
            initial: initial.name,
            initial_path: initial.path,
            screens: HashMap::new(),
            style: None,
            ref_fill: None,
        })))
    }
}

/// Builder methods. Wrapping in `Bound<NavigatorHandle>` keeps the
/// `.bind(ref)` type-check working — same pattern as every other
/// primitive's builder.
impl Bound<NavigatorHandle> {
    /// Register a screen. `route` is the typed key; `render` is the
    /// per-route subtree builder, which receives the route's typed
    /// params. The `'static` bound on `P` is required to box the
    /// params across the framework's type-erased boundary; the
    /// `RouteParams` bound is what lets web/SSR backends map URLs to
    /// typed payloads.
    pub fn screen<P: RouteParams>(
        mut self,
        route: Route<P>,
        render: impl Fn(P) -> Primitive + 'static,
    ) -> Self {
        if let Primitive::Navigator(nav) = &mut self.primitive {
            let render = Rc::new(render);
            let build: ScreenBuilder = Rc::new(move |boxed: Box<dyn Any>| {
                let params: Box<P> = boxed.downcast().unwrap_or_else(|_| {
                    panic!(
                        "Navigator: screen param type mismatch for route — \
                         declared params don't match dispatched params"
                    )
                });
                render(*params)
            });
            let from_segments: ParamsFromSegments = Rc::new(|segs| {
                P::from_segments(segs).map(|p| Box::new(p) as Box<dyn Any>)
            });
            nav.screens.insert(
                route.name,
                RouteEntry { path: route.path, build, from_segments },
            );
        }
        self
    }

    /// Bind a `Ref<NavigatorHandle>` so the handle is filled at mount
    /// time. Matches the standard primitive bind shape.
    pub fn bind(mut self, r: Ref<NavigatorHandle>) -> Self {
        if let Primitive::Navigator(nav) = &mut self.primitive {
            nav.ref_fill = Some(RefFill::Navigator(Box::new(move |h| r.fill(h))));
        }
        self
    }
}
