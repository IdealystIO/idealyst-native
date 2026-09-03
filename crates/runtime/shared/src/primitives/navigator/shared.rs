//! Shared navigator substrate — the kind-agnostic core every navigator
//! SDK builds on.
//!
//! The framework owns the substrate (routing, screen scopes, ambient
//! capture, command queue, reactive nav state, per-screen state stack).
//! SDK crates own everything kind-specific (chrome, animations, gestures,
//! typed handles, typed screen options). No kind names appear in this
//! module — `Stack` / `Tab` / `Drawer` are SDK concepts, not framework
//! concepts.
//!
//! What lives here:
//!
//! - `Route<P>` + `RouteParams` — typed route declaration + URL ⇄ params.
//! - `ScreenBuilder` / `RouteEntry` / `ParamsFromSegments` — type-erased
//!   per-route registry the framework walks.
//! - `Screen` + `MountResult` — what a screen builder returns, what
//!   `mount_screen` hands back. SDK-defined options ride as
//!   `Box<dyn Any>`.
//! - `NavCommand` — the command channel. Built-in verbs cover the
//!   common shapes (Push / Pop / Replace / Reset / Select); SDKs add
//!   their own via `NavCommand::Custom(Rc<dyn Any>)`.
//! - `NavigatorControl` — the dispatcher + reactive nav-state bridge.
//! - `NavigatorHandle` — the framework-side handle. Just dispatch +
//!   control accessor; SDK typed handles wrap it.
//! - `NavigatorOps` — the trait the handle's `&dyn NavigatorOps`
//!   points to (currently empty; reserved for backend extension hooks).
//! - `NavState` — reactive `active_route` / `active_path` / `depth` /
//!   `can_go_back` signals layout/chrome subscribes to.
//! - `AmbientNavGuard` / `ambient_navigator()` — thread-local stack
//!   `Link` reads at build time.
//! - `ScreenStateGuard` / `screen_state` / `screen_query` — the
//!   per-screen navigation state (query params) the screen render
//!   closure reads to seed its signals. See the `query` module for the
//!   `ScreenState` trait and why state is encoded as query params.
//! - `NavigatorConfig` — the framework-owned routing config (initial
//!   route, screen registry, defer flag). Kind-specific config lives
//!   on the SDK's presentation payload.
//! - `match_pattern` — pure-Rust URL-against-pattern matcher.

use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::rc::Rc;

use super::query::{QueryParams, ScreenState};

// ---------------------------------------------------------------------------
// Ambient navigator stack — Link primitives find their navigator here
// ---------------------------------------------------------------------------

thread_local! {
    static AMBIENT_NAV: RefCell<Vec<Rc<NavigatorControl>>> =
        const { RefCell::new(Vec::new()) };
}

/// RAII guard that pushes a navigator's control plane onto the ambient
/// stack while a screen is building. The `Link` primitive captures the
/// top of the stack at construction time.
pub struct AmbientNavGuard;

impl AmbientNavGuard {
    pub fn push(control: Rc<NavigatorControl>) -> Self {
        AMBIENT_NAV.with(|s| s.borrow_mut().push(control));
        AmbientNavGuard
    }
}

impl Drop for AmbientNavGuard {
    fn drop(&mut self) {
        AMBIENT_NAV.with(|s| {
            let _ = s.borrow_mut().pop();
        });
    }
}

/// Read the top of the ambient-navigator stack. `None` when called
/// outside any navigator's `mount_screen`.
pub fn ambient_navigator() -> Option<Rc<NavigatorControl>> {
    AMBIENT_NAV.with(|s| s.borrow().last().cloned())
}

// ---------------------------------------------------------------------------
// Global navigator registry — robot introspection
// ---------------------------------------------------------------------------
//
// Unlike `AMBIENT_NAV` (transient — only the navigator currently building a
// screen), this is a persistent slab of every MOUNTED navigator, so the robot
// bridge can enumerate "all loaded navigators" and report which is current.
// Entries hold a `Weak<NavigatorControl>` (the control is owned by the backend
// instance + SDK handler; the registry must not keep it alive) and are removed
// when the navigator's build scope drops (`deregister_navigator`, wired as an
// `on_cleanup` in the walker — same lifetime as the navigator's robot element
// entry). Robot-feature-gated: dead code in production builds.

/// Stable id for a navigator in the global registry. Cheap to copy.
/// `pub` regardless of feature so `NavigatorControl`'s `nav_id` field type
/// is always nameable, but only populated/used under `robot`.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct NavId(pub u32);

#[cfg(feature = "robot")]
#[derive(Default)]
struct NavRegistry {
    /// Slab; index = `NavId`. `None` = freed slot (LIFO recycled via `free`).
    entries: Vec<Option<NavRegEntry>>,
    free: Vec<u32>,
    /// Most-recently-dispatched-against navigator — the inspector's "current".
    active: Option<NavId>,
}

#[cfg(feature = "robot")]
struct NavRegEntry {
    control: std::rc::Weak<NavigatorControl>,
    /// SDK presentation type name (e.g. `stack_navigator::Presentation`) —
    /// the honest, kind-agnostic kind carrier (the framework can't know
    /// Stack vs Tab vs Drawer; the dashboard classifies from this string).
    type_name: &'static str,
    /// Raw robot `ElementId` of this navigator's `Element::Navigator` entry,
    /// so the inspector can `get_children` to read the current screen's
    /// elements. `None` if the robot element wasn't registered.
    element_id: Option<u32>,
}

#[cfg(feature = "robot")]
thread_local! {
    static NAV_REGISTRY: RefCell<NavRegistry> = RefCell::new(NavRegistry::default());
}

/// A read-only snapshot of one registered navigator, returned by
/// [`all_navigators`] / [`navigator_snapshot`]. All reactive reads are
/// untracked (querying never subscribes a scope).
#[cfg(feature = "robot")]
#[derive(Clone, Debug)]
pub struct NavSnapshot {
    pub nav_id: u32,
    pub element_id: Option<u32>,
    pub type_name: &'static str,
    pub active_route: String,
    pub active_path: String,
    pub depth: usize,
    pub can_go_back: bool,
    /// `true` for the most-recently-dispatched-against navigator.
    pub is_current: bool,
    pub base: String,
    /// Back-stack `(route, path)` pairs, root-first, current last.
    pub stack: Vec<(String, String)>,
}

/// Register a freshly-built navigator. Returns its stable [`NavId`]; the
/// walker stores it on the control via [`NavigatorControl::set_nav_id`] and
/// arranges [`deregister_navigator`] on scope drop. If no navigator is yet
/// marked current, this one becomes current (cold-start root).
#[cfg(feature = "robot")]
pub fn register_navigator(
    control: &Rc<NavigatorControl>,
    type_name: &'static str,
    element_id: Option<u32>,
) -> NavId {
    NAV_REGISTRY.with(|r| {
        let mut reg = r.borrow_mut();
        let entry = NavRegEntry {
            control: Rc::downgrade(control),
            type_name,
            element_id,
        };
        let id = if let Some(idx) = reg.free.pop() {
            reg.entries[idx as usize] = Some(entry);
            NavId(idx)
        } else {
            let idx = reg.entries.len() as u32;
            reg.entries.push(Some(entry));
            NavId(idx)
        };
        if reg.active.is_none() {
            reg.active = Some(id);
        }
        id
    })
}

/// Remove a navigator from the registry (its build scope dropped). Clears
/// `active` if it pointed here. No-op for an unknown id.
#[cfg(feature = "robot")]
pub fn deregister_navigator(id: NavId) {
    NAV_REGISTRY.with(|r| {
        let mut reg = r.borrow_mut();
        if let Some(slot) = reg.entries.get_mut(id.0 as usize) {
            if slot.take().is_some() {
                reg.free.push(id.0);
            }
        }
        if reg.active == Some(id) {
            reg.active = None;
        }
    });
}

/// Mark a navigator as the current/active one (last driven). Called from
/// [`NavigatorControl::dispatch`].
#[cfg(feature = "robot")]
pub(crate) fn mark_active_navigator(id: NavId) {
    NAV_REGISTRY.with(|r| {
        let mut reg = r.borrow_mut();
        if reg
            .entries
            .get(id.0 as usize)
            .map(|s| s.is_some())
            .unwrap_or(false)
        {
            reg.active = Some(id);
        }
    });
    // Current-navigator change → live-update subscribers should refresh.
    crate::robot::bump_revision();
}

