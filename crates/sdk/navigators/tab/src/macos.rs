//! macOS-backend handler for the Tab navigator SDK.
//!
//! Layout: Column { tabbar; outlet } (or Column { outlet; tabbar }
//! for `TabPlacement::Bottom`). Tabbar is a horizontal row of
//! `Button` controls; clicking dispatches `Select { name }`. Outlet
//! swaps its child on each `Select`.
//!
//! Per `project_macos_navigator_design` there's no animated tab
//! transition — the swap is instant. Active-tab visual state is
//! deferred to the SDK's theme integration; the macOS handler ships
//! the structural layout + dispatch, and authors can style the
//! active button via their stylesheet's state-overlay rules once
//! that wiring lands. (Same posture as terminal: structure first,
//! affordances second.)

use crate::{TabPresentation, TabPlacement};
use backend_macos::{with_global_backend, MacosBackend, MacosNode};
use runtime_core::primitives::navigator::{
    NavCommand, NavigatorControl, NavigatorHandler, NavigatorHost, NavigatorOps,
};
use runtime_core::{
    Action, AlignItems, Backend, FlexDirection, Length, StyleRules,
};
use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

struct ScreenEntry {
    node: MacosNode,
    scope_id: u64,
    name: &'static str,
}

pub struct MacosTabHandler {
    container: Option<MacosNode>,
    outlet: Option<MacosNode>,
    tabbar: Option<MacosNode>,
    current: Rc<RefCell<Option<ScreenEntry>>>,
    initial_route: Option<&'static str>,
    control: Option<Rc<NavigatorControl>>,
}

impl MacosTabHandler {
    pub fn new() -> Self {
        Self {
            container: None,
            outlet: None,
            tabbar: None,
            current: Rc::new(RefCell::new(None)),
            initial_route: None,
            control: None,
        }
    }
}

impl Default for MacosTabHandler {
    fn default() -> Self {
        Self::new()
    }
}

struct NoopTabOps;
impl NavigatorOps for NoopTabOps {}
static NOOP_TAB_OPS: NoopTabOps = NoopTabOps;

