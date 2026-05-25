//! Web-backend handler for the Drawer navigator SDK.
//!
//! Synthesizes a `WebDrawerCallbacks` from the framework-supplied
//! `NavigatorHost` + the SDK's `DrawerPresentation`, then calls
//! `web_navigator_helpers::create_drawer`. Kind-specific callback
//! types live in `web-navigator-helpers` after the navigator-substrate
//! refactor — the SDK's local `DrawerSide` / `DrawerType` /
//! `MountPolicy` enums translate to the helpers crate's
//! identically-shaped variants via the per-enum shims below.
//!
//! Sidebar materialization: the SDK's `DrawerPresentation.sidebar`
//! slot holds a `SidebarBuilder` (closure that takes
//! `DrawerSlotProps` and returns a `Primitive`). The web handler
//! wraps it in a `Fn() -> Node` closure that defers to a microtask,
//! invokes `host.build_node` against the synthesized props, and
//! returns the materialized Node. The closure is handed to the
//! helpers crate via `WebDrawerCallbacks.build_content` for the
//! helper engine to mount alongside the screen outlet.

use crate::{
    DrawerCmd, DrawerPresentation, DrawerSide, DrawerSlotProps, DrawerType, MountPolicy,
};
use backend_web::WebBackend;
use runtime_core::primitives::navigator::{
    MountResult, NavCommand, NavigatorHandler, NavigatorHost,
};
use std::any::Any;
use std::rc::Rc;
use web_navigator_helpers::{
    DrawerSide as HelpersDrawerSide, DrawerType as HelpersDrawerType,
    MountPolicy as HelpersMountPolicy, WebDrawerCallbacks, WebNavCallbacks,
};
use web_sys::Node;

pub struct WebDrawerHandler {
    /// Container `Node` returned by `helpers::create_drawer`. Same
    /// posture as the stack/tab handlers — retained for post-init
    /// dispatch.
    container: Option<Node>,
}

impl WebDrawerHandler {
    pub fn new() -> Self {
        Self { container: None }
    }
}
impl Default for WebDrawerHandler {
    fn default() -> Self {
        Self::new()
    }
}

struct NoopDrawerOps;
impl runtime_core::primitives::navigator::NavigatorOps for NoopDrawerOps {}

fn side_to_helpers(s: DrawerSide) -> HelpersDrawerSide {
    match s {
        DrawerSide::Start => HelpersDrawerSide::Left,
        DrawerSide::End => HelpersDrawerSide::Right,
    }
}

fn type_to_helpers(t: DrawerType) -> HelpersDrawerType {
    match t {
        // SDK's `Front` (slides over content with backdrop) maps to
        // the helpers crate's `Overlay`; SDK's `Slide` (pushes
        // content sideways) maps to `Slide`. The third helpers
        // variant `Permanent` is exposed only via SDK `drawer_type`
        // = a future "always visible" variant, which the SDK doesn't
        // currently expose.
        DrawerType::Front => HelpersDrawerType::Overlay,
        DrawerType::Slide => HelpersDrawerType::Slide,
    }
}

fn mount_policy_to_helpers(m: MountPolicy) -> HelpersMountPolicy {
    match m {
        MountPolicy::EagerPersistent => HelpersMountPolicy::Eager,
        MountPolicy::LazyPersistent | MountPolicy::LazyDisposing => HelpersMountPolicy::Lazy,
    }
}

impl NavigatorHandler<WebBackend> for WebDrawerHandler {
    fn init(
        &mut self,
        backend: &mut WebBackend,
        host: NavigatorHost<Node>,
        presentation: Rc<dyn Any>,
    ) -> Node {
        let presentation = presentation
            .downcast::<DrawerPresentation>()
            .expect("WebDrawerHandler: presentation must be DrawerPresentation");

        let NavigatorHost {
            initial_route,
            initial_path,
            defer_initial_mount,
            mount_screen,
            release_screen,
            match_path,
            nav_state,
            depth_changed,
            active_changed,
            control,
            build_node,
            build_in_screen: _,
        } = host;

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
            depth_changed,
            // Pass the substrate's reactive `nav_state` straight through;
            // the helpers engine updates active_route / active_path as
            // screens mount, and the sidebar builder's
            // `DrawerSlotProps` mirrors them.
            nav_state: nav_state.clone(),
            build_layout: None,
            defer_initial_mount,
        };

