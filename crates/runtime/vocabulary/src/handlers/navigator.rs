//! Navigator handlers: `swap_navigator`, `stack_navigator`, and the
//! `navigator_outlet` — the port of `walker/navigator.rs` plus the two
//! backend-neutral SDK handlers (`swap-navigator`, `stack-navigator`)
//! onto the new-core mount contract.
//!
//! # Architecture (vs the old core)
//!
//! The old path was three-layered: walker substrate → per-backend
//! `NavigatorRegistry` → SDK handler, with a `NavigatorHost` bundle of
//! backend-erased closures and a `schedule_microtask` deferral for the
//! author-layout build (which couldn't run inside the `create_navigator`
//! `&mut B` borrow). The new mount contract removes the borrow window —
//! handlers hold `Rc<RefCell<H>>` and borrow per call — so the layout
//! builds **synchronously inside the mount handler**, and the whole
//! navigator is one ordinary registry handler. The mount ORDER still
//! mirrors the old walker exactly (root → initial screen → author style
//! → chrome/outlet → seat), so the parity goldens match byte-for-byte.
//!
//! # Dispatch runs on the world's flush (handler-safety invariant)
//!
//! Event handlers run OUTSIDE `World::enter`, and realizing a screen
//! creates effects (which needs the ambient world). So `on_select` /
//! `pop` / `NavHandle::dispatch` never mount anything directly: they
//! push the command into a plain queue and bump a tick signal — both
//! handle-routed, safe anywhere — and a **driver effect** (created at
//! mount, owned by the navigator's `Realized`) drains the queue inside
//! the flush, where realize/effect creation is legal. This also gives
//! "one navigation = one logical update" for free (the staged-commit
//! model): the route-mirror writes and the structural swap commit in the
//! same flush.
//!
//! # Screen lifecycle = `Realized` retention
//!
//! A mounted screen is `(root node, Realized)`. Persistent policies keep
//! the pair in the navigator's cache while the node is DETACHED from the
//! tree (`clear_children` on the outlet removes it); returning re-inserts
//! the SAME node — the route builder does not re-run, row state survives.
//! Dropping the `Realized` (LazyDisposing evict, stack pop/replace/reset,
//! navigator teardown) is the entire screen teardown: effects die,
//! cleanups fire. Detached retained screens hold no layout/tree
//! membership — the old macOS bug where cached persistent screens were
//! re-framed on every apply-frames pass cannot recur from this layer,
//! because a cached screen simply isn't reachable from any live root.
//!
//! "Cleanups fire" covers the author's too, and that is a guarantee the
//! kernel provides rather than something this module arranges. A screen
//! is realized from two different places — the initial one inline in the
//! mount handler, every later one from the driver effect — and neither
//! context may become the screen's cleanup anchor. `component_scope` and
//! the mount walk run `runtime_world::unanchored`, so an author's
//! `on_scope_drop` / scoped timer / resource in a screen body belongs to
//! the screen's own `Owned` in both. Without that the seated screen
//! anchored to whatever effect mounted the navigator (a `when` gate's
//! driver, which never re-runs ⇒ its registrations leaked for the
//! navigator's whole life) and a selected screen anchored to the driver
//! effect (⇒ fired on the NEXT navigation, retracting a retained
//! screen's registration while it was still mounted). Regressions:
//! `regression_swap_seated_screen_under_a_reactive_region_fires_its_teardown`,
//! `regression_swap_cached_screen_keeps_its_claim_across_navigation`,
//! `regression_stack_screen_claims_track_the_stack_not_the_driver`.
//!
//! # URL sync (conformance wave)
//!
//! Platform-URL synchronization is a SEAM here, not an implementation:
//! a URL-bearing host installs a [`url_sync::UrlSyncService`] and both
//! navigators register with it at mount, hook every dispatch
//! (`before_command`, history writes while the outlet still shows the
//! outgoing screen) and every driver commit (`after_commit`, scroll
//! restore/reset). The web implementation lives in backend-web's
//! `newcore_url_sync` (installed by `newcore::start`); hosts without
//! URLs install nothing and the hooks vanish. Cold-start deep links
//! need no seam: the handlers already `peek_initial_path()`, which the
//! web boot seeds from `location.pathname` — `defer_initial_mount` is
//! unnecessary on the new core because the seed happens before the
//! synchronous mount.
//!
//! A navigator that mounts LATER does need the seam, and takes it
//! through [`url_sync::UrlSyncService::current_url`]: the launch slot is
//! one-shot (the root clears it once its subtree is up), so a rebuilt
//! subtree would otherwise have no way to ask what the address bar says
//! at the moment IT mounts. See [`resolve_initial`] for the two sources
//! and the boundary between them.
//!
//! # What is intentionally NOT ported here (each returns with its phase)
//!
//! - Native system-back routing (`on_system_back`) and the iOS/Android
//!   native push surfaces — P4/P5 backend work.
//! - ~~Robot nav registry / back-stack snapshots~~ — LANDED (P5 robot
//!   remainder): robot builds register each navigator into
//!   `crate::robot`'s nav registry (element-registry entry +
//!   snapshot-closure over the mirror signals / back-stack, dispatch
//!   marks "current", teardown deregisters); `list_navigators` /
//!   `get_navigator_state` serve wire-identical JSON. `type_name` is
//!   the builder's `.nav_label(...)` (the SDK sets its old
//!   presentation type name — wire parity), falling back to the
//!   vocabulary builder name (`"swap_navigator"`/`"stack_navigator"`)
//!   for bare vocabulary mounts.
//! - ~~Stack per-screen header options~~ — LANDED (P6 SDK retarget):
//!   the screen contract is [`crate::prims::Screen`] (`Element` +
//!   opaque options, `Into<Screen>` keeps bare-`Element` builders
//!   source-compatible); the stack publishes the ACTIVE screen's
//!   options into [`StackNav::screen_chrome`] as a rev-stamped
//!   [`ScreenChrome`] on every navigation (the old `set_always`
//!   republish contract), and the stack SDK's `header_state` downcasts
//!   to `StackScreenOptions` → `StackHeaderState`.
//! - ~~`link(route = …)`~~ — LANDED (P6): both navigators `provide` a
//!   [`LinkActivator`] (swap = `Select`, stack = `Push`) around every
//!   screen build AND the author-layout build; the vocabulary link
//!   handler captures it at mount (`prims::RouteLink`). Same
//!   `Dyn`-rebuild recapture limitation as `ScreenNav` (world context
//!   has no scoping).
//! - `ScreenNav` portal-hiding context — re-lands with the portal
//!   handler port (the portal agent owns that seam).

use std::any::Any;
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

use runtime_shared::primitives::navigator::{
    consumed_prefix, current_nav_base, join_path, match_pattern, match_prefix,
    navigator_fill_rules, outlet_fill_rules, peek_initial_path, record_route_paths,
    screen_flow_fill_rules, set_initial_path, split_query, NavBaseGuard, NavCommand, QueryParams,
    ScreenRouteGuard, ScreenStateGuard,
};
use runtime_shared::StyleRules;
use runtime_scene::{component_scope, realize, Element, MountCx, Realized, Registry};
use runtime_world::{collect_owned, effect, provide, signal, Owned, Signal};

use crate::caps::{IntrospectionOps, LifecycleOps, ViewOps};
use crate::prims::{
    LinkActivator, MountPolicy, NavConfig, NavHandle, NavScreenEntry, NavigatorOutletPrim,
    PrimCell, ScreenChrome, StackNav, StackNavigatorPrim, StackRetention, SwapNav,
    SwapNavigatorPrim,
};
use crate::style_attach::{attach_style, StyleProp, StyleServices};

/// The capability bundle both navigator handlers need: view creation for
/// the root, the style service for fill rules + author styles, the
/// lifecycle hook for the post-navigation layout pass, and (robot
/// builds) the introspection surface the element-registry registration
/// closes over — bounded unconditionally like every other handler
/// (`mount_view`), since `IntrospectionOps` is data-only defaults.
/// Structural ops come from the `Host` supertrait.
pub trait NavCaps: ViewOps + StyleServices + LifecycleOps + IntrospectionOps {}
impl<T: ViewOps + StyleServices + LifecycleOps + IntrospectionOps> NavCaps for T {}

/// Install the three navigator handlers on `registry`. Called by
/// [`register_builtins`](crate::handlers::register_builtins).
pub fn register_navigator<H: NavCaps + 'static>(registry: &mut Registry<H>) {
    registry.register::<PrimCell<SwapNavigatorPrim>, _>(|cx, p, children| {
        mount_swap_navigator(cx, p.take(), children)
    });
    registry.register::<PrimCell<StackNavigatorPrim>, _>(|cx, p, children| {
        mount_stack_navigator(cx, p.take(), children)
    });
    registry.register::<PrimCell<NavigatorOutletPrim>, _>(|cx, p, children| {
        mount_navigator_outlet(cx, p.take(), children)
    });
}

// ===========================================================================
// Outlet capture — port of the walker's `OutletCaptureGuard`
// ===========================================================================

thread_local! {
    /// Innermost-last stack of outlet-capture cells. Each navigator
    /// pushes a typed cell around its author-layout build; the outlet
    /// handler writes its node into the TOP cell. A stack (not one
    /// slot) so a nested navigator building its own layout inside a
    /// parent's layout captures into its own cell, never the parent's —
    /// the same invariant the old walker's `OUTLET_CAPTURE` pinned.
    static OUTLET_CAPTURE: RefCell<Vec<Box<dyn Any>>> = const { RefCell::new(Vec::new()) };
}

struct OutletCaptureGuard<N: Clone + 'static> {
    cell: Rc<RefCell<Option<N>>>,
}

impl<N: Clone + 'static> OutletCaptureGuard<N> {
    fn push() -> Self {
        let cell: Rc<RefCell<Option<N>>> = Rc::new(RefCell::new(None));
        OUTLET_CAPTURE.with(|s| s.borrow_mut().push(Box::new(cell.clone())));
        OutletCaptureGuard { cell }
    }

    /// The captured outlet node, or `None` if the built layout contained
    /// no `navigator_outlet()` (author forgot to splat it).
    fn take(self) -> Option<N> {
        // `self` drops after this returns, popping the stack.
        self.cell.borrow_mut().take()
    }
}

impl<N: Clone + 'static> Drop for OutletCaptureGuard<N> {
    fn drop(&mut self) {
        OUTLET_CAPTURE.with(|s| {
            s.borrow_mut().pop();
        });
    }
}

fn outlet_capture_record<N: Clone + 'static>(node: &N) {
    OUTLET_CAPTURE.with(|s| {
        if let Some(top) = s.borrow().last() {
            if let Some(cell) = top.downcast_ref::<Rc<RefCell<Option<N>>>>() {
                *cell.borrow_mut() = Some(node.clone());
            }
        }
    });
}

// ===========================================================================
// Style-override fold — port of `Element::with_style_overrides`
// ===========================================================================

