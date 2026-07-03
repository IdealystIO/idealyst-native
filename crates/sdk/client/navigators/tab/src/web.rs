//! Web-backend handler for the Tab navigator SDK.
//!
//! Synthesizes a `WebTabCallbacks` from the framework-supplied
//! `NavigatorHost` + the SDK's `TabPresentation`, then calls
//! `web_navigator_helpers::create_tab`. Kind-specific callback types
//! live in `web-navigator-helpers` after the navigator-substrate
//! refactor — the SDK's local `TabPlacement` / `MountPolicy` enums
//! translate to the helpers crate's identically-shaped variants via
//! the `placement_to_helpers` / `mount_policy_to_helpers` shims.

use crate::{MountPolicy, TabPlacement, TabPresentation};
use backend_web::WebBackend;
use runtime_core::primitives::navigator::{
    MountResult, NavigatorHandler, NavigatorHost,
};
use std::any::Any;
use std::rc::Rc;
use web_navigator_helpers::{
    MountPolicy as HelpersMountPolicy, TabPlacement as HelpersTabPlacement, TabRegistration,
    WebNavCallbacks, WebTabCallbacks,
};
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

/// Translate the SDK's `TabPlacement` enum to the helpers crate's
/// identically-shaped `TabPlacement`. The helpers crate only models
/// the two placements its layout engine knows how to position
/// (`Top` / `Bottom`); `Auto` and `Sidebar` collapse to `Bottom` /
/// `Top` for now — author chrome owns the actual positioning via
/// `.layout(...)`, so this mapping is informational on web.
fn placement_to_helpers(p: TabPlacement) -> HelpersTabPlacement {
    match p {
        TabPlacement::Top | TabPlacement::Sidebar => HelpersTabPlacement::Top,
        TabPlacement::Auto | TabPlacement::Bottom => HelpersTabPlacement::Bottom,
    }
}

/// Translate the SDK's `MountPolicy` to the helpers crate's. The
/// helpers crate models only `Lazy` / `Eager`; the SDK's two lazy
/// variants (`LazyPersistent` / `LazyDisposing`) both collapse to
/// `Lazy` — the helpers crate's screen-swap engine doesn't currently
/// implement persistent-but-hidden lazy mounting on web, so disposing
/// is the only honest fit.
fn mount_policy_to_helpers(m: MountPolicy) -> HelpersMountPolicy {
    match m {
        MountPolicy::EagerPersistent => HelpersMountPolicy::Eager,
        MountPolicy::LazyPersistent | MountPolicy::LazyDisposing => HelpersMountPolicy::Lazy,
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
            resolve_entry,
            base,
            nav_state,
            depth_changed,
            active_changed,
            control,
            build_node: _,
            build_node_scoped: _,
            build_node_into: _,
            build_in_screen: _,
            // Node-splice ops are for backend-neutral native handlers;
            // web drives layout via the DOM.
            ..
        } = host;

        // Adapter: 3-arg `mount_screen` → 2-arg; state is discarded
        // (the helpers crate's screen-swap engine doesn't currently
        // thread per-screen state into the URL stack).
        let mount_2arg: Rc<dyn Fn(&'static str, Box<dyn Any>) -> MountResult<Node>> = {
            let m = mount_screen;
            Rc::new(move |name, params| m(name, params, None))
        };

        let navigator = WebNavCallbacks {
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

        // Collect tab registrations for the helpers crate. The helper
        // itself doesn't render the tab bar (authors build their own
        // via `.layout(...)`); these are kept for any helper-side
        // lookups in the future.
        let tabs: Vec<TabRegistration> = presentation
            .tab_order
            .iter()
            .map(|(route, spec)| TabRegistration {
                route,
                path: "",
                label: Some(spec.label.clone()),
            })
            .collect();

        let tab_callbacks = WebTabCallbacks {
            navigator,
            tabs,
            placement: placement_to_helpers(presentation.placement),
            mount_policy: mount_policy_to_helpers(presentation.mount_policy),
            active_changed,
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
        _options: Box<dyn Any>,
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

/// Install the tab navigator handler on a web backend. Call once at
/// startup so `Element::Navigator`s carrying a [`TabPresentation`]
/// resolve to this backend's chrome.
pub fn register(backend: &mut WebBackend) {
    backend.register_navigator::<TabPresentation, _>(|| Box::new(WebTabHandler::new()));
}

// Self-register at backend construction (no app-side `register` call needed).
// See [[project_inventory_self_registration]].
inventory::submit! {
    backend_web::WebNavigatorRegistrar(register)
}
