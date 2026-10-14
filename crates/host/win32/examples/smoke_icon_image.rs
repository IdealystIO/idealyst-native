//! Icon + Image smoke test.
//!
//! - Three Lucide-style icons (outlined stroke, filled, custom color) at a
//!   couple of sizes, rendered via the GDI+ SVG-path renderer.
//! - One image (a gradient SVG passed as a percent-encoded `data:` URI)
//!   shown at `object-fit: contain` inside a bordered card.
//!
//! ```text
//! cargo run -p host-win32 --example smoke_icon_image
//! ```

use std::rc::Rc;

use runtime_core::{
    icon, image, text, view, Color, Element, FillRule, FlexDirection, IconData, ObjectFit,
    StyleRules, StyleSheet,
};

// A few inline Lucide-style glyphs (24×24 view box).
const CHECK: IconData = IconData {
    view_box: (24, 24),
    paths: &["M20 6 9 17l-5-5"],
    fill_rule: FillRule::NonZero,
    filled: false,
};
const CIRCLE: IconData = IconData {
    view_box: (24, 24),
    paths: &["M12 2a10 10 0 1 0 0 20 10 10 0 0 0 0-20z"],
    fill_rule: FillRule::NonZero,
    filled: false,
};
const DOT_FILLED: IconData = IconData {
    view_box: (24, 24),
    paths: &["M12 2a10 10 0 1 0 0 20 10 10 0 0 0 0-20z"],
    fill_rule: FillRule::NonZero,
    filled: true,
};

fn card_style(w: f32, h: f32) -> Rc<StyleSheet> {
    Rc::new(StyleSheet::r#static(StyleRules {
        background: Some("#f2f2f7".into()),
        border_top_left_radius: Some(12.0.into()),
        border_top_right_radius: Some(12.0.into()),
        border_bottom_right_radius: Some(12.0.into()),
        border_bottom_left_radius: Some(12.0.into()),
        // Lay the icons out left-to-right with spacing + padding so all
        // four fit the card (the framework default is Column).
        flex_direction: Some(FlexDirection::Row),
        gap: Some(12.0.into()),
        padding_top: Some(12.0.into()),
        padding_right: Some(12.0.into()),
        padding_bottom: Some(12.0.into()),
        padding_left: Some(12.0.into()),
        width: Some(w.into()),
        height: Some(h.into()),
        ..Default::default()
    }))
}

fn image_card(w: f32, h: f32, fit: ObjectFit) -> Rc<StyleSheet> {
    Rc::new(StyleSheet::r#static(StyleRules {
        object_fit: Some(fit),
        border_top_width: Some(2.0.into()),
        border_right_width: Some(2.0.into()),
        border_bottom_width: Some(2.0.into()),
        border_left_width: Some(2.0.into()),
        border_top_color: Some("#2f6fed".into()),
        border_right_color: Some("#2f6fed".into()),
        border_bottom_color: Some("#2f6fed".into()),
        border_left_color: Some("#2f6fed".into()),
        width: Some(w.into()),
        height: Some(h.into()),
        ..Default::default()
    }))
}

/// A 64×64 diagonal gradient as an SVG document. Shipped to `image()` as a
/// percent-encoded `data:` URI — a `src` every backend loads
/// (`docs/image-src.md`). This example used to stage a BMP to `%TEMP%` and
/// load it by `file://` URL, which the contract rejects: `file:` is readable
/// on some platforms only.
const GRADIENT_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64">
<defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1">
<stop offset="0" stop-color="#0000c8"/><stop offset="1" stop-color="#ffffc8"/>
</linearGradient></defs>
<rect width="64" height="64" fill="url(#g)"/>
</svg>"##;

/// `data:image/svg+xml,<percent-encoded document>`.
fn svg_data_uri(svg: &str) -> String {
    let mut out = String::from("data:image/svg+xml,");
    for b in svg.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn app() -> Element {
    let url = svg_data_uri(GRADIENT_SVG);

    view(vec![
        text("Icons (stroke / filled / colored):").into(),
        // A row of icons on a light card.
        view(vec![
            icon(CHECK).size(48.0).into(),
            icon(CIRCLE).size(48.0).into(),
            icon(DOT_FILLED).size(48.0).color(|| Color("#2f6fed".into())).into(),
            icon(CHECK).size(32.0).color(|| Color("#e5484d".into())).into(),
        ])
        .with_style(card_style(300.0, 80.0))
        .into(),
        text("Image (object-fit: contain):").into(),
        image(url.clone()).with_style(image_card(200.0, 140.0, ObjectFit::Contain)).into(),
    ])
    .into()
}

fn main() {
    let opts = host_win32::RunOptions {
        title: "Idealyst — Win32 icon + image smoke".to_string(),
        width: 380,
        height: 420,
    };
    std::process::exit(host_win32::run(opts, app));
}
