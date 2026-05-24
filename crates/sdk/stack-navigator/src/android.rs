//! Android-backend handler for the Stack navigator SDK.
//!
//! Phase-1 adapter: synthesizes legacy `NavigatorCallbacks` and calls
//! `AndroidBackend::create_navigator` (which drives `RustNavigator` +
//! FragmentManager via JNI). Sets `NavExtKind::Stack` so the unified
//! trait method overrides route to the stack storage map.

use crate::StackPresentation;
use backend_android_mobile::AndroidBackend;
use jni::objects::GlobalRef;
use runtime_core::{
    accessibility::AccessibilityProps, Backend, MountResult, NavExtKind, NavigatorCallbacks,
    NavigatorHandler, NavigatorHost,
};
use std::any::Any;
use std::rc::Rc;

pub struct AndroidStackHandler;

impl AndroidStackHandler {
    pub fn new() -> Self {
        Self
    }
}
impl Default for AndroidStackHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl NavigatorHandler<AndroidBackend> for AndroidStackHandler {
    fn init(
        &mut self,
        backend: &mut AndroidBackend,
        host: NavigatorHost<GlobalRef>,
        _presentation: Rc<dyn Any>,
    ) -> GlobalRef {
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

        let mount_2arg: Rc<dyn Fn(&'static str, Box<dyn Any>) -> MountResult<GlobalRef>> = {
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

        let node = backend.create_navigator(callbacks, control, &AccessibilityProps::default());
        backend.set_nav_ext_kind(&node, NavExtKind::Stack);
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
    backend.register_navigator::<StackPresentation, _>(|| Box::new(AndroidStackHandler::new()));
}
