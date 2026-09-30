//! Headless screenshot pipeline tests.
//!
//! Proves the offscreen path end-to-end: spin up a windowless wgpu
//! device, mount a real tree, render to an offscreen texture, read the
//! pixels back, and assert the content actually rasterized. Runs the
//! SAME `Renderer` + shaders the windowed host uses.
//!
//! Runtime v2: the tree is a `runtime_scene::Element` built with the
//! vocabulary builders and mounted with
//! `render_wgpu::newcore::start(shot.backend(), register, build)` — the
//! same offscreen idiom `newcore-gpu-smoke`'s `NEWCORE_SMOKE_HEADLESS`
//! mode and `idea-ui-docs-gpu`'s render test use — replacing the old
//! `Element::View` literal + `Screenshotter::mount`. Same renderer, same
//! shaders, same assertions.
//!
//! Requires a usable wgpu adapter. On macOS Metal is always available;
//! on a GPU-less Linux box it needs a software adapter (Mesa lavapipe).
//! If no adapter exists at all, the tests **skip** (print + return)
//! rather than fail — a screenshot test can't run without a renderer,
//! and we don't want green CI to hinge on the runner having a GPU.
//!
//! Run with: `cargo test -p render-wgpu --features headless`

#![cfg(feature = "headless")]

use render_wgpu::headless::Screenshotter;
use runtime_scene::Element;
use runtime_shared::{Color, Length, StyleRules, Tokenized};

/// A root View that fills the whole viewport with a solid background
/// color. `hex` like `"#2255cc"`.
fn colored_fill(hex: &'static str) -> Element {
    runtime_vocabulary::view()
        .style(StyleRules {
            width: Some(Tokenized::Literal(Length::Percent(100.0))),
            height: Some(Tokenized::Literal(Length::Percent(100.0))),
            background: Some(Tokenized::Literal(Color(hex.to_string()))),
            ..Default::default()
        })
        .build()
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
    let _app = render_wgpu::newcore::start(shot.backend(), |_| {}, || colored_fill("#2255cc")); // blue-dominant

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
    let _blue_app = render_wgpu::newcore::start(blue.backend(), |_| {}, || colored_fill("#2233dd"));
    let blue_px = center_pixel(&blue.capture_rgba(), w, h);

    let Some(mut red) = try_screenshotter(w, h) else {
        return;
    };
    let _red_app = render_wgpu::newcore::start(red.backend(), |_| {}, || colored_fill("#dd3322"));
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
    let _app = render_wgpu::newcore::start(shot.backend(), |_| {}, || colored_fill("#33aa55"));
    let png = shot.capture_png().expect("PNG encode");
    // PNG magic number.
    assert!(
        png.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]),
        "output must be a PNG (magic header)"
    );
    assert!(
        png.len() > 67,
        "PNG must carry more than just the header/IHDR"
    );
}

// ---------------------------------------------------------------------------
// Patterned borders
// ---------------------------------------------------------------------------

/// A black root holding a `bw`-px white-bordered box of `w × h` at the
/// origin, with corner `radius` and border `style`.
fn bordered_box(
    w: f32,
    h: f32,
    bw: f32,
    radius: f32,
    style: Option<runtime_shared::BorderStyle>,
) -> Element {
    let white = || Some(Tokenized::Literal(Color("#ffffff".to_string())));
    let px = |v: f32| Some(Tokenized::Literal(Length::Px(v)));
    runtime_vocabulary::view()
        .style(StyleRules {
            width: Some(Tokenized::Literal(Length::Percent(100.0))),
            height: Some(Tokenized::Literal(Length::Percent(100.0))),
            background: Some(Tokenized::Literal(Color("#000000".to_string()))),
            ..Default::default()
        })
        .child(
            runtime_vocabulary::view()
                .style(StyleRules {
                    width: px(w),
                    height: px(h),
                    border_top_width: Some(Tokenized::Literal(bw)),
                    border_right_width: Some(Tokenized::Literal(bw)),
                    border_bottom_width: Some(Tokenized::Literal(bw)),
                    border_left_width: Some(Tokenized::Literal(bw)),
                    border_top_color: white(),
                    border_right_color: white(),
                    border_bottom_color: white(),
                    border_left_color: white(),
                    border_top_left_radius: px(radius),
                    border_top_right_radius: px(radius),
                    border_bottom_right_radius: px(radius),
                    border_bottom_left_radius: px(radius),
                    border_style: style,
                    ..Default::default()
                })
                .build(),
        )
        .build()
}

/// Ink flags along row `y` for x in `x0..x1` (bright = inked).
fn row_ink(rgba: &[u8], w: u32, y: u32, x0: u32, x1: u32) -> Vec<bool> {
    (x0..x1)
        .map(|x| {
            let i = ((y * w + x) * 4) as usize;
            rgba[i] > 128 && rgba[i + 1] > 128 && rgba[i + 2] > 128
        })
        .collect()
}

/// Optional eyeballing: `DASHED_BORDER_PNG_DIR=/some/dir` writes each
/// patterned-border capture there as a PNG.
fn maybe_dump(shot: &mut Screenshotter, name: &str) {
    if let Ok(dir) = std::env::var("DASHED_BORDER_PNG_DIR") {
        let png = shot.capture_png().expect("PNG encode");
        std::fs::write(std::path::Path::new(&dir).join(name), png).expect("write PNG");
    }
}

