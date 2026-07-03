//! Backend-neutral **desktop** handler for the Drawer navigator.
//!
//! This is the generalization of [`crate::macos`]: the same
//! single-window, persistent-sidebar layout (`Row { sidebar; outlet }`,
//! header above when a `top_with` slot is set), but written against the
//! plain `Backend` trait + `NavigatorHost` closures so it runs on **any**
//! backend — the wgpu GPU renderer, AppKit, a future desktop Linux/Windows
//! backend — without per-backend code. It's selected at compile time by
//! the `idealyst_form = "desktop"` cfg (see [`crate::register_native`]).
//!
//! ## Why this can be generic when `macos.rs` couldn't
//!
//! The macOS handler reaches the backend after `init` (to splice the
//! deferred sidebar/header build, and to swap the outlet's child on
//! `Select`) through `backend_macos::with_global_backend` — a
//! backend-specific global. The portable equivalents are the
//! `NavigatorHost` closures: `build_node_scoped` (build chrome inside the
//! retained scope) and the node-typed `insert_node` / `clear_children`
//! (splice / clear already-built nodes, backend erased). Those are all a
//! generic handler needs; there is no AppKit-specific surface left.
//!
//! ## Deliberate differences from `macos.rs`
//!
//! - No `coalesce_layout_passes` / `run_layout_pass_now`. Those are macOS
//!   perf/no-flash optimizations; correctness doesn't depend on them.
//!   `NavigatorControl` already requests one layout pass after every
//!   dispatch (`install_request_layout` in the walker), so the swap
//!   relays out — at most one frame later than a synchronous pass. A
//!   backend that wants the synchronous-before-paint behavior keeps its
//!   own handler (macOS does).

use crate::{
    DrawerCmd, DrawerPresentation, DrawerScreenOptions, DrawerSide, DrawerSlotProps, LeadingIntent,
    MountPolicy, SlotProps, TopSlot, TrailingIntent,
};
use runtime_core::primitives::navigator::{
    AmbientNavGuard, NavCommand, NavigatorHandler, NavigatorHost, NavigatorOps, RegisterNavigator,
};
use runtime_core::{AlignItems, Backend, FlexDirection, Length, StyleRules};
use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

struct ScreenEntry<N> {
    node: N,
    scope_id: u64,
    /// Active route name — used by the dispatcher's same-route guard so
    /// reselecting the active link is a no-op (avoids spurious
    /// mount→release cycles that drop the sidebar's reactive
    /// subscriptions). Mirrors the macOS / terminal handlers.
    name: &'static str,
    /// Per-screen effective `MountPolicy` (screen override if declared,
    /// else the navigator-global default). The dispatcher reads it on the
    /// NEXT `Select` to decide whether to release this (outgoing) screen
    /// or orphan-and-cache it.
    effective_policy: MountPolicy,
}

/// Generic persistent-sidebar drawer handler. `B` is the backend; all
/// node manipulation goes through `B`'s `Backend` impl or the host's
/// node-typed closures.
pub struct DesktopDrawerHandler<B: Backend + 'static> {
    outlet: Option<B::Node>,
    current: Rc<RefCell<Option<ScreenEntry<B::Node>>>>,
    /// Cache for `LazyPersistent` / `EagerPersistent` screens that were
    /// visited and then blurred — node held alive here (orphaned off the
    /// outlet) with its reactive scope preserved, ready for instant
    /// re-attach on re-focus.
    mounted: Rc<RefCell<HashMap<&'static str, ScreenEntry<B::Node>>>>,
    navigator_default_policy: MountPolicy,
    initial_route: Option<&'static str>,
}

impl<B: Backend + 'static> DesktopDrawerHandler<B> {
    /// Create an unmounted handler. The outlet/sidebar nodes are built in
    /// [`init`](NavigatorHandler::init).
    pub fn new() -> Self {
        Self {
            outlet: None,
            current: Rc::new(RefCell::new(None)),
            mounted: Rc::new(RefCell::new(HashMap::new())),
            navigator_default_policy: MountPolicy::default(),
            initial_route: None,
        }
    }
}

impl<B: Backend + 'static> Default for DesktopDrawerHandler<B> {
    fn default() -> Self {
        Self::new()
    }
}

struct NoopDrawerOps;
impl NavigatorOps for NoopDrawerOps {}
static NOOP_DRAWER_OPS: NoopDrawerOps = NoopDrawerOps;