impl NavigatorHandler<MacosBackend> for MacosTabHandler {
    fn init(
        &mut self,
        backend: &mut MacosBackend,
        host: NavigatorHost<MacosNode>,
        presentation: Rc<dyn Any>,
    ) -> MacosNode {
        let presentation = presentation
            .downcast::<TabPresentation>()
            .expect("MacosTabHandler: presentation must be TabPresentation");

        // Outer Column container — full size.
        let mut container = backend.create_view(&Default::default());
        let mut container_style = StyleRules::default();
        container_style.flex_direction = Some(FlexDirection::Column);
        container_style.align_items = Some(AlignItems::Stretch);
        container_style.width = Some(Length::pct(100.0).into());
        container_style.height = Some(Length::pct(100.0).into());
        backend.apply_style(&container, &Rc::new(container_style));

        // Tabbar — Row of Button widgets, fixed height. `flex_shrink:
        // 0` so wide outlet content can't squash it. Height is
        // intrinsic from NSButton; we don't pin it here so the
        // platform-default chrome height takes effect.
        let tabbar = backend.create_view(&Default::default());
        let mut tabbar_style = StyleRules::default();
        tabbar_style.flex_direction = Some(FlexDirection::Row);
        tabbar_style.width = Some(Length::pct(100.0).into());
        tabbar_style.flex_shrink = Some(0.0f32.into());
        backend.apply_style(&tabbar, &Rc::new(tabbar_style));

        // Outlet — flex-grow into remaining height.
        let outlet = backend.create_view(&Default::default());
        let mut outlet_style = StyleRules::default();
        outlet_style.flex_grow = Some(1.0f32.into());
        outlet_style.flex_basis = Some(Length::Px(0.0).into());
        outlet_style.width = Some(Length::pct(100.0).into());
        outlet_style.flex_direction = Some(FlexDirection::Column);
        backend.apply_style(&outlet, &Rc::new(outlet_style));

        // Build the tab buttons. Each button dispatches Select with
        // the matching route name. We capture each name via a
        // `Rc<Fn()>` closure keyed by `name` (the &'static str the
        // SDK stored on `tab_order`).
        let control = host.control.clone();
        let mut tabbar_mut = tabbar.clone();
        for (name, spec) in &presentation.tab_order {
            let on_click_control = control.clone();
            let route_name = *name;
            let on_click: Rc<dyn Fn()> = Rc::new(move || {
                on_click_control.dispatch(NavCommand::Select {
                    name: route_name,
                    url: String::new(),
                    params: Box::new(()),
                    state: None,
                });
            });
            // Wrap in an IntoAction via the closure path — the
            // SDK's Action doesn't need typed inputs here.
            let action = runtime_core::IntoAction::into_action(move || (on_click)());
            let button = backend.create_button(
                &spec.label,
                &action,
                None,
                None,
                &Default::default(),
            );
            backend.insert(&mut tabbar_mut, button);
        }

        // Insert tabbar + outlet in placement order.
        match presentation.placement {
            TabPlacement::Bottom => {
                backend.insert(&mut container, outlet.clone());
                backend.insert(&mut container, tabbar.clone());
            }
            _ => {
                // Top / Auto / Sidebar — for macOS we treat
                // anything non-Bottom as a top tabbar. A true
                // Sidebar layout would mirror the drawer-navigator's
                // Row layout; deferring that variant to keep this
                // handler small.
                backend.insert(&mut container, tabbar.clone());
                backend.insert(&mut container, outlet.clone());
            }
        }

        self.container = Some(container.clone());
        self.outlet = Some(outlet.clone());
        self.tabbar = Some(tabbar);
        self.initial_route = Some(host.initial_route);
        self.control = Some(control.clone());

        // Map `Link(route=…)` activations to `Select` so sidebar /
        // in-content links activate the correct tab. Same pattern
        // as drawer's link_activator.
        let select_activator: Rc<
            dyn Fn(&'static str, String, Box<dyn Any>) -> NavCommand,
        > = Rc::new(|name, url, params| NavCommand::Select {
            name,
            url,
            params,
            state: None,
        });
        control.install_link_activator(select_activator);

        let current_rc = self.current.clone();
        let outlet_for_dispatch = outlet;
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
            NavCommand::Custom(_) => {
                // Tab navigators don't define a Custom vocabulary
                // today. Silently ignore — different from drawer
                // which uses Custom for Open/Close/Toggle.
            }
            NavCommand::Push { .. }
            | NavCommand::Pop
            | NavCommand::Replace { .. }
            | NavCommand::Reset { .. } => {
                panic!(
                    "tab Navigator received a stack-shaped NavCommand on \
                     macOS — tab kind only accepts Select"
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
        self.tabbar = None;
        self.container = None;
        self.control = None;
    }

    fn make_handle(&self) -> runtime_core::NavigatorHandle {
        match self.control.as_ref() {
            Some(c) => runtime_core::NavigatorHandle::with_control(
                Rc::new(()),
                &NOOP_TAB_OPS,
                c.clone(),
            ),
            None => runtime_core::NavigatorHandle::new(Rc::new(()), &NOOP_TAB_OPS),
        }
    }
}

/// Install the tab navigator handler on a macOS backend. Call once at
/// startup so `Element::Navigator`s carrying a [`TabPresentation`]
/// resolve to this backend's chrome.
pub fn register(backend: &mut MacosBackend) {
    backend.register_navigator::<TabPresentation, _>(|| {
        Box::new(MacosTabHandler::new())
    });
}

// Self-register at backend construction (no app-side `register` call needed).
// See [[project_inventory_self_registration]].
inventory::submit! {
    backend_macos::MacosNavigatorRegistrar(register)
}
