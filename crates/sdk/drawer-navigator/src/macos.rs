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

use crate::{DrawerCmd, DrawerPresentation, DrawerSide, DrawerSlotProps};
use backend_macos::{with_global_backend, MacosBackend, MacosNode};
use runtime_core::primitives::navigator::{
    AmbientNavGuard, NavCommand, NavigatorHandler, NavigatorHost, NavigatorOps,
};
use runtime_core::{AlignItems, Backend, FlexDirection, Length, StyleRules};
use std::any::Any;
use std::cell::RefCell;
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
}

pub struct MacosDrawerHandler {
    container: Option<MacosNode>,
    outlet: Option<MacosNode>,
    sidebar: Option<MacosNode>,
    current: Rc<RefCell<Option<ScreenEntry>>>,
    initial_route: Option<&'static str>,
}

impl MacosDrawerHandler {
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

        // Materialise the sidebar Element via the SDK's builder.
        // Must run outside the outer backend borrow window per the
        // host docs — defer via `schedule_microtask`.
        let sidebar_slot = presentation.sidebar.borrow().clone();
        if let Some(sidebar_builder) = sidebar_slot {
            let build_node = host.build_node.clone();
            let control_for_sidebar = control.clone();
            let nav_state_for_sidebar = nav_state.clone();
            let sidebar_node = sidebar.clone();
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
                // Push the navigator onto the ambient stack so
                // `Link` primitives inside the sidebar capture this
                // navigator as their dispatch target — without it,
                // `ambient_navigator()` returns `None` and every
                // sidebar link's on_activate silently no-ops.
                // See `[[project_drawer_sidebar_ambient_nav]]`.
                let _guard = AmbientNavGuard::push(control_for_sidebar.clone());
                let prim = sidebar_builder(props);
                let sidebar_node_materialised = build_node(prim);
                with_global_backend(|b| {
                    let mut sb = sidebar_node.clone();
                    b.insert(&mut sb, sidebar_node_materialised);
                });
            });
        }

        // Install dispatcher. `Select` swaps the outlet's child;
        // `Custom(DrawerCmd::*)` flips `is_open`; stack-shaped
        // commands panic (drawer kind doesn't accept Push / Pop /
        // Replace / Reset).
        let current_rc = self.current.clone();
        let outlet_for_dispatch = outlet.clone();
        let dispatching = Rc::new(RefCell::new(false));
        let mount_screen = host.mount_screen.clone();
        let release_screen = host.release_screen.clone();
        let active_changed = host.active_changed.clone();

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

                let result = mount_screen(name, params, None);
                let prev = current_rc.borrow_mut().take();
                with_global_backend(|b| {
                    let mut outlet_node = outlet_for_dispatch.clone();
                    // Clear the outlet's current child (if any) before
                    // mounting the new screen. macOS's clear_children
                    // walks subviews + removes from Taffy, mirroring
                    // what the terminal handler's detach+destroy
                    // sequence does.
                    if prev.is_some() {
                        b.clear_children(&outlet_node);
                    }
                    b.insert(&mut outlet_node, result.node.clone());
                });
                *current_rc.borrow_mut() = Some(ScreenEntry {
                    node: result.node,
                    scope_id: result.scope_id,
                    name,
                });
                active_changed(name, url);
                if let Some(prev) = prev {
                    release_screen(prev.scope_id);
                }
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

        container
    }

    fn attach_initial(
        &mut self,
        backend: &mut MacosBackend,
        screen: MacosNode,
        scope_id: u64,
        _options: Box<dyn Any>,
    ) {
        let Some(outlet) = self.outlet.clone() else { return };
        let mut outlet_mut = outlet;
        backend.insert(&mut outlet_mut, screen.clone());
        let name = self.initial_route.unwrap_or("");
        *self.current.borrow_mut() = Some(ScreenEntry { node: screen, scope_id, name });
    }

    fn release(&mut self, _backend: &mut MacosBackend) {
        *self.current.borrow_mut() = None;
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

pub fn register(backend: &mut MacosBackend) {
    backend.register_navigator::<DrawerPresentation, _>(|| {
        Box::new(MacosDrawerHandler::new())
    });
}
