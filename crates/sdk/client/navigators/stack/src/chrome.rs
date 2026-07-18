//! Backend-neutral "primitive chrome" handler for the Stack navigator.
//!
//! Unlike the web/iOS/Android handlers — which drive native chrome
//! (DOM router, `UINavigationController`, Android `Toolbar`) — this one
//! builds the header bar from framework primitives (`View` + `Text`),
//! using only the generic [`Backend`] trait. So it works on *any*
//! backend that renders primitives; the SSR backend registers it today
//! to emit `<header>`-style markup for first paint, but nothing here is
//! SSR-specific (hence no `backend-ssr` dependency and no target cfg).
//!
//! The header is the screen's title (from `StackScreenOptions`); native
//! push/pop and the back button are the live runtime's job on hydration.

use crate::{StackPresentation, StackScreenOptions};
use runtime_core::accessibility::AccessibilityProps;
use runtime_core::primitives::navigator::{
    stack_screen_fill_rules, NavigatorHandler, NavigatorHost, RegisterNavigator,
    NAV_ROOT_HYDRATION_CLASS,
};
use runtime_core::Backend;
use std::any::Any;
use std::rc::Rc;

/// Renders a stack navigator's header + screen as primitives on `B`.
pub struct StackChromeHandler<B: Backend> {
    /// Column container; header (optional) + screen nest inside it.
    root: Option<B::Node>,
}

impl<B: Backend> StackChromeHandler<B> {
    /// Create a handler with no root container yet; the root is built
    /// when the navigator initializes its chrome.
    pub fn new() -> Self {
        Self { root: None }
    }
}

impl<B: Backend> Default for StackChromeHandler<B> {
    fn default() -> Self {
        Self::new()
    }
}

impl<B: Backend + 'static> NavigatorHandler<B> for StackChromeHandler<B> {
    fn init(
        &mut self,
        backend: &mut B,
        host: NavigatorHost<B::Node>,
        _presentation: Rc<dyn Any>,
    ) -> B::Node {
        // The framework walker mounts the path-matched screen and hands
        // it to `attach_initial`; we only need the container here. Its
        // styling (`stack_container_rules()` — fill + `position:relative`)
        // is the navigator element's default style, applied by the walker
        // through the normal pipeline after `init` — so the server
        // resolves the identical content-hashed class the hydrated client
        // mints, and no stylesheet or class rule is injected here.
        let root = backend.create_view(&AccessibilityProps::default());
        // Structural hydration marker (NOT a styled class): the hydrating
        // client finds this container via
        // `WebBackend::hydrate_adopt_container(NAV_ROOT_HYDRATION_CLASS)`.
        // Without it hydration can't find the nav root, rebuilds it fresh,
        // and orphans the whole SSR screen subtree — dropping the screen's
        // reactive text. No-op on native backends (default
        // `attach_html_class`); the SSR backend adds the class.
        backend.attach_html_class(&root, NAV_ROOT_HYDRATION_CLASS);
        // Screens mount directly into the container (this handler has no
        // author layout), so ask the substrate to pin every mounted screen
        // full-bleed via its style override layer — the style-system
        // replacement for the old `ui-nav-screen` class stamp.
        host.set_screen_style_overlay(stack_screen_fill_rules());
        self.root = Some(root.clone());
        root
    }

    fn attach_initial(
        &mut self,
        backend: &mut B,
        screen: B::Node,
        _scope_id: u64,
        options: Box<dyn Any>,
    ) {
        let Some(mut root) = self.root.clone() else { return };
        let a11y = AccessibilityProps::default();

        // Header bar — only when the screen declared a title.
        if let Some(title) = options
            .downcast_ref::<StackScreenOptions>()
            .and_then(|o| o.title.as_deref())
        {
            let mut header = backend.create_view(&a11y);
            let title_node = backend.create_text(title, &a11y);
            backend.insert(&mut header, title_node);
            backend.insert(&mut root, header);
        }

        // Full-bleed screen placement already rides the screen root's style
        // (the `set_screen_style_overlay` set in `init` — applied by the
        // substrate before the screen was built), so the node arrives here
        // styled; nothing to stamp.
        backend.insert(&mut root, screen);
    }
}

/// Register the Stack navigator's primitive-chrome handler on any backend
/// that renders primitives (the SSR backend today). Call during setup,
/// before rendering UI that uses `stack_navigator::Navigator::new(...)`.
pub fn register<B: RegisterNavigator>(backend: &mut B) {
    backend.register_navigator::<StackPresentation, _>(|| Box::new(StackChromeHandler::<B>::new()));
}
