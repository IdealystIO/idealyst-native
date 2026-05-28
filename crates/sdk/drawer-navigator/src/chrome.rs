//! Backend-neutral "primitive chrome" handler for the Drawer navigator.
//!
//! Builds `[sidebar | body-outlet]` from framework primitives using only
//! the generic [`Backend`] trait + `NavigatorHost`, so it works on any
//! primitive-rendering backend (the SSR backend registers it today).
//! Nothing here is SSR-specific — no `backend-ssr` dependency, no target
//! cfg.
//!
//! The sidebar is an author `Element` (`DrawerPresentation.sidebar`), so
//! it must be materialized via `host.build_node_into` — deferred to a
//! microtask because it can't run inside the `create_navigator` borrow
//! (the SSR backend installs a queuing scheduler so the deferred build
//! runs after the borrow releases). `build_node_into` splices the built
//! sidebar into its slot without touching any backend's node internals.
//!
//! Open/close animation + gestures are the live runtime's job on
//! hydration; the server just needs the structural first paint with the
//! sidebar's nav links present (so crawlers see site navigation).

use crate::{DrawerPresentation, DrawerSlotProps};
use runtime_core::accessibility::AccessibilityProps;
use runtime_core::primitives::navigator::{
    AmbientNavGuard, NavigatorHandler, NavigatorHost, RegisterNavigator,
};
use runtime_core::{schedule_microtask, Backend, Signal};
use std::any::Any;
use std::rc::Rc;

/// Renders a drawer navigator's `[sidebar | outlet]` chrome on `B`.
pub struct DrawerChromeHandler<B: Backend> {
    /// Where the path-matched screen mounts (inside the chrome).
    outlet: Option<B::Node>,
}

impl<B: Backend> DrawerChromeHandler<B> {
    pub fn new() -> Self {
        Self { outlet: None }
    }
}

impl<B: Backend> Default for DrawerChromeHandler<B> {
    fn default() -> Self {
        Self::new()
    }
}

impl<B: Backend + 'static> NavigatorHandler<B> for DrawerChromeHandler<B> {
    fn init(
        &mut self,
        backend: &mut B,
        host: NavigatorHost<B::Node>,
        presentation: Rc<dyn Any>,
    ) -> B::Node {
        let a11y = AccessibilityProps::default();
        // Structural chrome: a container holding [sidebar | outlet].
        let mut root = backend.create_view(&a11y);
        let sidebar_slot = backend.create_view(&a11y);
        let outlet = backend.create_view(&a11y);
        backend.insert(&mut root, sidebar_slot.clone());
        backend.insert(&mut root, outlet.clone());
        self.outlet = Some(outlet);

        // The sidebar is an author Element — defer its build past the
        // create_navigator borrow (drained by render_path post-mount),
        // and splice it into the slot via build_node_into.
        if let Some(pres) = presentation.downcast_ref::<DrawerPresentation>() {
            if let Some(sidebar_builder) = pres.sidebar.borrow().clone() {
                let build_node_into = host.build_node_into.clone();
                let control = host.control.clone();
                let nav_state = host.nav_state.clone();
                // Created in the navigator's scope so it stays valid
                // through the post-mount drain.
                let is_open = Signal::new(true);
                let on_select: Rc<dyn Fn(&'static str)> = Rc::new(|_| {});
                let on_close: Rc<dyn Fn()> = Rc::new(|| {});
                schedule_microtask(move || {
                    let props = DrawerSlotProps {
                        active_route: nav_state.active_route,
                        active_path: nav_state.active_path,
                        depth: nav_state.depth,
                        can_go_back: nav_state.can_go_back,
                        is_open,
                        on_select,
                        on_close,
                    };
                    // Ambient guard so `Link`s inside the sidebar capture
                    // the navigator (matches the web handler).
                    let _ambient = AmbientNavGuard::push(control);
                    build_node_into(sidebar_slot, sidebar_builder(props));
                });
            }
        }
        root
    }

    fn attach_initial(
        &mut self,
        backend: &mut B,
        screen: B::Node,
        _scope_id: u64,
        _options: Box<dyn Any>,
    ) {
        if let Some(mut outlet) = self.outlet.clone() {
            backend.insert(&mut outlet, screen);
        }
    }
}

/// Register the Drawer navigator's primitive-chrome handler on any
/// primitive-rendering backend (the SSR backend today).
pub fn register<B: RegisterNavigator>(backend: &mut B) {
    backend.register_navigator::<DrawerPresentation, _>(|| Box::new(DrawerChromeHandler::<B>::new()));
}