/// Fold `rules` onto `prop` as the override layer (overrides win on
/// conflicts, exactly like the old element override layer which
/// resolves last).
fn fold_prop(prop: Option<StyleProp>, rules: &Rc<StyleRules>) -> StyleProp {
    let rules = rules.clone();
    match prop {
        None => StyleProp::Static(rules),
        Some(StyleProp::Static(base)) => {
            StyleProp::Static(Rc::new((*base).clone().merge(&rules)))
        }
        Some(StyleProp::Dynamic(f)) => {
            StyleProp::Dynamic(Box::new(move || Rc::new((*f()).clone().merge(&rules))))
        }
        Some(StyleProp::Sheet(app)) => {
            StyleProp::Sheet(Box::new((*app).with_overrides((*rules).clone())))
        }
        Some(StyleProp::SheetDynamic(f)) => {
            StyleProp::SheetDynamic(Box::new(move || f().with_overrides((*rules).clone())))
        }
        Some(StyleProp::SignalClass(mut spec)) => {
            let inner = spec.compute.clone();
            let rules_for_compute = rules.clone();
            spec.compute = Rc::new(move || inner().with_overrides((*rules_for_compute).clone()));
            StyleProp::SignalClass(spec)
        }
        Some(StyleProp::PremintedDynamic { class_of, overrides }) => {
            StyleProp::PremintedDynamic {
                class_of,
                overrides: Some(match overrides {
                    Some(prev) => Rc::new((*prev).clone().merge(&rules)),
                    None => rules,
                }),
            }
        }
        // The navigator's screen-option fold layers its rules as OVERRIDES.
        // The inline layer passes through untouched: it sits above overrides
        // in the merge order, so folding here must not disturb it.
        Some(StyleProp::Preminted { class, overrides, inline }) => StyleProp::Preminted {
            class,
            overrides: Some(match overrides {
                Some(prev) => Rc::new((*prev).clone().merge(&rules)),
                None => rules,
            }),
            inline,
        },
    }
}

macro_rules! try_fold_payload {
    ($data:expr, $rules:expr, $($prim:ty),+ $(,)?) => {
        $(
            if let Some(cell) = $data.downcast_mut::<PrimCell<$prim>>() {
                cell.with_mut(|p| {
                    p.style = Some(fold_prop(p.style.take(), $rules));
                });
                return;
            }
        )+
    };
}

/// Layer `rules` onto a screen ROOT element's style override layer —
/// the new-core port of `Element::with_style_overrides`, restricted to
/// what `NavigatorHost::set_screen_style_overlay` used it for (the
/// stack's `screen_flow_fill_rules` full-bleed/flow-fill placement).
///
/// The old enum matched on every style-bearing variant; here the payload
/// is type-erased, so the fold enumerates the built-in payload types.
/// A screen whose root is a third-party primitive, a `Dyn` hole, or a
/// `Keyed` list is left untouched (the old core's node-less variants
/// were skipped the same way) — give such screens an `Item` root.
fn fold_style_overrides(element: &mut Element, rules: &Rc<StyleRules>) {
    use crate::prims::{
        ActivityIndicatorPrim, ButtonPrim, IconPrim, ImagePrim, LinkPrim, PressablePrim,
        ScrollViewPrim, SliderPrim, TextAreaPrim, TextInputPrim, TextPrim, TogglePrim, ViewPrim,
    };
    match element {
        Element::Item { data, .. } => {
            try_fold_payload!(
                data, rules,
                ViewPrim, PressablePrim, ScrollViewPrim, TextPrim, ButtonPrim, ImagePrim,
                IconPrim, TogglePrim, SliderPrim, ActivityIndicatorPrim, LinkPrim,
                TextInputPrim, TextAreaPrim, SwapNavigatorPrim, StackNavigatorPrim,
                NavigatorOutletPrim,
            );
        }
        // A component boundary wraps the real root — fold through it.
        Element::Owned { element, .. } => fold_style_overrides(element, rules),
        // Node-less / type-erased roots: skipped (see doc comment).
        _ => {}
    }
}

// ===========================================================================
// Screen mounting
// ===========================================================================

/// Tell the host to free a discarded screen's subtree, then drop the
/// screen.
///
/// Order matters both ways. The host call comes FIRST, while the
/// subtree is still assembled — a host that walks its own children to
/// find what to free (iOS walks `subviews()`) would find nothing after
/// the teardown. And the backend borrow is released BEFORE the drop,
/// because dropping a `LiveScreen` runs author cleanups that may
/// navigate and re-enter these cells — the same borrow hygiene the
/// eviction path has always kept.
fn release_and_drop<H: NavCaps + 'static>(
    backend: &Rc<RefCell<H>>,
    screen: Option<LiveScreen<H::Node>>,
) {
    let Some(screen) = screen else { return };
    if let Ok(mut b) = backend.try_borrow_mut() {
        b.release_subtree(&screen.node);
    }
    drop(screen);
}

/// A mounted screen: its root node + the `Realized` that owns its
/// entire reactive scope + the screen's opaque options (its
/// [`Screen`](crate::prims::Screen) payload — the stack publishes the
/// top's options into `screen_chrome`). Dropping this IS the screen
/// teardown — the `realized` field is never *read*; holding it is the
/// whole point (Realized retention, module docs).
struct LiveScreen<N> {
    /// The screen's `provide`d context (`ScreenNav`, `LinkActivator`).
    /// Declared FIRST so it is retracted before the subtree tears down —
    /// a teardown cleanup that injects then sees the enclosing screen's
    /// context, never this screen's half-dead one.
    ///
    /// Held here rather than bounded to the build window because a
    /// reactive region INSIDE the screen (a `when` holding a `link`, a
    /// modal's portal) rebuilds long after `realize_screen` returns and
    /// must still resolve its screen's nav — while a `provide` from the
    /// driver-effect path has no ambient collector of its own to belong
    /// to. The screen is the correct owner: exactly as long as the
    /// handles the entries carry.
    #[allow(dead_code)]
    ctx_scope: Owned,
    node: N,
    #[allow(dead_code)]
    realized: Realized<N>,
    options: Option<Rc<dyn Any>>,
}

/// Realize one screen: run the route builder inside the screen's own
/// scope (component_scope: untracked, creations collected into the
/// element's `Owned`), with the nav-base / screen-state / screen-route
/// thread-local guards held across BOTH the builder and the realize —
/// the old `mount_screen`'s `with_scope(|| builder() … build())` shape.
///
/// [`crate::prims::ScreenNav`] is `provide`d into a scope THIS SCREEN
/// owns (`LiveScreen::ctx_scope`) so any portal in the subtree can
/// install its hide-when-inactive visibility effect — the old
/// `mount_screen`'s `reactive::provide(ScreenNav …)`.
///
/// The ownership is load-bearing, not tidiness. `ScreenNav` carries
/// `active_route`, a `Copy` handle owned by the NAVIGATOR's scope, and
/// this fn runs both at mount and later from the driver effect — where
/// there is no ambient collector for a plain `provide` to belong to.
/// Unowned, the entry outlives the navigator that a route gate or an
/// auth swap tore down, and the next portal to mount injects it and
/// aborts with `stale-signal-handle` on its first `active_route.get()`.
/// Screen ownership also replaces the old save/restore-`prev` idiom,
/// which could *reinstate* a dead `ScreenNav` by re-providing a snapshot
/// taken before the teardown.
///
/// World context is a per-type stack, not a tree, so with two screens
/// retained at once a `Dyn` region rebuilt LATER injects whichever
/// screen provided most recently rather than its own. That is the old
/// core's `AmbientNavContext` recapture, which rides the P3 driver work
/// — it is a wrong-value limitation now, no longer a crash, because
/// every entry on the stack is backed by live signals.
///
/// Must run with the owning world ambient (mount handlers and driver
/// effects both qualify) — the handler-safety invariant in the module
/// docs.
fn realize_screen<H: NavCaps + 'static>(
    backend: &Rc<RefCell<H>>,
    registry: &Rc<Registry<H>>,
    screens: &Rc<HashMap<&'static str, NavScreenEntry>>,
    screen_path: &str,
    active_route: Signal<&'static str>,
    link_activator: &LinkActivator,
    name: &'static str,
    params: Box<dyn Any>,
    query: QueryParams,
    overlay: Option<&Rc<StyleRules>>,
) -> LiveScreen<H::Node> {
    let entry = screens
        .get(name)
        .unwrap_or_else(|| panic!("navigator: route '{name}' is not registered"));
    // Publish the base prefix for any navigator nested in THIS screen
    // (`current_nav_base()` reads it) — hierarchy port, old `NavBaseGuard`.
    //
    // This is the screen's CONCRETE path (`/projects/p1`), not
    // `join_path(base, entry.path)`: the registered pattern still holds
    // `:placeholder` segments, and a nested navigator composes its base
    // into every URL it emits (`compose_url`). Publishing the pattern
    // made a navigator nested under `/:id` push `/projects/:id/schedule`
    // — a literal `:id` in the address bar, which then resolves back to
    // a project whose id is the string ":id" on reload.
    let _base_guard = NavBaseGuard::push(screen_path.to_string());
    let _state_guard = ScreenStateGuard::push(query);
    let _route_guard = ScreenRouteGuard::push(name);
    // Both provisions are collected into a scope this screen OWNS (it
    // rides in the returned `LiveScreen`), so they die exactly when the
    // screen does — see `LiveScreen::ctx_scope`. The `collect_owned`
    // wraps only the provides: everything the builder creates belongs to
    // `component_scope`'s nested collector below.
    let ((), ctx_scope) = collect_owned(|| {
        provide(crate::prims::ScreenNav {
            active_route: active_route.read_only(),
            route: name,
        });
        // The ambient route-link seam (P6): a `link(route = …)` mounted
        // in THIS screen's subtree resolves to THIS navigator's dispatch
        // — push-vs-select decided by the navigator kind, the old
        // `install_link_activator` contract. Owned by the screen so a
        // nested navigator's screens re-shadow correctly, and so the
        // entry cannot outlive the dispatch signals it closes over.
        provide(link_activator.clone());
    });
    let build = entry.build.clone();
    // `component_scope` wraps one Element; the Screen's options ride
    // out through a side slot (they're plain data, not scope-owned).
    let options_slot: Rc<RefCell<Option<Rc<dyn Any>>>> = Rc::new(RefCell::new(None));
    let mut element = component_scope({
        let options_slot = options_slot.clone();
        move || {
            let crate::prims::Screen { element, options } = build(params);
            *options_slot.borrow_mut() = options;
            element
        }
    });
    let options = options_slot.borrow_mut().take();
    // Handler-requested screen placement (the stack's flow-fill) rides
    // the root element's style OVERRIDE layer, composing with the
    // screen's own styles — old `set_screen_style_overlay` semantics.
    if let Some(rules) = overlay {
        fold_style_overrides(&mut element, rules);
    }
    let realized = realize(backend, registry, element);
    let mut nodes = realized.collect_nodes();
    match nodes.len() {
        1 => LiveScreen {
            ctx_scope,
            node: nodes.pop().expect("len checked"),
            realized,
            options,
        },
        n => panic!(
            "navigator: screen '{name}' must have a single root node (got {n}) — \
             wrap fragment roots in a view"
        ),
    }
}

