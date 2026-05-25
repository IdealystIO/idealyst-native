//! iOS-backend handler for the Drawer navigator SDK.
//!
//! Stores the navigator's container `IosNode` and `NavigatorControl`
//! clone from `init`, then implements the post-init `NavigatorHandler`
//! methods on top of the backend's existing per-kind inherent helpers.

use crate::{DrawerPresentation, DrawerSide, DrawerType, MountPolicy};
use backend_ios::{IosBackend, IosNode};
use runtime_core::{
    accessibility::AccessibilityProps,
    primitives::navigator::{DrawerContentProps, NavCommand, NavigatorOps},
    DrawerNavigatorCallbacks, DrawerSide as CoreDrawerSide, DrawerType as CoreDrawerType,
    MountPolicy as CoreMountPolicy, MountResult, NavigatorCallbacks, NavigatorControl,
    NavigatorHandle, NavigatorHandler, NavigatorHost,
};
use std::any::Any;
use std::rc::Rc;

pub struct IosDrawerHandler {
    container: Option<IosNode>,
    control: Option<Rc<NavigatorControl>>,
}

impl IosDrawerHandler {
    pub fn new() -> Self {
        Self { container: None, control: None }
    }
}
impl Default for IosDrawerHandler {
    fn default() -> Self {
        Self::new()
    }
}

fn side_to_core(s: DrawerSide) -> CoreDrawerSide {
    match s {
        DrawerSide::Start => CoreDrawerSide::Start,
        DrawerSide::End => CoreDrawerSide::End,
    }
}
fn type_to_core(t: DrawerType) -> CoreDrawerType {
    match t {
        DrawerType::Front => CoreDrawerType::Front,
        DrawerType::Slide => CoreDrawerType::Slide,
    }
}
fn mount_policy_to_core(m: MountPolicy) -> CoreMountPolicy {
    match m {
        MountPolicy::EagerPersistent => CoreMountPolicy::EagerPersistent,
        MountPolicy::LazyPersistent => CoreMountPolicy::LazyPersistent,
        MountPolicy::LazyDisposing => CoreMountPolicy::LazyDisposing,
    }
}

