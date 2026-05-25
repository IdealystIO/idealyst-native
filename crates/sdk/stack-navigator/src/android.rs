//! Android-backend handler for the Stack navigator SDK.
//!
//! Retains the container `GlobalRef` returned by
//! `AndroidBackend::create_stack_navigator` (the navigator's
//! FragmentManager-host view) and the `NavigatorControl` clone handed
//! to `init`. Post-init dispatch (`attach_initial` / `release` /
//! `make_handle` / `apply_slot_style`) forwards to the matching
//! backend inherent helper with the right node.

use crate::StackPresentation;
use backend_android::AndroidBackend;
use jni::objects::GlobalRef;
use runtime_core::{
    accessibility::AccessibilityProps, primitives::navigator::NavigatorOps, MountResult,
    NavigatorCallbacks, NavigatorControl, NavigatorHandle, NavigatorHandler, NavigatorHost,
};
use std::any::Any;
use std::rc::Rc;

pub struct AndroidStackHandler {
    container: Option<GlobalRef>,
    control: Option<Rc<NavigatorControl>>,
}

impl AndroidStackHandler {
    pub fn new() -> Self {
        Self { container: None, control: None }
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
            build_node: _,
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

        self.control = Some(control.clone());
        let node = backend.create_stack_navigator(callbacks, control, &AccessibilityProps::default());
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
            backend.stack_navigator_attach_initial(&container, screen, scope_id, options);
        }
    }

    fn on_command(&mut self, _cmd: runtime_core::NavCommand) {
        unreachable!(
            "AndroidStackHandler::on_command — backend.create_stack_navigator \
             owns the control-plane dispatcher"
        );
    }

    fn release(&mut self, backend: &mut AndroidBackend) {
        if let Some(container) = self.container.take() {
            backend.release_stack_navigator(&container);
        }
        self.control = None;
    }

    fn make_handle(&self) -> NavigatorHandle {
        match self.control.clone() {
            Some(c) => NavigatorHandle::with_control(Rc::new(()), &NoopStackOps, c),
            None => NavigatorHandle::new(Rc::new(()), &NoopStackOps),
        }
    }

    fn apply_slot_style(
        &mut self,
        backend: &mut AndroidBackend,
        slot: &'static str,
        style: &Rc<runtime_core::StyleRules>,
    ) {
        let Some(container) = self.container.clone() else { return };
        match slot {
            "header" => backend.apply_navigator_header_style(&container, style),
            "title" => backend.apply_navigator_title_style(&container, style),
            "button" => backend.apply_navigator_button_style(&container, style),
            "body" => backend.apply_navigator_body_style(&container, style),
            _ => {}
        }
    }
}

struct NoopStackOps;
impl NavigatorOps for NoopStackOps {}

pub fn register(backend: &mut AndroidBackend) {
    backend.register_navigator::<StackPresentation, _>(|| Box::new(AndroidStackHandler::new()));
}