/// Prefix-resolve `path` against `screens` (base already stripped by the
/// caller via `match_prefix(path, base)`): the route whose relative
/// pattern consumes the MOST segments wins (a specific route beats an
/// index `""`), returning the unconsumed tail for a nested navigator.
/// Port of the walker's `resolve_entry`.
fn resolve_entry(
    screens: &HashMap<&'static str, NavScreenEntry>,
    base: &str,
    path: &str,
) -> Option<(&'static str, Box<dyn Any>, String)> {
    let rel = match_prefix(path, base).map(|(_, rem)| rem)?;
    let mut best: Option<(&'static str, Box<dyn Any>, String, usize)> = None;
    for (name, entry) in screens.iter() {
        if let Some((segs, rem)) = match_prefix(&rel, entry.path) {
            if let Some(params) = (entry.from_segments)(&segs) {
                let pat_len = entry.path.split('/').filter(|s| !s.is_empty()).count();
                let better = best.as_ref().map(|(_, _, _, l)| pat_len > *l).unwrap_or(true);
                if better {
                    best = Some((*name, params, rem, pat_len));
                }
            }
        }
    }
    best.map(|(n, p, r, _)| (n, p, r))
}

/// Full-match `path` against `screens` — the walker's `match_path`,
/// used by the stack to re-mount cold (disposed) entries from their URL.
fn match_path(
    screens: &HashMap<&'static str, NavScreenEntry>,
    base: &str,
    path: &str,
) -> Option<(&'static str, Box<dyn Any>)> {
    let rel = match_prefix(path, base).map(|(_, rem)| rem)?;
    for (name, entry) in screens.iter() {
        if let Some(segs) = match_pattern(&rel, entry.path) {
            if let Some(params) = (entry.from_segments)(&segs) {
                return Some((*name, params));
            }
        }
    }
    None
}

// ===========================================================================
// URL-sync seam — the backend-installed platform-URL service
// ===========================================================================

/// Platform-URL synchronization seam (the new-core counterpart of the
/// old `NavigatorControl::enable_url_sync` opt-in). The handlers are
/// backend-neutral and never touch a URL themselves; a URL-bearing host
/// (backend-web's new-core boot) installs a [`UrlSyncService`] and every
/// navigator mounted afterwards registers with it. Hosts without URLs
/// install nothing and the seam is invisible — exactly the old
/// provider-model behavior (only web ever touched a URL).
pub mod url_sync {
    use super::*;

    /// Which navigator flavor registered — decides how the service
    /// translates a browser-back into commands (`Select` for the
    /// depth-less swap, `Pop`(s) for a stack), the role the old
    /// `build_link_command` played.
    #[derive(Copy, Clone, PartialEq, Eq, Debug)]
    pub enum NavSyncKind {
        Swap,
        Stack,
    }

    /// Coarse post-commit classification — mirror of the old
    /// `url_sync::CommandKind`, computed by the driver BEFORE the
    /// command moves into the commit match.
    #[derive(Copy, Clone, PartialEq, Eq, Debug)]
    pub enum CommittedKind {
        /// Push / Select / Replace / Reset — a fresh screen now shows.
        Forward,
        Pop,
        Other,
    }

    impl CommittedKind {
        pub(super) fn of(cmd: &NavCommand) -> Self {
            match cmd {
                NavCommand::Push { .. }
                | NavCommand::Select { .. }
                | NavCommand::Replace { .. }
                | NavCommand::Reset { .. } => CommittedKind::Forward,
                NavCommand::Pop => CommittedKind::Pop,
                NavCommand::Custom(_) => CommittedKind::Other,
            }
        }
    }

    /// Everything one navigator hands the service at mount.
    pub struct NavSyncRegistration {
        pub kind: NavSyncKind,
        /// This navigator's base prefix ("" for the root).
        pub base: String,
        /// Full hierarchical path of the CONFIGURED initial screen.
        pub initial_full_path: String,
        /// Committed active full path at registration (deep-link aware).
        pub active_path: String,
        /// Back-stack depth at registration (swap: always 1). Lets the
        /// service seed browser history for a cold-start deep link whose
        /// stack synthesized an index entry below.
        pub depth: usize,
        /// This navigator's first screen was resolved from the COLD-START
        /// launch URL.
        ///
        /// Gates the history seed, and ONLY the history seed. A launch
        /// resolution may have reconstructed a back-stack the browser has
        /// no entries for (the page just loaded with one entry), so the
        /// service synthesizes them. A navigator that resolved the LIVE
        /// URL is being rebuilt mid-session, underneath history the
        /// browser already holds — synthesizing there would duplicate
        /// entries the user already has to press Back through.
        pub from_launch_url: bool,
        /// Hierarchical prefix resolver: full path →
        /// `(route, params, unconsumed remainder)`.
        pub resolve_entry:
            Rc<dyn Fn(&str) -> Option<(&'static str, Box<dyn Any>, String)>>,
        /// The handler-safe STAGED dispatch (composes this navigator's
        /// base onto navigator-relative command URLs). Safe to call from
        /// a platform event — commands commit on the next flush.
        pub dispatch: Rc<dyn Fn(NavCommand)>,
        /// The outlet node, type-erased (`Rc<dyn Any>` around the
        /// backend's `Node`). A web service downcasts it to read/write
        /// the outlet's scroll offset for back-restore; `None`able in
        /// spirit — services must tolerate a foreign node type.
        pub outlet: Rc<dyn Any>,
    }

    /// The service a URL-bearing host installs. All methods run on the
    /// UI thread.
    pub trait UrlSyncService {
        /// A navigator mounted. `None` ⇒ the service declines (e.g. no
        /// platform URL surface) and no further hooks fire for it.
        fn register(&self, reg: NavSyncRegistration) -> Option<u64>;
        /// The platform's LIVE URL right now (path + query), or `None`
        /// when this service has no readable URL surface.
        ///
        /// Read by [`resolve_initial`] for a navigator that mounts
        /// AFTER boot, when the launch slot
        /// (`peek_initial_path`) has already been consumed and cleared
        /// by the root. Without it a navigator rebuilt mid-session — the
        /// whole point of [`MountPolicy::LazyDisposing`] — opens on its
        /// CONFIGURED initial route no matter what the address bar says,
        /// so a browser Back into a disposed subtree re-selects the
        /// section and silently drops every segment below it.
        ///
        /// The launch slot cannot serve this: it is a one-shot boot
        /// value, and the comment on the root's `set_initial_path(None)`
        /// names why it must stay one-shot — a later rebuild must not be
        /// poisoned by a STALE path. The live URL is by definition not
        /// stale, which is why it is a second, separately-typed source
        /// rather than a re-seed of the first.
        ///
        /// Default `None` so a service without a URL surface (test
        /// probes, future non-URL hosts) need not implement it; a host
        /// that owns an address bar MUST override it.
        ///
        /// [`MountPolicy::LazyDisposing`]: crate::prims::MountPolicy::LazyDisposing
        fn current_url(&self) -> Option<String> {
            None
        }
        /// Runs at DISPATCH time with the base-composed command, before
        /// it is staged — the outlet still shows the outgoing screen, so
        /// scroll snapshots see it (old `before_command` contract).
        ///
        /// Returns `true` when this dispatch is the service's own echo
        /// (popstate reconciliation): the driver must then SKIP
        /// [`after_commit`] for the command. The old core checked a
        /// RECONCILING flag at commit time, which was synchronous with
        /// dispatch; the new core commits on a later flush, so the flag
        /// would be stale by then — the suppress bit captures it at the
        /// only moment it is valid.
        ///
        /// [`after_commit`]: UrlSyncService::after_commit
        fn before_command(&self, id: u64, cmd: &NavCommand) -> bool;
        /// Runs in the driver right after the handler committed a
        /// command — the outlet shows the new screen (old
        /// `after_command` contract: forward = scroll-to-top, pop =
        /// history bookkeeping + scroll restore).
        fn after_commit(&self, id: u64, kind: CommittedKind);
        /// Navigator teardown.
        fn deregister(&self, id: u64);
    }

    thread_local! {
        static SERVICE: RefCell<Option<Rc<dyn UrlSyncService>>> =
            const { RefCell::new(None) };
    }

    /// Install the host's URL-sync service (idempotent replace). Called
    /// once at boot by a URL-bearing host, BEFORE the app mounts.
    pub fn install_url_sync_service(service: Rc<dyn UrlSyncService>) {
        SERVICE.with(|s| *s.borrow_mut() = Some(service));
    }

    /// Remove the installed service (host teardown / tests).
    pub fn clear_url_sync_service() {
        SERVICE.with(|s| *s.borrow_mut() = None);
    }

    pub(super) fn service() -> Option<Rc<dyn UrlSyncService>> {
        SERVICE.with(|s| s.borrow().clone())
    }
}

/// Native stack transitions — the seam a host uses to drive a real
/// platform navigation container instead of the outlet swap.
///
/// Deliberately shaped like [`url_sync`]: same install-at-boot
/// lifecycle, same `Rc<dyn Any>` node erasure, same "consult if present,
/// behave exactly as before if absent" contract. Both solve the same
/// problem — a capability only some hosts can provide, consulted from a
/// handler that is generic over every host and cannot name their types.
///
/// # Why the stack needs this and the swap does not
///
/// A swap has no depth: "which screen is showing" is the whole story,
/// and `clear_children` + `insert` tells it completely. A stack push and
/// a stack pop produce the *same* outlet content change — the top screen
/// is swapped — and differ only in **direction**. Direction is exactly
/// what `UINavigationController` needs in order to animate, and what the
/// interactive swipe-back gesture needs in order to exist at all. So
/// the single direction-blind reveal this handler used to have could
/// not be the hook. The five directions of [`Reveal`] are.
///
/// # The two-way contract
///
/// Navigation initiated by the app flows *down*: the handler mutates its
/// `Vec<StackEntry>`, then calls the matching handle fn so the presenter
/// animates. Navigation initiated by the *user* — a completed swipe-back
/// or a system Back press — flows *up*: the native container has already
/// popped, so the presenter calls the closure it was given via
/// [`NativePushHandle::set_user_pop`], which pops the logical stack and
/// republishes state WITHOUT calling back down into the presenter. Going
/// back down on a user-initiated pop would double-pop.
pub mod native_push {
    use super::*;

    /// What a presenter hands back: the content host plus the five
    /// direction-tagged reveals the handler drives, plus the reverse
    /// channel for a user-initiated back.
    ///
    /// Every reveal takes the screen node the handler wants visible,
    /// erased to `Rc<dyn Any>` (an `Rc<H::Node>`). A presenter downcasts
    /// to its own node type and ignores anything else.
    pub struct NativePushHandle {
        /// The node the handler should treat as the content host — the
        /// native container the presenter seated inside the outlet.
        pub host: Rc<dyn Any>,
        /// Seat the bottom screen, unanimated.
        pub seat: Rc<dyn Fn(Rc<dyn Any>)>,
        /// Push on top, animated.
        pub push: Rc<dyn Fn(Rc<dyn Any>)>,
        /// Pop the top, animated, revealing the given screen.
        pub pop: Rc<dyn Fn(Rc<dyn Any>)>,
        /// Swap the top in place, unanimated (Replace).
        pub replace: Rc<dyn Fn(Rc<dyn Any>)>,
        /// Collapse to a single screen, unanimated (Reset).
        pub reset: Rc<dyn Fn(Rc<dyn Any>)>,
        /// Install the handler's LOGICAL-ONLY pop. The presenter calls
        /// this when the user completed a swipe-back or hit system Back,
        /// i.e. the native stack ALREADY popped. The handler then pops
        /// its `Vec<StackEntry>`, republishes depth/chrome/active_*, and
        /// notifies url-sync — and must NOT call [`Self::pop`] back into
        /// the presenter.
        pub set_user_pop: Rc<dyn Fn(Rc<dyn Fn()>)>,
    }

    /// The per-host presenter, installed once at boot.
    pub trait StackPresenter {
        /// One-time setup inside the navigator's outlet, before the
        /// initial screen is seated. Return the handle the stack should
        /// drive, or `None` to decline — declining must leave today's
        /// behavior byte-identical.
        ///
        /// `outlet` is an `Rc<H::Node>` erased to `Rc<dyn Any>`. A
        /// presenter MUST return `None` for a foreign node type: one
        /// presenter is installed per process, but a process can host
        /// more than one backend (a runtime-server sidecar beside the
        /// real one), and attaching to a node it cannot drive would
        /// leave that navigator with no reveal path at all.
        fn attach(&self, outlet: Rc<dyn Any>) -> Option<NativePushHandle>;
    }

    thread_local! {
        static PRESENTER: RefCell<Option<Rc<dyn StackPresenter>>> =
            const { RefCell::new(None) };
    }

    /// Install the host's stack presenter (idempotent replace). Called
    /// once at boot, BEFORE the app mounts.
    pub fn install_stack_presenter(presenter: Rc<dyn StackPresenter>) {
        PRESENTER.with(|p| *p.borrow_mut() = Some(presenter));
    }

    /// Remove the installed presenter (host teardown / tests).
    pub fn clear_stack_presenter() {
        PRESENTER.with(|p| *p.borrow_mut() = None);
    }

    pub(super) fn presenter() -> Option<Rc<dyn StackPresenter>> {
        PRESENTER.with(|p| p.borrow().clone())
    }
}

use native_push::NativePushHandle;
use url_sync::{CommittedKind, NavSyncKind, NavSyncRegistration};

/// Which direction a stack reveal is going. A presenter animates on
/// this; the no-presenter path ignores it and performs the same outlet
/// swap for all five (see [`StackShared::reveal`]).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum Reveal {
    /// The bottom screen arriving at mount — never animated.
    Seat,
    Push,
    Pop,
    Replace,
    Reset,
}

/// The per-navigator sync half the dispatch closure and driver share:
/// the service id arrives only after registration (end of mount), while
/// both closures are built earlier — the cell decouples them.
#[derive(Clone, Default)]
struct SyncSlot {
    id: Rc<std::cell::Cell<Option<u64>>>,
}

impl SyncSlot {
    /// Dispatch-time hook. Returns the suppress-after bit (see
    /// [`url_sync::UrlSyncService::before_command`]).
    fn before(&self, cmd: &NavCommand) -> bool {
        match (url_sync::service(), self.id.get()) {
            (Some(svc), Some(id)) => svc.before_command(id, cmd),
            _ => false,
        }
    }

    /// Commit-time hook (driver).
    fn after(&self, kind: CommittedKind) {
        if let (Some(svc), Some(id)) = (url_sync::service(), self.id.get()) {
            svc.after_commit(id, kind);
        }
    }

    /// Register with the installed service (if any) and arm teardown.
    fn register(&self, reg: NavSyncRegistration) {
        if let Some(svc) = url_sync::service() {
            if let Some(id) = svc.register(reg) {
                self.id.set(Some(id));
                crate::style_attach::on_teardown(move || {
                    if let Some(svc) = url_sync::service() {
                        svc.deregister(id);
                    }
                });
            }
        }
    }
}

// ===========================================================================
// Command channel — queue + tick + driver effect
// ===========================================================================

/// The handler-safe dispatch half: commands queue here (plain interior
/// state), and the tick signal wakes the driver on the next flush. The
/// per-command bool is the URL-sync suppress-after bit (`SyncSlot`).
struct CommandChannel {
    queue: Rc<RefCell<VecDeque<(NavCommand, bool)>>>,
    tick: Signal<u64>,
}

impl CommandChannel {
    /// Stage `cmd`. Safe from event handlers (outside `World::enter`):
    /// the queue is plain interior state and `tick.update` is
    /// handle-routed. Two dispatches in one window compose (tick +2 →
    /// one driver wake draining both, in order).
    fn dispatch(&self, cmd: NavCommand, suppress_sync_after: bool) {
        self.queue.borrow_mut().push_back((cmd, suppress_sync_after));
        self.tick.update(|n| n + 1);
    }
}

/// Compose this navigator's base prefix onto a command's
/// (navigator-relative) url — old `NavigatorControl::compose_url`.
fn compose_url(base: &str, cmd: NavCommand) -> NavCommand {
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

/// Mirror a route-carrying command into the active route/path signals
/// BEFORE the handler commits it — the old dispatch's pre-write, so
/// chrome effects and the structural swap land in the same flush.
/// `Pop` carries no route; the driver writes the revealed entry after
/// committing (old `active_changed` contract).
fn mirror_command(
    cmd: &NavCommand,
    route: Signal<&'static str>,
    path: Signal<String>,
    query: Signal<QueryParams>,
) {
    match cmd {
        NavCommand::Push { name, url, query: q, .. }
        | NavCommand::Replace { name, url, query: q, .. }
        | NavCommand::Reset { name, url, query: q, .. }
        | NavCommand::Select { name, url, query: q, .. } => {
            route.set(name);
            path.set(url.clone());
            // Published here rather than inside the handlers' commit paths
            // because a navigation to the SAME path with a DIFFERENT query
            // is deliberately not a remount — the swap's `select` and the
            // stack's dedupe both short-circuit on the path key, so this is
            // the only point that observes every navigation. Screens
            // reacting to their state through `SwapNav::query` /
            // `StackNav::query` update; nothing rebuilds.
            query.set(q.clone());
        }
        NavCommand::Pop | NavCommand::Custom(_) => {}
    }
}

/// What [`resolve_initial`] settled on for one navigator's first screen.
struct InitialResolution {
    route: &'static str,
    params: Box<dyn Any>,
    /// The screen's FULL (base-composed, concrete) path.
    path: String,
    query: QueryParams,
    /// The screen came from the COLD-START launch URL, not the live
    /// address bar or the configured initial. Rides out to the URL-sync
    /// registration, which may only synthesize browser-history entries
    /// for a launch resolution — see `NavSyncRegistration::from_launch_url`.
    from_launch_url: bool,
}

/// Resolve the initial (route, params, full path) for a navigator from
/// the first URL source that resolves against `base`, falling back to
/// the configured initial. Port of the walker's non-deferred
/// initial-mount resolution, including the concrete-path (not pattern)
/// mirror fix.
///
/// Two sources, in order, because a navigator can mount at two very
/// different moments:
///
/// 1. **The launch slot** ([`peek_initial_path`]) — the cold-start
///    deep link (a pasted link, a reload, a restored tab, an OS deep
///    link, SSR hydration). PEEK, not take: every navigator in the
///    mounting cascade strips its own base off the same launch URL, and
///    the ROOT clears the slot once its subtree is up. One-shot by
///    design — a rebuild an hour later must not reopen the URL the page
///    was *launched* on.
/// 2. **The host's LIVE URL**
///    ([`UrlSyncService::current_url`]) — for a navigator that mounts
///    AFTER boot, when the slot is already empty. A `LazyDisposing`
///    section that is disposed and rebuilt (browser Back into it, an
///    auth-signal remount, any other cause) resolves the address bar as
///    it reads *now*. Without this the rebuilt navigator opens on its
///    configured initial and the URL silently stops describing the
///    screen — the same URL renders two different screens depending on
///    how you arrived.
///
///    Consulted ONLY when the live path extends past `base`: a URL that
///    stops at this navigator's base names nothing for this navigator to
///    decide, so the configured initial stands. That keeps this source to
///    exactly the case it exists for — restoring a slice the URL really
///    does carry — and is why source 1 is not simply re-read from here.
///
/// Both carry the query out alongside the route, so a screen restored
/// from the URL gets exactly the state an in-app navigation would have
/// handed it — the second half of the [`ScreenState`] contract.
///
/// [`ScreenState`]: runtime_shared::primitives::navigator::ScreenState
/// [`UrlSyncService::current_url`]: url_sync::UrlSyncService::current_url
fn resolve_initial(config: &NavConfig, base: &str) -> InitialResolution {
    let resolve_from = |raw: Option<String>, from_launch_url: bool| {
        let raw = raw?;
        let (path, query) = split_query(&raw);
        let (route, params, rem) = resolve_entry(&config.screens, base, path)?;
        Some(InitialResolution {
            route,
            params,
            path: consumed_prefix(path, &rem),
            query,
            from_launch_url,
        })
    };
    if let Some(hit) = resolve_from(peek_initial_path(), true) {
        return hit;
    }
    // Second source, read only when the first missed: the live-URL read
    // is a host call (a `window.location` touch on web), and the launch
    // slot answers every cold mount — the overwhelming majority.
    //
    // Gated on the live path carrying a slice BELOW `base`. When the
    // address bar stops exactly AT this navigator's base, the URL has no
    // opinion about which of THIS navigator's screens to show — the
    // parent consumed all of it — and the configured initial is the
    // answer, same as before the live-URL source existed. Without the
    // gate a nested navigator whose initial is a non-index route would
    // switch to its `""` route the moment a parent pushed the bare base,
    // because an empty relative path matches `""`: a silent change to
    // every such navigator, far outside the deep-URL restoration this
    // source exists for. Pinned by
    // `nested_swap_on_a_non_index_initial_route_keeps_its_initial_at_the_bare_base`.
    if let Some(live) = url_sync::service().and_then(|svc| svc.current_url()) {
        let names_a_screen_below_us = match_prefix(split_query(&live).0, base)
            .is_some_and(|(_, rem)| !rem.is_empty());
        if names_a_screen_below_us {
            if let Some(hit) = resolve_from(Some(live), false) {
                return hit;
            }
        }
    }
    // The configured initial path may itself carry defaults
    // (`initial_path: "/inbox?filter=unread"`).
    let (path, query) = split_query(config.initial_path);
    InitialResolution {
        route: config.initial,
        params: Box::new(()),
        path: join_path(base, path),
        query,
        from_launch_url: false,
    }
}

/// Trailing-slash-tolerant URL key (`/docs` == `/docs/`) — the swap
/// cache key normalizer (matches the substrate URL layer's tolerance).
fn url_key(url: &str) -> String {
    let t = url.trim_end_matches('/');
    if t.is_empty() {
        "/".to_string()
    } else {
        t.to_string()
    }
}

// ===========================================================================
// swap_navigator
// ===========================================================================

/// Shared per-swap-navigator state. Owned by the driver effect's closure
/// (plus any `NavHandle`/`on_select` clones), which the navigator's
/// `Realized` owns — dropping the navigator drops this, which drops
/// every cached screen's `Realized` (cleanups fire). No keepalive
/// effect, no `release_navigator` cycle-break: there is no backend-held
/// handler map to cycle through.
struct SwapShared<H: NavCaps + 'static> {
    backend: Rc<RefCell<H>>,
    registry: Rc<Registry<H>>,
    screens: Rc<HashMap<&'static str, NavScreenEntry>>,
    outlet: RefCell<Option<H::Node>>,
    /// Mounted screens keyed by normalized URL — NOT route name: a
    /// parameterized route funnels many screens through one name;
    /// name-keying made same-route selects no-ops and served entry A's
    /// cached screen for entry B (the docs-app catalog bug, ported).
    mounted: RefCell<HashMap<String, LiveScreen<H::Node>>>,
    /// Currently shown `(route name, normalized url)`.
    active: RefCell<Option<(&'static str, String)>>,
    mount_policy: MountPolicy,
    /// The route mirror, captured for `ScreenNav` provision at screen
    /// mount (the dispatch closure owns the write side).
    active_route: Signal<&'static str>,
    /// The ambient route-link seam provided around every screen build
    /// (this navigator's `Select` dispatch). A cell because the
    /// activator closes over `dispatch`, which is built AFTER `shared`
    /// (the robot registration in between needs `shared`); filled
    /// before anything can select.
    link_activator: RefCell<Option<LinkActivator>>,
}

impl<H: NavCaps + 'static> SwapShared<H> {
    /// This navigator's link activator (filled at mount, module docs).
    fn activator(&self) -> LinkActivator {
        self.link_activator
            .borrow()
            .clone()
            .expect("navigator: link activator installed before any screen mounts")
    }

    /// Insert `node` as the outlet's sole child (clearing the prior
    /// screen's node — its `Realized` survives in `mounted` for
    /// persistent policies).
    fn show_in_outlet(&self, node: &H::Node) {
        if let Some(outlet) = self.outlet.borrow().clone() {
            let mut b = self.backend.borrow_mut();
            b.clear_children(&outlet);
            let mut parent = outlet;
            b.insert(&mut parent, node.clone());
        }
    }

    /// Resolve the screen for `name` (reuse the cached `Realized` for
    /// persistent policies — the route builder is NOT re-run — else
    /// mount fresh) and show it. `LazyDisposing` drops the
    /// previously-active screen's `Realized` first.
    fn select(
        &self,
        name: &'static str,
        url: &str,
        params: Box<dyn Any>,
        query: QueryParams,
    ) {
        let key = url_key(url);
        // Already showing this exact URL — no-op. URL (not route-name)
        // comparison is what makes parameterized routes swap.
        if self
            .active
            .borrow()
            .as_ref()
            .is_some_and(|(_, active_url)| *active_url == key)
        {
            return;
        }

        if self.mount_policy == MountPolicy::LazyDisposing {
            // Take the evicted screen OUT of the map and end every
            // borrow before dropping it: the drop runs author cleanups
            // that may navigate (re-entering these cells). Under the
            // new core a re-entrant navigation only stages a command
            // (the driver picks it up next round), but the borrow
            // hygiene is kept anyway — cleanups can also read state.
            let prev_key = self.active.borrow().as_ref().map(|(_, u)| u.clone());
            let evicted = prev_key.and_then(|k| self.mounted.borrow_mut().remove(&k));
            // Screen teardown: effects die, cleanups fire — and the host
            // frees whatever it kept for the subtree. This is the ONLY
            // unambiguous "really gone" for a swap screen; the
            // `clear_children` that detached it is also what a
            // `LazyPersistent` switch-away does.
            release_and_drop(&self.backend, evicted);
        }

        let cached = self
            .mounted
            .borrow()
            .get(&key)
            .map(|live| live.node.clone());
        let node = if let Some(n) = cached {
            n
        } else {
            let live = realize_screen(
                &self.backend,
                &self.registry,
                &self.screens,
                &key,
                self.active_route,
                &self.activator(),
                name,
                params,
                query,
                None,
            );
            let node = live.node.clone();
            self.mounted.borrow_mut().insert(key.clone(), live);
            node
        };
        self.show_in_outlet(&node);
        *self.active.borrow_mut() = Some((name, key));
    }
}

/// Mount a `swap_navigator` — port of `walker/navigator.rs::build` +
/// `swap_navigator::SwapHandler` (one handler, all backends). See the
/// module docs for the mount-order contract.
pub fn mount_swap_navigator<H: NavCaps + 'static>(
    cx: &mut MountCx<'_, H>,
    prim: SwapNavigatorPrim,
    _children: Vec<Element>,
) -> H::Node {
    let backend = cx.backend().clone();
    let registry = cx.registry().clone();
    let base = current_nav_base();

    // Root container + fill-the-container default (a bare root hugs
    // content, collapsing a viewport-height app). The author's style is
    // attached later, onto the same node, so it overrides this — the old
    // init-then-walker-style order.
    let root = backend.borrow_mut().create_view(&prim.a11y);
    backend.borrow_mut().apply_style(&root, &navigator_fill_rules());
    // Identity registration (robot builds): the navigator surfaces as an
    // `ElementKind::Navigator` element whose children are everything
    // mounted inside this handler (initial screen + chrome) — the guard
    // lives to the end of the fn so those mounts link here. Screens
    // mounted LATER by the driver effect register as orphan roots (the
    // old registry's exact behavior for post-walk screen mounts).
    #[cfg(feature = "robot")]
    let robot_reg = crate::robot::register_mount(
        &backend,
        &root,
        crate::robot::ElementKind::Navigator,
        None,
        None,
        None,
        crate::robot::MountActions::default(),
    );

    // SSG route discovery (the new-core leg of `backend_ssr::render_all`):
    // publish this navigator's screen path patterns to the shared
    // route collector — the same hook the old walker's
    // `dispatch_navigator` fires (`record_routes`), so the crawl
    // harvests nested navigators' routes as their parent screen mounts.
    // No-op when no collector is enabled (live backends).
    record_route_paths(prim.config.screens.values().map(|e| e.path));

    // Resolve the initial BEFORE creating the nav-state signals so a
    // cold-start deep link is their *committed* initial value (the new
    // core stages writes; creating-then-setting would leave chrome's
    // first build reading the configured initial).
    let InitialResolution {
        route: initial_route,
        params: initial_params,
        path: initial_path,
        query: initial_query,
        from_launch_url,
    } = resolve_initial(&prim.config, &base);
    // The CONFIGURED initial's full path (vs the resolved, possibly
    // deep-linked `initial_path`) — the URL-sync registration needs both.
    let initial_cfg_full = join_path(&base, split_query(prim.config.initial_path).0);

    // Nav-state mirror. Created inside the handler ⇒ collected into the
    // navigator's Realized ⇒ freed exactly at navigator teardown. This
    // replaces the old dedicated-scope-retained-on-the-control dance
    // (the QuillEMR "signal used after its scope was dropped" fix) — the
    // ownership is now structural.
    let active_route = signal(initial_route);
    let active_path = signal(initial_path.clone());
    let active_query = signal(initial_query.clone());

    let shared = Rc::new(SwapShared {
        backend: backend.clone(),
        registry,
        screens: Rc::new(prim.config.screens),
        outlet: RefCell::new(None),
        mounted: RefCell::new(HashMap::new()),
        active: RefCell::new(None),
        mount_policy: prim.mount_policy,
        active_route,
        link_activator: RefCell::new(None),
    });

    // Robot nav registry (P5): snapshot closure over the mirror signals
    // + a Weak of the shared state (liveness gate, the old registry's
    // dead-`Weak` prune); deregistration rides a teardown probe owned by
    // the navigator's Realized. A swap navigator is depth-less: depth 1,
    // no back, stack = its single active entry.
    #[cfg(feature = "robot")]
    let nav_id = {
        let weak = Rc::downgrade(&shared);
        let base = base.clone();
        let nav_id = crate::robot::register_navigator(
            // Presentation label: the SDK supplies its old presentation
            // type name (wire parity with the old bridge); bare
            // vocabulary mounts fall back to the builder name.
            prim.nav_label.unwrap_or("swap_navigator"),
            Some(robot_reg.id().0),
            Rc::new(move || {
                let _live = weak.upgrade()?;
                let route = active_route.peek().to_string();
                let path = active_path.peek();
                Some(crate::robot::NavSnapshotData {
                    active_route: route.clone(),
                    active_path: path.clone(),
                    depth: 1,
                    can_go_back: false,
                    base: base.clone(),
                    stack: vec![(route, path)],
                })
            }),
        );
        crate::style_attach::on_teardown(move || {
            crate::robot::deregister_navigator(nav_id);
        });
        nav_id
    };

    let channel = Rc::new(CommandChannel {
        queue: Rc::new(RefCell::new(VecDeque::new())),
        tick: signal(0u64),
    });

    // The handler-safe dispatch: compose base, pre-write the mirror,
    // URL-sync before-hook (history write), queue for the driver.
    let sync = SyncSlot::default();
    let dispatch: Rc<dyn Fn(NavCommand)> = {
        let channel = channel.clone();
        let base = base.clone();
        let sync = sync.clone();
        Rc::new(move |cmd| {
            // Last-driven navigator = the inspector's "current".
            #[cfg(feature = "robot")]
            crate::robot::mark_active_navigator(nav_id);
            let cmd = compose_url(&base, cmd);
            mirror_command(&cmd, active_route, active_path, active_query);
            let suppress = sync.before(&cmd);
            channel.dispatch(cmd, suppress);
        })
    };

    // Route links inside this navigator's screens/chrome Select — the
    // swap half of the old `install_select_link_activator` contract
    // ("links switch, never push"). Handler-safe: rides the staged
    // dispatch.
    let link_activator = {
        let dispatch = dispatch.clone();
        LinkActivator::new(move |name, url, params| {
            // A link may target a route whose pattern carries default query
            // params; split them out so routing never sees the `?`.
            let (path, query) = split_query(&url);
            dispatch(NavCommand::Select { name, url: path.to_string(), params, query });
        })
    };
    *shared.link_activator.borrow_mut() = Some(link_activator.clone());

    // Driver effect: drains the queue inside the flush (module docs).
    // Owned by the navigator's Realized via the ambient collector.
    {
        let shared = shared.clone();
        let queue = channel.queue.clone();
        let tick = channel.tick;
        let sync = sync.clone();
        let _driver = effect(move || {
            let _ = tick.get(); // subscribe; first run sees an empty queue
            loop {
                let next = queue.borrow_mut().pop_front();
                let Some((cmd, suppress_sync)) = next else { break };
                let kind = CommittedKind::of(&cmd);
                match cmd {
                    NavCommand::Select { name, url, params, query } => {
                        shared.select(name, &url, params, query);
                    }
                    // Swap navigators have no stack; stray stack verbs
                    // are ignored, never a panic (the old tab-handler
                    // panic regression, ported as a comment-guard).
                    _ => {}
                }
                if !suppress_sync {
                    sync.after(kind);
                }
            }
            // Centralized post-navigation layout guarantee — the old
            // `install_request_layout(|| B::schedule_layout_pass())`.
            H::schedule_layout_pass();
        });
    }

    // Initial screen — mounted BEFORE the author layout builds, matching
    // the old walker (screen ops precede the microtask-deferred chrome).
    // Under SSR hydration the server document nests this screen inside
    // the outlet (built below) — the begin/end pair steers the adoption
    // cursor there and back. See `LifecycleOps::hydrate_nav_screen_begin`.
    backend.borrow_mut().hydrate_nav_screen_begin(&root, &base);
    let initial = realize_screen(
        &shared.backend,
        &shared.registry,
        &shared.screens,
        &initial_path,
        active_route,
        &link_activator,
        initial_route,
        initial_params,
        initial_query,
        None,
    );
    backend.borrow_mut().hydrate_nav_screen_end();
    // Root navigator: the nested subtree mounted synchronously above, so
    // every nested navigator already peeked the launch URL. Clear the
    // slot so a later rebuild isn't poisoned (old walker contract).
    if base.is_empty() {
        set_initial_path(None);
    }

    if let Some(style) = prim.style {
        attach_style(&backend, &root, style);
    }
    if let Some(fill) = prim.on_handle {
        fill(NavHandle::new(dispatch.clone()));
    }

    // Author layout (or a bare outlet). Built synchronously — the new
    // mount contract has no borrow window to defer around — inside the
    // navigator's Realized collector, so author `effect()`s in chrome
    // are owned by the navigator (the old retained-chrome-scope
    // guarantee, now structural). `SwapNav` is provided into the world
    // context for `inject`; the previous value is restored afterwards so
    // nested navigators don't clobber their parent's context.
    let on_select: Rc<dyn Fn(&'static str)> = {
        let dispatch = dispatch.clone();
        let select_args = prim.select_args;
        Rc::new(move |name| {
            // Build the route's REAL url + typed params (recorded by the
            // builder) — `None` ⇒ unregistered or needs path params a
            // bare name can't supply; ignore rather than panic (chrome
            // taps must not crash; use `NavHandle::select` for
            // typed-param routes). The old select-args fix, ported.
            let Some((url, params)) = select_args.get(name).and_then(|build| build()) else {
                return;
            };
            let (path, query) = split_query(&url);
            dispatch(NavCommand::Select { name, url: path.to_string(), params, query });
        })
    };
    // Owned by the navigator's own mount scope — this handler runs inside
    // the navigator's `Realized` collector, so the entry is retracted
    // when the navigator unmounts. That is the lifetime the contents
    // demand (`SwapNav` carries this navigator's `active_route` /
    // `active_path`), and it keeps the context resolvable for chrome that
    // rebuilds reactively after mount. The old save/restore-`prev` idiom
    // left it published FOREVER at the top level, which is how a
    // destroyed navigator's signals stayed reachable through `inject`.
    provide(SwapNav { active_route, active_path, query: active_query, on_select });
    // Chrome links target this navigator too (a nav bar of
    // `link(route = …)`s) — same ownership.
    provide(link_activator.clone());
    let guard = OutletCaptureGuard::<H::Node>::push();
    // `unanchored` for the same reason `realize_screen`'s body build is
    // (module docs): the author's chrome closure is a BUILD, and its
    // teardowns belong to this handler's collector — the navigator's
    // `Realized` — not to whichever effect mounted the navigator. It runs
    // here rather than inside `realize_detached`'s producer because the
    // outlet-capture guard has to bracket the element construction.
    let layout_element = runtime_world::unanchored(|| match &prim.layout {
        Some(f) => f(),
        None => crate::builders::navigator_outlet().build(),
    });
    let (layout_root, chrome) = cx.realize_detached(layout_element);
    let outlet = guard.take();
    debug_assert!(
        outlet.is_some(),
        "swap_navigator: the author layout must splat `navigator_outlet()` exactly once — \
         no outlet was found in the built layout"
    );
    // SSR: stamp the outlet with the hydration marker so the client can
    // steer its adoption cursor (see `hydrate_nav_screen_begin` above).
    if let Some(outlet) = &outlet {
        backend.borrow_mut().annotate_nav_outlet(outlet, &base);
    }

    // Retain the chrome subtree for the navigator's lifetime. Riding on
    // `shared` (which the driver effect owns) keeps teardown one drop.
    {
        let mut parent = root.clone();
        backend.borrow_mut().insert(&mut parent, layout_root);
    }
    *shared.outlet.borrow_mut() = outlet;
    // Seat the initial screen: cache under the RESOLVED url (deep-link
    // aware) and show it — old `seat_initial`.
    let key = url_key(&initial_path);
    shared.show_in_outlet(&initial.node);
    shared.mounted.borrow_mut().insert(key.clone(), initial);
    *shared.active.borrow_mut() = Some((initial_route, key));

    // URL sync: register with the host's service (no-op when none is
    // installed — non-URL platforms). After seat, so the registration's
    // committed state (active path, outlet) is real.
    {
        let resolve = {
            let screens = shared.screens.clone();
            let base = base.clone();
            Rc::new(move |path: &str| resolve_entry(&screens, &base, path))
                as Rc<dyn Fn(&str) -> Option<(&'static str, Box<dyn Any>, String)>>
        };
        let outlet_erased: Rc<dyn Any> = match shared.outlet.borrow().clone() {
            Some(node) => Rc::new(node),
            None => Rc::new(()),
        };
        sync.register(NavSyncRegistration {
            kind: NavSyncKind::Swap,
            base: base.clone(),
            initial_full_path: initial_cfg_full,
            active_path: initial_path.clone(),
            depth: 1,
            from_launch_url,
            resolve_entry: resolve,
            dispatch: dispatch.clone(),
            outlet: outlet_erased,
        });
    }

    // The chrome Realized must live as long as the navigator: park it on
    // a teardown probe owned by the navigator's Realized.
    crate::style_attach::on_teardown(move || drop(chrome));

    root
}

// ===========================================================================
// stack_navigator
// ===========================================================================

/// One back-stack entry. `live: None` is a **cold** entry — a screen the
/// stack knows by URL only (covered under `Rebuild`, or a deep-link
/// parent never actually visited) that `materialize_top` re-mounts from
/// its URL when a pop reveals it.
struct StackEntry<N> {
    route: &'static str,
    path: String,
    /// The navigation's query params, kept so a cold re-mount seeds the
    /// screen with the same state a live mount did.
    query: QueryParams,
    live: Option<LiveScreen<N>>,
}

struct StackShared<H: NavCaps + 'static> {
    backend: Rc<RefCell<H>>,
    registry: Rc<Registry<H>>,
    screens: Rc<HashMap<&'static str, NavScreenEntry>>,
    base: String,
    outlet: RefCell<Option<H::Node>>,
    stack: RefCell<Vec<StackEntry<H::Node>>>,
    /// A `Cell` because a successful presenter attach TIGHTENS it: a
    /// native container retains covered screens by construction, so
    /// `Rebuild` (which disposes the screen a push covers) would tear
    /// down a subtree the container is still displaying. Resolved at
    /// mount, tightened a few lines later once `attach` has answered.
    retention: std::cell::Cell<StackRetention>,
    /// The attached native presenter, when a host installed one AND it
    /// accepted this navigator's outlet node. `None` is the universal
    /// path: outlet swap, no animation, no gesture.
    presenter: RefCell<Option<NativePushHandle>>,
    initial_route: &'static str,
    initial_path: String,
    /// The configured initial route's own query defaults (from a
    /// `initial_path: "/inbox?filter=unread"`), used whenever the stack
    /// seats or re-seats that screen.
    initial_query: QueryParams,
    /// Every mounted stack screen gets these rules layered onto its
    /// root's style override layer — the ported
    /// `set_screen_style_overlay(screen_flow_fill_rules())` (without it
    /// a `flex_grow` screen collapses to content height in the outlet).
    screen_overlay: Rc<StyleRules>,
    active_route: Signal<&'static str>,
    active_path: Signal<String>,
    active_query: Signal<QueryParams>,
    depth: Signal<usize>,
    can_go_back: Signal<bool>,
    /// The active screen's chrome slot (see [`StackNav::screen_chrome`])
    /// + its rev stamp (`sync_chrome` republishes on EVERY stack change,
    /// the old `set_always` contract — screens with identical options
    /// still swapped underneath).
    screen_chrome: Signal<ScreenChrome>,
    chrome_rev: std::cell::Cell<u64>,
    /// This navigator's route-link seam (`Push`) — a cell for the same
    /// construction-order reason as [`SwapShared::link_activator`].
    link_activator: RefCell<Option<LinkActivator>>,
}