/// Snapshot every live navigator. Dead `Weak`s (control dropped without a
/// clean deregister) are skipped and pruned.
#[cfg(feature = "robot")]
pub fn all_navigators() -> Vec<NavSnapshot> {
    // Collect (id, type_name, element_id, control, is_current) under a short
    // borrow, then build snapshots after dropping it — snapshotting reads
    // signals (untracked), which must not happen while NAV_REGISTRY is borrowed.
    let collected: Vec<(u32, &'static str, Option<u32>, Rc<NavigatorControl>, bool)> =
        NAV_REGISTRY.with(|r| {
            let reg = r.borrow();
            let active = reg.active;
            let mut out = Vec::new();
            for (i, slot) in reg.entries.iter().enumerate() {
                if let Some(entry) = slot {
                    if let Some(control) = entry.control.upgrade() {
                        out.push((
                            i as u32,
                            entry.type_name,
                            entry.element_id,
                            control,
                            active == Some(NavId(i as u32)),
                        ));
                    }
                }
            }
            out
        });
    collected
        .into_iter()
        .filter_map(|(nav_id, type_name, element_id, control, is_current)| {
            build_nav_snapshot(nav_id, type_name, element_id, &control, is_current)
        })
        .collect()
}

/// Snapshot one navigator by id. `None` if absent or its control is gone.
#[cfg(feature = "robot")]
pub fn navigator_snapshot(id: NavId) -> Option<NavSnapshot> {
    let (type_name, element_id, control, is_current) = NAV_REGISTRY.with(|r| {
        let reg = r.borrow();
        let entry = reg.entries.get(id.0 as usize)?.as_ref()?;
        let control = entry.control.upgrade()?;
        Some((entry.type_name, entry.element_id, control, reg.active == Some(id)))
    })?;
    build_nav_snapshot(id.0, type_name, element_id, &control, is_current)
}

#[cfg(feature = "robot")]
fn build_nav_snapshot(
    nav_id: u32,
    type_name: &'static str,
    element_id: Option<u32>,
    control: &Rc<NavigatorControl>,
    is_current: bool,
) -> Option<NavSnapshot> {
    let (active_route, active_path, depth, can_go_back) = control.nav_state_snapshot()?;
    Some(NavSnapshot {
        nav_id,
        element_id,
        type_name,
        active_route: active_route.to_string(),
        active_path,
        depth,
        can_go_back,
        is_current,
        base: control.base(),
        stack: control.stack_routes(),
    })
}

/// Test-support: clear the registry between cases (it's thread-local, so
/// this is just belt-and-suspenders for same-thread test ordering).
/// Compiled under `robot` (not `cfg(test)`) because runtime-core's own
/// tests also call it across the crate boundary during the transition.
#[cfg(feature = "robot")]
#[doc(hidden)]
pub fn nav_registry_reset() {
    NAV_REGISTRY.with(|r| *r.borrow_mut() = NavRegistry::default());
}

#[cfg(all(test, feature = "robot"))]
mod nav_registry_tests {
    //! Registry-level coverage. The SDK navigator *handlers* (stack/tab/
    //! drawer) live in separate crates, so a full walker-driven mount isn't
    //! reachable from runtime-core; these drive the registry API with a
    //! hand-built `NavigatorControl` (the part this change owns) plus a
    //! scope-drop test mirroring the walker's `on_cleanup` deregister wiring.
    //! End-to-end walker registration is exercised by the inspector app.

    use super::*;
    use crate::reactive::{with_scope, Scope, Signal};

    fn control_with_state(
        route: &'static str,
        path: &str,
        depth: usize,
        base: &str,
    ) -> (Rc<NavigatorControl>, Box<Scope>) {
        let control = Rc::new(NavigatorControl::new());
        let mut scope = Box::new(Scope::new());
        let ns = with_scope(&mut scope, || NavState {
            active_route: Signal::new(route),
            active_path: Signal::new(path.to_string()),
            depth: Signal::new(depth),
            can_go_back: Signal::new(depth > 1),
        });
        control.attach_nav_state(ns);
        control.set_base(base.to_string());
        (control, scope)
    }

    #[test]
    fn enumerates_and_reports_state() {
        nav_registry_reset();
        let (control, _scope) = control_with_state("detail", "/items/5", 3, "");
        let id = register_navigator(&control, "stack_navigator::Presentation", Some(7));
        control.set_nav_id(id);

        let all = all_navigators();
        assert_eq!(all.len(), 1);
        let s = &all[0];
        assert_eq!(s.nav_id, id.0);
        assert_eq!(s.element_id, Some(7));
        assert_eq!(s.type_name, "stack_navigator::Presentation");
        assert_eq!(s.active_route, "detail");
        assert_eq!(s.active_path, "/items/5");
        assert_eq!(s.depth, 3);
        assert!(s.can_go_back);
        assert!(s.is_current, "first-registered navigator is current (cold start)");
        nav_registry_reset();
    }

    #[test]
    fn current_tracks_mark_active() {
        nav_registry_reset();
        let (a, _sa) = control_with_state("home", "/", 1, "");
        let (b, _sb) = control_with_state("profile", "/profile", 1, "/tab2");
        let id_a = register_navigator(&a, "tab", None);
        a.set_nav_id(id_a);
        let id_b = register_navigator(&b, "stack", None);
        b.set_nav_id(id_b);

        // Cold start: the first registered (a) is current.
        let cur = |id: NavId| navigator_snapshot(id).unwrap().is_current;
        assert!(cur(id_a) && !cur(id_b));

        // Dispatching against b marks it current.
        mark_active_navigator(id_b);
        assert!(!cur(id_a) && cur(id_b));
        nav_registry_reset();
    }

    #[test]
    fn prunes_dropped_control() {
        nav_registry_reset();
        let (control, _scope) = control_with_state("home", "/", 1, "");
        let id = register_navigator(&control, "stack", None);
        control.set_nav_id(id);
        drop(control); // the Weak can no longer upgrade
        assert!(all_navigators().is_empty(), "dropped control must not enumerate");
        nav_registry_reset();
    }

    #[test]
    fn deregister_removes_entry() {
        nav_registry_reset();
        let (control, _scope) = control_with_state("home", "/", 1, "");
        let id = register_navigator(&control, "stack", None);
        control.set_nav_id(id);
        assert!(navigator_snapshot(id).is_some());
        deregister_navigator(id);
        assert!(navigator_snapshot(id).is_none());
        assert!(all_navigators().is_empty());
        nav_registry_reset();
    }

    #[test]
    fn stack_routes_default_and_installed() {
        nav_registry_reset();
        let (control, _scope) = control_with_state("detail", "/items/5", 1, "");
        let id = register_navigator(&control, "stack", None);
        control.set_nav_id(id);

        // No reporter installed → single current route.
        assert_eq!(
            navigator_snapshot(id).unwrap().stack,
            vec![("detail".to_string(), "/items/5".to_string())]
        );

        // Installed reporter → full history, root-first.
        control.install_stack_snapshot(Box::new(|| {
            vec![
                ("home".to_string(), "/".to_string()),
                ("list".to_string(), "/items".to_string()),
                ("detail".to_string(), "/items/5".to_string()),
            ]
        }));
        let stack = navigator_snapshot(id).unwrap().stack;
        assert_eq!(stack.len(), 3);
        assert_eq!(stack.first().unwrap().0, "home");
        assert_eq!(stack.last().unwrap().0, "detail", "current route is last");
        nav_registry_reset();
    }

    /// Mirrors the walker's wiring: registration paired with an
    /// `on_cleanup(deregister)` anchored to the navigator's build scope.
    /// When that scope drops (navigator unmounts / `when`-branch flips), the
    /// registry entry must be gone — no phantom navigator in the inspector.
    #[test]
    fn regression_navigator_registry_freed_on_scope_drop() {
        nav_registry_reset();
        let (control, _ns_scope) = control_with_state("home", "/", 1, "");

        // The navigator's build scope (separate from the nav_state scope the
        // control retains, exactly as in the walker).
        let mut build_scope = Box::new(Scope::new());
        let id = with_scope(&mut build_scope, || {
            let id = register_navigator(&control, "stack", None);
            control.set_nav_id(id);
            crate::reactive::on_cleanup(move || deregister_navigator(id));
            id
        });
        assert!(navigator_snapshot(id).is_some(), "registered while mounted");

        drop(build_scope); // navigator unmounts
        assert!(
            navigator_snapshot(id).is_none(),
            "registry entry must be freed when the build scope drops"
        );
        nav_registry_reset();
    }
}

// ---------------------------------------------------------------------------
// Hierarchical base path — a nested navigator's URL prefix
// ---------------------------------------------------------------------------
//
// Navigators form a tree; each owns a URL PREFIX (its "base"). The root's
// base is empty. When a navigator mounts a screen, it pushes `base +
// route.path()` here for the duration of building that screen's body, so a
// child `Element::Navigator` nested in that screen reads its own base. Route
// patterns are therefore RELATIVE to the navigator they're registered on; the
// framework composes the full URL up the tree (`join_path`) and peels prefixes
// down it (`match_prefix`). A single root navigator (base "") is unaffected:
// `join_path("", p) == p`, so existing apps behave identically.

thread_local! {
    static NAV_BASE: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// RAII guard pushing the base prefix a nested navigator resolves relative
/// to. Held by `mount_screen` while building a screen body.
pub struct NavBaseGuard;

impl NavBaseGuard {
    pub fn push(base: String) -> Self {
        NAV_BASE.with(|s| s.borrow_mut().push(base));
        NavBaseGuard
    }
}

impl Drop for NavBaseGuard {
    fn drop(&mut self) {
        NAV_BASE.with(|s| {
            let _ = s.borrow_mut().pop();
        });
    }
}

/// The base prefix the navigator currently being built resolves its routes
/// relative to. Empty (`""`) for the root navigator.
pub fn current_nav_base() -> String {
    NAV_BASE.with(|s| s.borrow().last().cloned().unwrap_or_default())
}

/// Join a base prefix with a (relative) route path into a full URL path,
/// collapsing duplicate/empty slashes. `join_path("/encounters", "/abc") ==
/// "/encounters/abc"`, `join_path("", "/today") == "/today"`,
/// `join_path("/encounters", "") == "/encounters"`, `join_path("", "") == "/"`.
pub fn join_path(base: &str, rel: &str) -> String {
    let b = base.trim_end_matches('/');
    let r = rel.trim_start_matches('/');
    if r.is_empty() {
        if b.is_empty() {
            "/".to_string()
        } else {
            b.to_string()
        }
    } else if b.is_empty() {
        format!("/{r}")
    } else {
        format!("{b}/{r}")
    }
}

/// Snapshot of the ambient navigator context (nav control, screen
/// state, screen route) at a point in the build. Reactive regions
/// (`when`/`switch`/`for`) capture this when first built — inside the
/// screen's ambient scope — and re-establish it around every rebuild,
/// so a subtree rebuilt by a signal change (e.g. a `link` whose active
/// styling flips) keeps the same ambient navigator it was born with.
/// Without this, a reactively-remounted `link` captures `None` and
/// silently stops navigating.
///
/// The navigator control is held WEAK on purpose: the navigator owns
/// the screen scopes, a screen scope owns the reactive region's Effect,
/// and that Effect would own this snapshot — a strong `Rc` here closes
/// a reference cycle that leaks the whole navigator. `enter()` upgrades;
/// if the navigator is gone (region tearing down) it simply restores
/// nothing.
#[derive(Clone, Default)]
pub struct AmbientNavContext {
    nav: Option<std::rc::Weak<NavigatorControl>>,
    // `Option` = "was a screen-state guard present at capture". An empty
    // `QueryParams` is a real value (a navigation with no state), so it
    // must stay distinguishable from "no guard, don't re-push one".
    state: Option<QueryParams>,
    route: Option<&'static str>,
}

/// Capture the current ambient context. Call this synchronously while
/// building a reactive region (i.e. while the screen's guards are still
/// on the stack), BEFORE creating the rebuild Effect.
pub fn capture_ambient_nav_context() -> AmbientNavContext {
    AmbientNavContext {
        nav: AMBIENT_NAV.with(|s| s.borrow().last().map(Rc::downgrade)),
        state: SCREEN_STATE.with(|s| s.borrow().last().cloned()),
        route: SCREEN_ROUTE.with(|s| s.borrow().last().copied()),
    }
}

impl AmbientNavContext {
    /// True when there is no navigator context to restore — lets callers
    /// cheaply skip when used outside any navigator.
    pub fn is_empty(&self) -> bool {
        self.nav.is_none() && self.state.is_none() && self.route.is_none()
    }

    /// Re-push the captured context. The returned guard pops all three
    /// stacks on drop. Hold it across the subtree rebuild.
    pub fn enter(&self) -> AmbientNavContextGuard {
        AmbientNavContextGuard {
            _nav: self.nav.as_ref().and_then(|w| w.upgrade()).map(AmbientNavGuard::push),
            _state: self.state.clone().map(ScreenStateGuard::push),
            _route: self.route.map(ScreenRouteGuard::push),
        }
    }
}

/// Drops in field order; each inner guard pops its own (independent)
/// stack, so order is irrelevant for correctness.
pub struct AmbientNavContextGuard {
    _nav: Option<AmbientNavGuard>,
    _state: Option<ScreenStateGuard>,
    _route: Option<ScreenRouteGuard>,
}

// ---------------------------------------------------------------------------
// RouteParams — URL ⇄ typed params
// ---------------------------------------------------------------------------

/// Convert route params to/from URL path segments. Implemented on every
/// type used as a `Route<P>` payload; built-in for `()` (the no-params
/// case). Web/SSR backends use this to map between URLs and typed
/// payloads; native backends ignore the path side.
pub trait RouteParams: 'static + Sized {
    fn to_path(&self, pattern: &str) -> String {
        let _ = self;
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

    fn from_segments(_segments: &HashMap<String, String>) -> Option<Self> {
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
// Route<P> — typed route name + URL pattern
// ---------------------------------------------------------------------------

/// A navigation route. `name` is the in-stack key; `path` is the URL
/// pattern used by web/SSR backends. The phantom `P` is what
/// `handle.push(route, params)` etc. type-check against.
#[derive(Clone)]
pub struct Route<P: RouteParams = ()> {
    name: &'static str,
    path: &'static str,
    _params: PhantomData<P>,
}

impl<P: RouteParams> Route<P> {
    pub const fn new(name: &'static str, path: &'static str) -> Self {
        Self { name, path, _params: PhantomData }
    }

    pub fn name(&self) -> &'static str {
        self.name
    }

    pub fn path(&self) -> &'static str {
        self.path
    }
}

// ---------------------------------------------------------------------------
// ScreenBuilder + RouteEntry — type-erased per-route registry
// ---------------------------------------------------------------------------




// ---------------------------------------------------------------------------
// Screen — what a route's render closure returns
// ---------------------------------------------------------------------------




/// Result of mounting a screen. `mount_screen` returns this so the
/// SDK handler has the body node, the framework-owned scope id (used
/// to release the scope later), and the screen's opaque options
/// (downcast inside the handler).
pub struct MountResult<N> {
    pub node: N,
    pub scope_id: u64,
    pub options: Box<dyn Any>,
}

// ---------------------------------------------------------------------------
// Path matching — pure-Rust matcher used by web + future SSR
// ---------------------------------------------------------------------------

/// Match `pattern` against the LEADING segments of `path`. Returns the
/// extracted `:placeholder` segments plus the unconsumed remainder of
/// `path` (a leading-slash string, or empty `""` when fully consumed).
/// `None` when a literal segment differs or `path` has fewer segments
/// than `pattern`.
///
/// This is the hierarchical primitive: a parent navigator matches its
/// route's pattern as a prefix and hands the `remainder` to the child
/// navigator nested in that screen (which prefix-matches in turn). A
/// full URL is resolved by peeling one prefix per level down the active
/// navigator tree. Trailing slashes are tolerated; empty path = `/`.
///
/// Any query string on `path` is stripped before matching. Routing runs on
/// the path axis only — the query configures a screen, it never selects
/// one. Callers are expected to have split the URL already
/// ([`split_query`](super::query::split_query)); stripping here too is
/// defense in depth, because a `?` that reaches the segment splitter does
/// not fail loudly, it silently binds `id` to `5?tab=a`.
pub fn match_prefix(path: &str, pattern: &str) -> Option<(HashMap<String, String>, String)> {
    let path = super::query::strip_query(path);
    let path_segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let pat_segs: Vec<&str> = pattern.split('/').filter(|s| !s.is_empty()).collect();
    if path_segs.len() < pat_segs.len() {
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
    let remainder_segs = &path_segs[pat_segs.len()..];
    let remainder = if remainder_segs.is_empty() {
        String::new()
    } else {
        format!("/{}", remainder_segs.join("/"))
    };
    Some((out, remainder))
}

/// The concrete prefix of `path` that a [`match_prefix`] resolution
/// consumed: `path` minus the trailing `remainder` segments, normalized
/// (leading slash, no empty segments; `/` when nothing was consumed).
///
/// Use this — not the registered pattern — when mirroring a matched URL
/// into `active_path`: the pattern still contains `:param` placeholders,
/// so reconstructing from it leaks a literal `:id` into the path mirror
/// on parameterized deep links.
pub fn consumed_prefix(path: &str, remainder: &str) -> String {
    let path = super::query::strip_query(path);
    let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let rem_count = remainder.split('/').filter(|s| !s.is_empty()).count();
    let consumed = &segs[..segs.len().saturating_sub(rem_count)];
    if consumed.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", consumed.join("/"))
    }
}

/// Match `path` against `pattern` requiring a FULL match (no leftover
/// segments). Returns `Some(map)` when segment counts agree and every
/// literal segment matches case-sensitively; `:placeholder` segments
/// become map entries. Thin wrapper over [`match_prefix`] that rejects
/// any non-empty remainder.
///
/// Trailing slashes are tolerated; empty path is treated as `/`.
pub fn match_pattern(path: &str, pattern: &str) -> Option<HashMap<String, String>> {
    match match_prefix(path, pattern) {
        Some((segs, remainder)) if remainder.is_empty() => Some(segs),
        _ => None,
    }
}

#[cfg(test)]
mod matcher_tests {
    use super::{consumed_prefix, join_path, match_pattern, match_prefix};

    #[test]
    fn join_path_composes_base_and_relative() {
        assert_eq!(join_path("", "/today"), "/today"); // root base
        assert_eq!(join_path("/encounters", "/abc"), "/encounters/abc");
        assert_eq!(join_path("/encounters", ""), "/encounters"); // index
        assert_eq!(join_path("", ""), "/");
        assert_eq!(join_path("/encounters/", "abc"), "/encounters/abc"); // slash tolerance
        // Round-trip: compose then peel returns the relative remainder.
        let full = join_path("/encounters", "/abc");
        let (_, rem) = match_prefix(&full, "/encounters").expect("base prefix");
        assert_eq!(rem, "/abc");
    }

    fn seg(segs: &std::collections::HashMap<String, String>, k: &str) -> Option<String> {
        segs.get(k).cloned()
    }

    #[test]
    fn prefix_consumes_leading_segments_and_returns_remainder() {
        // Parent navigator owns `/encounters`; the child sees `/abc`.
        let (segs, rem) = match_prefix("/encounters/abc", "/encounters").expect("matches");
        assert!(segs.is_empty());
        assert_eq!(rem, "/abc");
    }

    #[test]
    fn prefix_extracts_placeholder_and_remainder() {
        let (segs, rem) = match_prefix("/encounters/abc/notes", "/encounters/:id").expect("matches");
        assert_eq!(seg(&segs, "id").as_deref(), Some("abc"));
        assert_eq!(rem, "/notes");
    }

    #[test]
    fn prefix_full_match_has_empty_remainder() {
        let (segs, rem) = match_prefix("/encounters/abc", "/encounters/:id").expect("matches");
        assert_eq!(seg(&segs, "id").as_deref(), Some("abc"));
        assert_eq!(rem, "");
    }

    #[test]
    fn prefix_rejects_shorter_path_and_literal_mismatch() {
        // Path shorter than pattern.
        assert!(match_prefix("/encounters", "/encounters/:id").is_none());
        // Literal segment differs.
        assert!(match_prefix("/patients/abc", "/encounters/:id").is_none());
    }

    #[test]
    fn consumed_prefix_is_concrete_not_pattern() {
        // Parameterized full match: the whole concrete path was consumed.
        let (_, rem) = match_prefix("/items/123", "/items/:id").expect("matches");
        assert_eq!(consumed_prefix("/items/123", &rem), "/items/123");
        // Partial match: the remainder's segments are stripped from the end.
        let (_, rem) = match_prefix("/encounters/abc/notes", "/encounters/:id").expect("matches");
        assert_eq!(consumed_prefix("/encounters/abc/notes", &rem), "/encounters/abc");
        // Nothing consumed (index route at root): normalized to "/".
        assert_eq!(consumed_prefix("/a/b", "/a/b"), "/");
        // Slash tolerance in the input path.
        assert_eq!(consumed_prefix("/items/123/", ""), "/items/123");
    }

    #[test]
    fn pattern_requires_full_match() {
        // Exact match: ok.
        assert!(match_pattern("/encounters/abc", "/encounters/:id").is_some());
        // Leftover segments: rejected (this is the pattern-vs-prefix distinction).
        assert!(match_pattern("/encounters/abc/notes", "/encounters/:id").is_none());
        assert!(match_pattern("/encounters/abc", "/encounters").is_none());
    }

    #[test]
    fn two_level_descent() {
        // Root drawer matches `/encounters` prefix; nested stack matches the rest.
        let (root_segs, rem) = match_prefix("/encounters/abc", "/encounters").expect("root");
        assert!(root_segs.is_empty());
        // Child stack's detail route is `/encounters/:id` *relative to root base* —
        // but the child only ever sees the remainder `/abc`, so its route pattern,
        // expressed relative to the base, is matched against `/abc`.
        let (child_segs, child_rem) = match_prefix(&rem, "/:id").expect("child");
        assert_eq!(seg(&child_segs, "id").as_deref(), Some("abc"));
        assert_eq!(child_rem, "");
    }
}

// ---------------------------------------------------------------------------
// NavigatorHandle — framework-side handle. Just dispatch + control accessor.
// SDK typed handles wrap it with kind-specific methods.
// ---------------------------------------------------------------------------

/// The handle the framework hands to `Ref<H>` bindings. Carries an
/// opaque node, a `&'static dyn NavigatorOps`, and an optional
/// `Rc<NavigatorControl>` for dispatch.
///
/// **No kind-specific methods here.** `push` / `pop` / `select` /
/// drawer open/close live on the SDK's typed handle (e.g.
/// `StackHandle`, `DrawerHandle`), which wraps `NavigatorHandle` and
/// dispatches via `self.dispatch(NavCommand::…)`.
#[derive(Clone)]
pub struct NavigatorHandle {
    node: Rc<dyn Any>,
    ops: &'static dyn NavigatorOps,
    control: Option<Rc<NavigatorControl>>,
}

impl NavigatorHandle {
    pub fn new(node: Rc<dyn Any>, ops: &'static dyn NavigatorOps) -> Self {
        Self { node, ops, control: None }
    }

    pub fn with_control(
        node: Rc<dyn Any>,
        ops: &'static dyn NavigatorOps,
        control: Rc<NavigatorControl>,
    ) -> Self {
        Self { node, ops, control: Some(control) }
    }

    /// Access the underlying control plane. SDK typed handles use this
    /// to dispatch their kind-specific commands.
    pub fn control(&self) -> Option<&Rc<NavigatorControl>> {
        self.control.as_ref()
    }

    /// Dispatch a NavCommand against this navigator. Silent no-op when
    /// the handle has no control (pre-mount).
    pub fn dispatch(&self, cmd: NavCommand) {
        if let Some(c) = &self.control {
            c.dispatch(cmd);
        }
    }

    /// Cached depth — set by the SDK handler via
    /// `NavigatorControl::set_depth`. Cheap; doesn't reach the SDK.
    pub fn depth(&self) -> usize {
        self.control.as_ref().map(|c| c.depth()).unwrap_or(0)
    }

    /// Type-erased access to the navigator's opaque node payload. SDK
    /// typed handles use this to look up SDK-owned per-instance state.
    pub fn node_as_any(&self) -> &dyn Any {
        &*self.node
    }

    /// The static ops pointer. Currently unused (`NavigatorOps` has no
    /// methods); reserved for future per-backend hooks the handle
    /// might want to dispatch through.
    #[allow(dead_code)]
    pub(crate) fn ops(&self) -> &'static dyn NavigatorOps {
        self.ops
    }
}

/// Backend hook trait the handle's `&dyn NavigatorOps` points to.
/// Reserved for backend extension methods that need to dispatch
/// through the handle's static vtable. Currently empty — the dispatch
/// path goes through `NavigatorControl` directly.
pub trait NavigatorOps {}

// ---------------------------------------------------------------------------
// NavigatorControl — dispatcher + reactive nav-state bridge
// ---------------------------------------------------------------------------

/// The shared control plane between framework substrate and SDK
/// handler. Wraps the command dispatcher closure the SDK installs at
/// `init` time, a depth cache the handle reads, and the reactive
/// `NavState` mirror chrome subscribes to.
// (was: cfg_attr(not(prim-navigator), allow(dead_code)) — the prim-*
// gates live in runtime-core; in the shared crate the reachability
// anchor is always the old walker or the new-core vocabulary.)
#[allow(dead_code)]
pub struct NavigatorControl {
    dispatch: RefCell<Option<Box<dyn Fn(NavCommand)>>>,
    depth: RefCell<usize>,
    nav_state: RefCell<Option<NavState>>,
    /// This navigator's URL prefix in the hierarchy (empty for the root).
    /// Route patterns are registered RELATIVE to this; `dispatch` composes
    /// `base + cmd.url` into the full hierarchical path that chrome and the
    /// platform URL see. Set once at build via [`set_base`](Self::set_base).
    base: RefCell<String>,
    /// Optional SDK-installed link activation builder. Maps the
    /// triple `(route_name, url, params)` to a `NavCommand`. The
    /// `Link` primitive calls this on activation to pick the right
    /// dispatch verb for the enclosing navigator — stack SDKs install
    /// one that builds `Push`; tab/drawer SDKs install one that builds
    /// `Select`. When not installed, `Link` defaults to `Push`.
    link_activator: RefCell<
        Option<Rc<dyn Fn(&'static str, String, Box<dyn Any>) -> NavCommand>>,
    >,
    /// Backend-provided "schedule a layout pass" hook, registered ONCE by the
    /// navigator walker (`|| B::schedule_layout_pass()`). `dispatch` calls it
    /// after every command so a freshly-mounted screen is always laid out — the
    /// guarantee lives here in the abstraction, not duplicated (and forgettable)
    /// in each navigator×backend handler. `None` until the walker registers it.
    request_layout: RefCell<Option<Box<dyn Fn()>>>,
    /// Reactive scope owning this navigator's `nav_state` signals (and any
    /// other framework-owned per-navigator reactive state). The control is
    /// the navigator's true lifetime anchor — it's an `Rc` held by the
    /// backend instance and the SDK handler, so it outlives the *transient*
    /// build scope that ran `build_navigator`.
    ///
    /// `nav_state` MUST be anchored here, not to the ambient build scope: a
    /// nested navigator (e.g. a stack hung under a drawer screen) is often
    /// built inside a short-lived dispatch/microtask scope. If `nav_state`
    /// were owned by that scope, its signals would be freed when the scope
    /// drops, and a later `active_route.set(...)` from `mount_internal` /
    /// `on_popstate` would hit a recycled arena slot — "signal used after
    /// its scope was dropped" / type-mismatch. Owning the scope here ties
    /// the signals to the navigator's real lifetime: freed when the control
    /// drops on navigator teardown (leak-free), never sooner.
    owning_scope: RefCell<Option<Box<crate::reactive::Scope>>>,
    /// This navigator's id in the global registry (robot introspection).
    /// Set once at build via [`set_nav_id`](Self::set_nav_id); `None` until
    /// then. `dispatch` reads it to mark this navigator "current" so the
    /// inspector can highlight the last-driven navigator.
    nav_id: RefCell<Option<NavId>>,
    /// SDK-installed back-stack reporter. Returns the full route history
    /// root-first (index 0 = bottom, last = current/top), each entry a
    /// `(route_name, full_path)`. The framework only tracks the current
    /// route + depth in `nav_state`; real history lives in the per-backend
    /// handler's screen vec (UINavigationController VCs, web history, the
    /// fragment back-stack), so each handler installs a closure reading its
    /// own container. `None` until installed — [`stack_routes`](Self::stack_routes)
    /// then falls back to the single current route. Contract is uniform
    /// across backends (root-first, current last); only the mechanism
    /// differs (CLAUDE.md §7).
    stack_snapshot: RefCell<Option<Box<dyn Fn() -> Vec<(String, String)>>>>,
    /// URL-sync context parked here by the walker (kind-agnostically) so a
    /// handler can OPT IN to substrate URL synchronization via
    /// [`enable_url_sync`](Self::enable_url_sync). `None` after opt-in
    /// (the context moves into the url_sync registry) or when the walker
    /// never provided one (hand-built controls in tests).
    url_sync_ctx: RefCell<Option<super::url_sync::UrlSyncContext>>,
    /// The url_sync registry id once [`enable_url_sync`](Self::enable_url_sync)
    /// activated; `dispatch` routes its before/after hooks through it.
    /// `None` for legacy handlers (which own their URL work) and on
    /// platforms without an installed URL provider.
    url_sync_id: std::cell::Cell<Option<u64>>,
}

// (was: cfg_attr(not(prim-navigator), allow(dead_code)) — the prim-*
// gates live in runtime-core; in the shared crate the reachability
// anchor is always the old walker or the new-core vocabulary.)
#[allow(dead_code)]
impl NavigatorControl {
    pub fn new() -> Self {
        Self {
            dispatch: RefCell::new(None),
            depth: RefCell::new(1),
            nav_state: RefCell::new(None),
            base: RefCell::new(String::new()),
            link_activator: RefCell::new(None),
            request_layout: RefCell::new(None),
            owning_scope: RefCell::new(None),
            nav_id: RefCell::new(None),
            stack_snapshot: RefCell::new(None),
            url_sync_ctx: RefCell::new(None),
            url_sync_id: std::cell::Cell::new(None),
        }
    }

    /// Park the URL-sync context for a possible handler opt-in. Called
    /// once by the navigator walker at build; kind-agnostic and inert
    /// until [`enable_url_sync`](Self::enable_url_sync).
    #[doc(hidden)]
    pub fn set_url_sync_context(&self, ctx: super::url_sync::UrlSyncContext) {
        *self.url_sync_ctx.borrow_mut() = Some(ctx);
    }

    /// Opt this navigator into substrate URL synchronization (browser
    /// pushState/popstate mirroring, deep links, scroll restore). Called
    /// by outlet-model handlers (`swap-navigator`, `stack-navigator`)
    /// in `init`. No-op on platforms without an installed URL provider,
    /// and for controls the walker gave no context (tests). Legacy
    /// class-based handlers must NOT call this — they do their own URL
    /// work and would double-write history.
    pub fn enable_url_sync(self: &Rc<Self>) {
        if self.url_sync_id.get().is_some() {
            return;
        }
        let Some(ctx) = self.url_sync_ctx.borrow_mut().take() else { return };
        self.url_sync_id.set(super::url_sync::register(self, ctx));
    }

    /// Install the outlet scroll accessors used for scroll snapshot /
    /// restore across navigation. Handlers call this once their outlet
    /// node exists (the deferred layout microtask). No-op unless
    /// [`enable_url_sync`](Self::enable_url_sync) activated first.
    pub fn install_scroll_accessor(
        &self,
        get: Rc<dyn Fn() -> (f32, f32)>,
        set: Rc<dyn Fn(f32, f32)>,
    ) {
        if let Some(id) = self.url_sync_id.get() {
            super::url_sync::install_scroll_accessor(id, get, set);
        }
    }

    /// Retain the reactive scope that owns this navigator's `nav_state`
    /// signals so they live for the control's lifetime, not the transient
    /// build scope's. Called once from `walker::navigator::build` right
    /// after the scope-anchored `nav_state` is constructed. See the
    /// `owning_scope` field doc for why this anchoring is required.
    #[doc(hidden)]
    pub fn retain_scope(&self, scope: Box<crate::reactive::Scope>) {
        *self.owning_scope.borrow_mut() = Some(scope);
    }

    /// Set this navigator's hierarchy base prefix. Called once at build
    /// from the navigator walker with [`current_nav_base`]. Empty for the
    /// root; e.g. `/encounters` for a stack nested under that drawer screen.
    pub fn set_base(&self, base: String) {
        *self.base.borrow_mut() = base;
    }

    /// This navigator's base prefix.
    pub fn base(&self) -> String {
        self.base.borrow().clone()
    }

    /// Wire the framework's reactive nav-state mirror. Called once
    /// from `walker::navigator::build` before `install`.
    pub fn attach_nav_state(&self, nav_state: NavState) {
        *self.nav_state.borrow_mut() = Some(nav_state);
    }

    /// Install the SDK's command dispatcher closure. Called once from
    /// the SDK handler's `init`.
    pub fn install(&self, dispatch: Box<dyn Fn(NavCommand)>) {
        *self.dispatch.borrow_mut() = Some(dispatch);
    }

    /// Register the backend's "schedule a layout pass" hook. Called once by the
    /// navigator walker with `|| B::schedule_layout_pass()`. After this, every
    /// [`dispatch`](Self::dispatch) guarantees a layout pass — so no
    /// navigator×backend handler has to (and none can forget to).
    pub fn install_request_layout(&self, f: Box<dyn Fn()>) {
        *self.request_layout.borrow_mut() = Some(f);
    }

    /// Install the SDK's `Link` activation builder. Optional; if not
    /// set, `Link` defaults to `NavCommand::Push`. Stack-like SDKs
    /// typically don't install (Push is the default); tab/drawer SDKs
    /// install one that returns `Select`.
    pub fn install_link_activator(
        &self,
        f: Rc<dyn Fn(&'static str, String, Box<dyn Any>) -> NavCommand>,
    ) {
        *self.link_activator.borrow_mut() = Some(f);
    }

    /// Build the activation command for a `Link` activating against
    /// this navigator. Falls back to `Push` when no activator was
    /// installed.
    pub fn build_link_command(
        &self,
        name: &'static str,
        url: String,
        params: Box<dyn Any>,
    ) -> NavCommand {
        if let Some(f) = self.link_activator.borrow().as_ref() {
            f(name, url, params)
        } else {
            // A route pattern may itself carry a query (`/search?sort=new`);
            // split it into the command's own slot so the substrate never
            // routes on it. See `NavCommand`'s doc on the url/query split.
            let (path, query) = super::query::split_query(&url);
            NavCommand::Push { name, url: path.to_string(), params, query }
        }
    }

    /// Update the cached depth. SDK handler calls this when stack
    /// depth changes so `handle.depth()` stays in sync.
    pub fn set_depth(&self, d: usize) {
        *self.depth.borrow_mut() = d;
    }

    pub fn depth(&self) -> usize {
        *self.depth.borrow()
    }

    /// Record this navigator's global-registry id. Called once at build
    /// from `walker::navigator::build` after [`register_navigator`].
    pub fn set_nav_id(&self, id: NavId) {
        *self.nav_id.borrow_mut() = Some(id);
    }

    /// This navigator's global-registry id, if registered.
    pub fn nav_id(&self) -> Option<NavId> {
        *self.nav_id.borrow()
    }

    /// Install the SDK handler's back-stack reporter (see the
    /// `stack_snapshot` field). Called once at `init`. The closure must
    /// return the history root-first with the current route last, reading
    /// its container untracked. Cheap no-op storage when nothing reads it
    /// (the `robot` bridge is the only reader).
    pub fn install_stack_snapshot(&self, f: Box<dyn Fn() -> Vec<(String, String)>>) {
        *self.stack_snapshot.borrow_mut() = Some(f);
    }

    /// The navigator's back-stack as `(route, path)` pairs, root-first,
    /// current last. Uses the SDK-installed reporter when present; else
    /// falls back to the single current route from `nav_state` (so a
    /// handler that hasn't opted in still reports something coherent).
    pub fn stack_routes(&self) -> Vec<(String, String)> {
        if let Some(f) = self.stack_snapshot.borrow().as_ref() {
            return f();
        }
        match self.nav_state_snapshot() {
            Some((route, path, _, _)) => vec![(route.to_string(), path)],
            None => Vec::new(),
        }
    }

    /// Snapshot the reactive nav-state as `(active_route, active_path,
    /// depth, can_go_back)`, reading every signal **untracked** so a
    /// caller (the robot bridge) never subscribes a scope. `None` if
    /// `nav_state` hasn't been attached yet.
    pub fn nav_state_snapshot(&self) -> Option<(&'static str, String, usize, bool)> {
        let st = self.nav_state.borrow();
        let st = st.as_ref()?;
        Some(crate::reactive::untrack(|| {
            (
                st.active_route.get(),
                st.active_path.get(),
                st.depth.get(),
                st.can_go_back.get(),
            )
        }))
    }

    /// Dispatch a NavCommand against this navigator. Updates the
    /// reactive nav-state mirror (for commands that change the active
    /// route) before forwarding to the SDK's installed dispatcher.
    pub fn dispatch(&self, cmd: NavCommand) {
        // Mark this navigator "current" for the inspector: a dispatch is
        // the framework-observable signal that this navigator is being
        // driven (programmatic nav, link taps, and native gestures all
        // route here or through the active_changed callback).
        #[cfg(feature = "robot")]
        if let Some(id) = *self.nav_id.borrow() {
            mark_active_navigator(id);
        }
        // Compose this navigator's base prefix onto the command's
        // (navigator-relative) url, so the nav-state mirror, chrome, and the
        // platform URL all see the full hierarchical path. For the root
        // navigator (base ""), `join_path("", url) == url` — a no-op, so a
        // single-navigator app is unaffected.
        let base = self.base.borrow().clone();
        let cmd = self.compose_url(&base, cmd);
        // Substrate URL sync (opt-in): mirror the command into browser
        // history + snapshot the outgoing screen's scroll BEFORE the
        // handler swaps the outlet (the outlet still shows the old
        // screen here). No-op for legacy handlers / URL-less platforms.
        // The kind tag is captured now because `cmd` moves into the
        // handler closure below; `after_command` only needs the shape.
        let url_sync_kind = super::url_sync::CommandKind::of(&cmd);
        if let Some(url_sync) = self.url_sync_id.get() {
            super::url_sync::before_command(url_sync, &cmd);
        }
        // Update the active route/path signals before the SDK sees
        // the command, so any effect reading them re-fires while the
        // SDK is still committing the change. Pop and Custom don't
        // carry a new route name — the SDK is responsible for
        // updating signals via `active_changed` after committing.
        // One navigation = ONE reactive update. We write the route/path mirror
        // here, AND the SDK's installed dispatcher re-writes the same two
        // signals via its `active_changed` callback after committing the swap
        // (the macOS/iOS drawer + stack handlers do this). `set` is the
        // always-notify primitive, so without batching those duplicate writes
        // fan out separately — waking chrome subscribers (e.g. every sidebar
        // item's active-state `derived`, the header's route `switch`) more than
        // once per navigation, and a backend whose layout flush hooks the
        // reactive-idle boundary (macOS) turns each fan-out into a full-tree
        // layout pass. Batching collapses the duplicate writes of a given signal
        // to a single subscriber wake (a signal recorded twice in one window
        // wakes its subscribers once at flush), so navigation drives exactly one
        // chrome relayout. Reads inside the dispatch (`get()`) still see the
        // updated value immediately — `batch` defers only the subscriber wake.
        crate::reactive::batch(|| {
            if let Some(state) = self.nav_state.borrow().as_ref() {
                match &cmd {
                    NavCommand::Push { name, url, .. }
                    | NavCommand::Replace { name, url, .. }
                    | NavCommand::Reset { name, url, .. }
                    | NavCommand::Select { name, url, .. } => {
                        state.active_route.set(name);
                        state.active_path.set(url.clone());
                    }
                    NavCommand::Pop | NavCommand::Custom(_) => {}
                }
            }
            if let Some(f) = self.dispatch.borrow().as_ref() {
                f(cmd);
            }
            // Centralized layout guarantee: after the SDK handler commits the
            // command (mounts/swaps the screen), ensure a layout pass is
            // scheduled. This is the ONE place every navigation triggers a
            // relayout, on every backend — replacing the per-handler
            // `schedule_layout_pass()` calls that some backends had and others
            // (Android stack) forgot.
            if let Some(f) = self.request_layout.borrow().as_ref() {
                f();
            }
            // URL-sync post-commit: the outlet now shows the new screen,
            // so scroll adjustments (reset-to-top on forward, restore on
            // pop) land on the right surface.
            if let Some(url_sync) = self.url_sync_id.get() {
                super::url_sync::after_command(url_sync, url_sync_kind);
            }
        });
    }

    /// Rebuild a command with `base + url` as its full hierarchical path.
    fn compose_url(&self, base: &str, cmd: NavCommand) -> NavCommand {
        match cmd {
            NavCommand::Push { name, url, params, query } => {
                NavCommand::Push { name, url: join_path(base, &url), params, query }
            }
            NavCommand::Replace { name, url, params, query } => {
                NavCommand::Replace { name, url: join_path(base, &url), params, query }
            }
            NavCommand::Reset { name, url, params, query } => {
                NavCommand::Reset { name, url: join_path(base, &url), params, query }
            }
            NavCommand::Select { name, url, params, query } => {
                NavCommand::Select { name, url: join_path(base, &url), params, query }
            }
            other => other,
        }
    }
}

impl Default for NavigatorControl {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for NavigatorControl {
    fn drop(&mut self) {
        // Tear the URL-sync entry down WITH the navigator. The entry
        // owns author-reachable `Rc`s (route resolver, scroll accessors
        // over handler state); leaving it in the thread-local registry
        // until thread death would run those destructors after the
        // reactive arena's TLS is gone — an abort, not a leak. See
        // `url_sync::deregister`.
        if let Some(id) = self.url_sync_id.get() {
            super::url_sync::deregister(id);
        }
    }
}

// ---------------------------------------------------------------------------
// NavCommand — the framework command vocabulary
// ---------------------------------------------------------------------------

/// Commands that flow through `NavigatorControl::dispatch`. The built-in
/// verbs cover the common navigation shapes; SDKs with novel verbs
/// (drawer open/close, multi-pane focus, etc.) use `Custom`.
///
/// The `query` field on route-carrying variants is the screen's durable
/// state (see [`ScreenState`](super::query::ScreenState)) riding alongside
/// the typed path `params`. The screen builder reads it back via
/// [`screen_state`] / [`screen_query`].
///
/// `url` is the PATH ONLY — never `path?query`. The two are kept apart all
/// the way through the substrate so routing, nav-base publication, and
/// screen cache keys can never see a query string; only the URL-bearing
/// backends recompose them when writing to browser history.
///
/// SDK handlers receive every dispatched command via their installed
/// dispatcher closure. Handlers that don't understand a variant
/// should silently no-op or panic according to their own contract.
pub enum NavCommand {
    Push {
        name: &'static str,
        url: String,
        params: Box<dyn Any>,
        query: QueryParams,
    },
    Pop,
    Replace {
        name: &'static str,
        url: String,
        params: Box<dyn Any>,
        query: QueryParams,
    },
    Reset {
        name: &'static str,
        url: String,
        params: Box<dyn Any>,
        query: QueryParams,
    },
    /// Switch the active screen by name without changing stack depth.
    /// Used by tab- and drawer-style SDKs.
    Select {
        name: &'static str,
        url: String,
        params: Box<dyn Any>,
        query: QueryParams,
    },
    /// SDK-specific command. The payload is downcast by the SDK
    /// handler's dispatcher to its expected type. Used for verbs the
    /// built-in variants don't cover (drawer Open/Close/Toggle, a
    /// multi-pane SDK's SplitFocus, etc.). Wire-protocol-aware SDKs
    /// register a serde pair via `register_navigator_command_serde`
    /// so `Custom` payloads round-trip across dev-mode wire frames.
    Custom(Rc<dyn Any>),
}

// ---------------------------------------------------------------------------
// Per-screen state stack — the query params that arrived with the
// navigation, readable inside the screen's render via
// `screen_state::<S>()` / `screen_query()`.
// ---------------------------------------------------------------------------

thread_local! {
    static SCREEN_STATE: RefCell<Vec<QueryParams>> =
        const { RefCell::new(Vec::new()) };
}

/// RAII guard the framework pushes around each screen build. SDK
/// handlers don't construct these directly — the navigator handler
/// pushes one for the duration of the build.
pub struct ScreenStateGuard;

impl ScreenStateGuard {
    pub fn push(query: QueryParams) -> Self {
        SCREEN_STATE.with(|s| s.borrow_mut().push(query));
        ScreenStateGuard
    }
}

impl Drop for ScreenStateGuard {
    fn drop(&mut self) {
        SCREEN_STATE.with(|s| {
            let _ = s.borrow_mut().pop();
        });
    }
}

/// The raw query params this screen was navigated with. Empty outside a
/// screen build, and empty for a navigation that carried no state.
///
/// This is a SNAPSHOT taken at build time — the value used to seed a
/// screen's signals. To react to later query changes (browser Back that
/// only alters the query, or an in-place `replace_with_state`), read the
/// `query` signal on the navigator context instead.
pub fn screen_query() -> QueryParams {
    SCREEN_STATE.with(|s| s.borrow().last().cloned().unwrap_or_default())
}

/// Decode the current screen's navigation state as `S`.
///
/// Returns `None` outside a screen build or when `S::from_query` rejects
/// the query. Note that a screen reached with NO state still calls
/// `from_query` with an empty [`QueryParams`] — an impl that fills missing
/// fields with defaults (the recommended shape) therefore yields `Some`
/// there, which is what makes one code path serve both an in-app
/// navigation and a cold URL load.
///
/// ```ignore
/// .screen(INBOX, |_| {
///     let filters = signal(screen_state::<Filters>().unwrap_or_default());
///     ui! { /* … */ }
/// })
/// ```
pub fn screen_state<S: ScreenState>() -> Option<S> {
    S::from_query(&screen_query())
}

// ---------------------------------------------------------------------------
// Per-screen route name stack — pushed by the walker at mount time
// so author code inside a screen build can ask "what route am I?"
// without plumbing the name through every component.
// ---------------------------------------------------------------------------

thread_local! {
    static SCREEN_ROUTE: RefCell<Vec<&'static str>> =
        const { RefCell::new(Vec::new()) };
}

/// RAII guard pushed around each screen build alongside
/// [`ScreenStateGuard`]. SDK handlers don't construct these directly;
/// the framework's `mount_screen` does.
pub struct ScreenRouteGuard;

impl ScreenRouteGuard {
    pub fn push(name: &'static str) -> Self {
        SCREEN_ROUTE.with(|s| s.borrow_mut().push(name));
        ScreenRouteGuard
    }
}

impl Drop for ScreenRouteGuard {
    fn drop(&mut self) {
        SCREEN_ROUTE.with(|s| {
            let _ = s.borrow_mut().pop();
        });
    }
}

/// Return the route name being built right now. `None` when called
/// outside a screen build. Author code uses this together with
/// [`ambient_navigator`] (and its `nav_state.active_route`) to derive
/// a per-screen focus signal — see [`use_focus`].
pub fn current_screen_route() -> Option<&'static str> {
    SCREEN_ROUTE.with(|s| s.borrow().last().copied())
}

/// Returns a function `() -> bool` that reads as `true` whenever the
/// current screen is the navigator's active route. Call inside a
/// screen render to wire focus-driven behavior (pause/resume an
/// embedded `host_wgpu::IosHostHandle`, mute a video, stop a poll,
/// rebind a keyboard shortcut, etc.).
///
/// The returned closure is reactive — read it inside an `effect!`
/// block (or any reactive context) and the effect re-runs whenever
/// focus changes:
///
/// ```ignore
/// use runtime_core::primitives::navigator::use_focus;
///
/// let is_focused = use_focus();
/// effect!(move || {
///     if is_focused() {
///         handle.resume();
///     } else {
///         handle.pause();
///     }
/// });
/// ```
///
/// Returns `|| false` when called outside a screen build (no ambient
/// navigator or no current route). Authors who need to distinguish
/// "no navigator" from "not focused" can check
/// [`current_screen_route`] / [`ambient_navigator`] directly.
pub fn use_focus() -> impl Fn() -> bool + 'static {
    let route = current_screen_route();
    // Capture the `active_route` signal at use-time. The signal is
    // an `Rc`, so the clone is cheap and keeps the source alive even
    // if the NavigatorControl itself is later dropped — that means
    // the returned closure stays callable for the rest of the
    // enclosing scope's lifetime.
    let active_route = ambient_navigator()
        .and_then(|n| n.nav_state.borrow().as_ref().map(|s| s.active_route));
    move || match (route, active_route) {
        (Some(r), Some(sig)) => sig.get() == r,
        _ => false,
    }
}

/// Returns a function `() -> bool` that reads as `true` when the ambient
/// navigator has a screen to pop back to — i.e. the active screen is NOT the
/// root of its stack. Reactive: read it inside an `effect!` block (or any
/// reactive context) and it re-fires whenever the stack depth changes (push,
/// pop, or a native back gesture).
///
/// ```ignore
/// use runtime_core::primitives::navigator::use_can_go_back;
///
/// let can_go_back = use_can_go_back();
/// // e.g. show a root-only FAB while at the stack root:
/// presence(|| fab()).present(move || !can_go_back());
/// ```
///
/// **Prefer this over [`use_focus`] for "am I the root screen" gating that must
/// survive a native back.** `use_focus` keys off `active_route`, which the
/// framework updates on push/replace/reset but a bare `pop` leaves to the SDK
/// handler's `active_changed` — and the native stack handlers (macOS/iOS/
/// Android) don't all emit it, so `active_route` can read stale after a pop.
/// `can_go_back` is derived from `depth`, which every backend updates on BOTH
/// push and pop via `depth_changed`, so it stays correct.
///
/// Returns `|| false` when called outside a navigator scope (no ambient
/// navigator).
pub fn use_can_go_back() -> impl Fn() -> bool + 'static {
    // Capture the `can_go_back` signal at use-time — cheap `Rc` clone that
    // outlives the `NavigatorControl`, same as [`use_focus`].
    let sig = ambient_navigator()
        .and_then(|n| n.nav_state.borrow().as_ref().map(|s| s.can_go_back));
    move || match sig {
        Some(s) => s.get(),
        None => false,
    }
}

// ---------------------------------------------------------------------------
// Headless initial-path override (server-side rendering).
//
// A backend rendering headlessly at a specific URL (the SSR backend
// emitting "/about") sets this before `mount`. The navigator walker's
// initial mount consults it once: if the path resolves to a registered
// route, that screen is mounted instead of the hardcoded `initial`, and
// the nav-state is synced so any chrome reads the right route. `take`
// semantics mean the first (root) navigator consumes it — a nested
// navigator won't re-apply the same path.
//
// Live backends (web/iOS/Android) never set this; they read the current
// path from their own platform (window.location, deep-link intent) in
// the SDK handler layer.
// ---------------------------------------------------------------------------

thread_local! {
    static INITIAL_PATH: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Set the path the next headlessly-mounted navigator should open to.
/// Pass `None` to clear. See module note above.
pub fn set_initial_path(path: Option<String>) {
    INITIAL_PATH.with(|p| *p.borrow_mut() = path);
}

/// Consume the headless initial-path override, if any. Called by the
/// navigator walker at initial mount.
pub fn take_initial_path() -> Option<String> {
    INITIAL_PATH.with(|p| p.borrow_mut().take())
}

/// Non-consuming PEEK of the headless initial-path override. Unlike
/// [`take_initial_path`], this clones and leaves the slot intact so that
/// EACH navigator in a synchronous (native/SSR) initial-mount cascade can
/// independently consult the same full deep-link URL and strip its own
/// base. The root navigator (detected via `current_nav_base().is_empty()`)
/// clears the slot with `set_initial_path(None)` once its whole subtree —
/// including any nested navigators — has finished mounting.
pub fn peek_initial_path() -> Option<String> {
    INITIAL_PATH.with(|p| p.borrow().clone())
}

// ---------------------------------------------------------------------------
// Route collector — SSG nav-hierarchy discovery
// ---------------------------------------------------------------------------
//
// The SSG driver (in `backend-ssr`) enables this before each
// `render_path` call. Every `Element::Navigator` the walker dispatches
// publishes its `RouteEntry.path` set to the collector. After mount,
// the driver drains discovered paths, queues unrendered literals, and
// loops — so nested navigators (a drawer with a stack inside) get their
// routes harvested when the parent screen mounts.
//
// Live backends never enable this. The hook is a single `with_collector`
// check in `dispatch_navigator`; when no collector is set the call is a
// thread-local borrow + branch, no allocation.

thread_local! {
    static ROUTE_COLLECTOR: RefCell<Option<Vec<&'static str>>> =
        const { RefCell::new(None) };
}

/// Enable the route collector. SSG calls this before each `render_path`
/// to harvest every navigator's screen paths during mount.
pub fn enable_route_collector() {
    ROUTE_COLLECTOR.with(|c| {
        let mut slot = c.borrow_mut();
        if slot.is_none() {
            *slot = Some(Vec::new());
        }
    });
}

/// Disable the collector and return everything pushed since enable.
/// Returns `None` if the collector wasn't enabled.
pub fn take_route_collector() -> Option<Vec<&'static str>> {
    ROUTE_COLLECTOR.with(|c| c.borrow_mut().take())
}


/// Path-iterator form of [`record_routes`] — the hook the NEW core's
/// vocabulary navigator handlers fire at mount (their `NavConfig` is a
/// vocabulary type this old-core module can't name, but the collector
/// and the crawl driver are shared across both cores, so both publish
/// through this one thread-local). A no-op when the collector is off.
pub fn record_route_paths<I: IntoIterator<Item = &'static str>>(paths: I) {
    ROUTE_COLLECTOR.with(|c| {
        if let Some(buf) = c.borrow_mut().as_mut() {
            buf.extend(paths);
        }
    });
}