impl NavigatorHandler<IosBackend> for IosDrawerHandler {
    fn init(
        &mut self,
        backend: &mut IosBackend,
        host: NavigatorHost<IosNode>,
        presentation: Rc<dyn Any>,
    ) -> IosNode {
        let presentation = presentation
            .downcast::<DrawerPresentation>()
            .expect("IosDrawerHandler: presentation must be DrawerPresentation");

        let NavigatorHost {
            initial_route,
            initial_path,
            defer_initial_mount,
            mount_screen,
            release_screen,
            match_path,
            build_layout,
            nav_state,
            depth_changed,
            active_changed,
            control,
            build_node,
        } = host;

        let mount_2arg: Rc<dyn Fn(&'static str, Box<dyn Any>) -> MountResult<IosNode>> = {
            let m = mount_screen;
            Rc::new(move |name, params| m(name, params, None))
        };

        let navigator = NavigatorCallbacks {
            initial_route,
            initial_path,
            mount_screen: mount_2arg,
            release_screen,
            match_path,
            build_layout,
            nav_state: nav_state.clone(),
            depth_changed,
            defer_initial_mount,
        };

        // Shared with the SDK builder's `.layout(...)` wrap — see
        // `DrawerPresentation::is_open` in `lib.rs`.
        let is_open = presentation.is_open;
        let open_changed: Rc<dyn Fn(bool)> = {
            Rc::new(move |o| is_open.set(o))
        };

        let active_changed_legacy: Rc<dyn Fn(&'static str)> = {
            let ac = active_changed;
            Rc::new(move |name| ac(name, String::new()))
        };

        let drawer_callbacks = DrawerNavigatorCallbacks {
            navigator,
            side: side_to_core(presentation.side),
            drawer_type: type_to_core(presentation.drawer_type),
            drawer_width: presentation.drawer_width,
            swipe_to_open: presentation.swipe_to_open,
            mount_policy: mount_policy_to_core(presentation.mount_policy),
            is_open,
            // Drawer panel content rendered through the user's
            // `.layout(...)` instead (Phase-1 keeps the legacy posture).
            build_content: None,
            active_changed: active_changed_legacy,
            open_changed,
            background_color: None,
        };

        self.control = Some(control.clone());
        let node = backend.create_drawer_navigator(
            drawer_callbacks,
            control.clone(),
            &AccessibilityProps::default(),
        );
        self.container = Some(node.clone());

        // Sidebar build + attach — deferred so the outer
        // `backend.borrow_mut()` window (held across this `init` call)
        // is released before the walker re-enters via `build_node`.
        // The microtask fires on the next runloop turn via the
        // installed iOS scheduler (NSTimer-backed).
        if let Some(content_builder) = presentation.content.clone() {
            let active_route = nav_state.active_route;
            let is_open_sig = presentation.is_open;
            let control_for_select = control.clone();
            let control_for_close = control;
            let node_for_attach = node.clone();
            runtime_core::schedule_microtask(move || {
                let on_select: Rc<dyn Fn(&'static str)> = {
                    let c = control_for_select;
                    Rc::new(move |name| {
                        c.dispatch(NavCommand::Select {
                            name,
                            url: String::new(),
                            params: Box::new(()),
                            state: None,
                        });
                    })
                };
                let on_close: Rc<dyn Fn()> = {
                    let c = control_for_close;
                    Rc::new(move || c.dispatch(NavCommand::CloseDrawer))
                };
                let content_props = DrawerContentProps {
                    active_route,
                    is_open: is_open_sig,
                    on_select,
                    on_close,
                };
                let sidebar_primitive = content_builder(content_props);
                let sidebar_node = build_node(sidebar_primitive);
                // Attach via the iOS backend's existing helper. Reach
                // the live `IosBackend` through the installed global
                // self ref — `install_global_self` runs at host
                // bootstrap so by the time this microtask fires it's
                // always present.
                //
                // Layout-pass kick AFTER attach: Taffy sizing for the
                // sidebar's root happens at compute time, but
                // `attach_sidebar` only adds the subview — it doesn't
                // re-run the iOS layout pipeline. Without this kick,
                // the sidebar UIView lands with a 0×0 frame and the
                // user sees an "empty" drawer panel even though its
                // children are correctly registered with Taffy.
                backend_ios::with_backend(|b| {
                    b.drawer_navigator_attach_sidebar(&node_for_attach, sidebar_node);
                    b.run_layout();
                });
            });
        }

        node
    }

    fn attach_initial(
        &mut self,
        backend: &mut IosBackend,
        screen: IosNode,
        scope_id: u64,
        options: runtime_core::ScreenOptions,
    ) {
        if let Some(container) = self.container.clone() {
            backend.drawer_navigator_attach_initial(&container, screen, scope_id, options);
        }
    }

    fn on_command(&mut self, _cmd: runtime_core::NavCommand) {
        unreachable!(
            "IosDrawerHandler::on_command — backend.create_drawer_navigator owns \
             the control-plane dispatcher"
        );
    }

    fn release(&mut self, backend: &mut IosBackend) {
        if let Some(container) = self.container.take() {
            backend.release_drawer_navigator(&container);
        }
        self.control = None;
    }

    fn make_handle(&self) -> NavigatorHandle {
        match self.control.clone() {
            Some(c) => NavigatorHandle::with_control(Rc::new(()), &NoopDrawerOps, c),
            None => NavigatorHandle::new(Rc::new(()), &NoopDrawerOps),
        }
    }

    fn apply_slot_style(
        &mut self,
        backend: &mut IosBackend,
        slot: &'static str,
        style: &Rc<runtime_core::StyleRules>,
    ) {
        let Some(container) = self.container.clone() else { return };
        // iOS supports "sidebar" via the drawer's panel UIView background.
        // The "scrim" slot has no native iOS counterpart yet — drawer
        // chrome there is owned by the drawer overlay view, not a
        // separate scrim element.
        match slot {
            "sidebar" => backend.apply_drawer_sidebar_style(&container, style),
            _ => {}
        }
    }
}

struct NoopDrawerOps;
impl NavigatorOps for NoopDrawerOps {}

pub fn register(backend: &mut IosBackend) {
    backend.register_navigator::<DrawerPresentation, _>(|| Box::new(IosDrawerHandler::new()));
}
