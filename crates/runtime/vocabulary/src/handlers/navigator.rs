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
//! # What is intentionally NOT ported here (each returns with its phase)
//!
//! - Web URL sync (`enable_url_sync`, pushState/popstate, scroll
//!   restore) — backend-web substrate, P3.
//! - `defer_initial_mount` (web reads the platform URL before mounting) —
//!   rides the same P3 work.
//! - Native system-back routing (`on_system_back`) and the iOS/Android
//!   native push surfaces — P4/P5 backend work.
//! - Robot nav registry / back-stack snapshots — identity/robot port, P5.
//! - Stack per-screen header options (`StackScreenOptions`,
//!   `screen_chrome`, `StackHeaderState`) — the old `Screen` options
//!   carrier rode the old `Element`; the new screen contract is a bare
//!   scene `Element`. Header chrome returns with the SDK retarget (P6).
//! - `ScreenNav` portal-hiding context — re-lands with the portal
//!   handler port (the portal agent owns that seam).

use std::any::Any;
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

use runtime_core::primitives::navigator::{
    consumed_prefix, current_nav_base, join_path, match_pattern, match_prefix,
    navigator_fill_rules, outlet_fill_rules, peek_initial_path, screen_flow_fill_rules,
    set_initial_path, NavBaseGuard, NavCommand, ScreenRouteGuard, ScreenStateGuard,
};
use runtime_core::StyleRules;
use runtime_scene::{component_scope, realize, Element, MountCx, Realized, Registry};
use runtime_world::{effect, inject, provide, signal, Signal};

use crate::caps::{LifecycleOps, ViewOps};
use crate::prims::{
    MountPolicy, NavConfig, NavHandle, NavScreenEntry, NavigatorOutletPrim, PrimCell,
    StackNav, StackNavigatorPrim, StackRetention, SwapNav, SwapNavigatorPrim,
};
use crate::style_attach::{attach_style, StyleProp, StyleServices};

