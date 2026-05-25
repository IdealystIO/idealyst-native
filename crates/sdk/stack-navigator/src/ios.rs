//! iOS-backend handler for the Stack navigator SDK.
//!
//! Phase-1 adapter: synthesizes legacy `NavigatorCallbacks` and calls
//! `IosBackend::create_stack_navigator` (which drives `UINavigationController`).
//! Sets `NavigatorKind::Stack` on the resulting node so the backend's
//! unified `navigator_extension_*` trait method overrides route to the
//! stack storage map.

use crate::StackPresentation;
use backend_ios_mobile::{IosBackend, IosNode};
use runtime_core::{
    accessibility::AccessibilityProps, Backend, MountResult, NavigatorKind, NavigatorCallbacks,
    NavigatorHandler, NavigatorHost,
};
use std::any::Any;
use std::rc::Rc;

pub struct IosStackHandler;

impl IosStackHandler {
    pub fn new() -> Self {
        Self
    }
}
impl Default for IosStackHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl NavigatorHandler<IosBackend> for IosStackHandler {
    fn init(
        &mut self,
        backend: &mut IosBackend,
        host: NavigatorHost<IosNode>,
        _presentation: Rc<dyn Any>,
    ) -> IosNode {
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
            active_changed: _,
            control,
        } = host;

        let mount_2arg: Rc<dyn Fn(&'static str, Box<dyn Any>) -> MountResult<IosNode>> = {
            let m = mount_screen;
            Rc::new(move |name, params| m(name, params, None))
        };

        let callbacks = NavigatorCallbacks {
            initial_route,
            initial_path,
            mount_screen: mount_2arg,
            release_screen,
            match_path,
            build_layout,
            nav_state,
            depth_changed,
            defer_initial_mount,
        };

        let node = backend.create_stack_navigator(callbacks, control, &AccessibilityProps::default());
        backend.set_nav_kind(&node, NavigatorKind::Stack);
        node
    }

    fn attach_initial(
        &mut self,
        _backend: &mut IosBackend,
        _screen: IosNode,
        _scope_id: u64,
        _options: runtime_core::ScreenOptions,
    ) {
        unreachable!(
            "IosStackHandler::attach_initial — IosBackend routes via \
             navigator_attach_initial + nav_kind"
        );
    }

    fn on_command(&mut self, _cmd: runtime_core::NavCommand) {
        unreachable!(
            "IosStackHandler::on_command — legacy stack dispatcher owns \
             the control plane until Phase-2"
        );
    }
}

pub fn register(backend: &mut IosBackend) {
    backend.register_navigator::<StackPresentation, _>(|| Box::new(IosStackHandler::new()));
}