// ---------------------------------------------------------------------------
// NavState — reactive bundle exposed to layout / chrome
// ---------------------------------------------------------------------------

/// Reactive nav-state mirror. Updated by `NavigatorControl::dispatch`
/// on every command commit (and by SDK handlers via
/// `host.depth_changed` / `host.active_changed` for asynchronous
/// state changes the framework can't see, like native back gestures).
#[derive(Clone)]
pub struct NavState {
    pub active_route: crate::Signal<&'static str>,
    pub active_path: crate::Signal<String>,
    pub depth: crate::Signal<usize>,
    pub can_go_back: crate::Signal<bool>,
}


/// The default sizing an outlet-model navigator handler applies to its root
/// view at `init`: fill the container. Without it the bare root view hugs
/// content, so an app whose root is a navigator renders collapsed instead of
/// filling the viewport (web `#app` and native window roots size CHILDREN
/// that ask for space; they don't force it). This is the outlet-model
/// counterpart of the class-styled tab/drawer navigators' `.ui-nav-root {
/// width/height: 100% }` rule (and of the stack's
/// [`stack_container_rules`]), expressed as backend-neutral `StyleRules` so
/// every backend gets the same behavior. `flex-grow: 1` + `min-height: 0` additionally let
/// a navigator share a flex column with author chrome (header above, bar
/// below) by absorbing the remaining space.
///
/// Authors override by styling the navigator element itself
/// (`.with_style(...)` on the SDK builder) — the walker applies that style
/// AFTER `init`, onto the same root node, so it wins.
///
/// The explicit `flex_direction: Column` is **load-bearing on web**: the CSS
/// emitter only promotes a class to `display: flex` when the rules carry a
/// flex-CONTAINER property (`rules_to_css`), and everything else here is a
/// size/item property. Without it the navigator root lowers to a plain block
/// `div`, so the outlet's `flex: 1 1 0` inside it is inert and the outlet
/// collapses to content height ("the board area renders as an empty void").
/// Native backends are unaffected — taffy's default display is already flex
/// column, which is exactly why the collapse only reproduced on web.
pub fn navigator_fill_rules() -> Rc<crate::style::StyleRules> {
    use crate::style::Length;
    Rc::new(crate::style::StyleRules {
        width: Some(Length::Percent(100.0).into()),
        height: Some(Length::Percent(100.0).into()),
        flex_direction: Some(crate::style::FlexDirection::Column),
        flex_grow: Some(1.0.into()),
        min_height: Some(Length::Px(0.0).into()),
        ..Default::default()
    })
}