impl<B: Backend + 'static> NavigatorHandler<B> for DesktopDrawerHandler<B> {
    fn init(
        &mut self,
        backend: &mut B,
        host: NavigatorHost<B::Node>,
        presentation: Rc<dyn Any>,
    ) -> B::Node {
        let presentation = presentation
            .downcast::<DrawerPresentation>()
            .expect("DesktopDrawerHandler: presentation must be DrawerPresentation");

        // Outer Row container: sidebar + outlet side-by-side, filling the
        // window. Taffy-driven flex — identical rules to the macOS handler.
        let mut container = backend.create_view(&Default::default());
        let mut container_style = StyleRules::default();
        container_style.flex_direction = Some(FlexDirection::Row);
        container_style.align_items = Some(AlignItems::Stretch);
        container_style.width = Some(Length::pct(100.0).into());
        container_style.height = Some(Length::pct(100.0).into());
        backend.apply_style(&container, &Rc::new(container_style));

        // Sidebar — fixed width, `flex_shrink: 0` so wide content can't
        // squash it (Taffy Row shrink would otherwise collapse it).
        let sidebar = backend.create_view(&Default::default());
        let mut sidebar_style = StyleRules::default();
        sidebar_style.width = Some(Length::Px(presentation.drawer_width).into());
        sidebar_style.height = Some(Length::pct(100.0).into());
        sidebar_style.flex_direction = Some(FlexDirection::Column);
        sidebar_style.flex_shrink = Some(0.0f32.into());
        backend.apply_style(&sidebar, &Rc::new(sidebar_style));

        // Outlet — flex-grow into the remaining width. `flex_basis: 0` so
        // the screen's intrinsic width doesn't inflate the basis sum and
        // trigger shrinkage on both columns.
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

        self.outlet = Some(outlet.clone());
        self.initial_route = Some(host.initial_route);
        self.navigator_default_policy = presentation.mount_policy;

        let nav_state = host.nav_state.clone();
        let control = host.control.clone();
        let is_open = presentation.is_open;

        // Map `Link(route=…)` activations to `Select` (drawer-shape), not
        // the substrate default `Push` (stack-shape). Same as every other
        // drawer handler.
        let select_activator: Rc<dyn Fn(&'static str, String, Box<dyn Any>) -> NavCommand> =
            Rc::new(|name, url, params| NavCommand::Select {
                name,
                url,
                params,
                state: None,
            });
        control.install_link_activator(select_activator);

        // --- Deferred sidebar build. Must run outside the outer backend
        // borrow (per host docs), so defer via microtask. Uses
        // `build_node_scoped` (effects owned by the chrome scope — the
        // animated active indicator must keep re-firing) + the host's
        // node-typed `insert_node` to splice it into the sidebar slot. ---
        let leading_slot = presentation.leading_slot.borrow_mut().take();
        let legacy_sidebar = presentation.sidebar.borrow().clone();
        if leading_slot.is_some() || legacy_sidebar.is_some() {
            let build_node_scoped = host.build_node_scoped.clone();
            let insert_node = host.insert_node.clone();
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
                        // Publish this navigator as ambient so sidebar
                        // `Link`s capture it as their dispatch target.
                        let _guard = AmbientNavGuard::push(control_for_sidebar.clone());
                        if let Some(builder) = leading_slot {
                            let open_drawer: Rc<dyn Fn()> = {
                                let c = control_for_sidebar.clone();
                                Rc::new(move || c.dispatch(NavCommand::Custom(Rc::new(DrawerCmd::Open))))
                            };
                            let close_drawer: Rc<dyn Fn()> = {
                                let c = control_for_sidebar.clone();
                                Rc::new(move || c.dispatch(NavCommand::Custom(Rc::new(DrawerCmd::Close))))
                            };
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
                                let c = control_for_sidebar.clone();
                                Rc::new(move || c.dispatch(NavCommand::Custom(Rc::new(DrawerCmd::Close))))
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
                            unreachable!("sidebar build with neither slot set")
                        }
                    });
                let materialised = build_node_scoped(builder_closure);
                insert_node(sidebar_node, materialised);
            });
        }

        // --- Top slot (header bar). Mirrors the macOS/web layout: a
        // full-width header ABOVE the sidebar+outlet Row. Built deferred,
        // like the sidebar. ---
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

            // Re-style the Row to fill the height BELOW the header (it was
            // `height: 100%`, which would overlap the header in a column).
            let mut row_style = StyleRules::default();
            row_style.flex_direction = Some(FlexDirection::Row);
            row_style.align_items = Some(AlignItems::Stretch);
            row_style.width = Some(Length::pct(100.0).into());
            row_style.flex_grow = Some(1.0f32.into());
            row_style.flex_basis = Some(Length::Px(0.0).into());
            row_style.min_height = Some(Length::Px(0.0).into());
            backend.apply_style(&container, &Rc::new(row_style));

            // Full-width header placeholder; `flex_shrink: 0` so it keeps
            // its content height and the Row absorbs the rest.
            let header_slot = backend.create_view(&Default::default());
            let mut header_style = StyleRules::default();
            header_style.width = Some(Length::pct(100.0).into());
            header_style.flex_shrink = Some(0.0f32.into());
            backend.apply_style(&header_slot, &Rc::new(header_style));

            let mut outer_mut = outer.clone();
            backend.insert(&mut outer_mut, header_slot.clone());
            backend.insert(&mut outer_mut, container.clone());

            // Materialise the header content deferred, then splice it in.
            let build_node_scoped = host.build_node_scoped.clone();
            let insert_node = host.insert_node.clone();
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
                            Rc::new(move || c.dispatch(NavCommand::Custom(Rc::new(DrawerCmd::Open))))
                        };
                        let close_drawer: Rc<dyn Fn()> = {
                            let c = control_for_top.clone();
                            Rc::new(move || c.dispatch(NavCommand::Custom(Rc::new(DrawerCmd::Close))))
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
                insert_node(header_slot, header_node);
            });
            outer
        } else {
            container.clone()
        };

        // --- Dispatcher. `Select` swaps the outlet's child via the host's
        // node-typed `clear_children` + `insert_node`; `Custom(DrawerCmd)`
        // flips `is_open`; stack-shaped commands panic (drawer kind). ---
        let current_rc = self.current.clone();
        let mounted_rc = self.mounted.clone();
        let outlet_for_dispatch = outlet.clone();
        let dispatching = Rc::new(RefCell::new(false));
        let mount_screen = host.mount_screen.clone();
        let release_screen = host.release_screen.clone();
        let active_changed = host.active_changed.clone();
        let insert_node = host.insert_node.clone();
        let clear_children = host.clear_children.clone();
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

                // Incoming: cache hit (Persistent, re-attach) or fresh mount.
                let cached = mounted_rc.borrow_mut().remove(name);
                let (incoming_node, incoming_scope, incoming_policy) = match cached {
                    Some(entry) => {
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

                // Outgoing: clear the outlet, then attach the incoming
                // screen. `clear_children` orphans a Persistent screen's
                // node (its scope stays alive in the cache for re-attach).
                let prev = current_rc.borrow_mut().take();
                if prev.is_some() {
                    clear_children(outlet_for_dispatch.clone());
                }
                insert_node(outlet_for_dispatch.clone(), incoming_node.clone());

                if let Some(prev) = prev {
                    match prev.effective_policy {
                        MountPolicy::LazyDisposing => release_screen(prev.scope_id),
                        MountPolicy::LazyPersistent | MountPolicy::EagerPersistent => {
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
                    "drawer Navigator received a stack-shaped NavCommand — \
                     drawer kind only accepts Select / Custom(DrawerCmd)"
                );
            }
        }));

        root_node
    }

    fn attach_initial(
        &mut self,
        backend: &mut B,
        screen: B::Node,
        scope_id: u64,
        options: Box<dyn Any>,
    ) {
        let Some(outlet) = self.outlet.clone() else { return };
        let mut outlet_mut = outlet;
        backend.insert(&mut outlet_mut, screen.clone());
        let name = self.initial_route.unwrap_or("");
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

    fn release(&mut self, _backend: &mut B) {
        *self.current.borrow_mut() = None;
        self.mounted.borrow_mut().clear();
        self.outlet = None;
    }

    fn make_handle(&self) -> runtime_core::NavigatorHandle {
        runtime_core::NavigatorHandle::new(Rc::new(()), &NOOP_DRAWER_OPS)
    }

    fn apply_slot_style(
        &mut self,
        backend: &mut B,
        slot: &'static str,
        style: &Rc<runtime_core::StyleRules>,
    ) {
        // Single-window persistent sidebar — no per-screen header chrome.
        // The "body" slot styles the outlet background (cross-platform
        // contract).
        if slot != "body" {
            return;
        }
        let Some(outlet) = self.outlet.clone() else { return };
        backend.apply_style(&outlet, style);
    }
}

/// Register the backend-neutral desktop drawer handler on any
/// primitive-rendering backend (the wgpu GPU backend today). Call once at
/// bootstrap so `Element::Navigator`s carrying a [`DrawerPresentation`]
/// resolve to the persistent-sidebar desktop chrome.
pub fn register<B: RegisterNavigator>(backend: &mut B) {
    backend.register_navigator::<DrawerPresentation, _>(|| Box::new(DesktopDrawerHandler::<B>::new()));
}
