//! Shared component definitions for the hot-reload demo. Both the
//! `dev-server` and `app` binaries pull `app_root()` from here so
//! the dev side and the app side ship from the same source.
//!
//! **Edit this file** to see hot reload at work. The dev-server
//! watches `src/`; on save it rebuilds and self-execs, the
//! browser's WebSocket closes briefly, the client reconnects and
//! receives the new tree. The browser console prints the
//! end-to-end "change → apply" latency.

use std::rc::Rc;

use framework_core::{
    AlignItems, Color, FlexDirection, JustifyContent, Length, Primitive, StyleApplication,
    StyleRules, StyleSheet, TextAlign, TextSource, ThemeTokens, Tokenized,
};

pub mod print_backend;

/// Minimal theme — the demo doesn't use named tokens, just literal
/// values. The framework requires *some* theme installed before
/// render; this satisfies the contract.
struct DemoTheme;

impl ThemeTokens for DemoTheme {
    fn tokens(&self) -> Vec<framework_core::TokenEntry> {
        Vec::new()
    }
}

/// Build a `StyleSource::Static` from a literal `StyleRules`. Used
/// inline in the tree below to keep the demo's style construction
/// readable next to the elements it styles.
fn style(rules: StyleRules) -> framework_core::StyleSource {
    let sheet = Rc::new(StyleSheet::new::<DemoTheme, _>(move |_| rules.clone()));
    framework_core::StyleSource::Static(StyleApplication::new(sheet))
}

fn px(v: f32) -> Tokenized<Length> {
    Tokenized::Literal(Length::Px(v))
}

fn color(hex: &str) -> Tokenized<Color> {
    Tokenized::Literal(Color(hex.to_string()))
}

/// The user-authored UI tree. Edit me!
pub fn app_root() -> Primitive {
    // Install the stub theme on every render call — calling more
    // than once is fine, it just overwrites.
    framework_core::install_theme(DemoTheme);

    let page = StyleRules {
        background: Some(color("#0f172a")),
        color: Some(color("#e2e8f0")),
        flex_direction: Some(FlexDirection::Column),
        align_items: Some(AlignItems::FlexStart),
        gap: Some(px(20.0)),
        padding_top: Some(px(48.0)),
        padding_bottom: Some(px(48.0)),
        padding_left: Some(px(56.0)),
        padding_right: Some(px(56.0)),
        min_height: Some(Tokenized::Literal(Length::Percent(100.0))),
        font_family: Some("system-ui, -apple-system, Segoe UI, sans-serif".into()),
        ..StyleRules::default()
    };

    let heading = StyleRules {
        color: Some(color("#f8fafc")),
        font_size: Some(px(34.0)),
        font_weight: Some(framework_core::FontWeight::Bold),
        ..StyleRules::default()
    };

    let subtitle = StyleRules {
        color: Some(color("#94a3b8")),
        font_size: Some(px(15.0)),
        max_width: Some(px(540.0)),
        ..StyleRules::default()
    };

    let badge = StyleRules {
        background: Some(color("#1e293b")),
        color: Some(color("#38bdf8")),
        font_size: Some(px(12.0)),
        font_weight: Some(framework_core::FontWeight::Medium),
        padding_top: Some(px(4.0)),
        padding_bottom: Some(px(4.0)),
        padding_left: Some(px(10.0)),
        padding_right: Some(px(10.0)),
        border_top_left_radius: Some(px(999.0)),
        border_top_right_radius: Some(px(999.0)),
        border_bottom_left_radius: Some(px(999.0)),
        border_bottom_right_radius: Some(px(999.0)),
        ..StyleRules::default()
    };

    let action_button = StyleRules {
        background: Some(color("#DD11B1")),
        color: Some(color("#ffffff")),
        font_size: Some(px(15.0)),
        font_weight: Some(framework_core::FontWeight::SemiBold),
        padding_top: Some(px(12.0)),
        padding_bottom: Some(px(12.0)),
        padding_left: Some(px(22.0)),
        padding_right: Some(px(22.0)),
        border_top_left_radius: Some(px(10.0)),
        border_top_right_radius: Some(px(10.0)),
        border_bottom_left_radius: Some(px(10.0)),
        border_bottom_right_radius: Some(px(10.0)),
        text_align: Some(TextAlign::Center),
        ..StyleRules::default()
    };

    let row = StyleRules {
        flex_direction: Some(FlexDirection::Row),
        align_items: Some(AlignItems::Center),
        justify_content: Some(JustifyContent::FlexStart),
        gap: Some(px(12.0)),
        ..StyleRules::default()
    };

    Primitive::View {
        children: vec![
            Primitive::View {
                children: vec![Primitive::Text {
                    source: TextSource::Static("hot reload · live".into()),
                    style: None,
                    ref_fill: None,
                }],
                style: Some(style(badge)),
                ref_fill: None,
            },
            Primitive::Text {
                source: TextSource::Static("Hot-reload demo".into()),
                style: Some(style(heading)),
                ref_fill: None,
            },
            Primitive::Text {
                source: TextSource::Static(
                    "Edit examples/hot-reload-demo/src/lib.rs and save. \
                     The dev server rebuilds, the browser reconnects, and \
                     the new tree applies in place — no page reload."
                        .into(),
                ),
                style: Some(style(subtitle)),
                ref_fill: None,
            },
            Primitive::View {
                children: vec![Primitive::Button {
                    label: TextSource::Static("Press me 123".into()),
                    on_click: Rc::new(|| {
                        println!("(dev) button activated");
                    }),
                    leading_icon: None,
                    trailing_icon: None,
                    style: Some(style(action_button)),
                    ref_fill: None,
                    disabled: None,
                }],
                style: Some(style(row)),
                ref_fill: None,
            },
        ],
        style: Some(style(page)),
        ref_fill: None,
    }
}
