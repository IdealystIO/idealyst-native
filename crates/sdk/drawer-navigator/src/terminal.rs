//! Terminal-backend handler for the Drawer navigator SDK.
//!
//! Minimalist by design (see `[[feedback_terminal_minimalism]]`): the
//! drawer is a persistent sidebar column next to the screen outlet,
//! with no animation, no scrim, and no Open/Close toggling — the
//! sidebar is always visible. `DrawerCmd::Open` / `Close` / `Toggle`
//! still flip the `is_open` signal (so the sidebar's reactive
//! renders see consistent state), but the layout never changes.
//!
//! Layout: `Row { sidebar; outlet }` (or `Row { outlet; sidebar }` if
//! `DrawerSide::End`). The sidebar has a fixed cell width derived
//! from `DrawerPresentation.drawer_width`; the outlet flexes to fill.
//!
//! Screen swap on `Select` mirrors the stack handler's `Replace` —
//! the outlet holds exactly one child at a time. The author renders
//! per-screen titles inside their page Element.

use crate::{DrawerCmd, DrawerPresentation, DrawerSide, DrawerSlotProps};
use backend_terminal::{TermNode, TerminalBackend};
use runtime_core::primitives::navigator::{
    AmbientNavGuard, NavCommand, NavigatorHandler, NavigatorHost, NavigatorOps,
};
use runtime_core::{AlignItems, Backend, FlexDirection, Length, StyleRules};
use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

struct ScreenEntry {
    node: TermNode,
    scope_id: u64,
    /// Route name of the currently-mounted screen — used by the
    /// dispatcher's same-route guard so reselecting the active route
    /// is a no-op (mirrors the web drawer helper's posture and
    /// avoids spurious mount→release→mount cycles that can drop the
    /// sidebar's reactive subscriptions).
    name: &'static str,
}

pub struct TerminalDrawerHandler {
    /// Outer Row container returned by `init`.
    container: Option<TermNode>,
    /// The screen-outlet column (right of sidebar for Start, left
    /// for End). All screen mounts attach as its child.
    outlet: Option<TermNode>,
    /// The sidebar column. Tracked so post-dispatch integrity checks
    /// can verify it's still a child of `container` and re-attach if
    /// not (defends against any future tree-mutation path that
    /// inadvertently detaches it).
    sidebar: Option<TermNode>,
    /// Most-recently-mounted screen — detached + released when a
    /// new Select replaces it.
    current: Rc<RefCell<Option<ScreenEntry>>>,
    /// Initial route name from the host. Captured at init so
    /// `attach_initial` (which doesn't get the route name from the
    /// framework) can seed the bookkeeping for the same-route guard.
    initial_route: Option<&'static str>,
}

impl TerminalDrawerHandler {
    pub fn new() -> Self {
        Self {
            container: None,
            outlet: None,
            sidebar: None,
            current: Rc::new(RefCell::new(None)),
            initial_route: None,
        }
    }
}

impl Default for TerminalDrawerHandler {
    fn default() -> Self {
        Self::new()
    }
}

struct NoopDrawerOps;
impl NavigatorOps for NoopDrawerOps {}
static NOOP_DRAWER_OPS: NoopDrawerOps = NoopDrawerOps;

