//! Android-backend handler for the Drawer navigator SDK.
//!
//! Phase-1 adapter: synthesizes legacy `DrawerNavigatorCallbacks` and
//! calls `AndroidBackend::create_drawer_navigator`. Sets
//! `NavExtKind::Drawer` for unified dispatch.

use crate::{DrawerPresentation, DrawerSide, DrawerType, MountPolicy};
use backend_android_mobile::AndroidBackend;
use jni::objects::GlobalRef;
use runtime_core::{
    accessibility::AccessibilityProps, Backend, DrawerNavigatorCallbacks,
    DrawerSide as CoreDrawerSide, DrawerType as CoreDrawerType, MountPolicy as CoreMountPolicy,
    MountResult, NavExtKind, NavigatorCallbacks, NavigatorHandler, NavigatorHost, Signal,
};
use std::any::Any;
use std::rc::Rc;

pub struct AndroidDrawerHandler;

impl AndroidDrawerHandler {
    pub fn new() -> Self {
        Self
    }
}
impl Default for AndroidDrawerHandler {
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

impl NavigatorHandler<AndroidBackend> for AndroidDrawerHandler {
    fn init(
        &mut self,
        backend: &mut AndroidBackend,
        host: NavigatorHost<GlobalRef>,
        presentation: Rc<dyn Any>,
    ) -> GlobalRef {
        let presentation = presentation
            .downcast::<DrawerPresentation>()
            .expect("AndroidDrawerHandler: presentation must be DrawerPresentation");

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

        let is_open = Signal::new(false);
        let open_changed: Rc<dyn Fn(bool)> = {
            let s = is_open;
            Rc::new(move |o| s.set(o))
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
            build_content: None,
            active_changed: active_changed_legacy,
            open_changed,
            background_color: None,
        };

        let node = backend.create_drawer_navigator(
            drawer_callbacks,
            control,
            &AccessibilityProps::default(),
        );
        backend.set_nav_ext_kind(&node, NavExtKind::Drawer);
        node
    }

    fn attach_initial(
        &mut self,
        _backend: &mut AndroidBackend,
        _screen: GlobalRef,
        _scope_id: u64,
        _options: runtime_core::ScreenOptions,
    ) {
        unreachable!();
    }

    fn on_command(&mut self, _cmd: runtime_core::NavCommand) {
        unreachable!();
    }
}

pub fn register(backend: &mut AndroidBackend) {
    backend.register_navigator::<DrawerPresentation, _>(|| Box::new(AndroidDrawerHandler::new()));
}