/// Bug: `border_style: Dashed` was ignored by the GPU engine — the rect
/// shader drew its unbroken SDF ring. The top edge of a dashed box must
/// show gaps where the solid box's edge is continuous.
#[test]
fn regression_gpu_dashed_border_not_painted_as_solid_ring() {
    let (w, h) = (96u32, 64u32);
    let Some(mut solid) = try_screenshotter(w, h) else {
        return;
    };
    let _a = render_wgpu::newcore::start(solid.backend(), |_| {}, || {
        bordered_box(62.0, 42.0, 2.0, 0.0, None)
    });
    let solid_rgba = solid.capture_rgba();
    maybe_dump(&mut solid, "gpu_solid_box.png");
    let solid_row = row_ink(&solid_rgba, w, 1, 4, 58);
    assert!(solid_row.iter().all(|&on| on), "solid top edge is continuous: {solid_row:?}");

    let Some(mut dashed) = try_screenshotter(w, h) else {
        return;
    };
    let _b = render_wgpu::newcore::start(dashed.backend(), |_| {}, || {
        bordered_box(62.0, 42.0, 2.0, 0.0, Some(runtime_shared::BorderStyle::Dashed))
    });
    let rgba = dashed.capture_rgba();
    maybe_dump(&mut dashed, "gpu_dashed_box.png");
    let row = row_ink(&rgba, w, 1, 4, 58);
    assert!(row.iter().any(|&on| on), "dashed edge has ink: {row:?}");
    assert!(row.iter().any(|&on| !on), "dashed edge has gaps: {row:?}");
    // The interior stays unpainted (the ring is gone, not just broken).
    assert_eq!(row_ink(&rgba, w, 10, 10, 50).iter().filter(|&&on| on).count(), 0);
}

/// A dotted pill: the straight top edge alternates dot / gap, and the
/// rounded ends carry ink too (the marks follow the arc).
#[test]
fn headless_dotted_pill_has_dots_on_edges_and_arcs() {
    let (w, h) = (128u32, 48u32);
    let Some(mut shot) = try_screenshotter(w, h) else {
        return;
    };
    let _app = render_wgpu::newcore::start(shot.backend(), |_| {}, || {
        bordered_box(100.0, 24.0, 2.0, 12.0, Some(runtime_shared::BorderStyle::Dotted))
    });
    let rgba = shot.capture_rgba();
    maybe_dump(&mut shot, "gpu_dotted_pill.png");
    let top = row_ink(&rgba, w, 1, 14, 86);
    let transitions = top.windows(2).filter(|p| p[0] != p[1]).count();
    assert!(transitions > 20, "dots alternate along the top edge: {top:?}");
    // The right-hand arc's outermost column (x ≈ 99, y = 12) is inked
    // somewhere near its middle.
    let arc_ink = (8..17).any(|y| row_ink(&rgba, w, y, 97, 100).iter().any(|&on| on));
    assert!(arc_ink, "the rounded end carries dots");
}


/// Bug: `rect.wgsl` computed the solid ring's coverage with the SDF sign
/// inverted (`-d`), which placed the ring OUTSIDE the quad — a solid
/// border painted nothing (a square box lit 0 pixels; rounded boxes lit
/// only slivers past their corners). The ring must be exactly the
/// border band: 62×42 minus the 58×38 interior = 400 pixels.
#[test]
fn regression_gpu_solid_border_ring_painted_nothing() {
    let (w, h) = (96u32, 64u32);
    let Some(mut shot) = try_screenshotter(w, h) else {
        return;
    };
    let _app = render_wgpu::newcore::start(shot.backend(), |_| {}, || {
        bordered_box(62.0, 42.0, 2.0, 0.0, None)
    });
    let rgba = shot.capture_rgba();
    let lit = rgba.chunks_exact(4).filter(|p| p[0] > 128).count();
    assert_eq!(lit, 400, "solid 2px ring on a 62×42 box");
    assert_eq!(row_ink(&rgba, w, 10, 10, 50).iter().filter(|&&on| on).count(), 0);
}

/// The border fades with its node: at `opacity: 0.5` a white ring over
/// black comes out mid-grey, dashed marks included.
#[test]
fn headless_border_fades_with_node_opacity() {
    let (w, h) = (96u32, 64u32);
    for style in [None, Some(runtime_shared::BorderStyle::Dashed)] {
        let Some(mut shot) = try_screenshotter(w, h) else {
            return;
        };
        let _app = render_wgpu::newcore::start(shot.backend(), |_| {}, move || {
            let root = bordered_box(62.0, 42.0, 2.0, 0.0, style);
            runtime_vocabulary::view()
                .style(StyleRules {
                    width: Some(Tokenized::Literal(Length::Percent(100.0))),
                    height: Some(Tokenized::Literal(Length::Percent(100.0))),
                    opacity: Some(Tokenized::Literal(0.5)),
                    ..Default::default()
                })
                .child(root)
                .build()
        });
        let rgba = shot.capture_rgba();
        // (4, 1): inside the first dash / on the solid ring.
        let i = ((1 * w + 4) * 4) as usize;
        let r = rgba[i];
        assert!((60..=200).contains(&r), "{style:?}: half-faded white, got {r}");
    }
}