/// The default sizing of an author-splatted `{nav.outlet}` that carries no
/// explicit style: a **bounded, fillable flex region**. Screens assume they
/// can fill their container (`flex: 1`, `min-height: 100%`, scroll views
/// needing a bounded height), so a bare outlet that hugs content breaks the
/// zero-config path — every author had to hand-plumb `flex: 1 1 0` +
/// `min-height: 0` onto it before anything scrolled or filled correctly.
///
/// `flex: 1 1 0` absorbs the remaining space of the author's layout column
/// (after bars/headers); `min-height/min-width: 0` keeps a tall screen from
/// blowing the column open instead of scrolling; the explicit column
/// direction makes the contract visible rather than inherited.
///
/// Opt out by styling the outlet itself: `ctx.outlet.with_style(...)` (the
/// walker uses the author style INSTEAD of this default when one is set).
pub fn outlet_fill_rules() -> crate::style::StyleRules {
    use crate::style::Length;
    crate::style::StyleRules {
        flex_direction: Some(crate::style::FlexDirection::Column),
        flex_grow: Some(1.0.into()),
        flex_shrink: Some(1.0.into()),
        flex_basis: Some(Length::Px(0.0).into()),
        min_height: Some(Length::Px(0.0).into()),
        min_width: Some(Length::Px(0.0).into()),
        ..Default::default()
    }
}