impl<H: NavCaps + 'static> StackShared<H> {
    /// This navigator's link activator (filled at mount).
    fn activator(&self) -> LinkActivator {
        self.link_activator
            .borrow()
            .clone()
            .expect("navigator: link activator installed before any screen mounts")
    }

    /// `path` is the entry's CONCRETE full path — it becomes the nav
    /// base for any navigator nested in this screen (see
    /// `realize_screen`), so it must be the URL the entry actually
    /// carries, never the registered pattern.
    fn mount(&self, name: &'static str, path: &str, params: Box<dyn Any>, query: QueryParams) -> LiveScreen<H::Node> {
        realize_screen(
            &self.backend,
            &self.registry,
            &self.screens,
            path,
            self.active_route,
            &self.activator(),
            name,
            params,
            query,
            Some(&self.screen_overlay),
        )
    }

    /// Publish the top screen's options into the scoped `screen_chrome`
    /// signal so an author `StackHeader` re-renders for the current
    /// screen — the old handler's `sync_chrome`, rev-stamped (module
    /// docs on [`ScreenChrome`]). Called after every stack mutation.
    fn sync_chrome(&self) {
        let options = self
            .stack
            .borrow()
            .last()
            .and_then(|e| e.live.as_ref())
            .and_then(|l| l.options.clone());
        let rev = self.chrome_rev.get() + 1;
        self.chrome_rev.set(rev);
        self.screen_chrome.set(ScreenChrome { rev, options });
    }

    /// Reveal the top entry's node, tagged with the direction the
    /// navigation went. Invariant: the top entry is live whenever this
    /// runs (`materialize_top` before reveal).
    fn reveal(&self, dir: Reveal) {
        let top = self
            .stack
            .borrow()
            .last()
            .and_then(|e| e.live.as_ref().map(|l| l.node.clone()));
        if let Some(node) = top {
            self.reveal_node(dir, node);
        }
    }

    /// The reveal itself, on an explicit node — `seat_initial` needs to
    /// reveal an entry that is not the top one (the synthesized index
    /// below a cold-start deep link).
    ///
    /// With no presenter attached, all five directions collapse to the
    /// same `clear_children` + `insert` this handler has always done, so
    /// the emitted op stream is byte-identical to the pre-presenter one
    /// — which is what the `nav_stack_push_pop` scene-parity golden pins.
    fn reveal_node(&self, dir: Reveal, node: H::Node) {
        if let Some(handle) = self.presenter.borrow().as_ref() {
            // The presenter owns placement: it seated a native container
            // inside the outlet at attach, and screens go into THAT.
            // Inserting into the outlet here as well would parent the
            // screen twice.
            let erased: Rc<dyn Any> = Rc::new(node);
            match dir {
                Reveal::Seat => (handle.seat)(erased),
                Reveal::Push => (handle.push)(erased),
                Reveal::Pop => (handle.pop)(erased),
                Reveal::Replace => (handle.replace)(erased),
                Reveal::Reset => (handle.reset)(erased),
            }
            return;
        }
        let _ = dir;
        if let Some(outlet) = self.outlet.borrow().clone() {
            let mut b = self.backend.borrow_mut();
            b.clear_children(&outlet);
            let mut parent = outlet;
            b.insert(&mut parent, node);
        }
    }

    fn publish_depth(&self) {
        let len = self.stack.borrow().len();
        self.depth.set(len);
        self.can_go_back.set(len > 1);
    }

    /// Ensure the top entry has a live surface, re-mounting a cold entry
    /// from its URL exactly like a fresh navigation (browser-refresh
    /// semantics). The unit fallback covers paths that no longer
    /// resolve; every entry minted by a real navigation round-trips.
    fn materialize_top(&self) {
        let cold = {
            let s = self.stack.borrow();
            match s.last() {
                Some(e) if e.live.is_none() => Some((e.route, e.path.clone(), e.query.clone())),
                _ => None,
            }
        };
        let Some((route, path, query)) = cold else { return };
        let params = match_path(&self.screens, &self.base, &path)
            .map(|(_, p)| p)
            .unwrap_or_else(|| Box::new(()));
        let live = self.mount(route, &path, params, query);
        if let Some(top) = self.stack.borrow_mut().last_mut() {
            top.live = Some(live);
        }
    }

    /// Under `Rebuild`, dispose the surface of the screen a push is
    /// about to cover. The `Realized` is taken out before dropping so no
    /// stack borrow is held across author cleanups.
    fn dispose_covered_top(&self) {
        if self.retention.get() != StackRetention::Rebuild {
            return;
        }
        let covered = self.stack.borrow_mut().last_mut().and_then(|e| e.live.take());
        release_and_drop(&self.backend, covered);
    }

    /// Seat the initial screen. When a cold-start deep link resolved a
    /// route DIFFERENT from the configured initial, seat the configured
    /// initial BELOW it so Back returns to the index — live (`Retain`)
    /// or cold (`Rebuild`: the parent was never visited; it must not run
    /// effects until a pop reveals it). Old `seat_initial`, invariant
    /// for invariant.
    fn seat_initial(
        &self,
        route: &'static str,
        path: String,
        query: QueryParams,
        live: LiveScreen<H::Node>,
    ) {
        if route != self.initial_route {
            // The screen seated UNDERNEATH gets the CONFIGURED initial's
            // own query, not the deep link's: the link's state describes
            // the screen it addressed, and leaking it onto the index below
            // would make Back land on an index filtered by the detail
            // screen's parameters.
            let under = match self.retention.get() {
                StackRetention::Rebuild => None,
                _ => Some(self.mount(
                    self.initial_route,
                    &self.initial_path.clone(),
                    Box::new(()),
                    self.initial_query.clone(),
                )),
            };
            self.stack.borrow_mut().push(StackEntry {
                route: self.initial_route,
                path: self.initial_path.clone(),
                query: self.initial_query.clone(),
                live: under,
            });
        }
        let synthesized_index = route != self.initial_route;
        self.stack.borrow_mut().push(StackEntry {
            route,
            path,
            query,
            live: Some(live),
        });
        // A cold-start deep link synthesized an index BELOW the linked
        // screen. With a presenter the native back-stack has to be given
        // both, or the container holds one entry while the logical stack
        // holds two and swipe-back dead-ends on the deep-linked screen.
        // Seat the index unanimated, then push the linked screen onto it
        // — which is also what gives the gesture something to pop to.
        //
        // The index is live here whenever a presenter is attached: attach
        // tightens retention to `Retain`, and only `Rebuild` leaves it
        // cold.
        let seated_index = if synthesized_index && self.presenter.borrow().is_some() {
            self.stack
                .borrow()
                .first()
                .and_then(|e| e.live.as_ref().map(|l| l.node.clone()))
        } else {
            None
        };
        match seated_index {
            Some(index_node) => {
                self.reveal_node(Reveal::Seat, index_node);
                self.reveal(Reveal::Push);
            }
            None => self.reveal(Reveal::Seat),
        }
        self.publish_depth();
        self.sync_chrome();
    }

    fn push(&self, name: &'static str, params: Box<dyn Any>, query: QueryParams, url: String) {
        let live = self.mount(name, &url, params, query.clone());
        self.dispose_covered_top();
        self.stack.borrow_mut().push(StackEntry { route: name, path: url, query, live: Some(live) });
        self.reveal(Reveal::Push);
        self.publish_depth();
        self.sync_chrome();
    }

    /// App-initiated pop: mutate the logical stack, then drive the
    /// presenter so the native container animates along with us.
    fn pop(&self) {
        self.pop_inner(true);
    }

    /// Presenter-initiated pop — a completed swipe-back or a system Back
    /// press. The native container has ALREADY popped, so this does
    /// everything except call back down into it; driving `pop` here
    /// would pop the container a second time.
    ///
    /// The teardown ordering necessarily differs from the app-initiated
    /// path. There, the popped screen's `Realized` is dropped BEFORE the
    /// revealed screen is shown. Here the transition has already run, so
    /// author cleanups fire with the revealed screen already on screen.
    /// That is inherent to an interactive gesture the user drives (and
    /// may cancel), not a defect — holding the popped subtree alive
    /// across the gesture would be the alternative, and it would leak
    /// for as long as the user hesitates mid-swipe.
    fn pop_logical(&self) {
        self.pop_inner(false);
    }

    fn pop_inner(&self, drive_presenter: bool) {
        // Never pop the root.
        if self.stack.borrow().len() <= 1 {
            return;
        }
        let popped = self.stack.borrow_mut().pop();
        // Popped screen teardown FIRST, then reveal — the old
        // release_screen-then-show ordering (cleanups fire before the
        // revealed screen's insert). Dropped outside any stack borrow.
        release_and_drop(&self.backend, popped.and_then(|e| e.live));
        self.materialize_top();
        // On the presenter-initiated path the container already shows
        // the revealed screen; revealing again would animate a pop that
        // has already happened. Without a presenter there is nothing to
        // have moved, so the outlet swap still has to run.
        if drive_presenter || self.presenter.borrow().is_none() {
            self.reveal(Reveal::Pop);
        }
        self.publish_depth();
        // Pop carries no route through the command — mirror the revealed
        // entry ourselves (old `active_changed`). Copied out so no stack
        // borrow is held across the signal writes.
        // The revealed entry's query goes back up with it, so a screen
        // reading `StackNav::query` sees the state it was pushed with —
        // not the state of the screen that just went away.
        let revealed = self
            .stack
            .borrow()
            .last()
            .map(|top| (top.route, top.path.clone(), top.query.clone()));
        if let Some((route, path, query)) = revealed {
            self.active_route.set(route);
            self.active_path.set(path);
            self.active_query.set(query);
        }
        self.sync_chrome();
    }

    fn replace(&self, name: &'static str, params: Box<dyn Any>, query: QueryParams, url: String) {
        // Replacing the top screen with the SAME path is a state change, not
        // a navigation: only the query differs. Update the entry in place
        // and let the reactive `query` signal carry the change into the live
        // screen — remounting here would defeat the entire point of
        // `replace_with_state` as the filter-change verb, tearing down and
        // rebuilding the list on every keystroke (and resetting its scroll
        // and focus with it). The swap navigator gets this for free from its
        // url-key dedupe in `select`; the stack needs it stated.
        let same_screen = self
            .stack
            .borrow()
            .last()
            .is_some_and(|top| top.route == name && top.path == url && top.live.is_some());
        if same_screen {
            if let Some(top) = self.stack.borrow_mut().last_mut() {
                top.query = query;
            }
            return;
        }
        let live = self.mount(name, &url, params, query.clone());
        let old = self.stack.borrow_mut().pop();
        release_and_drop(&self.backend, old.and_then(|e| e.live));
        self.stack.borrow_mut().push(StackEntry { route: name, path: url, query, live: Some(live) });
        self.reveal(Reveal::Replace);
        self.publish_depth();
        self.sync_chrome();
    }

    fn reset(&self, name: &'static str, params: Box<dyn Any>, query: QueryParams, url: String) {
        // Release the whole stack, then seat the new single screen.
        let old: Vec<_> = self.stack.borrow_mut().drain(..).collect();
        for entry in old {
            release_and_drop(&self.backend, entry.live);
        }
        let live = self.mount(name, &url, params, query.clone());
        self.stack.borrow_mut().push(StackEntry { route: name, path: url, query, live: Some(live) });
        self.reveal(Reveal::Reset);
        self.publish_depth();
        self.sync_chrome();
    }
}

