//! Android-backend handler for the Drawer navigator SDK.

use crate::{DrawerPresentation, DrawerSide, DrawerType, MountPolicy};
use backend_android::AndroidBackend;
use jni::objects::GlobalRef;
use runtime_core::{
    accessibility::AccessibilityProps, primitives::navigator::NavigatorOps,
    DrawerNavigatorCallbacks, DrawerSide as CoreDrawerSide, DrawerType as CoreDrawerType,
    MountPolicy as CoreMountPolicy, MountResult, NavigatorCallbacks, NavigatorControl,
    NavigatorHandle, NavigatorHandler, NavigatorHost,
};
use std::any::Any;
use std::rc::Rc;

pub struct AndroidDrawerHandler {
    container: Option<GlobalRef>,
    control: Option<Rc<NavigatorControl>>,
}

impl AndroidDrawerHandler {
    pub fn new() -> Self {
        Self { container: None, control: None }
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

        // Shared with the SDK builder's `.layout(...)` wrap — see
        // `DrawerPresentation::is_open` in `lib.rs`.
        let is_open = presentation.is_open;
        let open_changed: Rc<dyn Fn(bool)> = {
            Rc::new(move |o| is_open.set(o))
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

        self.control = Some(control.clone());
        let node = backend.create_drawer_navigator(
            drawer_callbacks,
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
            backend.drawer_navigator_attach_initial(&container, screen, scope_id, options);
        }
    }

    fn on_command(&mut self, _cmd: runtime_core::NavCommand) {
        unreachable!(
            "AndroidDrawerHandler::on_command — backend.create_drawer_navigator \
             owns the control-plane dispatcher"
        );
    }

    fn release(&mut self, backend: &mut AndroidBackend) {
        if let Some(container) = self.container.take() {
            backend.release_drawer_navigator(&container);
        }
        self.control = None;
    }

    fn make_handle(&self) -> NavigatorHandle {
        match self.control.clone() {
            Some(c) => NavigatorHandle::with_control(Rc::new(()), &NoopDrawerOps, c),
            None => NavigatorHandle::new(Rc::new(()), &NoopDrawerOps),
        }
    }

    fn apply_slot_style(
        &mut self,
        _backend: &mut AndroidBackend,
        _slot: &'static str,
        _style: &Rc<runtime_core::StyleRules>,
    ) {
        // Android drawer chrome currently has no per-slot styling
        // helper on the backend. The `sidebar` / `scrim` slots would
        // wire here when those land — same shape as iOS sidebar.
    }
}

struct NoopDrawerOps;
impl NavigatorOps for NoopDrawerOps {}

pub fn register(backend: &mut AndroidBackend) {
    backend.register_navigator::<DrawerPresentation, _>(|| Box::new(AndroidDrawerHandler::new()));
}