/// The stack navigator's default container style — the app shell fills
/// its parent box on every backend (the iOS/Android handlers materialize
/// the container's Taffy node from the navigator element's style ALONE,
/// so `None` collapses to 0 and renders blank), and `position: relative`
/// makes the container the containing block the
/// [`stack_screen_fill_rules`] absolute screen placement resolves
/// against on web/SSR. `Relative` is already the framework's default
/// `Position`, so declaring it is a no-op for native layout — it only
/// matters where unset position lowers to CSS `static`.
///
/// This is the single source of the container's styling: the stack SDK
/// installs it as the `Element::Navigator` default style (when the
/// author sets none), the walker applies it through the normal style
/// pipeline, and web + SSR therefore resolve the identical
/// content-hashed class — the replacement for the previously injected
/// `.ui-nav-root { … }` class rule. An author `.with_style(...)` on the
/// navigator element replaces it wholesale (same contract as before);
/// a restyled container should re-declare its size and position.
pub fn stack_container_rules() -> Rc<crate::style::StyleRules> {
    use crate::style::Length;
    Rc::new(crate::style::StyleRules {
        position: Some(crate::style::Position::Relative),
        width: Some(Length::Percent(100.0).into()),
        height: Some(Length::Percent(100.0).into()),
        flex_grow: Some(1.0f32.into()),
        ..Default::default()
    })
}