/// The capability bundle both navigator handlers need: view creation for
/// the root, the style service for fill rules + author styles, and the
/// lifecycle hook for the post-navigation layout pass. Structural ops
/// come from the `Host` supertrait.
pub trait NavCaps: ViewOps + StyleServices + LifecycleOps {}
impl<T: ViewOps + StyleServices + LifecycleOps> NavCaps for T {}

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
        Some(StyleProp::Sheet(app)) => StyleProp::Sheet(app.with_overrides((*rules).clone())),
        Some(StyleProp::SheetDynamic(f)) => {
            StyleProp::SheetDynamic(Box::new(move || f().with_overrides((*rules).clone())))
        }
        Some(StyleProp::SignalClass(mut spec)) => {
            let inner = spec.compute.clone();
            let rules_for_compute = rules.clone();
            spec.compute = Rc::new(move || inner().with_overrides((*rules_for_compute).clone()));
            StyleProp::SignalClass(spec)
        }
        Some(StyleProp::Preminted { class, overrides }) => StyleProp::Preminted {
            class,
            overrides: Some(match overrides {
                Some(prev) => Rc::new((*prev).clone().merge(&rules)),
                None => rules,
            }),
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

/// A mounted screen: its root node + the `Realized` that owns its
/// entire reactive scope. Dropping this IS the screen teardown — the
/// `realized` field is never *read*; holding it is the whole point
/// (Realized retention, module docs).
struct LiveScreen<N> {
    node: N,
    #[allow(dead_code)]
    realized: Realized<N>,
}

/// Realize one screen: run the route builder inside the screen's own
/// scope (component_scope: untracked, creations collected into the
/// element's `Owned`), with the nav-base / screen-state / screen-route
/// thread-local guards held across BOTH the builder and the realize —
/// the old `mount_screen`'s `with_scope(|| builder() … build())` shape.
///
/// [`crate::prims::ScreenNav`] is `provide`d for the build window (and
/// the previous value restored after) so any portal in the subtree can
/// install its hide-when-inactive visibility effect — the old
/// `mount_screen`'s `reactive::provide(ScreenNav …)`. World context has
/// no scoping, so a `Dyn` region rebuilt LATER injects whatever was
/// provided last — same class of limitation the old core patched with
/// `AmbientNavContext` recapture; that recapture rides the P3 driver
/// work, not this port.
///
/// Must run with the owning world ambient (mount handlers and driver
/// effects both qualify) — the handler-safety invariant in the module
/// docs.
fn realize_screen<H: NavCaps + 'static>(
    backend: &Rc<RefCell<H>>,
    registry: &Rc<Registry<H>>,
    screens: &Rc<HashMap<&'static str, NavScreenEntry>>,
    base: &str,
    active_route: Signal<&'static str>,
    name: &'static str,
    params: Box<dyn Any>,
    state: Option<Rc<dyn Any>>,
    overlay: Option<&Rc<StyleRules>>,
) -> LiveScreen<H::Node> {
    let entry = screens
        .get(name)
        .unwrap_or_else(|| panic!("navigator: route '{name}' is not registered"));
    // Publish the base prefix for any navigator nested in THIS screen
    // (`current_nav_base()` reads it) — hierarchy port, old `NavBaseGuard`.
    let _base_guard = NavBaseGuard::push(join_path(base, entry.path));
    let _state_guard = ScreenStateGuard::push(state);
    let _route_guard = ScreenRouteGuard::push(name);
    let prev_screen_nav = inject::<crate::prims::ScreenNav>();
    provide(crate::prims::ScreenNav {
        active_route: active_route.read_only(),
        route: name,
    });
    let build = entry.build.clone();
    let mut element = component_scope(|| build(params));
    // Handler-requested screen placement (the stack's flow-fill) rides
    // the root element's style OVERRIDE layer, composing with the
    // screen's own styles — old `set_screen_style_overlay` semantics.
    if let Some(rules) = overlay {
        fold_style_overrides(&mut element, rules);
    }
    let realized = realize(backend, registry, element);
    if let Some(prev) = prev_screen_nav {
        provide(prev);
    }
    let mut nodes = realized.collect_nodes();
    match nodes.len() {
        1 => LiveScreen { node: nodes.pop().expect("len checked"), realized },
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
// Command channel — queue + tick + driver effect
// ===========================================================================

/// The handler-safe dispatch half: commands queue here (plain interior
/// state), and the tick signal wakes the driver on the next flush.
struct CommandChannel {
    queue: Rc<RefCell<VecDeque<NavCommand>>>,
    tick: Signal<u64>,
}

impl CommandChannel {
    /// Stage `cmd`. Safe from event handlers (outside `World::enter`):
    /// the queue is plain interior state and `tick.update` is
    /// handle-routed. Two dispatches in one window compose (tick +2 →
    /// one driver wake draining both, in order).
    fn dispatch(&self, cmd: NavCommand) {
        self.queue.borrow_mut().push_back(cmd);
        self.tick.update(|n| n + 1);
    }
}

/// Compose this navigator's base prefix onto a command's
/// (navigator-relative) url — old `NavigatorControl::compose_url`.
fn compose_url(base: &str, cmd: NavCommand) -> NavCommand {
    match cmd {
        NavCommand::Push { name, url, params, state } => {
            NavCommand::Push { name, url: join_path(base, &url), params, state }
        }
        NavCommand::Replace { name, url, params, state } => {
            NavCommand::Replace { name, url: join_path(base, &url), params, state }
        }
        NavCommand::Reset { name, url, params, state } => {
            NavCommand::Reset { name, url: join_path(base, &url), params, state }
        }
        NavCommand::Select { name, url, params, state } => {
            NavCommand::Select { name, url: join_path(base, &url), params, state }
        }
        other => other,
    }
}

/// Mirror a route-carrying command into the active route/path signals
/// BEFORE the handler commits it — the old dispatch's pre-write, so
/// chrome effects and the structural swap land in the same flush.
/// `Pop` carries no route; the driver writes the revealed entry after
/// committing (old `active_changed` contract).
fn mirror_command(cmd: &NavCommand, route: Signal<&'static str>, path: Signal<String>) {
    match cmd {
        NavCommand::Push { name, url, .. }
        | NavCommand::Replace { name, url, .. }
        | NavCommand::Reset { name, url, .. }
        | NavCommand::Select { name, url, .. } => {
            route.set(name);
            path.set(url.clone());
        }
        NavCommand::Pop | NavCommand::Custom(_) => {}
    }
}

/// Resolve the initial (route, params, full path) for a navigator:
/// consult the headless launch/deep-link path (PEEK, not take — each
/// navigator in a nested cascade strips its own base; the root clears
/// the slot after its subtree mounted), falling back to the configured
/// initial. Port of the walker's non-deferred initial-mount resolution,
/// including the concrete-path (not pattern) mirror fix.
fn resolve_initial(
    config: &NavConfig,
    base: &str,
) -> (&'static str, Box<dyn Any>, String) {
    if let Some(path) = peek_initial_path() {
        if let Some((name, params, rem)) = resolve_entry(&config.screens, base, &path) {
            return (name, params, consumed_prefix(&path, &rem));
        }
    }
    (
        config.initial,
        Box::new(()),
        join_path(base, config.initial_path),
    )
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
    base: String,
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
}

impl<H: NavCaps + 'static> SwapShared<H> {
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
        state: Option<Rc<dyn Any>>,
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
            drop(evicted); // screen teardown: effects die, cleanups fire
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
                &self.base,
                self.active_route,
                name,
                params,
                state,
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

    // Resolve the initial BEFORE creating the nav-state signals so a
    // cold-start deep link is their *committed* initial value (the new
    // core stages writes; creating-then-setting would leave chrome's
    // first build reading the configured initial).
    let (initial_route, initial_params, initial_path) = resolve_initial(&prim.config, &base);

    // Nav-state mirror. Created inside the handler ⇒ collected into the
    // navigator's Realized ⇒ freed exactly at navigator teardown. This
    // replaces the old dedicated-scope-retained-on-the-control dance
    // (the QuillEMR "signal used after its scope was dropped" fix) — the
    // ownership is now structural.
    let active_route = signal(initial_route);
    let active_path = signal(initial_path.clone());

    let shared = Rc::new(SwapShared {
        backend: backend.clone(),
        registry,
        screens: Rc::new(prim.config.screens),
        base: base.clone(),
        outlet: RefCell::new(None),
        mounted: RefCell::new(HashMap::new()),
        active: RefCell::new(None),
        mount_policy: prim.mount_policy,
        active_route,
    });

    let channel = Rc::new(CommandChannel {
        queue: Rc::new(RefCell::new(VecDeque::new())),
        tick: signal(0u64),
    });

    // The handler-safe dispatch: compose base, pre-write the mirror,
    // queue for the driver.
    let dispatch: Rc<dyn Fn(NavCommand)> = {
        let channel = channel.clone();
        let base = base.clone();
        Rc::new(move |cmd| {
            let cmd = compose_url(&base, cmd);
            mirror_command(&cmd, active_route, active_path);
            channel.dispatch(cmd);
        })
    };

    // Driver effect: drains the queue inside the flush (module docs).
    // Owned by the navigator's Realized via the ambient collector.
    {
        let shared = shared.clone();
        let queue = channel.queue.clone();
        let tick = channel.tick;
        let _driver = effect(move || {
            let _ = tick.get(); // subscribe; first run sees an empty queue
            loop {
                let next = queue.borrow_mut().pop_front();
                let Some(cmd) = next else { break };
                match cmd {
                    NavCommand::Select { name, url, params, state } => {
                        shared.select(name, &url, params, state);
                    }
                    // Swap navigators have no stack; stray stack verbs
                    // are ignored, never a panic (the old tab-handler
                    // panic regression, ported as a comment-guard).
                    _ => {}
                }
            }
            // Centralized post-navigation layout guarantee — the old
            // `install_request_layout(|| B::schedule_layout_pass())`.
            H::schedule_layout_pass();
        });
    }

    // Initial screen — mounted BEFORE the author layout builds, matching
    // the old walker (screen ops precede the microtask-deferred chrome).
    let initial = realize_screen(
        &shared.backend,
        &shared.registry,
        &shared.screens,
        &base,
        active_route,
        initial_route,
        initial_params,
        None,
        None,
    );
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
            dispatch(NavCommand::Select { name, url, params, state: None });
        })
    };
    let prev_ctx = inject::<SwapNav>();
    provide(SwapNav { active_route, active_path, on_select });
    let guard = OutletCaptureGuard::<H::Node>::push();
    let layout_element = match &prim.layout {
        Some(f) => f(),
        None => crate::builders::navigator_outlet().build(),
    };
    let (layout_root, chrome) = cx.realize_detached(layout_element);
    let outlet = guard.take();
    if let Some(prev) = prev_ctx {
        provide(prev);
    }
    debug_assert!(
        outlet.is_some(),
        "swap_navigator: the author layout must splat `navigator_outlet()` exactly once — \
         no outlet was found in the built layout"
    );

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
    /// Opaque command state, kept so a cold re-mount sees the same
    /// screen-state a live mount did.
    state: Option<Rc<dyn Any>>,
    live: Option<LiveScreen<N>>,
}

