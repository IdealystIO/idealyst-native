//! macOS-backend handler for the Drawer navigator SDK.
//!
//! Single-window, persistent sidebar layout — the design baked into
//! `project_macos_navigator_design`. There is no scrim, no slide-in
//! animation, no Open/Close visual state. The sidebar is always
//! visible and the outlet swaps its child on `Select`.
//!
//! Layout: `Row { sidebar; outlet }` (or `Row { outlet; sidebar }`
//! if `DrawerSide::End`). Sidebar width comes from
//! `DrawerPresentation.drawer_width`; the outlet flexes to fill the
//! remaining width. `DrawerCmd::Open/Close/Toggle` flip the shared
//! `is_open` signal so reactive sidebars stay coherent, but the
//! layout itself never changes — same minimalism the terminal
//! handler ships with.
//!
//! This handler doesn't depend on a macOS-navigator-helpers crate;
//! it uses only the public `Backend` trait surface (create_view,
//! apply_style, insert, clear_children) plus macOS-specific
//! `with_global_backend` for the microtask re-entry. The handler
//! is portable across SDK refactors and stays small.

use crate::{
    DrawerCmd, DrawerPresentation, DrawerScreenOptions, DrawerSide, DrawerSlotProps, LeadingIntent,
    MountPolicy, SlotProps, TopSlot, TrailingIntent,
};
use backend_macos::{with_global_backend, MacosBackend, MacosNode};
use runtime_core::primitives::navigator::{
    AmbientNavGuard, NavCommand, NavigatorHandler, NavigatorHost, NavigatorOps,
};
use runtime_core::{AlignItems, Backend, FlexDirection, Length, StyleRules};
use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

struct ScreenEntry {
    node: MacosNode,
    scope_id: u64,
    /// Active route name — used by the dispatcher's same-route
    /// guard so reselecting the active link is a no-op (avoids
    /// spurious mount→release cycles that would drop the
    /// sidebar's reactive subscriptions). Matches the terminal
    /// drawer's posture.
    name: &'static str,
    /// Per-screen effective `MountPolicy` (screen's own override if
    /// declared via `DrawerScreenExt::mount_policy`, else the
    /// navigator-global default). The dispatcher reads this on the
    /// NEXT `Select` to decide what to do with this (now outgoing)
    /// screen — release for `LazyDisposing`, orphan-and-cache for
    /// `LazyPersistent`/`EagerPersistent`.
    effective_policy: MountPolicy,
}

pub struct MacosDrawerHandler {
    container: Option<MacosNode>,
    outlet: Option<MacosNode>,
    sidebar: Option<MacosNode>,
    current: Rc<RefCell<Option<ScreenEntry>>>,
    /// Cached entries for `LazyPersistent` / `EagerPersistent`
    /// screens that have been visited and then blurred. Their
    /// `MacosNode` is held alive here (outside the outlet's
    /// subview chain — `clear_children` orphaned them on blur)
    /// AND their reactive scope is preserved (we don't call
    /// `release_screen` for Persistent policies). On re-focus,
    /// the entry moves back into `current` and its node is
    /// re-inserted into the outlet.
    mounted: Rc<RefCell<HashMap<&'static str, ScreenEntry>>>,
    /// Navigator-global default policy from `DrawerPresentation`.
    /// Per-screen `DrawerScreenOptions::mount_policy` overrides
    /// this on a route-by-route basis.
    navigator_default_policy: MountPolicy,
    initial_route: Option<&'static str>,
}

impl MacosDrawerHandler {
    pub fn new() -> Self {
        Self {
            container: None,
            outlet: None,
            sidebar: None,
            current: Rc::new(RefCell::new(None)),
            mounted: Rc::new(RefCell::new(HashMap::new())),
            navigator_default_policy: MountPolicy::default(),
            initial_route: None,
        }
    }
}

impl Default for MacosDrawerHandler {
    fn default() -> Self {
        Self::new()
    }
}

struct NoopDrawerOps;
impl NavigatorOps for NoopDrawerOps {}
static NOOP_DRAWER_OPS: NoopDrawerOps = NoopDrawerOps;

