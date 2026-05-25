//! iOS-backend handler for the Tab navigator SDK.
//!
//! Phase-1 adapter: synthesizes legacy `TabNavigatorCallbacks` and
//! calls `IosBackend::create_tab_navigator` (which drives
//! `UITabBarController`). Sets `NavigatorKind::Tab` for unified dispatch.

use crate::{MountPolicy, TabPlacement, TabPresentation};
use backend_ios_mobile::{IosBackend, IosNode};
use runtime_core::{
    accessibility::AccessibilityProps, primitives::navigator::tabs::TabRegistration, Backend,
    MountPolicy as CoreMountPolicy, MountResult, NavigatorKind, NavigatorCallbacks, NavigatorHandler,
    NavigatorHost, TabNavigatorCallbacks, TabPlacement as CoreTabPlacement,
};
use std::any::Any;
use std::rc::Rc;

pub struct IosTabHandler;

impl IosTabHandler {
    pub fn new() -> Self {
        Self
    }
}
impl Default for IosTabHandler {
    fn default() -> Self {
        Self::new()
    }
}

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

impl NavigatorHandler<IosBackend> for IosTabHandler {
    fn init(
        &mut self,
        backend: &mut IosBackend,
        host: NavigatorHost<IosNode>,
        presentation: Rc<dyn Any>,
    ) -> IosNode {
        let presentation = presentation
            .downcast::<TabPresentation>()
            .expect("IosTabHandler: presentation must be TabPresentation");

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
            nav_state,
            depth_changed,
            defer_initial_mount,
        };

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

        let node = backend.create_tab_navigator(
            tab_callbacks,
            control,
            &AccessibilityProps::default(),
        );
        backend.set_nav_kind(&node, NavigatorKind::Tab);
        node
    }

    fn attach_initial(
        &mut self,
        _backend: &mut IosBackend,
        _screen: IosNode,
        _scope_id: u64,
        _options: runtime_core::ScreenOptions,
    ) {
        unreachable!();
    }

    fn on_command(&mut self, _cmd: runtime_core::NavCommand) {
        unreachable!();
    }
}

pub fn register(backend: &mut IosBackend) {
    backend.register_navigator::<TabPresentation, _>(|| Box::new(IosTabHandler::new()));
}
