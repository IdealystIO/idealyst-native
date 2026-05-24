//! Web-backend handler for the Stack navigator SDK.
//!
//! Phase-1 adapter: thin handler whose `init` synthesizes the legacy
//! `NavigatorCallbacks` from the framework-supplied `NavigatorHost`
//! and calls `WebBackend::create_navigator` directly. The legacy
//! `Backend::create_navigator` impl already installs the right
//! dispatcher on the control plane, so `on_command` is unreachable.
//!
//! Phase-2 (later): port the dispatcher closure + DOM machinery from
//! `backend-web/src/primitives/navigator.rs::create_navigator` inline
//! and drop the dependency on the legacy method. The architecture is
//! the same — only the wiring shifts.

use crate::StackPresentation;
use backend_web::WebBackend;
use runtime_core::{
    accessibility::AccessibilityProps, Backend, MountResult, NavigatorCallbacks, NavigatorHandler,
    NavigatorHost,
};
use std::any::Any;
use std::rc::Rc;
use web_sys::Node;

pub struct WebStackHandler;

impl WebStackHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for WebStackHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl NavigatorHandler<WebBackend> for WebStackHandler {
    fn init(
        &mut self,
        backend: &mut WebBackend,
        host: NavigatorHost<Node>,
        _presentation: Rc<dyn Any>,
    ) -> Node {
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

        // Adapter: the legacy `NavigatorCallbacks::mount_screen` is
        // 2-arg `(name, params)`; the host's is 3-arg `(name, params,
        // state)`. State is discarded for the legacy path — no current
        // first-party stack consumer reads `current_screen_state()`,
        // and Phase-2 will get a real handler that threads it through.
        let mount_2arg: Rc<dyn Fn(&'static str, Box<dyn Any>) -> MountResult<Node>> = {
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

        backend.create_navigator(callbacks, control, &AccessibilityProps::default())
    }

    fn attach_initial(
        &mut self,
        _backend: &mut WebBackend,
        _screen: Node,
        _scope_id: u64,
        _options: runtime_core::ScreenOptions,
    ) {
        // The web backend's `Backend::navigator_extension_attach_initial`
        // impl delegates directly to `primitives::navigator::attach_initial`
        // (the legacy machinery is uniform across kinds). The handler
        // never sees this call — kept here so the trait is satisfied
        // and Phase-2 handlers can take it over.
        unreachable!(
            "WebStackHandler::attach_initial — WebBackend dispatches \
             this directly; handler doesn't see the call"
        );
    }

    fn on_command(&mut self, _cmd: runtime_core::NavCommand) {
        // Legacy `create_navigator` already installed the stack
        // dispatcher on the control plane. Phase-2 will take this over.
        unreachable!(
            "WebStackHandler::on_command — legacy stack dispatcher \
             owns the control plane until the Phase-2 port lands"
        );
    }
}

/// Register the Stack navigator handler factory with `backend`. Call
/// once during app bootstrap before mounting any UI that uses
/// `stack_navigator::Navigator::new(...)`.
pub fn register(backend: &mut WebBackend) {
    backend.register_navigator::<StackPresentation, _>(|| Box::new(WebStackHandler::new()));
}
