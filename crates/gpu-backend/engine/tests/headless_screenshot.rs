//! Headless screenshot pipeline tests.
//!
//! Proves the offscreen path end-to-end: spin up a windowless wgpu
//! device, mount a real tree, render to an offscreen texture, read the
//! pixels back, and assert the content actually rasterized. Runs the
//! SAME `Renderer` + shaders the windowed host uses.
//!
//! Requires a usable wgpu adapter. On macOS Metal is always available;
//! on a GPU-less Linux box it needs a software adapter (Mesa lavapipe).
//! If no adapter exists at all, the tests **skip** (print + return)
//! rather than fail — a screenshot test can't run without a renderer,
//! and we don't want green CI to hinge on the runner having a GPU.
//!
//! Run with: `cargo test -p render-wgpu --features headless`

#![cfg(feature = "headless")]

use std::rc::Rc;

use render_wgpu::headless::Screenshotter;
use runtime_core::{
    Color, Element, IntoStyleSource, Length, SafeAreaSides, StyleApplication, StyleRules,
    StyleSheet, Tokenized,
};

/// A root View that fills the whole viewport with a solid background
/// color. `hex` like `"#2255cc"`.
fn colored_fill(hex: &'static str) -> Element {
    let sheet = Rc::new(StyleSheet::r#static({
        let mut r = StyleRules::default();
        r.width = Some(Tokenized::Literal(Length::Percent(100.0)));
        r.height = Some(Tokenized::Literal(Length::Percent(100.0)));
        r
    }));
    let style = StyleApplication::new(sheet).override_background(Color(hex.to_string()));
    Element::View {
        children: vec![],
        style: Some(style.into_style_source()),
        ref_fill: None,
        safe_area_sides: SafeAreaSides::NONE,
        on_touch: None,
        on_wheel: None,
        on_hover: None,
        is_container: false,
        accessibility: Default::default(),
    }
}

/// `Some(shot)` if a wgpu adapter is available, else `None` (test skips).
fn try_screenshotter(w: u32, h: u32) -> Option<Screenshotter> {
    match Screenshotter::new(w, h) {
        Ok(s) => Some(s),
        Err(e) => {
            eprintln!("[headless test] skipping — no usable wgpu adapter: {e}");
            None
        }
    }
}

fn center_pixel(rgba: &[u8], w: u32, h: u32) -> [u8; 4] {
    let (cx, cy) = (w / 2, h / 2);
    let i = ((cy * w + cx) * 4) as usize;
    [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
}

#[test]
fn headless_renders_full_bleed_color_to_pixels() {
    let (w, h) = (96u32, 64u32);
    let Some(mut shot) = try_screenshotter(w, h) else {
        return;
    };
    shot.mount(|| colored_fill("#2255cc")); // blue-dominant

    let rgba = shot.capture_rgba();
    assert_eq!(
        rgba.len(),
        (w * h * 4) as usize,
        "readback must be tightly-packed w*h*4 RGBA"
    );

    let [r, g, b, a] = center_pixel(&rgba, w, h);
    // Robust to sRGB/linear color-management differences: a #2255cc
    // fill must come back blue-dominant and opaque. (Exact channel
    // values depend on the blend/encoding path; dominance does not.)
    assert!(a > 200, "filled view must be opaque (alpha={a})");
    assert!(
        b > 120 && b > r && b > g,
        "center pixel must be blue-dominant for a #2255cc fill, got ({r},{g},{b},{a}). \
         If this is all-zero, the tree didn't render (layout/style/shader/readback broke)."
    );
}

#[test]
fn headless_distinguishes_distinct_colors() {
    let (w, h) = (64u32, 64u32);
    let Some(mut blue) = try_screenshotter(w, h) else {
        return;
    };
    blue.mount(|| colored_fill("#2233dd"));
    let blue_px = center_pixel(&blue.capture_rgba(), w, h);

    let Some(mut red) = try_screenshotter(w, h) else {
        return;
    };
    red.mount(|| colored_fill("#dd3322"));
    let red_px = center_pixel(&red.capture_rgba(), w, h);

    // Proves the renderer paints the actual content, not a constant
    // clear color: blue fill is blue-dominant, red fill is red-dominant.
    assert!(
        blue_px[2] > blue_px[0],
        "blue fill center should be blue-dominant, got {blue_px:?}"
    );
    assert!(
        red_px[0] > red_px[2],
        "red fill center should be red-dominant, got {red_px:?}"
    );
}

#[test]
fn headless_encodes_png() {
    let (w, h) = (48u32, 48u32);
    let Some(mut shot) = try_screenshotter(w, h) else {
        return;
    };
    shot.mount(|| colored_fill("#33aa55"));
    let png = shot.capture_png().expect("PNG encode");
    // PNG magic number.
    assert!(
        png.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]),
        "output must be a PNG (magic header)"
    );
    assert!(png.len() > 67, "PNG must carry more than just the header/IHDR");
}