/// Full-bleed placement for a screen mounted directly into the stack
/// container (no author layout): absolute, pinned to all four edges of
/// the [`stack_container_rules`] box. Handlers request it via
/// [`NavigatorHost::set_screen_style_overlay`](super::host::NavigatorHost::set_screen_style_overlay),
/// which layers it onto each screen root's style **override** layer —
/// the style-system replacement for the injected
/// `.ui-nav-screen { position:absolute!important; inset:0!important }`
/// class: the override layer resolves last, so the pin wins over the
/// screen's own position rules deterministically instead of via CSS
/// `!important` against stylesheet source order.
///
/// No width/height on purpose: with all four edges pinned and auto
/// size, the box stretches to the container — and a screen that sets
/// its own width/height keeps it (matching the legacy rule, whose
/// `width/height:100%` was NOT `!important` precisely so authors could
/// override it).
pub fn stack_screen_fill_rules() -> Rc<crate::style::StyleRules> {
    use crate::style::Length;
    Rc::new(crate::style::StyleRules {
        position: Some(crate::style::Position::Absolute),
        top: Some(Length::Px(0.0).into()),
        right: Some(Length::Px(0.0).into()),
        bottom: Some(Length::Px(0.0).into()),
        left: Some(Length::Px(0.0).into()),
        ..Default::default()
    })
}

