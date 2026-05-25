//! Web-backend handler for the Drawer navigator SDK.
//!
//! Phase-1 adapter: synthesizes a legacy `DrawerNavigatorCallbacks`
//! from the framework-supplied `NavigatorHost` + the SDK's
//! `DrawerPresentation`, then calls `WebBackend::create_drawer_navigator`.
//! Phase-2 will inline the dispatcher closure here.

use crate::{DrawerPresentation, DrawerSide, DrawerType, MountPolicy};
use backend_web::WebBackend;
use runtime_core::{
    accessibility::AccessibilityProps, Backend, DrawerNavigatorCallbacks,
    DrawerSide as CoreDrawerSide, DrawerType as CoreDrawerType, MountPolicy as CoreMountPolicy,
    MountResult, NavigatorCallbacks, NavigatorHandler, NavigatorHost, Signal,
};
use std::any::Any;
use std::rc::Rc;
use web_sys::Node;

pub struct WebDrawerHandler;

impl WebDrawerHandler {
    pub fn new() -> Self {
        Self
    }
}
impl Default for WebDrawerHandler {
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
            build_layout,
            nav_state,
            depth_changed,
            active_changed,
            control,
        } = host;

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

        // Drawer-specific callbacks: open-state signal + content
        // factory + active/open change observers.
        let is_open = Signal::new(false);
        let open_changed: Rc<dyn Fn(bool)> = {
            let is_open = is_open;
            Rc::new(move |o| is_open.set(o))
        };

        let active_changed_legacy: Rc<dyn Fn(&'static str)> = {
            let ac = active_changed;
            Rc::new(move |name| ac(name, String::new()))
        };

        // `build_content`: the SDK presentation carries a
        // `ContentBuilder` closure that takes `DrawerContentProps` and
        // returns a `Primitive`. The legacy `build_content` expects a
        // closure that returns the realized Node directly. Wrap the
        // SDK closure with the framework's screen-build machinery so
        // its reactive effects survive the navigator's lifetime.
        //
        // Phase-2: this should ideally drive through `mount_screen` so
        // the per-screen scope owns the panel's effects. For Phase-1
        // adapter, we leave `build_content: None` and route drawer
        // panel rendering through the user's `.layout(...)` instead —
        // which is how the legacy DrawerNavigator handles the web case
        // anyway (web doesn't ship a native drawer chrome).
        let build_content = None;

        let drawer_callbacks = DrawerNavigatorCallbacks {
            navigator,
            side: side_to_core(presentation.side),
            drawer_type: type_to_core(presentation.drawer_type),
            drawer_width: presentation.drawer_width,
            swipe_to_open: presentation.swipe_to_open,
            mount_policy: mount_policy_to_core(presentation.mount_policy),
            is_open,
            build_content,
            active_changed: active_changed_legacy,
            open_changed,
            background_color: None,
        };

        backend.create_drawer_navigator(drawer_callbacks, control, &AccessibilityProps::default())
    }

    fn attach_initial(
        &mut self,
        _backend: &mut WebBackend,
        _screen: Node,
        _scope_id: u64,
        _options: runtime_core::ScreenOptions,
    ) {
        unreachable!(
            "WebDrawerHandler::attach_initial — WebBackend dispatches via \
             navigator_attach_initial → uniform machinery"
        );
    }

    fn on_command(&mut self, _cmd: runtime_core::NavCommand) {
        unreachable!(
            "WebDrawerHandler::on_command — legacy drawer dispatcher owns \
             the control plane until Phase-2"
        );
    }
}

pub fn register(backend: &mut WebBackend) {
    backend.register_navigator::<DrawerPresentation, _>(|| Box::new(WebDrawerHandler::new()));
}