impl NavigatorHandler<MacosBackend> for MacosDrawerHandler {
    fn init(
        &mut self,
        backend: &mut MacosBackend,
        host: NavigatorHost<MacosNode>,
        presentation: Rc<dyn Any>,
    ) -> MacosNode {
        let presentation = presentation
            .downcast::<DrawerPresentation>()
            .expect("MacosDrawerHandler: presentation must be DrawerPresentation");

        // Outer Row container — Taffy-driven flex layout. `width:
        // 100%, height: 100%, flex_direction: Row` so the sidebar
        // and outlet sit side-by-side and fill the window.
        let mut container = backend.create_view(&Default::default());
        let mut container_style = StyleRules::default();
        container_style.flex_direction = Some(FlexDirection::Row);
        container_style.align_items = Some(AlignItems::Stretch);
        container_style.width = Some(Length::pct(100.0).into());
        container_style.height = Some(Length::pct(100.0).into());
        backend.apply_style(&container, &Rc::new(container_style));

        // Sidebar — fixed width. `flex_shrink: 0` so wide screen
        // content can't squash it (same load-bearing rule the
        // terminal drawer documents — when content is wider than
        // the viewport, Taffy's Row shrink kicks in and the
        // sidebar would collapse without this).
        let sidebar = backend.create_view(&Default::default());
        let mut sidebar_style = StyleRules::default();
        sidebar_style.width = Some(Length::Px(presentation.drawer_width).into());
        sidebar_style.height = Some(Length::pct(100.0).into());
        sidebar_style.flex_direction = Some(FlexDirection::Column);
        sidebar_style.flex_shrink = Some(0.0f32.into());
        backend.apply_style(&sidebar, &Rc::new(sidebar_style));

        // Outlet — flex-grow into remaining width. `flex_basis: 0`
        // + `flex_grow: 1` so the outlet absorbs every remaining
        // pixel after the sidebar's fixed basis. Without `basis: 0`
        // the screen content's intrinsic width would inflate the
        // basis sum and trigger shrinkage on both columns.
        let outlet = backend.create_view(&Default::default());
        let mut outlet_style = StyleRules::default();
        outlet_style.flex_grow = Some(1.0f32.into());
        outlet_style.flex_basis = Some(Length::Px(0.0).into());
        outlet_style.height = Some(Length::pct(100.0).into());
        outlet_style.flex_direction = Some(FlexDirection::Column);
        backend.apply_style(&outlet, &Rc::new(outlet_style));

        match presentation.side {
            DrawerSide::Start => {
                backend.insert(&mut container, sidebar.clone());
                backend.insert(&mut container, outlet.clone());
            }
            DrawerSide::End => {
                backend.insert(&mut container, outlet.clone());
                backend.insert(&mut container, sidebar.clone());
            }
        }

        self.container = Some(container.clone());
        self.outlet = Some(outlet.clone());
        self.sidebar = Some(sidebar.clone());
        self.initial_route = Some(host.initial_route);
        self.navigator_default_policy = presentation.mount_policy;

        let nav_state = host.nav_state.clone();
        let control = host.control.clone();
        let is_open = presentation.is_open;

        // Map `Link(route=…)` activations to `Select` (drawer-shape)
        // instead of the substrate default `Push` (stack-shape).
        // Same fix as the iOS / Android / web / terminal handlers.
        let select_activator: Rc<
            dyn Fn(&'static str, String, Box<dyn Any>) -> NavCommand,
        > = Rc::new(|name, url, params| NavCommand::Select {
            name,
            url,
            params,
            state: None,
        });
        control.install_link_activator(select_activator);

        // Materialise the sidebar Element via the SDK's builder. Must run
        // outside the outer backend borrow window per the host docs — defer
        // via `schedule_microtask`.
        //
        // Two API surfaces (mirrors iOS / web / Android): prefer the new
        // `leading_slot` (SlotProps-based, set via `leading_with` — the
        // tutorial / website path) over the legacy `sidebar` closure
        // (DrawerSlotProps). Without the leading_slot branch the macOS sidebar
        // was never built — apps using `.leading_with(...)` got a blank panel.
        let leading_slot = presentation.leading_slot.borrow_mut().take();
        let legacy_sidebar = presentation.sidebar.borrow().clone();
        if leading_slot.is_some() || legacy_sidebar.is_some() {
            // Use `build_node_scoped`, NOT `build_node`: the sidebar Element is
            // produced by `#[component]` bodies (idea-ui's animated `Switch`)
            // whose standalone `Effect::new` runs at CONSTRUCTION time. Building
            // the Element here and only then calling `build_node` would create
            // those effects with no active scope — their handle owns them and
            // frees them when the body returns, so they run once and never
            // re-fire (the Switch thumb froze). `build_node_scoped` runs the
            // builder INSIDE the retained chrome scope, so the effects are owned
            // by it and stay reactive. See `NavigatorHost::build_node_scoped`.
            let build_node_scoped = host.build_node_scoped.clone();
            let control_for_sidebar = control.clone();
            let nav_state_for_sidebar = nav_state.clone();
            let sidebar_node = sidebar.clone();
            runtime_core::schedule_microtask(move || {
                let builder_closure: Box<dyn FnOnce() -> runtime_core::Element> =
                    Box::new(move || {
                        let on_select: Rc<dyn Fn(&'static str)> = {
                            let control = control_for_sidebar.clone();
                            Rc::new(move |name| {
                                control.dispatch(NavCommand::Select {
                                    name,
                                    url: String::new(),
                                    params: Box::new(()),
                                    state: None,
                                });
                            })
                        };
                        // Push the navigator onto the ambient stack so `Link`
                        // primitives inside the sidebar capture this navigator as
                        // their dispatch target — without it,
                        // `ambient_navigator()` returns `None` and every sidebar
                        // link's on_activate silently no-ops. See
                        // `[[project_drawer_sidebar_ambient_nav]]`.
                        let _guard = AmbientNavGuard::push(control_for_sidebar.clone());
                        if let Some(builder) = leading_slot {
                            let open_drawer: Rc<dyn Fn()> = {
                                let c = control_for_sidebar.clone();
                                Rc::new(move || {
                                    c.dispatch(NavCommand::Custom(Rc::new(DrawerCmd::Open)));
                                })
                            };
                            let close_drawer: Rc<dyn Fn()> = {
                                let c = control_for_sidebar.clone();
                                Rc::new(move || {
                                    c.dispatch(NavCommand::Custom(Rc::new(DrawerCmd::Close)));
                                })
                            };
                            // The drawer has no stack, so `pop` is a no-op.
                            let pop: Rc<dyn Fn()> = Rc::new(|| {});
                            let props = SlotProps {
                                active_route: nav_state_for_sidebar.active_route,
                                active_path: nav_state_for_sidebar.active_path.clone(),
                                depth: nav_state_for_sidebar.depth,
                                can_go_back: nav_state_for_sidebar.can_go_back,
                                is_open,
                                leading_intent: runtime_core::signal!(LeadingIntent::OpenDrawer),
                                trailing_intent: runtime_core::signal!(TrailingIntent::None),
                                screen_title: runtime_core::signal!(String::new()),
                                on_select,
                                open_drawer,
                                close_drawer,
                                pop,
                                scroll: None,
                            };
                            builder(props)
                        } else if let Some(sidebar_builder) = legacy_sidebar {
                            let on_close: Rc<dyn Fn()> = {
                                let control = control_for_sidebar.clone();
                                Rc::new(move || {
                                    control.dispatch(NavCommand::Custom(Rc::new(DrawerCmd::Close)));
                                })
                            };
                            let props = DrawerSlotProps {
                                active_route: nav_state_for_sidebar.active_route,
                                active_path: nav_state_for_sidebar.active_path.clone(),
                                depth: nav_state_for_sidebar.depth,
                                can_go_back: nav_state_for_sidebar.can_go_back,
                                is_open,
                                on_select,
                                on_close,
                            };
                            sidebar_builder(props)
                        } else {
                            // Unreachable: guarded by the `is_some()` check above.
                            unreachable!("sidebar build with neither slot set")
                        }
                    });
                let sidebar_node_materialised = build_node_scoped(builder_closure);
                with_global_backend(|b| {
                    let mut sb = sidebar_node.clone();
                    b.insert(&mut sb, sidebar_node_materialised);
                });
            });
        }

        // Top slot (header bar). macOS mirrors the web layout: a full-width
        // header ABOVE the sidebar+outlet Row. Without a top slot the
        // navigator root stays the bare Row (the prior, headerless layout).
        // Built deferred (like the sidebar) so it runs outside the outer
        // backend borrow; `build_node_scoped` publishes the ambient navigator
        // so header links/buttons dispatch.
        let root_node = if let Some(TopSlot::Custom(top_builder)) =
            presentation.top_slot.borrow_mut().take()
        {
            // Outer Column: header on top, the Row filling the rest.
            let outer = backend.create_view(&Default::default());
            let mut outer_style = StyleRules::default();
            outer_style.flex_direction = Some(FlexDirection::Column);
            outer_style.align_items = Some(AlignItems::Stretch);
            outer_style.width = Some(Length::pct(100.0).into());
            outer_style.height = Some(Length::pct(100.0).into());
            backend.apply_style(&outer, &Rc::new(outer_style));

            // Re-style the Row to fill the height BELOW the header. It was
            // `height: 100%`, which would overlap the header inside a column.
            let mut row_style = StyleRules::default();
            row_style.flex_direction = Some(FlexDirection::Row);
            row_style.align_items = Some(AlignItems::Stretch);
            row_style.width = Some(Length::pct(100.0).into());
            row_style.flex_grow = Some(1.0f32.into());
            row_style.flex_basis = Some(Length::Px(0.0).into());
            row_style.min_height = Some(Length::Px(0.0).into());
            backend.apply_style(&container, &Rc::new(row_style));

            // Full-width header placeholder; `flex_shrink: 0` so it keeps its
            // content height and the Row absorbs the rest.
            let header_slot = backend.create_view(&Default::default());
            let mut header_style = StyleRules::default();
            header_style.width = Some(Length::pct(100.0).into());
            header_style.flex_shrink = Some(0.0f32.into());
            backend.apply_style(&header_slot, &Rc::new(header_style));

            let mut outer_mut = outer.clone();
            backend.insert(&mut outer_mut, header_slot.clone());
            backend.insert(&mut outer_mut, container.clone());

            // Materialise the header content deferred, then drop it into the
            // placeholder — mirrors the sidebar microtask above.
            let build_node_scoped = host.build_node_scoped.clone();
            let control_for_top = control.clone();
            let nav_state_for_top = nav_state.clone();
            let is_open_for_top = is_open;
            runtime_core::schedule_microtask(move || {
                let builder_closure: Box<dyn FnOnce() -> runtime_core::Element> =
                    Box::new(move || {
                        let on_select: Rc<dyn Fn(&'static str)> = {
                            let c = control_for_top.clone();
                            Rc::new(move |name| {
                                c.dispatch(NavCommand::Select {
                                    name,
                                    url: String::new(),
                                    params: Box::new(()),
                                    state: None,
                                });
                            })
                        };
                        let open_drawer: Rc<dyn Fn()> = {
                            let c = control_for_top.clone();
                            Rc::new(move || {
                                c.dispatch(NavCommand::Custom(Rc::new(DrawerCmd::Open)));
                            })
                        };
                        let close_drawer: Rc<dyn Fn()> = {
                            let c = control_for_top.clone();
                            Rc::new(move || {
                                c.dispatch(NavCommand::Custom(Rc::new(DrawerCmd::Close)));
                            })
                        };
                        let props = SlotProps {
                            active_route: nav_state_for_top.active_route,
                            active_path: nav_state_for_top.active_path.clone(),
                            depth: nav_state_for_top.depth,
                            can_go_back: nav_state_for_top.can_go_back,
                            is_open: is_open_for_top,
                            leading_intent: runtime_core::signal!(LeadingIntent::OpenDrawer),
                            trailing_intent: runtime_core::signal!(TrailingIntent::None),
                            screen_title: runtime_core::signal!(String::new()),
                            on_select,
                            open_drawer,
                            close_drawer,
                            pop: Rc::new(|| {}),
                            scroll: None,
                        };
                        top_builder(props)
                    });
                let header_node = build_node_scoped(builder_closure);
                with_global_backend(|b| {
                    let mut h = header_slot.clone();
                    b.insert(&mut h, header_node);
                });
            });
            outer
        } else {
            container.clone()
        };

        // Install dispatcher. `Select` swaps the outlet's child;
        // `Custom(DrawerCmd::*)` flips `is_open`; stack-shaped
        // commands panic (drawer kind doesn't accept Push / Pop /
        // Replace / Reset).
        //
        // Per-screen `MountPolicy` is honored: `LazyDisposing` screens
        // release their reactive scope on blur (current behavior up
        // through previous versions); `LazyPersistent` /
        // `EagerPersistent` screens have their `MacosNode` orphaned
        // off the outlet but their scope is kept alive in the
        // framework's scopes map, and the entry caches in `mounted`
        // for instant re-attach on re-focus. Mirrors the iOS drawer
        // helper's branch — see [[project-ios-drawer-per-screen-policy]].
        let current_rc = self.current.clone();
        let mounted_rc = self.mounted.clone();
        let outlet_for_dispatch = outlet.clone();
        let dispatching = Rc::new(RefCell::new(false));
        let mount_screen = host.mount_screen.clone();
        let release_screen = host.release_screen.clone();
        let active_changed = host.active_changed.clone();
        let navigator_default_policy = presentation.mount_policy;

        control.install(Box::new(move |cmd| match cmd {
            NavCommand::Select { name, url, params, .. } => {
                if *dispatching.borrow() {
                    drop(params);
                    return;
                }
                let already_active = current_rc
                    .borrow()
                    .as_ref()
                    .map(|e| e.name == name)
                    .unwrap_or(false);
                if already_active {
                    drop(params);
                    return;
                }
                *dispatching.borrow_mut() = true;

                // Coalesce every layout pass this swap would trigger into one.
                // Building the incoming screen crosses a reactive-window
                // boundary per `attach_style` effect, and `active_changed` below
                // fans `active_route` out to every sidebar item — the macOS idle
                // hook would fire a full-tree layout pass at each, so one
                // navigation cost 4–9+ redundant passes. Held across mount +
                // insert + `active_changed`; the guard runs ONE coalesced pass
                // when it drops at the end of this arm. The `run_layout_pass_now`
                // below still lays the new screen out before paint (no flash).
                let _coalesce = backend_macos::coalesce_layout_passes();

                // ---- Incoming: cache hit or fresh mount? ----
                //
                // Check the persistence cache first. A hit means the
                // screen was previously visited under a Persistent
                // policy and its node + scope are still alive — just
                // re-attach. A miss falls through to a fresh
                // `mount_screen` call.
                let cached = mounted_rc.borrow_mut().remove(name);
                let (incoming_node, incoming_scope, incoming_policy) = match cached {
                    Some(entry) => {
                        // Cache hit: params from this Select are
                        // ignored (the cached screen already has its
                        // mount params from the original visit).
                        drop(params);
                        (entry.node, entry.scope_id, entry.effective_policy)
                    }
                    None => {
                        let result = mount_screen(name, params, None);
                        let policy = result
                            .options
                            .downcast_ref::<DrawerScreenOptions>()
                            .and_then(|o| o.mount_policy)
                            .unwrap_or(navigator_default_policy);
                        (result.node, result.scope_id, policy)
                    }
                };

                // ---- Outgoing: hide or release ----
                let prev = current_rc.borrow_mut().take();
                with_global_backend(|b| {
                    let mut outlet_node = outlet_for_dispatch.clone();
                    // Clear the outlet's current child (if any). For
                    // Persistent screens this orphans the node from
                    // the subview chain but doesn't release its
                    // backing NSView — the cache holds it ready for
                    // re-attach.
                    if prev.is_some() {
                        b.clear_children(&outlet_node);
                    }
                    b.insert(&mut outlet_node, incoming_node.clone());
                    // Lay out the incoming screen SYNCHRONOUSLY before returning
                    // to the event loop. `insert` only schedules a coalesced
                    // (microtask-deferred) pass, so without this the new screen
                    // paints once unsized — the visible navigation delay/flash.
                    // We already hold the backend borrow here, so a direct
                    // synchronous pass is both correct and cheaper than waiting a
                    // runloop turn.
                    b.run_layout_pass_now();
                });
                if let Some(prev) = prev {
                    match prev.effective_policy {
                        MountPolicy::LazyDisposing => {
                            release_screen(prev.scope_id);
                        }
                        MountPolicy::LazyPersistent
                        | MountPolicy::EagerPersistent => {
                            // Stash the orphaned-but-still-mounted
                            // entry. The framework's scopes map
                            // continues to own the scope; the next
                            // Select hitting `name` will pull it back
                            // out of `mounted` instead of re-mounting.
                            mounted_rc.borrow_mut().insert(prev.name, prev);
                        }
                    }
                }
                *current_rc.borrow_mut() = Some(ScreenEntry {
                    node: incoming_node,
                    scope_id: incoming_scope,
                    name,
                    effective_policy: incoming_policy,
                });
                active_changed(name, url);
                *dispatching.borrow_mut() = false;
            }
            NavCommand::Custom(payload) => {
                if let Some(cmd) = payload.downcast_ref::<DrawerCmd>() {
                    match cmd {
                        DrawerCmd::Open => is_open.set(true),
                        DrawerCmd::Close => is_open.set(false),
                        DrawerCmd::Toggle => is_open.set(!is_open.get()),
                    }
                }
            }
            NavCommand::Push { .. }
            | NavCommand::Pop
            | NavCommand::Replace { .. }
            | NavCommand::Reset { .. } => {
                panic!(
                    "drawer Navigator received a stack-shaped NavCommand on \
                     macOS — drawer kind only accepts Select / Custom(DrawerCmd)"
                );
            }
        }));

        root_node
    }

    fn attach_initial(
        &mut self,
        backend: &mut MacosBackend,
        screen: MacosNode,
        scope_id: u64,
        options: Box<dyn Any>,
    ) {
        let Some(outlet) = self.outlet.clone() else { return };
        let mut outlet_mut = outlet;
        backend.insert(&mut outlet_mut, screen.clone());
        let name = self.initial_route.unwrap_or("");
        // Initial screen's effective policy comes from its own
        // `DrawerScreenOptions::mount_policy` if set, else the
        // navigator-global default. Tracking it here means the
        // FIRST `Select` away from the initial screen reads the
        // correct outgoing policy — without this, the initial
        // screen would always be treated as if it had the
        // navigator default, regardless of its own override.
        let effective_policy = options
            .downcast_ref::<DrawerScreenOptions>()
            .and_then(|o| o.mount_policy)
            .unwrap_or(self.navigator_default_policy);
        *self.current.borrow_mut() = Some(ScreenEntry {
            node: screen,
            scope_id,
            name,
            effective_policy,
        });
    }

    fn release(&mut self, _backend: &mut MacosBackend) {
        *self.current.borrow_mut() = None;
        self.mounted.borrow_mut().clear();
        self.outlet = None;
        self.sidebar = None;
        self.container = None;
    }

    fn make_handle(&self) -> runtime_core::NavigatorHandle {
        runtime_core::NavigatorHandle::new(Rc::new(()), &NOOP_DRAWER_OPS)
    }

    fn apply_slot_style(
        &mut self,
        backend: &mut MacosBackend,
        slot: &'static str,
        style: &Rc<runtime_core::StyleRules>,
    ) {
        // We don't render a header bar (single-window, persistent
        // sidebar — no per-screen chrome). "body" slot styles the
        // outlet's background to match the cross-platform contract.
        if slot != "body" {
            return;
        }
        let Some(outlet) = self.outlet.clone() else { return };
        backend.apply_style(&outlet, style);
    }
}

/// Install the drawer navigator handler on a macOS backend. Call once
/// at startup so `Element::Navigator`s carrying a [`DrawerPresentation`]
/// resolve to this backend's chrome.
pub fn register(backend: &mut MacosBackend) {
    backend.register_navigator::<DrawerPresentation, _>(|| {
        Box::new(MacosDrawerHandler::new())
    });
}

// Self-register at backend construction (no app-side `register` call needed).
// See [[project_inventory_self_registration]].
inventory::submit! {
    backend_macos::MacosNavigatorRegistrar(register)
}
