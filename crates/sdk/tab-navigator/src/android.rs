//! Android-backend handler for the Tab navigator SDK.
//!
//! Synthesizes an `AndroidTabCallbacks` from the framework-supplied
//! `NavigatorHost` + the SDK's `TabPresentation`, then calls
//! `android_navigator_helpers::create_tab`. The SDK's typed enums
//! (`TabPlacement` / `MountPolicy`) translate to the helpers crate's
//! identically-shaped variants via per-enum shims.

use crate::{MountPolicy, TabPlacement, TabPresentation};
use android_navigator_helpers::{
    AndroidNavCallbacks, AndroidScreenOptions, AndroidTabCallbacks,
    MountPolicy as HelpersMountPolicy, TabPlacement as HelpersTabPlacement, TabRegistration,
};
use backend_android::AndroidBackend;
use jni::objects::GlobalRef;
use runtime_core::{
    primitives::navigator::{MountResult, NavigatorHandler, NavigatorHost, NavigatorOps},
    NavigatorHandle,
};
use std::any::Any;
use std::rc::Rc;

pub struct AndroidTabHandler {
    container: Option<GlobalRef>,
}

impl AndroidTabHandler {
    pub fn new() -> Self {
        Self { container: None }
    }
}

impl Default for AndroidTabHandler {
    fn default() -> Self {
        Self::new()
    }
}

fn placement_to_helpers(p: TabPlacement) -> HelpersTabPlacement {
    match p {
        TabPlacement::Auto => HelpersTabPlacement::Auto,
        TabPlacement::Top => HelpersTabPlacement::Top,
        TabPlacement::Bottom => HelpersTabPlacement::Bottom,
        TabPlacement::Sidebar => HelpersTabPlacement::Sidebar,
    }
}

fn mount_policy_to_helpers(m: MountPolicy) -> HelpersMountPolicy {
    match m {
        MountPolicy::EagerPersistent => HelpersMountPolicy::EagerPersistent,
        MountPolicy::LazyPersistent => HelpersMountPolicy::LazyPersistent,
        MountPolicy::LazyDisposing => HelpersMountPolicy::LazyDisposing,
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
            nav_state,
            depth_changed,
            active_changed,
            control,
            build_node: _,
            build_in_screen: _,
        } = host;

        let mount_2arg: Rc<dyn Fn(&'static str, Box<dyn Any>) -> MountResult<GlobalRef>> = {
            let m = mount_screen;
            Rc::new(move |name, params| m(name, params, None))
        };

        let navigator = AndroidNavCallbacks {
            initial_route,
            initial_path,
            mount_screen: mount_2arg,
            release_screen,
            match_path,
            depth_changed,
            nav_state,
            defer_initial_mount,
        };

        let tabs: Vec<TabRegistration> = presentation
            .tab_order
            .iter()
            .map(|(route, spec)| TabRegistration {
                route,
                path: "",
                label: Some(spec.label.clone()),
                icon: spec.icon.clone(),
            })
            .collect();

        let tab_callbacks = AndroidTabCallbacks {
            navigator,
            tabs,
            placement: placement_to_helpers(presentation.placement),
            mount_policy: mount_policy_to_helpers(presentation.mount_policy),
            active_changed,
        };

        let node = android_navigator_helpers::create_tab(backend, tab_callbacks, control);
        self.container = Some(node.clone());
        node
    }

    fn attach_initial(
        &mut self,
        _backend: &mut AndroidBackend,
        screen: GlobalRef,
        scope_id: u64,
        _options: Box<dyn Any>,
    ) {
        let Some(container) = self.container.clone() else { return };
        // Tabs don't carry the same header chrome stack screens do — pass
        // through the default empty options. The helpers crate's tab path
        // ignores the toolbar slot regardless.
        android_navigator_helpers::attach_initial(
            &container,
            screen,
            scope_id,
            &AndroidScreenOptions::default(),
        );
    }

    fn on_command(&mut self, _cmd: runtime_core::NavCommand) {
        unreachable!(
            "AndroidTabHandler::on_command — helpers::create_tab owns the \
             control-plane dispatcher"
        );
    }

    fn release(&mut self, _backend: &mut AndroidBackend) {
        if let Some(container) = self.container.take() {
            android_navigator_helpers::release(&container);
        }
    }

    fn make_handle(&self) -> NavigatorHandle {
        match self.container.as_ref() {
            Some(c) => android_navigator_helpers::make_handle(c),
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
        // on the backend. When a styled-tabs implementation lands, wire
        // `tab_bar` / `tab_icon` / `tab_label` here.
    }
}

struct NoopTabOps;
impl NavigatorOps for NoopTabOps {}

pub fn register(backend: &mut AndroidBackend) {
    backend.register_navigator::<TabPresentation, _>(|| Box::new(AndroidTabHandler::new()));
}