struct StackShared<H: NavCaps + 'static> {
    backend: Rc<RefCell<H>>,
    registry: Rc<Registry<H>>,
    screens: Rc<HashMap<&'static str, NavScreenEntry>>,
    base: String,
    outlet: RefCell<Option<H::Node>>,
    stack: RefCell<Vec<StackEntry<H::Node>>>,
    retention: StackRetention,
    initial_route: &'static str,
    initial_path: String,
    /// Every mounted stack screen gets these rules layered onto its
    /// root's style override layer — the ported
    /// `set_screen_style_overlay(screen_flow_fill_rules())` (without it
    /// a `flex_grow` screen collapses to content height in the outlet).
    screen_overlay: Rc<StyleRules>,
    active_route: Signal<&'static str>,
    active_path: Signal<String>,
    depth: Signal<usize>,
    can_go_back: Signal<bool>,
}

impl<H: NavCaps + 'static> StackShared<H> {
    fn mount(&self, name: &'static str, params: Box<dyn Any>, state: Option<Rc<dyn Any>>) -> LiveScreen<H::Node> {
        realize_screen(
            &self.backend,
            &self.registry,
            &self.screens,
            &self.base,
            self.active_route,
            name,
            params,
            state,
            Some(&self.screen_overlay),
        )
    }

    /// Show the top entry's node in the outlet. Invariant: the top entry
    /// is live whenever this runs (`materialize_top` before reveal).
    fn show_top(&self) {
        let top = self
            .stack
            .borrow()
            .last()
            .and_then(|e| e.live.as_ref().map(|l| l.node.clone()));
        if let (Some(outlet), Some(node)) = (self.outlet.borrow().clone(), top) {
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
                Some(e) if e.live.is_none() => Some((e.route, e.path.clone(), e.state.clone())),
                _ => None,
            }
        };
        let Some((route, path, state)) = cold else { return };
        let params = match_path(&self.screens, &self.base, &path)
            .map(|(_, p)| p)
            .unwrap_or_else(|| Box::new(()));
        let live = self.mount(route, params, state);
        if let Some(top) = self.stack.borrow_mut().last_mut() {
            top.live = Some(live);
        }
    }

    /// Under `Rebuild`, dispose the surface of the screen a push is
    /// about to cover. The `Realized` is taken out before dropping so no
    /// stack borrow is held across author cleanups.
    fn dispose_covered_top(&self) {
        if self.retention != StackRetention::Rebuild {
            return;
        }
        let covered = self.stack.borrow_mut().last_mut().and_then(|e| e.live.take());
        drop(covered);
    }

    /// Seat the initial screen. When a cold-start deep link resolved a
    /// route DIFFERENT from the configured initial, seat the configured
    /// initial BELOW it so Back returns to the index — live (`Retain`)
    /// or cold (`Rebuild`: the parent was never visited; it must not run
    /// effects until a pop reveals it). Old `seat_initial`, invariant
    /// for invariant.
    fn seat_initial(&self, route: &'static str, path: String, live: LiveScreen<H::Node>) {
        if route != self.initial_route {
            let under = match self.retention {
                StackRetention::Rebuild => None,
                _ => Some(self.mount(self.initial_route, Box::new(()), None)),
            };
            self.stack.borrow_mut().push(StackEntry {
                route: self.initial_route,
                path: self.initial_path.clone(),
                state: None,
                live: under,
            });
        }
        self.stack.borrow_mut().push(StackEntry {
            route,
            path,
            state: None,
            live: Some(live),
        });
        self.show_top();
        self.publish_depth();
    }

    fn push(&self, name: &'static str, params: Box<dyn Any>, state: Option<Rc<dyn Any>>, url: String) {
        let live = self.mount(name, params, state.clone());
        self.dispose_covered_top();
        self.stack.borrow_mut().push(StackEntry { route: name, path: url, state, live: Some(live) });
        self.show_top();
        self.publish_depth();
    }

    fn pop(&self) {
        // Never pop the root.
        if self.stack.borrow().len() <= 1 {
            return;
        }
        let popped = self.stack.borrow_mut().pop();
        // Popped screen teardown FIRST, then reveal — the old
        // release_screen-then-show ordering (cleanups fire before the
        // revealed screen's insert). Dropped outside any stack borrow.
        drop(popped);
        self.materialize_top();
        self.show_top();
        self.publish_depth();
        // Pop carries no route through the command — mirror the revealed
        // entry ourselves (old `active_changed`). Copied out so no stack
        // borrow is held across the signal writes.
        let revealed = self.stack.borrow().last().map(|top| (top.route, top.path.clone()));
        if let Some((route, path)) = revealed {
            self.active_route.set(route);
            self.active_path.set(path);
        }
    }

    fn replace(&self, name: &'static str, params: Box<dyn Any>, state: Option<Rc<dyn Any>>, url: String) {
        let live = self.mount(name, params, state.clone());
        let old = self.stack.borrow_mut().pop();
        drop(old);
        self.stack.borrow_mut().push(StackEntry { route: name, path: url, state, live: Some(live) });
        self.show_top();
        self.publish_depth();
    }

    fn reset(&self, name: &'static str, params: Box<dyn Any>, state: Option<Rc<dyn Any>>, url: String) {
        // Release the whole stack, then seat the new single screen.
        let old: Vec<_> = self.stack.borrow_mut().drain(..).collect();
        drop(old);
        let live = self.mount(name, params, state.clone());
        self.stack.borrow_mut().push(StackEntry { route: name, path: url, state, live: Some(live) });
        self.show_top();
        self.publish_depth();
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

    // Browser semantics on web, native-stack semantics elsewhere
    // (resolved at mount, old handler contract).
    let retention = match prim.retention {
        StackRetention::PlatformDefault => {
            if matches!(runtime_core::platform(), runtime_core::Platform::Web) {
                StackRetention::Rebuild
            } else {
                StackRetention::Retain
            }
        }
        resolved => resolved,
    };

    let (initial_route, initial_params, initial_path) = resolve_initial(&prim.config, &base);

    let active_route = signal(initial_route);
    let active_path = signal(initial_path.clone());
    let depth = signal(1usize);
    let can_go_back = signal(false);

    let shared = Rc::new(StackShared {
        backend: backend.clone(),
        registry,
        screens: Rc::new(prim.config.screens),
        base: base.clone(),
        outlet: RefCell::new(None),
        stack: RefCell::new(Vec::new()),
        retention,
        initial_route: prim.config.initial,
        initial_path: join_path(&base, prim.config.initial_path),
        screen_overlay: screen_flow_fill_rules(),
        active_route,
        active_path,
        depth,
        can_go_back,
    });

    let channel = Rc::new(CommandChannel {
        queue: Rc::new(RefCell::new(VecDeque::new())),
        tick: signal(0u64),
    });
    let dispatch: Rc<dyn Fn(NavCommand)> = {
        let channel = channel.clone();
        let base = base.clone();
        Rc::new(move |cmd| {
            let cmd = compose_url(&base, cmd);
            mirror_command(&cmd, active_route, active_path);
            channel.dispatch(cmd);
        })
    };

    {
        let shared = shared.clone();
        let queue = channel.queue.clone();
        let tick = channel.tick;
        let _driver = effect(move || {
            let _ = tick.get();
            loop {
                let next = queue.borrow_mut().pop_front();
                let Some(cmd) = next else { break };
                match cmd {
                    NavCommand::Push { name, params, state, url } => {
                        shared.push(name, params, state, url)
                    }
                    NavCommand::Pop => shared.pop(),
                    NavCommand::Replace { name, params, state, url } => {
                        shared.replace(name, params, state, url)
                    }
                    NavCommand::Reset { name, params, state, url } => {
                        shared.reset(name, params, state, url)
                    }
                    // A stack never receives Select; ignore (old
                    // dispatcher contract).
                    NavCommand::Select { .. } | NavCommand::Custom(_) => {}
                }
            }
            H::schedule_layout_pass();
        });
    }

    // Initial screen before chrome (old walker order).
    let initial = shared.mount(initial_route, initial_params, None);
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
    let prev_ctx = inject::<StackNav>();
    provide(StackNav { active_route, active_path, depth, can_go_back, pop });
    let guard = OutletCaptureGuard::<H::Node>::push();
    let layout_element = match &prim.layout {
        Some(f) => f(),
        None => crate::builders::navigator_outlet().build(),
    };
    let (layout_root, chrome) = cx.realize_detached(layout_element);
    let outlet = guard.take();
    if let Some(prev) = prev_ctx {
        provide(prev);
    }
    debug_assert!(
        outlet.is_some(),
        "stack_navigator: the author layout must splat `navigator_outlet()`"
    );

    {
        let mut parent = root.clone();
        backend.borrow_mut().insert(&mut parent, layout_root);
    }
    *shared.outlet.borrow_mut() = outlet;
    shared.seat_initial(initial_route, initial_path, initial);

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
    let style = prim
        .style
        .unwrap_or_else(|| StyleProp::Static(Rc::new(outlet_fill_rules())));
    attach_style(&backend, &node, style);
    // Record into the innermost active capture cell so the enclosing
    // navigator can address this node for screen swaps.
    outlet_capture_record::<H::Node>(&node);
    node
}
