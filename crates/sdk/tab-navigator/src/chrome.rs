//! Backend-neutral "primitive chrome" handler for the Tab navigator.
//!
//! Builds `[content-outlet | tab-bar]` from framework primitives using
//! only the generic [`Backend`] trait. The tab bar is the tab labels
//! (from `TabPresentation.tab_order`) — plain strings, so it builds
//! synchronously, no `build_node` needed (unlike the drawer sidebar).
//! The active screen mounts into the outlet via `attach_initial`.
//!
//! Not SSR-specific: no `backend-ssr` dependency, no target cfg. The SSR
//! backend registers it via `tab_navigator::chrome::register`.

use crate::TabPresentation;
use runtime_core::accessibility::AccessibilityProps;
use runtime_core::primitives::navigator::{NavigatorHandler, NavigatorHost, RegisterNavigator};
use runtime_core::Backend;
use std::any::Any;
use std::rc::Rc;

/// Renders a tab navigator's `[outlet | tab-bar]` chrome on `B`.
pub struct TabChromeHandler<B: Backend> {
    /// Where the active tab's screen mounts.
    outlet: Option<B::Node>,
}

impl<B: Backend> TabChromeHandler<B> {
    pub fn new() -> Self {
        Self { outlet: None }
    }
}

impl<B: Backend> Default for TabChromeHandler<B> {
    fn default() -> Self {
        Self::new()
    }
}

impl<B: Backend + 'static> NavigatorHandler<B> for TabChromeHandler<B> {
    fn init(
        &mut self,
        backend: &mut B,
        _host: NavigatorHost<B::Node>,
        presentation: Rc<dyn Any>,
    ) -> B::Node {
        let a11y = AccessibilityProps::default();
        let mut root = backend.create_view(&a11y);
        let outlet = backend.create_view(&a11y);
        backend.insert(&mut root, outlet.clone());
        self.outlet = Some(outlet);

        // Tab bar: one label per tab, in declared order.
        if let Some(pres) = presentation.downcast_ref::<TabPresentation>() {
            let mut tab_bar = backend.create_view(&a11y);
            for (_route, spec) in &pres.tab_order {
                let label = backend.create_text(&spec.label, &a11y);
                backend.insert(&mut tab_bar, label);
            }
            backend.insert(&mut root, tab_bar);
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

/// Register the Tab navigator's primitive-chrome handler on any
/// primitive-rendering backend (the SSR backend today).
pub fn register<B: RegisterNavigator>(backend: &mut B) {
    backend.register_navigator::<TabPresentation, _>(|| Box::new(TabChromeHandler::<B>::new()));
}
