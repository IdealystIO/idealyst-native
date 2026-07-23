//! Icon + Image smoke test.
//!
//! - Three Lucide-style icons (outlined stroke, filled, custom color) at a
//!   couple of sizes, rendered via the GDI+ SVG-path renderer.
//! - One bitmap image (a gradient BMP generated to `%TEMP%` at startup)
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

/// Build a `size × size` 24-bit BGR gradient BMP (blue→green diagonal) and
/// return its bytes. Uncompressed BI_RGB, bottom-up rows padded to 4 bytes.
fn gradient_bmp(size: u32) -> Vec<u8> {
    let row_stride = ((size * 3 + 3) / 4) * 4;
    let pixel_bytes = row_stride * size;
    let file_size = 54 + pixel_bytes;
    let mut b = Vec::with_capacity(file_size as usize);
    // BITMAPFILEHEADER
    b.extend_from_slice(b"BM");
    b.extend_from_slice(&file_size.to_le_bytes());
    b.extend_from_slice(&0u32.to_le_bytes()); // reserved
    b.extend_from_slice(&54u32.to_le_bytes()); // pixel data offset
    // BITMAPINFOHEADER
    b.extend_from_slice(&40u32.to_le_bytes());
    b.extend_from_slice(&(size as i32).to_le_bytes());
    b.extend_from_slice(&(size as i32).to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes()); // planes
    b.extend_from_slice(&24u16.to_le_bytes()); // bpp
    b.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
    b.extend_from_slice(&pixel_bytes.to_le_bytes());
    b.extend_from_slice(&2835i32.to_le_bytes()); // 72 dpi
    b.extend_from_slice(&2835i32.to_le_bytes());
    b.extend_from_slice(&0u32.to_le_bytes());
    b.extend_from_slice(&0u32.to_le_bytes());
    // Pixels (BGR, bottom-up).
    for y in 0..size {
        let mut written = 0u32;
        for x in 0..size {
            let r = (255 * x / size.max(1)) as u8;
            let g = (255 * y / size.max(1)) as u8;
            let bl = 200u8;
            b.push(bl);
            b.push(g);
            b.push(r);
            written += 3;
        }
        while written < row_stride {
            b.push(0);
            written += 1;
        }
    }
    b
}

fn app() -> Element {
    // Stage a real bitmap to disk and reference it by file:// URL.
    let mut path = std::env::temp_dir();
    path.push("idealyst-smoke-gradient.bmp");
    std::fs::write(&path, gradient_bmp(64)).expect("write temp bmp");
    let url = format!("file:///{}", path.to_string_lossy().replace('\\', "/"));

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
