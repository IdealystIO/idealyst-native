//! Regression: a navigator handler's screen style overlay
//! (`NavigatorHost::set_screen_style_overlay`) must be layered onto every
//! mounted screen root's style — winning over the screen's own conflicting
//! fields while preserving the rest of the author's styling.
//!
//! This is the substrate mechanism that replaced the legacy web/SSR stack
//! `ui-nav-screen` class stamp (raw class + `!important` CSS injected into
//! `<head>`). The class-splice approach had a latent bug this test guards
//! against by construction: the web backend's style apply does a full
//! `className` replace, so a spliced class silently vanished on the first
//! reactive restyle of the screen root. Riding the style OVERRIDE layer
//! instead means the placement re-merges on every resolution.

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use mock_backend::MockBackend;
use runtime_core::primitives::navigator::{
    stack_screen_fill_rules, NavigatorHandler, NavigatorHost, Screen,
};
use runtime_core::{
    text, view, Backend, Color, Length, Position, Route, StyleApplication, StyleRules,
    StyleSheet, Tokenized,
};
use stack_navigator::{StackBuilder, StackNavigator, StackPresentation};

const HOME: Route<()> = Route::<()>::new("home", "/");

/// Minimal handler mirroring the web/SSR stack: it asks the substrate to
/// pin mounted screens full-bleed, and records the screen node it's
/// handed so the test can inspect the style the walker actually applied.
struct OverlayHandler {
    screen_node_out: Rc<RefCell<Option<u64>>>,
}

impl NavigatorHandler<MockBackend> for OverlayHandler {
    fn init(
        &mut self,
        backend: &mut MockBackend,
        host: NavigatorHost<u64>,
        _presentation: Rc<dyn Any>,
    ) -> u64 {
        // Set during init — strictly before the framework's initial
        // mount — exactly like the real stack handlers.
        host.set_screen_style_overlay(stack_screen_fill_rules());
        backend.create_view(&Default::default())
    }

    fn attach_initial(
        &mut self,
        _backend: &mut MockBackend,
        screen: u64,
        _scope_id: u64,
        _options: Box<dyn Any>,
    ) {
        *self.screen_node_out.borrow_mut() = Some(screen);
    }
}

#[test]
fn screen_style_overlay_merges_fill_rules_over_author_style() {
    let screen_node_out: Rc<RefCell<Option<u64>>> = Rc::new(RefCell::new(None));

    let mut mock = MockBackend::new();
    {
        let cell = screen_node_out.clone();
        mock.register_navigator::<StackPresentation, _>(move || {
            Box::new(OverlayHandler { screen_node_out: cell.clone() })
        });
    }

    let backend = Rc::new(RefCell::new(mock));
    let _owner = runtime_core::mount(backend.clone(), move || {
        StackNavigator::new(&HOME)
            .screen(HOME, |_| {
                // The screen's author style deliberately CONFLICTS on
                // `position` (the field the legacy CSS needed `!important`
                // for) and sets non-conflicting fields that must survive.
                let author = Rc::new(StyleSheet::r#static(StyleRules {
                    position: Some(Position::Relative),
                    background: Some(Tokenized::Literal(Color("#123456".into()))),
                    width: Some(Tokenized::Literal(Length::Px(240.0))),
                    ..Default::default()
                }));
                Screen::new(
                    view(vec![text("HOME").into()])
                        .with_style(StyleApplication::new(author)),
                )
            })
            .into()
    });

    let screen = screen_node_out
        .borrow()
        .expect("handler received the initial screen node");
    let b = backend.borrow();
    let node = b.node(screen).expect("screen node exists in the mock backend");
    let style = node
        .last_style
        .as_ref()
        .expect("walker applied a style to the screen root");

    // Overlay fields win over the author's conflicting position…
    assert_eq!(style.position, Some(Position::Absolute), "overlay pins the screen absolute");
    assert_eq!(style.top, Some(Tokenized::Literal(Length::Px(0.0))));
    assert_eq!(style.right, Some(Tokenized::Literal(Length::Px(0.0))));
    assert_eq!(style.bottom, Some(Tokenized::Literal(Length::Px(0.0))));
    assert_eq!(style.left, Some(Tokenized::Literal(Length::Px(0.0))));

    // …while the author's non-conflicting styling survives (the legacy
    // class stamp composed the same way; a wholesale style replace here
    // would be a regression).
    assert_eq!(
        style.background,
        Some(Tokenized::Literal(Color("#123456".into()))),
        "author background survives the overlay"
    );
    assert_eq!(
        style.width,
        Some(Tokenized::Literal(Length::Px(240.0))),
        "author width survives — the overlay deliberately sets no size"
    );
}