        // Capture the shared open-state signal from the presentation —
        // it's the SAME `Signal<bool>` the `DrawerHandle` exposes via
        // `is_open_signal()` and that the SDK's dispatcher flips on
        // `DrawerCmd::Open/Close/Toggle`. Stash a copy for the
        // change-observer closure below.
        let is_open = presentation.is_open;
        let open_changed: Rc<dyn Fn(bool)> = {
            let signal = is_open;
            Rc::new(move |o| signal.set(o))
        };

        // Build a `build_content` closure for the helpers crate. The
        // SDK's `SidebarBuilder` takes typed `DrawerSlotProps` and
        // returns a `Primitive`; the helpers crate's slot expects a
        // `Fn() -> Node` (no args, returns the materialized node
        // directly). Bridge the two:
        //   1. Pull the SDK's `SidebarBuilder` out of the presentation
        //      slot. It's wrapped in `RefCell<Option<...>>` so the
        //      closure can `take()` on first invocation (sidebars are
        //      built exactly once per navigator lifetime).
        //   2. Synthesize `DrawerSlotProps` from the substrate's
        //      reactive `nav_state` + the shared `is_open` signal +
        //      a `Select` dispatcher + a close callback.
        //   3. Invoke `host.build_node` to materialize the SDK's
        //      returned `Primitive` into a Node.
        //
        // `host.build_node` MUST be called outside the outer
        // `backend.borrow_mut()` window (per its docstring). The
        // helpers crate already defers its layout-build closure to a
        // microtask before invoking it — so the closure body here runs
        // post-borrow and the synchronous `build_node` call is safe.
        let build_content: Option<Rc<dyn Fn() -> Node>> = {
            let sidebar_slot = presentation.sidebar.borrow().clone();
            sidebar_slot.map(|sidebar_builder| {
                let build_node = build_node.clone();
                let nav_state = nav_state.clone();
                let is_open = is_open;
                let control = control.clone();
                let cb: Rc<dyn Fn() -> Node> = Rc::new(move || {
                    let on_select: Rc<dyn Fn(&'static str)> = {
                        let control = control.clone();
                        Rc::new(move |name| {
                            // The sidebar's `on_select` is a click
                            // handler the author hooks up to drawer
                            // items. The natural verb is `Select` —
                            // tabs use the same shape. Path/params
                            // are empty here because the substrate
                            // only knows the route name at this
                            // callback level; richer flows go
                            // through `DrawerHandle::select` from
                            // the author's code.
                            control.dispatch(NavCommand::Select {
                                name,
                                url: String::new(),
                                params: Box::new(()),
                                state: None,
                            });
                        })
                    };
                    let on_close: Rc<dyn Fn()> = {
                        let control = control.clone();
                        Rc::new(move || {
                            control.dispatch(NavCommand::Custom(
                                Rc::new(DrawerCmd::Close),
                            ));
                        })
                    };
                    let props = DrawerSlotProps {
                        active_route: nav_state.active_route,
                        active_path: nav_state.active_path.clone(),
                        depth: nav_state.depth,
                        can_go_back: nav_state.can_go_back,
                        is_open,
                        on_select,
                        on_close,
                    };
                    let prim = sidebar_builder(props);
                    build_node(prim)
                });
                cb
            })
        };

        let drawer_callbacks = WebDrawerCallbacks {
            navigator,
            side: side_to_helpers(presentation.side),
            drawer_type: type_to_helpers(presentation.drawer_type),
            drawer_width: presentation.drawer_width,
            mount_policy: mount_policy_to_helpers(presentation.mount_policy),
            is_open,
            build_content,
            active_changed,
            open_changed,
            background_color: None,
        };

        let node = web_navigator_helpers::create_drawer(backend, drawer_callbacks, control);
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

    fn on_command(&mut self, _cmd: NavCommand) {
        unreachable!(
            "WebDrawerHandler::on_command — helpers::create_drawer owns the \
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
            None => runtime_core::NavigatorHandle::new(Rc::new(()), &NoopDrawerOps),
        }
    }
}

pub fn register(backend: &mut WebBackend) {
    backend.register_navigator::<DrawerPresentation, _>(|| Box::new(WebDrawerHandler::new()));
}
