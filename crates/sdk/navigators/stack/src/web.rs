//! Web-backend handler for the Stack navigator SDK.
//!
//! The DOM + history-API machinery (`NavigatorInstance`, popstate
//! reconciliation, mount/release helpers) lives in the
//! `web-navigator-helpers` crate, shared with tab + drawer. This
//! module's `WebStackHandler` is a thin wrapper: it constructs a
//! `WebNavCallbacks` from the framework-supplied `NavigatorHost`,
//! drives the helpers crate's `create()` at init time, retains the
//! returned container `Node`, and forwards subsequent post-init
//! dispatch (`attach_initial` / `release` / `make_handle`) to the
//! matching helpers entry point.
//!
//! After the navigator-substrate refactor, the kind-specific callback
//! bundle types (`WebNavCallbacks`, ...) live in `web-navigator-helpers`
//! itself — runtime-core no longer ships any of these.

use crate::StackPresentation;
use backend_web::WebBackend;
use runtime_core::primitives::navigator::{
    MountResult, NavigatorHandler, NavigatorHost,
};
use std::any::Any;
use std::rc::Rc;
use web_sys::Node;
use web_navigator_helpers::WebNavCallbacks;

pub struct WebStackHandler {
    /// Container `Node` the helpers crate returns from `create()`.
    /// Stored so `attach_initial` / `release` / `make_handle` can look
    /// the instance back up by `data-navigator-id` without having to
    /// thread the node down from the framework's dispatch site.
    container: Option<Node>,
}

impl WebStackHandler {
    pub fn new() -> Self {
        Self { container: None }
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
            resolve_entry,
            base,
            nav_state,
            depth_changed,
            active_changed: _,
            control,
            build_node: _,
            build_node_into: _,
            build_in_screen: _,
        } = host;

        // Adapter: the helpers-crate `WebNavCallbacks::mount_screen` is
        // 2-arg `(name, params)`; the substrate's host is 3-arg
        // `(name, params, state)`. Discard `state` for the stack-on-web
        // path — the helpers crate doesn't currently thread per-screen
        // state into the URL stack, and no first-party stack screen on
        // web reads `current_screen_state()`.
        let mount_2arg: Rc<dyn Fn(&'static str, Box<dyn Any>) -> MountResult<Node>> = {
            let m = mount_screen;
            Rc::new(move |name, params| m(name, params, None))
        };

        let callbacks = WebNavCallbacks {
            initial_route,
            initial_path,
            mount_screen: mount_2arg,
            release_screen,
            match_path,
            base,
            resolve_entry,
            depth_changed,
            nav_state,
            build_layout: None,
            defer_initial_mount,
        };

        let node = web_navigator_helpers::create(backend, callbacks, control);
        self.container = Some(node.clone());
        node
    }

    fn attach_initial(
        &mut self,
        _backend: &mut WebBackend,
        screen: Node,
        scope_id: u64,
        _options: Box<dyn Any>,
    ) {
        if let Some(container) = self.container.as_ref() {
            web_navigator_helpers::attach_initial(container, screen, scope_id);
        }
    }

    fn on_command(&mut self, _cmd: runtime_core::NavCommand) {
        // `web_navigator_helpers::create` installs the stack dispatcher
        // closure on the control plane during init, so commands route
        // directly through that closure instead of back through the
        // handler.
        unreachable!(
            "WebStackHandler::on_command — helpers::create owns the \
             control-plane dispatcher"
        );
    }

    fn release(&mut self, _backend: &mut WebBackend) {
        if let Some(container) = self.container.take() {
            web_navigator_helpers::release(&container);
        }
    }

    fn make_handle(&self) -> runtime_core::NavigatorHandle {
        match self.container.as_ref() {
            Some(c) => web_navigator_helpers::make_handle(c),
            None => runtime_core::NavigatorHandle::new(
                Rc::new(()),
                &NoopStackOps,
            ),
        }
    }

    fn apply_slot_style(
        &mut self,
        _backend: &mut WebBackend,
        slot: &'static str,
        style: &Rc<runtime_core::StyleRules>,
    ) {
        let Some(container) = self.container.clone() else { return };
        match slot {
            // `body` paints the screen-outlet div's background, matching
            // Android's `apply_body_style`. Header/title/button slots
            // belong to the per-screen native chrome that the web stack
            // currently delegates to the framework's normal style pass.
            "body" => web_navigator_helpers::apply_body_style(&container, style),
            _ => {}
        }
    }
}

struct NoopStackOps;
impl runtime_core::primitives::navigator::NavigatorOps for NoopStackOps {}

/// Register the Stack navigator handler factory with `backend`. Call
/// once during app bootstrap before mounting any UI that uses
/// `stack_navigator::Navigator::new(...)`.
pub fn register(backend: &mut WebBackend) {
    backend.register_navigator::<StackPresentation, _>(|| Box::new(WebStackHandler::new()));
}
