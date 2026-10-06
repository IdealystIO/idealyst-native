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

fn fill_rect(s: &mut Scene, x: f32, y: f32, w: f32, h: f32, rgba: [u8; 4]) {
    s.fill_path(
        canvas_core::Path::rect(x, y, w, h),
        canvas_core::Color::new(rgba[0], rgba[1], rgba[2], rgba[3]),
    );
}

fn full_layer(id: u64, rect: (f32, f32, f32, f32), rgba: [u8; 4]) -> TextureLayer {
    let img = solid(id, 4, 4, rgba);
    TextureLayer::image(Rc::new(move || Some(img.clone())), Rc::new(move || rect))
        .fit(canvas_core::Fit::Fill)
}

fn assert_px(data: &[u8], w: u32, x: u32, y: u32, want: [u8; 3], what: &str) {
    let (r, g, b, a) = px(data, w, x, y);
    let close = |got: u8, want: u8| (got as i32 - want as i32).abs() < 40;
    assert!(
        close(r, want[0]) && close(g, want[1]) && close(b, want[2]) && a > 200,
        "{what} at ({x},{y}): want {want:?}, got {:?}",
        (r, g, b, a)
    );
}

const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];
const YELLOW: [u8; 4] = [255, 255, 0, 255];

/// `Scene::texture(i)` composites the layer at that point: an opaque fill drawn
/// after it is ON TOP of the texture there, the texture shows elsewhere — and
/// the author's transform (set before the texture) still applies after it.
#[test]
fn ops_after_a_texture_op_draw_over_the_texture() {
    let Some(mut c) = HeadlessCompositor::new() else {
        eprintln!("no GPU adapter — skipping texture-op test");
        return;
    };
    let (w, h) = (16u32, 16u32);
    let layer = full_layer(10, (0.0, 0.0, 16.0, 16.0), GREEN);
    let mut base = Scene::new();
    base.transform(canvas_core::Transform::translate(8.0, 0.0));
    base.texture(0);
    fill_rect(&mut base, 0.0, 0.0, 8.0, 16.0, RED); // → x 8..16 under the translate

    c.composite(&base, std::slice::from_ref(&layer), &Scene::new(), w, h, 1.0);
    let data = c.read_rgba().unwrap().data;
    assert_px(&data, w, 12, 8, [255, 0, 0], "fill drawn after the texture is on top");
    assert_px(&data, w, 3, 8, [0, 255, 0], "texture shows where nothing covers it");
}

/// A layer the scene never places still composites over the whole scene (the
/// behavior before texture ops existed).
#[test]
fn an_unplaced_layer_composites_over_the_scene() {
    let Some(mut c) = HeadlessCompositor::new() else {
        eprintln!("no GPU adapter — skipping unplaced-layer test");
        return;
    };
    let (w, h) = (16u32, 16u32);
    let layer = full_layer(11, (8.0, 0.0, 8.0, 16.0), GREEN);
    let mut base = Scene::new();
    fill_rect(&mut base, 0.0, 0.0, 16.0, 16.0, BLUE);

    c.composite(&base, std::slice::from_ref(&layer), &Scene::new(), w, h, 1.0);
    let data = c.read_rgba().unwrap().data;
    assert_px(&data, w, 12, 8, [0, 255, 0], "unplaced layer over the scene");
    assert_px(&data, w, 3, 8, [0, 0, 255], "scene where the layer isn't");
}

/// Two texture ops each followed by vector ops: both runs render through the
/// same overlay texture, so the first run's composite must reach the GPU before
/// the second run's vello pass overwrites it. If it didn't, the first run would
/// composite the SECOND run's pixels (blue missing, yellow under the red layer).
#[test]
fn two_texture_runs_each_composite_their_own_content_in_order() {
    let Some(mut c) = HeadlessCompositor::new() else {
        eprintln!("no GPU adapter — skipping two-run test");
        return;
    };
    let (w, h) = (16u32, 16u32);
    let layers = [
        full_layer(12, (0.0, 0.0, 16.0, 16.0), GREEN),
        full_layer(13, (4.0, 4.0, 8.0, 8.0), RED),
    ];
    let mut base = Scene::new();
    base.texture(0);
    fill_rect(&mut base, 0.0, 0.0, 16.0, 8.0, BLUE); // run 1: top half
    base.texture(1);
    fill_rect(&mut base, 0.0, 10.0, 16.0, 6.0, YELLOW); // run 2: bottom strip

    c.composite(&base, &layers, &Scene::new(), w, h, 1.0);
    let data = c.read_rgba().unwrap().data;
    assert_px(&data, w, 2, 2, [0, 0, 255], "run 1 over layer 0");
    assert_px(&data, w, 8, 6, [255, 0, 0], "layer 1 over run 1");
    assert_px(&data, w, 2, 9, [0, 255, 0], "layer 0 alone");
    assert_px(&data, w, 8, 11, [255, 255, 0], "run 2 over layer 1");
    assert_px(&data, w, 2, 14, [255, 255, 0], "run 2 over layer 0");
}
