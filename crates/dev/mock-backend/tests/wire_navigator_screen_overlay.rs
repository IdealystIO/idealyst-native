//! Regression: a stack navigator's screen style overlay
//! (`screen_flow_fill_rules`, applied by the stack handler's
//! `screen_overlay`) must be layered onto every mounted screen root's
//! style — and it must survive the trip over the wire, so a thin client
//! lays screens out the same way a locally-rendered app does.
//!
//! This is the substrate mechanism that replaced the legacy web/SSR
//! `ui-nav-screen` class stamp (raw class + `!important` CSS injected
//! into `<head>`). The class-splice approach had a latent bug this test
//! guards against by construction: the web backend's style apply does a
//! full `className` replace, so a spliced class silently vanished on the
//! first reactive restyle of the screen root. Riding the style OVERRIDE
//! layer means the placement re-merges on every resolution — and the
//! recorder ships the RESOLVED rules, so the client sees the merged
//! result rather than a class it would have to know about.
//!
//! (Ported from the deleted `navigator_screen_style_overlay.rs`, which
//! drove the same invariant through the old `NavigatorHost::
//! set_screen_style_overlay` seam on a locally-mounted handler.)

use mock_backend::WireHarness;
use runtime_shared::primitives::navigator::Route;
use runtime_shared::{Length, StyleRules, Tokenized};
use runtime_vocabulary::builders::{navigator_outlet, stack_navigator, text, view};

const HOME: Route<()> = Route::<()>::new("home", "/");

#[test]
fn stack_screen_overlay_reaches_the_client_merged_with_the_author_style() {
    // The screen root carries an author style whose fields must SURVIVE
    // (width) alongside the overlay's placement fields (flex_grow /
    // flex_basis / min_height), and one the overlay must WIN
    // (flex_grow: the author asks for 0, the overlay pins 1 so a screen
    // fills the outlet instead of collapsing to content height).
    let author = StyleRules {
        width: Some(Tokenized::Literal(Length::Px(321.0))),
        flex_grow: Some(Tokenized::Literal(0.0)),
        ..Default::default()
    };

    let h = WireHarness::mount(move || {
        stack_navigator(&HOME)
            .screen(HOME, move |_| {
                view()
                    .style(author.clone())
                    .child(text().content("HOME CONTENT"))
                    .build()
            })
            .layout(|| view().child(navigator_outlet()).build())
            .build()
    });

    let scene = h.scene();
    let text_node = scene
        .find_node_with_text("HOME CONTENT")
        .expect("home screen replayed");
    let screen_root = scene.parent_of(text_node).expect("screen root");
    let style = scene
        .node(screen_root)
        .and_then(|n| n.last_style.clone())
        .expect("the screen root's style crossed the wire");

    assert_eq!(
        style.flex_grow,
        Some(Tokenized::Literal(1.0)),
        "the navigator's screen overlay must WIN over the screen's own \
         conflicting field (a flex_grow:0 screen would collapse in the outlet)"
    );
    assert_eq!(
        style.flex_basis,
        Some(Tokenized::Literal(Length::Px(0.0))),
        "the overlay's placement fields must reach the client"
    );
    assert_eq!(
        style.width,
        Some(Tokenized::Literal(Length::Percent(100.0))),
        "the overlay's width pins full-bleed"
    );
}
