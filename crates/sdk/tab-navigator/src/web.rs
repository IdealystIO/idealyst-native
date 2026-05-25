//! Web-backend handler for the Tab navigator SDK.
//!
//! Phase-1 adapter: synthesizes a legacy `TabNavigatorCallbacks` from
//! the framework-supplied `NavigatorHost` + the SDK's
//! `TabPresentation`, then calls `WebBackend::create_tab_navigator`.
//! Phase-2 will inline the dispatcher closure here.

use crate::{TabPresentation, TabPlacement, MountPolicy};
use backend_web::WebBackend;
use runtime_core::{
    primitives::navigator::tabs::TabRegistration, MountResult, NavigatorCallbacks,
    NavigatorHandler, NavigatorHost, TabNavigatorCallbacks,
    TabPlacement as CoreTabPlacement, MountPolicy as CoreMountPolicy,
};
use std::any::Any;
use std::rc::Rc;
use web_sys::Node;

pub struct WebTabHandler {
    /// Container `Node` returned from `helpers::create_tab`. Same
    /// posture as `WebStackHandler::container` — retained so the
    /// post-init dispatch can look the instance up by
    /// `data-navigator-id` later.
    container: Option<Node>,
}

impl WebTabHandler {
    pub fn new() -> Self {
        Self { container: None }
    }
}
impl Default for WebTabHandler {
    fn default() -> Self {
        Self::new()
    }
}

struct NoopTabOps;
impl runtime_core::primitives::navigator::NavigatorOps for NoopTabOps {}

/// Translate the SDK's `TabPlacement` enum to core's identical-shape
/// `TabPlacement`. The SDK defines its own copy so the SDK doesn't
/// have to re-export every kind-specific value type from core.
fn placement_to_core(p: TabPlacement) -> CoreTabPlacement {
    match p {
        TabPlacement::Auto => CoreTabPlacement::Auto,
        TabPlacement::Top => CoreTabPlacement::Top,
        TabPlacement::Bottom => CoreTabPlacement::Bottom,
        TabPlacement::Sidebar => CoreTabPlacement::Sidebar,
    }
}

fn mount_policy_to_core(m: MountPolicy) -> CoreMountPolicy {
    match m {
        MountPolicy::EagerPersistent => CoreMountPolicy::EagerPersistent,
        MountPolicy::LazyPersistent => CoreMountPolicy::LazyPersistent,
        MountPolicy::LazyDisposing => CoreMountPolicy::LazyDisposing,
    }
}

impl NavigatorHandler<WebBackend> for WebTabHandler {
    fn init(
        &mut self,
        backend: &mut WebBackend,
        host: NavigatorHost<Node>,
        presentation: Rc<dyn Any>,
    ) -> Node {
        let presentation = presentation
            .downcast::<TabPresentation>()
            .expect("WebTabHandler: presentation must be TabPresentation");

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
            build_node: _,
        } = host;

        // Adapter: 3-arg mount_screen → 2-arg; state discarded for the
        // legacy path. Same posture as WebStackHandler.
        let mount_2arg: Rc<dyn Fn(&'static str, Box<dyn Any>) -> MountResult<Node>> = {
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
            nav_state,
            depth_changed,
            defer_initial_mount,
        };

        // The TabPresentation Rc may still have other strong refs (the
        // Primitive::Navigator payload itself); clone the inner data
        // out instead of trying to Rc::try_unwrap.
        let tabs: Vec<TabRegistration> = presentation
            .tab_order
            .iter()
            .map(|(route, spec)| TabRegistration {
                route,
                label: spec.label.clone(),
                icon: spec.icon.clone(),
                badge: spec.badge.clone(),
            })
            .collect();

        // Bridge the host's typed `active_changed(name, path)` to the
        // legacy callback's `active_changed(name)` shape. The legacy
        // tab dispatcher already updates active_path via the route's
        // `url`, so the path arg is redundant for the existing impl.
        let active_changed_legacy: Rc<dyn Fn(&'static str)> = {
            let ac = active_changed;
            Rc::new(move |name| ac(name, String::new()))
        };

        let tab_callbacks = TabNavigatorCallbacks {
            navigator,
            tabs,
            placement: placement_to_core(presentation.placement),
            mount_policy: mount_policy_to_core(presentation.mount_policy),
            active_changed: active_changed_legacy,
        };

        let node = web_navigator_helpers::create_tab(backend, tab_callbacks, control);
        self.container = Some(node.clone());
        node
    }

    fn attach_initial(
        &mut self,
        _backend: &mut WebBackend,
        screen: Node,
        scope_id: u64,
        _options: runtime_core::ScreenOptions,
    ) {
        if let Some(container) = self.container.as_ref() {
            web_navigator_helpers::attach_initial(container, screen, scope_id);
        }
    }

    fn on_command(&mut self, _cmd: runtime_core::NavCommand) {
        unreachable!(
            "WebTabHandler::on_command — helpers::create_tab owns the \
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
            None => runtime_core::NavigatorHandle::new(Rc::new(()), &NoopTabOps),
        }
    }
}

pub fn register(backend: &mut WebBackend) {
    backend.register_navigator::<TabPresentation, _>(|| Box::new(WebTabHandler::new()));
}