/// Mount a `stack_navigator` — port of `walker/navigator.rs::build` +
/// `stack_navigator::StackHandler`.
pub fn mount_stack_navigator<H: NavCaps + 'static>(
    cx: &mut MountCx<'_, H>,
    prim: StackNavigatorPrim,
    _children: Vec<Element>,
) -> H::Node {
    let backend = cx.backend().clone();
    let registry = cx.registry().clone();
    let base = current_nav_base();

    let root = backend.borrow_mut().create_view(&prim.a11y);
    backend.borrow_mut().apply_style(&root, &navigator_fill_rules());
    // Identity registration (robot builds) — see mount_swap_navigator.
    #[cfg(feature = "robot")]
    let robot_reg = crate::robot::register_mount(
        &backend,
        &root,
        crate::robot::ElementKind::Navigator,
        None,
        None,
        None,
        crate::robot::MountActions::default(),
    );

    // Browser semantics on web, native-stack semantics elsewhere
    // (resolved at mount, old handler contract).
    let retention = match prim.retention {
        StackRetention::PlatformDefault => {
            if matches!(runtime_shared::platform(), runtime_shared::Platform::Web) {
                StackRetention::Rebuild
            } else {
                StackRetention::Retain
            }
        }
        resolved => resolved,
    };

    // SSG route discovery — see the swap mount's note (`record_route_paths`).
    record_route_paths(prim.config.screens.values().map(|e| e.path));

    let InitialResolution {
        route: initial_route,
        params: initial_params,
        path: initial_path,
        query: initial_query,
        from_launch_url,
    } = resolve_initial(&prim.config, &base);
    let (cfg_initial_path, cfg_initial_query) = split_query(prim.config.initial_path);

    let active_route = signal(initial_route);
    let active_path = signal(initial_path.clone());
    let active_query = signal(initial_query.clone());
    let depth = signal(1usize);
    let can_go_back = signal(false);
    let screen_chrome = signal(ScreenChrome { rev: 0, options: None });

    let shared = Rc::new(StackShared {
        backend: backend.clone(),
        registry,
        screens: Rc::new(prim.config.screens),
        base: base.clone(),
        outlet: RefCell::new(None),
        stack: RefCell::new(Vec::new()),
        retention: std::cell::Cell::new(retention),
        presenter: RefCell::new(None),
        initial_route: prim.config.initial,
        initial_path: join_path(&base, cfg_initial_path),
        initial_query: cfg_initial_query,
        screen_overlay: screen_flow_fill_rules(),
        active_route,
        active_path,
        active_query,
        depth,
        can_go_back,
        screen_chrome,
        chrome_rev: std::cell::Cell::new(0),
        link_activator: RefCell::new(None),
    });

    // Robot nav registry (P5): the stack's snapshot reads the live
    // back-stack through a Weak of the shared state (root-first
    // `(route, path)` pairs — the old `NavSnapshot.stack` contract).
    #[cfg(feature = "robot")]
    let nav_id = {
        let weak = Rc::downgrade(&shared);
        let base = base.clone();
        let nav_id = crate::robot::register_navigator(
            // Presentation label — see the swap mount's note.
            prim.nav_label.unwrap_or("stack_navigator"),
            Some(robot_reg.id().0),
            Rc::new(move || {
                let live = weak.upgrade()?;
                let stack: Vec<(String, String)> = live
                    .stack
                    .borrow()
                    .iter()
                    .map(|e| (e.route.to_string(), e.path.clone()))
                    .collect();
                Some(crate::robot::NavSnapshotData {
                    active_route: active_route.peek().to_string(),
                    active_path: active_path.peek(),
                    depth: depth.peek(),
                    can_go_back: can_go_back.peek(),
                    base: base.clone(),
                    stack,
                })
            }),
        );
        crate::style_attach::on_teardown(move || {
            crate::robot::deregister_navigator(nav_id);
        });
        nav_id
    };

    let channel = Rc::new(CommandChannel {
        queue: Rc::new(RefCell::new(VecDeque::new())),
        tick: signal(0u64),
    });
    let sync = SyncSlot::default();
    let dispatch: Rc<dyn Fn(NavCommand)> = {
        let channel = channel.clone();
        let base = base.clone();
        let sync = sync.clone();
        Rc::new(move |cmd| {
            // Last-driven navigator = the inspector's "current".
            #[cfg(feature = "robot")]
            crate::robot::mark_active_navigator(nav_id);
            let cmd = compose_url(&base, cmd);
            mirror_command(&cmd, active_route, active_path, active_query);
            let suppress = sync.before(&cmd);
            channel.dispatch(cmd, suppress);
        })
    };

    // Route links inside this navigator's screens/chrome PUSH — the
    // stack half of the old link-activator contract (the old stack
    // installed no activator and links fell back to `Push`; here the
    // fallback is explicit). Handler-safe: rides the staged dispatch.
    let link_activator = {
        let dispatch = dispatch.clone();
        LinkActivator::new(move |name, url, params| {
            // A link may target a route whose pattern carries default query
            // params; split them out so routing never sees the `?`.
            let (path, query) = split_query(&url);
            dispatch(NavCommand::Push { name, url: path.to_string(), params, query });
        })
    };
    *shared.link_activator.borrow_mut() = Some(link_activator.clone());

    {
        let shared = shared.clone();
        let queue = channel.queue.clone();
        let tick = channel.tick;
        let sync = sync.clone();
        let _driver = effect(move || {
            let _ = tick.get();
            loop {
                let next = queue.borrow_mut().pop_front();
                let Some((cmd, suppress_sync)) = next else { break };
                let kind = CommittedKind::of(&cmd);
                match cmd {
                    NavCommand::Push { name, params, query, url } => {
                        shared.push(name, params, query, url)
                    }
                    NavCommand::Pop => shared.pop(),
                    NavCommand::Replace { name, params, query, url } => {
                        shared.replace(name, params, query, url)
                    }
                    NavCommand::Reset { name, params, query, url } => {
                        shared.reset(name, params, query, url)
                    }
                    // A stack never receives Select; ignore (old
                    // dispatcher contract).
                    NavCommand::Select { .. } | NavCommand::Custom(_) => {}
                }
                if !suppress_sync {
                    sync.after(kind);
                }
            }
            H::schedule_layout_pass();
        });
    }

    // Initial screen before chrome (old walker order). Under SSR
    // hydration the server document nests this screen inside the outlet
    // (built below) — the begin/end pair steers the adoption cursor
    // there and back so the out-of-document-order build still adopts.
    // See `LifecycleOps::hydrate_nav_screen_begin`.
    backend.borrow_mut().hydrate_nav_screen_begin(&root, &base);
    let initial = shared.mount(initial_route, &initial_path, initial_params, initial_query.clone());
    backend.borrow_mut().hydrate_nav_screen_end();
    if base.is_empty() {
        set_initial_path(None);
    }

    if let Some(style) = prim.style {
        attach_style(&backend, &root, style);
    }
    if let Some(fill) = prim.on_handle {
        fill(NavHandle::new(dispatch.clone()));
    }

    // Author layout with StackNav provided (save/restore for nesting).
    let pop: Rc<dyn Fn()> = {
        let dispatch = dispatch.clone();
        Rc::new(move || dispatch(NavCommand::Pop))
    };
    // Owned by the navigator's mount scope — see `mount_swap_navigator`.
    provide(StackNav {
        active_route,
        active_path,
        query: active_query,
        depth,
        can_go_back,
        pop,
        screen_chrome: shared.screen_chrome,
    });
    // Chrome links target this navigator too — same ownership.
    provide(link_activator.clone());
    let guard = OutletCaptureGuard::<H::Node>::push();
    // `unanchored` — the chrome closure is a build, so its teardowns
    // belong to the navigator's `Realized`, not to whatever effect mounted
    // the navigator. See the swap handler's twin for the full note.
    let layout_element = runtime_world::unanchored(|| match &prim.layout {
        Some(f) => f(),
        None => crate::builders::navigator_outlet().build(),
    });
    let (layout_root, chrome) = cx.realize_detached(layout_element);
    let outlet = guard.take();
    debug_assert!(
        outlet.is_some(),
        "stack_navigator: the author layout must splat `navigator_outlet()`"
    );
    // SSR: stamp the outlet with the hydration marker so the client can
    // steer its adoption cursor (see `hydrate_nav_screen_begin` above).
    if let Some(outlet) = &outlet {
        backend.borrow_mut().annotate_nav_outlet(outlet, &base);
    }

    {
        let mut parent = root.clone();
        backend.borrow_mut().insert(&mut parent, layout_root);
    }
    *shared.outlet.borrow_mut() = outlet;

    // Native transitions, if a host installed a presenter AND it accepts
    // this outlet's node type. Must run BEFORE `seat_initial`: the seat
    // is the first reveal, and it has to land inside the presenter's
    // container rather than in the outlet it is about to cover.
    if let Some(p) = native_push::presenter() {
        let outlet_erased: Rc<dyn Any> = match shared.outlet.borrow().clone() {
            Some(node) => Rc::new(node),
            None => Rc::new(()),
        };
        if let Some(handle) = p.attach(outlet_erased) {
            // A native container keeps covered screens alive, so the
            // `Rebuild` disposal would tear down a subtree it is still
            // displaying. Tighten before the first reveal.
            shared.retention.set(StackRetention::Retain);
            // The reverse channel: a completed swipe-back or a system
            // Back press has already moved the container, so the
            // presenter tells US, and we reconcile without driving it
            // back. `Weak` so the closure the presenter holds for the
            // navigator's lifetime cannot keep the navigator alive.
            let weak = Rc::downgrade(&shared);
            let user_pop: Rc<dyn Fn()> = Rc::new(move || {
                if let Some(shared) = weak.upgrade() {
                    shared.pop_logical();
                }
            });
            (handle.set_user_pop)(user_pop);
            *shared.presenter.borrow_mut() = Some(handle);
        }
    }

    shared.seat_initial(initial_route, initial_path.clone(), initial_query, initial);

    // URL sync registration — after seat, so `depth` reflects any
    // synthesized deep-link back-stack (the service's history seed).
    {
        let resolve = {
            let screens = shared.screens.clone();
            let base = base.clone();
            Rc::new(move |path: &str| resolve_entry(&screens, &base, path))
                as Rc<dyn Fn(&str) -> Option<(&'static str, Box<dyn Any>, String)>>
        };
        let outlet_erased: Rc<dyn Any> = match shared.outlet.borrow().clone() {
            Some(node) => Rc::new(node),
            None => Rc::new(()),
        };
        sync.register(NavSyncRegistration {
            kind: NavSyncKind::Stack,
            base: base.clone(),
            initial_full_path: shared.initial_path.clone(),
            active_path: initial_path,
            depth: shared.stack.borrow().len(),
            from_launch_url,
            resolve_entry: resolve,
            dispatch: dispatch.clone(),
            outlet: outlet_erased,
        });
    }

    crate::style_attach::on_teardown(move || drop(chrome));

    root
}

// ===========================================================================
// navigator_outlet
// ===========================================================================

/// Mount a `navigator_outlet` — port of `dispatch_navigator_outlet`:
/// an empty container view (`mark_container`, like a plain container
/// view) whose sole child the enclosing navigator swaps. Style-less
/// outlets get the bounded, fillable flex default (`outlet_fill_rules`);
/// an author style REPLACES it entirely.
pub fn mount_navigator_outlet<H: NavCaps + 'static>(
    cx: &mut MountCx<'_, H>,
    prim: NavigatorOutletPrim,
    children: Vec<Element>,
) -> H::Node {
    let backend = cx.backend().clone();
    let mut node = backend.borrow_mut().create_view(&prim.a11y);
    backend.borrow_mut().mark_container(&node);
    cx.realize_children_into(&mut node, children);
    // The default rides the STATIC-SHEET path (not a raw
    // `StyleProp::Static` apply) so it resolves through `apply_sheet` —
    // which enrolls the outlet in the theme cohort AND fills the theme's
    // default text font.
    //
    // The rules sit in the sheet's BASE layer, not behind
    // `with_computed("__navigator_outlet_fill", …)`. `outlet_fill_rules`
    // is a pure constant — no theme read, no runtime input — and the
    // computed wrapper existed only to keep the minted class
    // byte-identical to old-core SSR, a core that no longer exists. What
    // the wrapper still cost was real: `computed.is_some()` is a premint
    // disqualifier (the dump cannot enumerate an arbitrary closure's
    // key), so EVERY navigator outlet in every app fell through to the
    // live style engine. As a plain constant sheet it premints, and the
    // class it wears is identical on SSR and on the client because both
    // mint it from this one definition.
    let style = prim.style.unwrap_or_else(|| {
        fn outlet_sheet() -> Rc<runtime_shared::StyleSheet> {
            static KEY: u8 = 0;
            runtime_shared::cached_stylesheet(&KEY as *const u8 as usize, || {
                runtime_shared::StyleSheet::r#static(outlet_fill_rules())
                    .premint_as("__navigator_outlet_fill")
            })
        }
        crate::style_attach::IntoStyleProp::into_style_prop(
            runtime_shared::StyleApplication::new(outlet_sheet()),
        )
    });
    attach_style(&backend, &node, style);
    // Record into the innermost active capture cell so the enclosing
    // navigator can address this node for screen swaps.
    outlet_capture_record::<H::Node>(&node);
    node
}
