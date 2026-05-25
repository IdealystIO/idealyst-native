//! Android-backend handler for the Tab navigator SDK.

use crate::{MountPolicy, TabPlacement, TabPresentation};
use backend_android::AndroidBackend;
use jni::objects::GlobalRef;
use runtime_core::{
    accessibility::AccessibilityProps, primitives::navigator::{tabs::TabRegistration, NavigatorOps},
    MountPolicy as CoreMountPolicy, MountResult, NavigatorCallbacks, NavigatorControl,
    NavigatorHandle, NavigatorHandler, NavigatorHost, TabNavigatorCallbacks,
    TabPlacement as CoreTabPlacement,
};
use std::any::Any;
use std::rc::Rc;

pub struct AndroidTabHandler {
    container: Option<GlobalRef>,
    control: Option<Rc<NavigatorControl>>,
}

impl AndroidTabHandler {
    pub fn new() -> Self {
        Self { container: None, control: None }
    }
}
impl Default for AndroidTabHandler {
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

impl NavigatorHandler<AndroidBackend> for AndroidTabHandler {
    fn init(
        &mut self,
        backend: &mut AndroidBackend,
        host: NavigatorHost<GlobalRef>,
        presentation: Rc<dyn Any>,
    ) -> GlobalRef {
        let presentation = presentation
            .downcast::<TabPresentation>()
            .expect("AndroidTabHandler: presentation must be TabPresentation");

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

        let mount_2arg: Rc<dyn Fn(&'static str, Box<dyn Any>) -> MountResult<GlobalRef>> = {
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

        self.control = Some(control.clone());
        let node = backend.create_tab_navigator(
            tab_callbacks,
            control,
            &AccessibilityProps::default(),
        );
        self.container = Some(node.clone());
        node
    }

    fn attach_initial(
        &mut self,
        backend: &mut AndroidBackend,
        screen: GlobalRef,
        scope_id: u64,
        options: runtime_core::ScreenOptions,
    ) {
        if let Some(container) = self.container.clone() {
            backend.tab_navigator_attach_initial(&container, screen, scope_id, options);
        }
    }

    fn on_command(&mut self, _cmd: runtime_core::NavCommand) {
        unreachable!(
            "AndroidTabHandler::on_command — backend.create_tab_navigator \
             owns the control-plane dispatcher"
        );
    }

    fn release(&mut self, backend: &mut AndroidBackend) {
        if let Some(container) = self.container.take() {
            backend.release_tab_navigator(&container);
        }
        self.control = None;
    }

    fn make_handle(&self) -> NavigatorHandle {
        match self.control.clone() {
            Some(c) => NavigatorHandle::with_control(Rc::new(()), &NoopTabOps, c),
            None => NavigatorHandle::new(Rc::new(()), &NoopTabOps),
        }
    }

    fn apply_slot_style(
        &mut self,
        _backend: &mut AndroidBackend,
        _slot: &'static str,
        _style: &Rc<runtime_core::StyleRules>,
    ) {
        // Android does not yet expose per-slot tab-bar styling helpers
        // on the backend. When a styled-tabs implementation lands,
        // wire `tab_bar` / `tab_icon` / `tab_label` here in the same
        // shape as the stack handler's header/title/button slots.
    }
}

struct NoopTabOps;
impl NavigatorOps for NoopTabOps {}

pub fn register(backend: &mut AndroidBackend) {
    backend.register_navigator::<TabPresentation, _>(|| Box::new(AndroidTabHandler::new()));
}