/// Flow-fill placement for a screen mounted as the outlet's sole child (the
/// outlet-model handlers' `clear_children` + `insert_node` swap): stretch to
/// exactly the outlet's box — `flex: 1 1 0` fills when the content is
/// smaller AND pins the screen to the outlet height when it's taller (its
/// own scroll surfaces then scroll), `min-height: 0` keeps tall content from
/// blowing the column open, `width: 100%` covers the cross axis. This is the
/// outlet-model successor of the legacy `.ui-nav-screen { width/height:100% }`
/// class: without it a screen that sizes itself with `flex_grow` (a canvas
/// board, a fill-the-viewport editor) collapses to content height inside the
/// outlet. Applied through the screen root's style OVERRIDE layer
/// ([`NavigatorHost::set_screen_style_overlay`](super::host::NavigatorHost::set_screen_style_overlay)),
/// so it composes with the screen's own styles and wins deterministically.
pub fn screen_flow_fill_rules() -> Rc<crate::style::StyleRules> {
    use crate::style::Length;
    Rc::new(crate::style::StyleRules {
        display: Some(crate::DisplayKind::Flex),
        width: Some(Length::Percent(100.0).into()),
        flex_grow: Some(1.0.into()),
        flex_shrink: Some(1.0.into()),
        flex_basis: Some(Length::Px(0.0).into()),
        min_height: Some(Length::Px(0.0).into()),
        ..Default::default()
    })
}

/// Structural marker class for SSR-hydration adoption of a stack
/// navigator container. Carries **no styling** — the container's visual
/// rules ride its normal `apply_style` class — it exists only so the
/// hydrating web client can locate the server-rendered container node
/// (`WebBackend::hydrate_adopt_container`) regardless of what styled
/// class the container resolved to. The SSR stack chrome handler stamps
/// it via `Backend::attach_html_class`; the live client never renders
/// it into fresh DOM.
pub const NAV_ROOT_HYDRATION_CLASS: &str = "idealyst-nav-root";

/// Structural marker attribute an SSR/SSG document carries on every
/// navigator OUTLET node: `data-iy-nav-outlet="<navigator base>"`
/// (`""` for the root navigator, `"/settings"` for one nested under
/// that prefix). Stamped server-side via
/// `LifecycleOps::annotate_nav_outlet`; consumed client-side by
/// `LifecycleOps::hydrate_nav_screen_begin`.
///
/// Why it exists: the navigator handlers realize the initial SCREEN
/// *before* the author layout builds the outlet (the mount-order
/// contract — screens must peek the launch URL before chrome), but the
/// server DOCUMENT nests the screen *inside* the outlet. Hydration
/// adopts in `create_*` order, so without a cursor jump the screen
/// build consumes the outlet's node and the whole subtree shifts one
/// level — the `[hydrate] SSR/client diverge` remount cascade. The
/// marker lets the hydrating client find where the server put the
/// screen and steer the adoption cursor there for the screen build.
/// The base is the value so nested navigators resolve their OWN outlet
/// (a nested navigator's base strictly extends its parent's, and
/// `querySelector` under the navigator's adopted root sees only its
/// subtree).
pub const NAV_OUTLET_HYDRATION_ATTR: &str = "data-iy-nav-outlet";



/// A header-bar button: an icon or a text label plus a tap handler. Used by the
/// stack navigator's per-screen `header_left` / `header_right` slots. Maps to a
/// native `UIBarButtonItem` / Android menu item on mobile, and renders in the
/// web/desktop `StackHeader`. Lives in the framework (not the stack SDK) so the
/// UI chrome crate can render it without depending on the SDK.
#[derive(Clone, Default)]
pub struct HeaderButton {
    /// Icon name (resolved against the framework icon registry). Takes
    /// precedence over `label` when both are set.
    pub icon: Option<String>,
    /// Text label (used when no `icon`).
    pub label: Option<String>,
    /// Tap handler.
    pub on_press: Option<Rc<dyn Fn()>>,
}

impl HeaderButton {
    /// An icon button.
    pub fn icon(name: impl Into<String>) -> Self {
        Self { icon: Some(name.into()), label: None, on_press: None }
    }
    /// A text button.
    pub fn text(label: impl Into<String>) -> Self {
        Self { icon: None, label: Some(label.into()), on_press: None }
    }
    /// Attach the tap handler.
    pub fn on_press<F: Fn() + 'static>(mut self, f: F) -> Self {
        self.on_press = Some(Rc::new(f));
        self
    }
}

/// The ACTIVE screen's header slots — the payload the stack navigator stores in
/// [`NavigatorHost::screen_chrome`](super::host::NavigatorHost::screen_chrome)
/// on every navigation. The native bar (mobile) or an author `StackHeader`
/// (web/desktop) renders from it.
#[derive(Clone, Default)]
pub struct StackHeaderState {
    /// The screen title.
    pub title: String,
    /// Leading slot.
    pub left: Option<HeaderButton>,
    /// Trailing slot.
    pub right: Option<HeaderButton>,
    /// When `true`, no header for this screen (native bar hidden / author
    /// header renders nothing).
    pub hidden: bool,
    /// `true` when a NATIVE bar is rendering this header (mobile). An author
    /// `StackHeader` reads it and renders nothing, avoiding a double header.
    pub native: bool,
}

/// Per-screen navigation context, `provide`d into each screen's scope by
/// `mount_screen` and `inject`ed by the portal build path (`walker::portal`).
///
/// A portal (modal / popover / tooltip / its click-away catcher) escapes its
/// screen's view tree to mount on the window, so it doesn't get detached when
/// the navigator swaps screens — and with a persistent `MountPolicy` the
/// screen's scope (hence the portal) stays alive across navigation. Without
/// this, an overlay opened on screen A keeps floating over screen B. The
/// portal builder installs an `Effect` that hides the portal whenever
/// `active_route != route` (its owning screen isn't the active one) and shows
/// it again on return — reactive, so it tracks every navigation without the
/// navigator imperatively reaching into portal views. `inject` resolves the
/// NEAREST navigator, so a screen's overlays follow that screen's own
/// navigator's active route.
#[derive(Clone)]
pub struct ScreenNav {
    pub active_route: crate::Signal<&'static str>,
    pub route: &'static str,
}