impl NavigatorHandler<TerminalBackend> for TerminalDrawerHandler {
    fn init(
        &mut self,
        backend: &mut TerminalBackend,
        host: NavigatorHost<TermNode>,
        presentation: Rc<dyn Any>,
    ) -> TermNode {
        let presentation = presentation
            .downcast::<DrawerPresentation>()
            .expect("TerminalDrawerHandler: presentation must be DrawerPresentation");

        // Outer Row container. `width: 100%, height: 100%, flex_direction: Row`.
        let mut container = backend.create_view(&Default::default());
        let mut container_style = StyleRules::default();
        container_style.flex_direction = Some(FlexDirection::Row);
        container_style.align_items = Some(AlignItems::Stretch);
        container_style.width = Some(Length::pct(100.0).into());
        container_style.height = Some(Length::pct(100.0).into());
        backend.apply_style(&container, &Rc::new(container_style));

        // Sidebar column — fixed cell width. `flex_shrink: 0` is the
        // load-bearing rule: without it, when the active screen has
        // a wide intrinsic content (e.g. a code-block line that
        // doesn't wrap), Taffy's Row-flex squashing kicks in and
        // shrinks BOTH the sidebar and the outlet proportionally to
        // fit the viewport. With wide-enough content the sidebar
        // collapses to zero and the user sees it "vanish" after
        // navigating to a content-heavy page.
        let sidebar = backend.create_view(&Default::default());
        let mut sidebar_style = StyleRules::default();
        sidebar_style.width = Some(Length::Px(presentation.drawer_width).into());
        sidebar_style.height = Some(Length::pct(100.0).into());
        sidebar_style.flex_direction = Some(FlexDirection::Column);
        sidebar_style.flex_shrink = Some(0.0f32.into());
        backend.apply_style(&sidebar, &Rc::new(sidebar_style));

        // Outlet — flex-grow into remaining width. `flex_basis: 0`
        // (paired with `flex_grow: 1`) means the outlet contributes
        // nothing to the initial basis sum; the sidebar's fixed
        // width is the only basis, and the outlet then absorbs
        // every remaining cell. Without `flex_basis: 0` the default
        // `auto` resolves to the screen content's intrinsic width
        // (often very wide for code blocks), inflating the basis
        // sum past the viewport and triggering shrinkage.
        let outlet = backend.create_view(&Default::default());
        let mut outlet_style = StyleRules::default();
        outlet_style.flex_grow = Some(1.0f32.into());
        outlet_style.flex_basis = Some(Length::Px(0.0).into());
        outlet_style.height = Some(Length::pct(100.0).into());
        outlet_style.flex_direction = Some(FlexDirection::Column);
        backend.apply_style(&outlet, &Rc::new(outlet_style));

        // Insert order respects DrawerSide.
        match presentation.side {
            DrawerSide::Start => {
                backend.insert(&mut container, sidebar);
                backend.insert(&mut container, outlet);
            }
            DrawerSide::End => {
                backend.insert(&mut container, outlet);
                backend.insert(&mut container, sidebar);
            }
        }

        self.container = Some(container);
        self.outlet = Some(outlet);
        self.sidebar = Some(sidebar);
        self.initial_route = Some(host.initial_route);

        let nav_state = host.nav_state.clone();
        let control = host.control.clone();
        let is_open = presentation.is_open;

        // Map `Link(route=...)` activations to `Select` (drawer-shape)
        // instead of the substrate default `Push` (stack-shape). Without
        // this, clicking a sidebar nav link would dispatch `Push`, which
        // the dispatcher below panics on. Same fix as iOS / Android /
        // web drawer helpers.
        let select_activator: Rc<
            dyn Fn(&'static str, String, Box<dyn Any>) -> NavCommand,
        > = Rc::new(|name, url, params| NavCommand::Select {
            name,
            url,
            params,
            state: None,
        });
        control.install_link_activator(select_activator);

        // Materialize the sidebar content via the SDK's builder. The
        // builder takes typed DrawerSlotProps and returns a Element;
        // `host.build_node` realizes it as a TermNode. The call MUST
        // run outside the outer backend borrow window (per host docs),
        // so we defer through `schedule_microtask`.
        let sidebar_slot = presentation.sidebar.borrow().clone();
        if let Some(sidebar_builder) = sidebar_slot {
            let build_node = host.build_node.clone();
            let control_for_sidebar = control.clone();
            let nav_state_for_sidebar = nav_state.clone();
            runtime_core::schedule_microtask(move || {
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
                let on_close: Rc<dyn Fn()> = {
                    let control = control_for_sidebar.clone();
                    Rc::new(move || {
                        control.dispatch(NavCommand::Custom(Rc::new(
                            DrawerCmd::Close,
                        )));
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
                // Push the navigator onto the ambient stack so `Link`
                // primitives built inside the sidebar capture this
                // navigator as their dispatch target. Same fix as the
                // iOS / Android / web drawer handlers — without it,
                // ambient_navigator() returns None and every Link's
                // on_activate silently no-ops. See
                // [[project_drawer_sidebar_ambient_nav]].
                let _guard = AmbientNavGuard::push(control_for_sidebar.clone());
                let prim = sidebar_builder(props);
                let sidebar_node_materialized = build_node(prim);
                backend_terminal::with_global_backend(|b| {
                    let mut sb = sidebar;
                    b.insert(&mut sb, sidebar_node_materialized);
                });
            });
        }

        // Install dispatcher. `Select` swaps the outlet's child;
        // `Custom(DrawerCmd::*)` flips `is_open` but doesn't change
        // layout (the drawer is always visible in the terminal).
        //
        // `dispatching` is a re-entrancy guard. If a Select dispatch
        // somehow re-enters itself (because an effect fired by the
        // active_route signal-set chain ends up calling back into
        // the navigator), the inner call no-ops instead of mounting
        // a second screen on top of the first. The outer call's
        // mid-state (current_rc taken, prev not yet released) is
        // not safe to observe.
        let current_rc = self.current.clone();
        let outlet_for_dispatch = outlet;
        let container_for_dispatch = container;
        let sidebar_for_dispatch = sidebar;
        let dispatching = Rc::new(RefCell::new(false));
        let mount_screen = host.mount_screen.clone();
        let release_screen = host.release_screen.clone();
        let active_changed = host.active_changed.clone();

        control.install(Box::new(move |cmd| match cmd {
            NavCommand::Select { name, url, params, .. } => {
                // Re-entrancy guard. If this Select dispatch fires
                // while a previous Select is still in flight (some
                // signal subscriber on the active-route chain
                // re-enters us), bail. The outer call finishes the
                // commit; the inner would observe half-swapped state
                // (current_rc taken, prev not yet released) and
                // either tear it down or duplicate the mount.
                if *dispatching.borrow() {
                    drop(params);
                    return;
                }
                // Reselecting the currently-active route is also a
                // no-op (matches the web drawer helper's `paths_equal`
                // early return). Without this, clicking the active
                // sidebar link triggers a mount→detach→release→insert
                // cycle that can drop reactive subscriptions wired
                // to the outgoing scope.
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

                let result = mount_screen(name, params, None);
                let prev = current_rc.borrow_mut().take();
                // Order: commit visible state FIRST (detach old,
                // insert new, update bookkeeping, fire
                // active_changed), then release the previous scope
                // LAST. Any cleanup path that tries to re-enter the
                // dispatcher hits the guard above.
                backend_terminal::with_global_backend(|b| {
                    if let Some(ref prev) = prev {
                        b.detach_child(&outlet_for_dispatch, &prev.node);
                    }
                    let mut outlet = outlet_for_dispatch;
                    b.insert(&mut outlet, result.node);

                    // Sidebar integrity check + auto-repair. Catches
                    // the case where any tree-mutation path detaches
                    // the sidebar from the container. Logs the
                    // recovery so the root cause stays findable.
                    let sb = sidebar_for_dispatch;
                    let container_children =
                        b.children_of_for_log(container_for_dispatch);
                    if !container_children.contains(&sb.id) {
                        eprintln!(
                            "[drawer-terminal] sidebar DETACHED during \
                             swap to {name} — re-attaching"
                        );
                        let mut ct = container_for_dispatch;
                        b.insert(&mut ct, sb);
                    }
                });
                *current_rc.borrow_mut() = Some(ScreenEntry {
                    node: result.node,
                    scope_id: result.scope_id,
                    name,
                });
                active_changed(name, url);
                if let Some(prev) = prev {
                    release_screen(prev.scope_id);
                    // Tear down the previous screen's backend
                    // nodes + Taffy slots. detach_child only
                    // removed the parent edge — the screen's
                    // root remained a parentless Taffy root,
                    // which `find_root` could non-deterministically
                    // pick instead of the container. When that
                    // happened, paint walked the OLD screen as the
                    // root and rendered its content at viewport
                    // origin, overlaying the sidebar.
                    backend_terminal::with_global_backend(|b| {
                        b.destroy_subtree(prev.node);
                    });
                }
                *dispatching.borrow_mut() = false;
            }
            NavCommand::Custom(payload) => {
                // Honor Open/Close/Toggle by updating the shared
                // signal so reactive sidebars (highlighting,
                // disclosure indicators) stay coherent — but the
                // terminal layout itself never changes.
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

        container
    }

    fn attach_initial(
        &mut self,
        backend: &mut TerminalBackend,
        screen: TermNode,
        scope_id: u64,
        _options: Box<dyn Any>,
    ) {
        let Some(mut outlet) = self.outlet else { return };
        backend.insert(&mut outlet, screen);
        // Seed the bookkeeping with the navigator's `initial_route`,
        // captured from the host at init. Without this, the
        // same-route guard in the Select dispatcher can't recognise
        // a reselect of the initial screen and falls back into the
        // remount-then-release cycle that drops the sidebar.
        let name = self
            .initial_route
            .unwrap_or("");
        *self.current.borrow_mut() = Some(ScreenEntry { node: screen, scope_id, name });
    }

    fn release(&mut self, _backend: &mut TerminalBackend) {
        // Framework drops the navigator's enclosing scope, which
        // releases every screen scope under it. Just clear local
        // bookkeeping; layout nodes drop with the container's parent.
        *self.current.borrow_mut() = None;
        self.outlet = None;
        self.container = None;
    }

    fn make_handle(&self) -> runtime_core::NavigatorHandle {
        runtime_core::NavigatorHandle::new(Rc::new(()), &NOOP_DRAWER_OPS)
    }

    fn apply_slot_style(
        &mut self,
        backend: &mut TerminalBackend,
        slot: &'static str,
        style: &Rc<runtime_core::StyleRules>,
    ) {
        // The terminal renders no header chrome (per
        // [[feedback_terminal_minimalism]]), so the "header" / "title"
        // / "button" slots are no-ops. The "body" slot paints the
        // screen-outlet's background — same contract as the other
        // backends so themed `HeaderStyle.body_background` reaches the
        // terminal too.
        if slot != "body" {
            return;
        }
        let Some(outlet) = self.outlet else { return };
        // Forward as a normal apply_style — the terminal renderer
        // already honors `bg` on a View, and the existing apply_style
        // path parses the StyleRules.background token + caches the
        // resolved Rgba on the NodeData.
        backend.apply_style(&outlet, style);
    }
}

pub fn register(backend: &mut TerminalBackend) {
    backend.register_navigator::<DrawerPresentation, _>(|| {
        Box::new(TerminalDrawerHandler::new())
    });
}
