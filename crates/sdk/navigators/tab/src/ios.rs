//! iOS-backend handler for the Tab navigator SDK.
//!
//! The UIKit machinery (`UIView` body, screen-swap on `Select`,
//! per-screen mount policy) lives in the `ios-navigator-helpers`
//! crate, shared with stack + drawer. This module's `IosTabHandler`
//! synthesizes an `IosTabCallbacks` from the framework-supplied
//! `NavigatorHost` + the SDK's `TabPresentation`, then calls
//! `ios_navigator_helpers::create_tab`.

use crate::{MountPolicy, TabPlacement, TabPresentation, TABS_OPS};
use backend_ios::{IosBackend, IosNode};
use ios_navigator_helpers::{
    self as helpers, IosNavCallbacks, IosTabCallbacks, MountPolicy as HelpersMountPolicy,
    TabPlacement as HelpersTabPlacement, TabRegistration as HelpersTabRegistration,
};
use runtime_core::primitives::navigator::{MountResult, NavigatorHandler, NavigatorHost};
use std::any::Any;
use std::rc::Rc;

pub struct IosTabHandler {
    container: Option<IosNode>,
}

impl IosTabHandler {
    pub fn new() -> Self {
        Self { container: None }
    }
}
impl Default for IosTabHandler {
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
            match_path: _,
            nav_state,
            depth_changed,
            active_changed,
            control,
            build_node: _,
            build_node_into: _,
            build_in_screen: _,
            // `resolve_entry` + `base`: framework/web deep-link plumbing; the
            // iOS tab handler doesn't read them.
            ..
        } = host;

        let mount_2arg: Rc<dyn Fn(&'static str, Box<dyn Any>) -> MountResult<IosNode>> = {
            let m = mount_screen;
            Rc::new(move |name, params| m(name, params, None))
        };

        let navigator = IosNavCallbacks {
            initial_route,
            initial_path,
            mount_screen: mount_2arg,
            release_screen,
            depth_changed,
            nav_state,
            defer_initial_mount,
        };

        let tabs: Vec<HelpersTabRegistration> = presentation
            .tab_order
            .iter()
            .map(|(route, spec)| HelpersTabRegistration {
                route,
                label: spec.label.clone(),
                icon: spec.icon.clone(),
                badge: spec.badge.clone(),
            })
            .collect();

        // Discard the URL-path component the substrate's `active_changed`
        // produces — the iOS tab engine doesn't track per-tab paths, so
        // the SDK's helpers-crate callback shape is single-arg
        // `Fn(&'static str)` for the name only.
        let active_changed_helpers: Rc<dyn Fn(&'static str)> = {
            let ac = active_changed;
            Rc::new(move |name| ac(name, String::new()))
        };

        let tab_callbacks = IosTabCallbacks {
            navigator,
            tabs,
            placement: placement_to_helpers(presentation.placement),
            mount_policy: mount_policy_to_helpers(presentation.mount_policy),
            active_changed: active_changed_helpers,
        };

        let node = helpers::create_tab(backend.mtm(), tab_callbacks, control);
        self.container = Some(node.clone());
        node
    }

    fn attach_initial(
        &mut self,
        _backend: &mut IosBackend,
        screen: IosNode,
        scope_id: u64,
        _options: Box<dyn Any>,
    ) {
        if let Some(container) = self.container.clone() {
            helpers::tab_attach_initial(&container, screen, scope_id);
        }
    }

    fn on_command(&mut self, _cmd: runtime_core::NavCommand) {
        unreachable!(
            "IosTabHandler::on_command — helpers::create_tab owns the \
             control-plane dispatcher"
        );
    }

    fn release(&mut self, _backend: &mut IosBackend) {
        if let Some(container) = self.container.take() {
            helpers::release_tab_drawer(&container);
        }
    }

    fn make_handle(&self) -> runtime_core::NavigatorHandle {
        match self.container.as_ref() {
            Some(c) => helpers::make_tab_handle(c),
            None => runtime_core::NavigatorHandle::new(Rc::new(()), &TABS_OPS),
        }
    }

    fn apply_slot_style(
        &mut self,
        _backend: &mut IosBackend,
        _slot: &'static str,
        _style: &Rc<runtime_core::StyleRules>,
    ) {
        // iOS does not yet expose per-slot tab-bar styling helpers. When
        // a styled-tabs implementation lands (likely via
        // `UITabBarAppearance` configuration), wire the `tab_bar` /
        // `tab_icon` / `tab_label` slots here in the same shape as the
        // stack handler's header/title/button slots.
    }
}

/// Install the tab navigator handler on an iOS backend. Call once at
/// startup so `Element::Navigator`s carrying a [`TabPresentation`]
/// resolve to this backend's chrome.
pub fn register(backend: &mut IosBackend) {
    backend.register_navigator::<TabPresentation, _>(|| Box::new(IosTabHandler::new()));
}
