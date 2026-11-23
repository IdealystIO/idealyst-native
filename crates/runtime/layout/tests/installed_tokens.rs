//! A token-valued layout property lays out at the INSTALLED value, the
//! way paint and the web's `var(--name)` cascade read it — not at the
//! token's fallback.
//!
//! REGRESSION (Timely on macOS): the app installed
//! `LayoutTheme { gutter.desktop = 12 }` and idea-ui-layout's page gutter
//! (`padding_left: LayoutTheme::tokens().gutter.desktop()`) laid out at
//! 32, the token's fallback (the standard theme's value). Web, reading
//! `var(--layout-gutter-desktop)`, padded by 12. Colors were unaffected:
//! the backends paint them through `Tokenized::resolve`.

use runtime_layout::LayoutTree;
use runtime_shared::{Length, StyleRules, TokenEntry, TokenValue, Tokenized};

const GUTTER: &str = "test-layout-gutter";
const GAP: &str = "test-layout-gap";
const SIDE: &str = "test-layout-side";

/// A 400-wide row padded by the `GUTTER` token, holding two `SIDE`-wide
/// boxes `GAP` apart. Every token's fallback differs from what installs.
fn row(t: &mut LayoutTree) -> (runtime_layout::LayoutNode, runtime_layout::LayoutNode, runtime_layout::LayoutNode) {
    let root = t.new_node();
    t.set_style(
        root,
        &StyleRules {
            flex_direction: Some(runtime_shared::FlexDirection::Row),
            padding_left: Some(Tokenized::token(GUTTER, Length::Px(32.0))),
            gap: Some(Tokenized::token(GAP, Length::Px(30.0))),
            ..Default::default()
        },
    );
    let side = || StyleRules {
        width: Some(Tokenized::token(SIDE, Length::Px(50.0))),
        height: Some(Length::Px(10.0).into()),
        ..Default::default()
    };
    let a = t.new_node();
    t.set_style(a, &side());
    let b = t.new_node();
    t.set_style(b, &side());
    t.add_child(root, a);
    t.add_child(root, b);
    (root, a, b)
}

fn px(name: &'static str, v: f32) -> TokenEntry {
    TokenEntry { name, value: TokenValue::Length(Length::Px(v)) }
}

#[test]
fn regression_installed_length_tokens_lay_out_at_their_installed_value() {
    runtime_shared::install_tokens(&[px(GUTTER, 12.0), px(GAP, 6.0), px(SIDE, 40.0)]);

    let mut t = LayoutTree::new();
    let (root, a, b) = row(&mut t);
    t.compute(root, 400.0, 100.0);

    let fa = t.frame_of(a);
    let fb = t.frame_of(b);
    assert_eq!(fa.x, 12.0, "padding_left is the installed gutter, not the 32 fallback");
    assert_eq!(fa.width, 40.0, "width is the installed size, not the 50 fallback");
    assert_eq!(fb.x, 12.0 + 40.0 + 6.0, "gap is the installed gap, not the 30 fallback");

    // A swap re-applies the node's rules (the theme cohort does this on
    // native); the re-apply lays out at the new value.
    runtime_shared::update_tokens(&[px(GUTTER, 20.0)]);
    t.set_style(
        root,
        &StyleRules {
            flex_direction: Some(runtime_shared::FlexDirection::Row),
            padding_left: Some(Tokenized::token(GUTTER, Length::Px(32.0))),
            gap: Some(Tokenized::token(GAP, Length::Px(30.0))),
            ..Default::default()
        },
    );
    t.compute(root, 400.0, 100.0);
    assert_eq!(t.frame_of(a).x, 20.0, "the swapped gutter lays out");
}

/// With nothing installed for a name, the fallback is the value — the
/// same answer `var(--name, fallback)` gives on web.
#[test]
fn an_uninstalled_token_lays_out_at_its_fallback() {
    let mut t = LayoutTree::new();
    let root = t.new_node();
    t.set_style(
        root,
        &StyleRules {
            padding_left: Some(Tokenized::token("test-layout-never-installed", Length::Px(32.0))),
            ..Default::default()
        },
    );
    let child = t.new_node();
    t.set_style(child, &StyleRules { height: Some(Length::Px(10.0).into()), ..Default::default() });
    t.add_child(root, child);
    t.compute(root, 400.0, 100.0);
    assert_eq!(t.frame_of(child).x, 32.0);
}