// ---------------------------------------------------------------------------
// NavigatorConfig — shared, kind-agnostic routing config
// ---------------------------------------------------------------------------



#[cfg(test)]
mod nav_state_lifetime_tests {
    //! Regression: a navigator's `nav_state` signals must outlive the
    //! *transient* scope it was built in.
    //!
    //! A nested navigator (e.g. a stack hung under a drawer screen, reached
    //! via a sidebar `on_select`) is built inside a short-lived
    //! dispatch/microtask scope. The walker creates `nav_state` in a DEDICATED
    //! scope retained on the long-lived `NavigatorControl` (an `Rc`) rather
    //! than letting the ambient build scope own it. Before that fix, the
    //! ambient scope owned the signals; when it dropped, a later
    //! `active_route.set(...)` from `mount_internal` / `on_popstate` hit a
    //! freed arena slot and panicked — the QuillEMR forward/back nested-stack
    //! crash ("signal used after its scope was dropped" / type mismatch).

    use super::*;
    use crate::reactive::{with_scope, Scope, Signal};

    fn fresh_nav_state() -> NavState {
        NavState {
            active_route: Signal::new("home"),
            active_path: Signal::new("/".to_string()),
            depth: Signal::new(1),
            can_go_back: Signal::new(false),
        }
    }

    /// THE FIX: `nav_state` anchored to the control's retained scope survives
    /// the transient build scope dropping, and stays writable afterwards.
    #[test]
    fn nav_state_survives_transient_build_scope() {
        let control = NavigatorControl::new();

        // Build INSIDE a transient ambient scope, mirroring the walker: the
        // nav_state lives in its own scope handed to the control, never the
        // ambient one.
        let mut ambient = Box::new(Scope::new());
        let nav_state = with_scope(&mut ambient, || {
            let mut nav_scope = Box::new(Scope::new());
            let st = with_scope(&mut nav_scope, fresh_nav_state);
            control.retain_scope(nav_scope);
            st
        });
        control.attach_nav_state(nav_state.clone());

        // The transient build scope drops, as it does after the
        // dispatch/microtask that triggered the nested-nav build returns.
        drop(ambient);

        // Pre-fix this panicked "signal used after its scope was dropped".
        nav_state.active_route.set("detail");
        nav_state.active_path.set("/detail".to_string());
        assert_eq!(nav_state.active_route.get(), "detail");
        assert_eq!(nav_state.active_path.get(), "/detail");

        // Leak-free: dropping the control frees the retained scope (and with
        // it the nav_state signals). Just assert it doesn't panic.
        drop(control);
    }

    /// COUNTER-TEST pinning the bug shape: the OLD layout (nav_state owned
    /// by the ambient build scope) is still wrong once that scope drops —
    /// the write is lost. But generational signal handles have downgraded
    /// it from a process-aborting panic ("signal used after its scope was
    /// dropped" / "type mismatch" → SIGABRT across the JNI boundary) to a
    /// SILENT NO-OP. The real fix is still the dedicated retained scope,
    /// proven by `nav_state_survives_transient_build_scope`; this pins the
    /// new, crash-free behavior so a regression can't reintroduce the
    /// abort.
    #[test]
    fn nav_state_owned_by_build_scope_stale_write_is_noop_not_panic() {
        let mut ambient = Box::new(Scope::new());
        let nav_state = with_scope(&mut ambient, fresh_nav_state);
        drop(ambient); // frees the signals — the old bug
        // Pre-generational-handles this aborted the whole app. Now the
        // stale write is a safe no-op.
        nav_state.active_route.set("detail"); // must NOT panic
        nav_state.depth.set(2); // must NOT panic
    }
}

#[cfg(test)]
mod layout_pass_contract_tests {
    //! The navigator abstraction must schedule a layout pass after EVERY
    //! command, in ONE place — so no navigator×backend handler has to remember
    //! to (the recurring "navigated, but the new screen renders at 0×0" bug;
    //! the Android stack handler forgot it). The walker registers
    //! `|| B::schedule_layout_pass()` as the request-layout hook; this proves
    //! `dispatch` invokes it for every command shape, and that a backend which
    //! opts out (default no-op) is safe.
    use super::*;
    use std::cell::Cell;

    #[test]
    fn dispatch_requests_a_layout_pass_for_every_command() {
        let control = NavigatorControl::new();
        let count = Rc::new(Cell::new(0u32));
        control.install(Box::new(|_cmd| {})); // SDK handler: no-op
        let c = count.clone();
        control.install_request_layout(Box::new(move || c.set(c.get() + 1)));

        control.dispatch(NavCommand::Push {
            name: "a",
            url: "/a".into(),
            params: Box::new(()),
            query: QueryParams::new(),
        });
        control.dispatch(NavCommand::Pop);
        control.dispatch(NavCommand::Replace {
            name: "b",
            url: "/b".into(),
            params: Box::new(()),
            query: QueryParams::new(),
        });
        control.dispatch(NavCommand::Reset {
            name: "c",
            url: "/c".into(),
            params: Box::new(()),
            query: QueryParams::new(),
        });
        control.dispatch(NavCommand::Select {
            name: "d",
            url: "/d".into(),
            params: Box::new(()),
            query: QueryParams::new(),
        });
        control.dispatch(NavCommand::Custom(Rc::new(())));

        assert_eq!(
            count.get(),
            6,
            "every NavCommand must trigger exactly one centralized layout-pass request"
        );
    }

    #[test]
    fn no_hook_registered_is_a_safe_noop() {
        // A backend that re-layouts automatically (web reflow) never registers
        // the hook — `dispatch` must not panic when it's absent.
        let control = NavigatorControl::new();
        control.install(Box::new(|_cmd| {}));
        control.dispatch(NavCommand::Pop);
    }
}

#[cfg(test)]
mod navigation_batch_tests {
    //! Regression: one navigation = ONE chrome fan-out. `dispatch` writes the
    //! route/path mirror, and the SDK handler re-writes the SAME two signals via
    //! its `active_changed` callback after committing the swap (macOS/iOS drawer
    //! + stack). `set` always notifies, so unbatched those duplicate writes woke
    //! chrome subscribers (sidebar active-state derives, header route switch)
    //! twice per navigation — and on macOS the reactive-idle layout hook turned
    //! each wake into a full-tree layout pass (a chunk of the navigation
    //! multi-pass lag). `dispatch` now batches the writes + SDK swap, so a signal
    //! written twice in the window wakes its subscribers once.
    use super::*;
    use crate::reactive::{Effect, Signal};
    use std::cell::Cell;

    #[test]
    fn one_navigation_wakes_route_subscribers_once_despite_duplicate_writes() {
        let control = NavigatorControl::new();
        let nav_state = NavState {
            active_route: Signal::new("home"),
            active_path: Signal::new("/".to_string()),
            depth: Signal::new(1),
            can_go_back: Signal::new(false),
        };
        control.attach_nav_state(nav_state.clone());

        // A chrome subscriber: re-runs whenever `active_route` fans out. Held in
        // `_eff` so it isn't dropped (which would unsubscribe).
        let runs = Rc::new(Cell::new(0u32));
        let r = runs.clone();
        let route = nav_state.active_route;
        let _eff = Effect::new(move || {
            let _ = route.get();
            r.set(r.get() + 1);
        });
        assert_eq!(runs.get(), 1, "subscriber runs once on creation");

        // SDK handler mirrors a backend dispatcher re-setting the same signals
        // via `active_changed` after committing the swap.
        let st = nav_state.clone();
        control.install(Box::new(move |cmd| {
            if let NavCommand::Select { name, url, .. } = &cmd {
                st.active_route.set(name);
                st.active_path.set(url.clone());
            }
        }));

        runs.set(0);
        control.dispatch(NavCommand::Select {
            name: "detail",
            url: "/detail".into(),
            params: Box::new(()),
            query: QueryParams::new(),
        });

        // Pre-fix (unbatched): the pre-dispatch write AND the handler's
        // `active_changed` write each fanned out → 2 subscriber runs → 2 macOS
        // full-tree passes. Batched: the route signal, written twice in one
        // window, wakes its subscriber exactly once.
        assert_eq!(
            runs.get(),
            1,
            "duplicate route writes within one navigation must coalesce to a \
             single subscriber wake"
        );
        assert_eq!(nav_state.active_route.get(), "detail", "value still updates");
    }
}

#[cfg(test)]
mod use_can_go_back_tests {
    //! Regression: `use_can_go_back` must track the `depth`-derived
    //! `can_go_back` signal — which every backend updates on push AND pop via
    //! `depth_changed` — NOT `active_route`, which native stack handlers leave
    //! stale after a bare `pop`. The whiteboard-demo gates its capture-excluded
    //! board chrome on this: a stale read would leave the toolbar hidden forever
    //! after returning from a pushed screen ("the private layer goes missing").

    use super::*;
    use crate::reactive::{with_scope, Scope};

    fn control_with_state() -> (Rc<NavigatorControl>, NavState, Box<Scope>) {
        let control = Rc::new(NavigatorControl::new());
        let mut nav_scope = Box::new(Scope::new());
        let nav_state = with_scope(&mut nav_scope, || NavState {
            active_route: crate::Signal::new("board"),
            active_path: crate::Signal::new("/".to_string()),
            depth: crate::Signal::new(1),
            can_go_back: crate::Signal::new(false),
        });
        control.attach_nav_state(nav_state.clone());
        (control, nav_state, nav_scope)
    }

    #[test]
    fn tracks_can_go_back_across_push_and_pop() {
        let (control, nav_state, _scope) = control_with_state();
        let _guard = AmbientNavGuard::push(control.clone());

        let can_go_back = use_can_go_back();
        // At the stack root: nothing to pop back to.
        assert!(!can_go_back(), "root screen: can_go_back is false");

        // Push a screen (depth 2): now there's a back target.
        nav_state.depth.set(2);
        nav_state.can_go_back.set(true);
        assert!(can_go_back(), "after push: can_go_back is true");

        // Pop back to the root (depth 1). This is the case `active_route` would
        // read stale on native handlers — `can_go_back` must flip back.
        nav_state.depth.set(1);
        nav_state.can_go_back.set(false);
        assert!(!can_go_back(), "after pop to root: can_go_back is false again");
    }

    #[test]
    fn false_without_an_ambient_navigator() {
        // No `AmbientNavGuard` in scope → no navigator → reads false.
        let can_go_back = use_can_go_back();
        assert!(!can_go_back());
    }
}
