//! GPU tests for the surface-less [`HeadlessCompositor`] and the static-image
//! [`TextureLayer`] path (the watermark primitive the `video-compose` SDK builds
//! on). They run on the machine's real GPU; if no adapter is available (some CI)
//! `HeadlessCompositor::new()` returns `None` and the test skips rather than
//! failing — the macOS dev machine (Metal) always exercises them.

use canvas_core::{ImageSource, Scene, TextureLayer};
use canvas_vello::HeadlessCompositor;
use std::rc::Rc;
use std::sync::Arc;

/// Sample the RGBA pixel at `(x, y)` in a tightly-packed top-down buffer.
fn px(data: &[u8], w: u32, x: u32, y: u32) -> (u8, u8, u8, u8) {
    let i = ((y * w + x) * 4) as usize;
    (data[i], data[i + 1], data[i + 2], data[i + 3])
}

fn solid(id: u64, w: u32, h: u32, rgba: [u8; 4]) -> Arc<ImageSource> {
    let mut buf = Vec::with_capacity((w * h * 4) as usize);
    for _ in 0..w * h {
        buf.extend_from_slice(&rgba);
    }
    Arc::new(ImageSource::from_rgba8(id, w, h, buf))
}

/// A static-image layer composites onto the target, and re-compositing the same
/// `id` reuses the cached upload (still correct); bumping the pixels under a new
/// generation re-uploads. This is the watermark path end to end on the GPU.
#[test]
fn image_layer_composites_and_recaches() {
    let Some(mut c) = HeadlessCompositor::new() else {
        eprintln!("no GPU adapter — skipping headless compositor test");
        return;
    };
    let (w, h) = (16u32, 16u32);

    // Fill-fit a solid green image over the whole 16×16 target.
    let green = solid(1, 4, 4, [0, 255, 0, 255]);
    let layer = TextureLayer::image(
        Rc::new(move || Some(green.clone())),
        Rc::new(move || (0.0, 0.0, w as f32, h as f32)),
    )
    .fit(canvas_core::Fit::Fill);

    c.composite(&Scene::new(), std::slice::from_ref(&layer), &Scene::new(), w, h, 1.0);
    let img = c.read_rgba().expect("composited once");
    assert_eq!((img.width, img.height), (w, h));
    let (r, g, b, a) = px(&img.data, w, 8, 8);
    assert!(g > 200 && r < 60 && b < 60 && a > 200, "center should be green, got {:?}", (r, g, b, a));

    // Re-composite the SAME id → served from the cache, still green.
    c.composite(&Scene::new(), std::slice::from_ref(&layer), &Scene::new(), w, h, 1.0);
    let (_, g2, _, _) = px(&c.read_rgba().unwrap().data, w, 8, 8);
    assert!(g2 > 200, "cached re-composite should stay green, got g={g2}");

    // A NEW image (different id) with red pixels re-uploads and overwrites.
    let red = solid(2, 4, 4, [255, 0, 0, 255]);
    let red_layer = TextureLayer::image(
        Rc::new(move || Some(red.clone())),
        Rc::new(move || (0.0, 0.0, w as f32, h as f32)),
    )
    .fit(canvas_core::Fit::Fill);
    c.composite(&Scene::new(), std::slice::from_ref(&red_layer), &Scene::new(), w, h, 1.0);
    let (r3, g3, _, _) = px(&c.read_rgba().unwrap().data, w, 8, 8);
    assert!(r3 > 200 && g3 < 60, "second image should be red, got r={r3} g={g3}");
}

/// The overlay scene composites ON TOP of the texture layers — drawn text /
/// graphics must sit above the video, not behind it. A red square drawn in the
/// overlay over a full-frame green layer must win where it's drawn.
#[test]
fn overlay_scene_draws_over_layers() {
    let Some(mut c) = HeadlessCompositor::new() else {
        eprintln!("no GPU adapter — skipping overlay test");
        return;
    };
    let (w, h) = (16u32, 16u32);

    let green = solid(7, 4, 4, [0, 255, 0, 255]);
    let layer = TextureLayer::image(
        Rc::new(move || Some(green.clone())),
        Rc::new(move || (0.0, 0.0, w as f32, h as f32)),
    )
    .fit(canvas_core::Fit::Fill);

    // Overlay: an opaque red 6×6 square at the top-left corner.
    let mut overlay = Scene::new();
    overlay.path().add_path(canvas_core::Path::rect(0.0, 0.0, 6.0, 6.0));
    overlay.fill(canvas_core::Color::new(255, 0, 0, 255));

    c.composite(&Scene::new(), std::slice::from_ref(&layer), &overlay, w, h, 1.0);
    let data = c.read_rgba().unwrap().data;
    let (r, g, _, _) = px(&data, w, 2, 2);
    assert!(r > 200 && g < 60, "overlay red should be on top at the corner, got r={r} g={g}");
    let (r2, g2, _, _) = px(&data, w, 12, 12);
    assert!(g2 > 200 && r2 < 60, "layer green should remain where the overlay isn't, got r={r2} g={g2}");
}

/// A partially-transparent watermark blends over the scene beneath it: the
/// shader's `use_src_alpha` path must honor the image's straight alpha (a bug
/// there would paint the watermark fully opaque or fully invisible).
#[test]
fn image_layer_respects_source_alpha() {
    let Some(mut c) = HeadlessCompositor::new() else {
        eprintln!("no GPU adapter — skipping alpha test");
        return;
    };
    let (w, h) = (8u32, 8u32);

    // Draw an opaque blue background, then a 50%-alpha white image over it.
    let mut scene = Scene::new();
    scene.path().add_path(canvas_core::Path::rect(0.0, 0.0, w as f32, h as f32));
    scene.fill(canvas_core::Color::new(0, 0, 255, 255));

    let translucent = solid(3, 2, 2, [255, 255, 255, 128]);
    let layer = TextureLayer::image(
        Rc::new(move || Some(translucent.clone())),
        Rc::new(move || (0.0, 0.0, w as f32, h as f32)),
    )
    .fit(canvas_core::Fit::Fill);

    c.composite(&scene, std::slice::from_ref(&layer), &Scene::new(), w, h, 1.0);
    let (r, g, b, _) = px(&c.read_rgba().unwrap().data, w, 4, 4);
    // ~50% white over blue → roughly (128, 128, 255): red/green lifted well off 0,
    // blue still high. A fully-opaque bug → (255,255,255); a no-alpha bug → blue.
    assert!(r > 60 && r < 220, "red should be a partial blend, got {r}");
    assert!(g > 60 && g < 220, "green should be a partial blend, got {g}");
    assert!(b > 150, "blue background should still show through, got {b}");
}
