//! An app-installed [`LayoutTheme`] reaches native layout.
//!
//! REGRESSION (Timely on macOS): the app installed a 12pt gutter at every
//! form factor and the page header still padded by 32, the standard
//! desktop gutter, while web padded by 12. Native layout read the
//! token's fallback instead of the installed value.

use host_mock::Harness;
use idea_ui::{install_idea_theme, light_theme};
use idea_ui_layout::LayoutTheme;
use runtime_core::Tokens as _;
use runtime_layout::LayoutTree;
use runtime_shared::{Length, StyleRules};

#[test]
fn regression_an_installed_gutter_lays_out_natively_at_the_installed_width() {
    host_mock::pump::install_scheduler();
    let h = Harness::new();
    // The rules a kit sheet's desktop arm holds, built BEFORE the app's
    // install, as a sheet resolved at startup would be.
    let gutter = |at: fn() -> runtime_shared::Tokenized<Length>| StyleRules {
        padding_left: Some(at()),
        padding_right: Some(at()),
        ..Default::default()
    };
    let desktop = || LayoutTheme::tokens().gutter.desktop();
    let early = h.world.enter(|| gutter(desktop));
    h.world.enter(|| {
        install_idea_theme(light_theme());
        LayoutTheme::install(LayoutTheme::standard().with(|t| {
            t.gutter.phone = 12.0.into();
            t.gutter.tablet = 12.0.into();
            t.gutter.desktop = 12.0.into();
        }));
    });
    h.flush();

    let mut tree = LayoutTree::new();
    let row = tree.new_node();
    tree.set_style(row, &early);
    let first = tree.new_node();
    tree.set_style(first, &StyleRules { height: Some(Length::Px(10.0).into()), ..Default::default() });
    tree.add_child(row, first);
    tree.compute(row, 1440.0, 100.0);

    let frame = tree.frame_of(first);
    assert_eq!(frame.x, 12.0, "the header row's first child stands at the installed gutter, not 32");
    assert_eq!(frame.width, 1440.0 - 24.0);
}
