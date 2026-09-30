//! Browser tests for canvas-native's web renderer: a scene replayed into
//! the `<canvas>` 2D context (checked by reading pixels back), the image and
//! stream texture layers, and the `captureStream` self-capture.
//!
//! Run with `cargo test -p canvas-native --target wasm32-unknown-unknown`
//! (the workspace runner supplies web-glue's JS; `wasm-pack test` cannot).

#![cfg(target_arch = "wasm32")]

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use canvas_core::{
    draw, Canvas, CanvasProps, Color, FillRule, GradientStop, ImageSource, Paint, Path, Rect,
    Stroke, TextureLayer,
};
use runtime_shared::{Length, StyleRules, Tokenized};
use runtime_vocabulary::glue::IntoElement;
use runtime_world::signal;
use wasm_bindgen_test::*;
use web_glue::dom::{window, Element, HtmlCanvasElement};
use web_glue::js::Promise;
use web_glue::{JsCast, JsFuture, JsValue};

wasm_bindgen_test_configure!(run_in_browser);

async fn sleep(ms: i32) {
    let promise = Promise::new(&mut |resolve, _| {
        window()
            .unwrap()
            .set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms)
            .unwrap();
    });
    JsFuture::from(JsValue::from(promise)).await.unwrap();
}

fn fresh_host() -> Element {
    backend_web::newcore::stop();
    let doc = window().unwrap().document().unwrap();
    if let Some(old) = doc.get_element_by_id("app") {
        old.remove();
    }
    let host = doc.create_element("div").unwrap();
    host.set_id("app");
    doc.body().unwrap().append_child(&host).unwrap();
    host
}

fn sized(w: f32, h: f32) -> StyleRules {
    let mut s = StyleRules::default();
    s.width = Some(Tokenized::Literal(Length::Px(w)));
    s.height = Some(Tokenized::Literal(Length::Px(h)));
    s
}

fn mounted_canvas(host: &Element) -> HtmlCanvasElement {
    host.query_selector("canvas[data-external-kind='canvas_core::CanvasProps']")
        .unwrap()
        .expect("the <canvas> is mounted")
        .dyn_into()
        .unwrap()
}

/// RGBA of the device pixel under LOGICAL `(x, y)`.
fn pixel(canvas: &HtmlCanvasElement, x: f64, y: f64) -> [u8; 4] {
    let dpr = window().unwrap().device_pixel_ratio();
    let ctx = canvas.get_context("2d").unwrap().expect("2d context");
    let args = [
        &JsValue::from_f64((x * dpr).floor()),
        &JsValue::from_f64((y * dpr).floor()),
        &JsValue::from_f64(1.0),
        &JsValue::from_f64(1.0),
    ];
    let data = ctx.call_method("getImageData", &args).unwrap().get("data").unwrap();
    let at = |i: u32| web_glue::js::Reflect::get_u32(&data, i).unwrap().as_f64().unwrap() as u8;
    [at(0), at(1), at(2), at(3)]
}

const RED: Color = Color::new(255, 0, 0, 255);
const GREEN: Color = Color::new(0, 255, 0, 255);
const YELLOW: Color = Color::new(255, 255, 0, 255);
const MAGENTA: Color = Color::new(255, 0, 255, 255);
const CYAN: Color = Color::new(0, 255, 255, 255);

/// Every op family the replay binds, each in its own cell of a 100×120
/// canvas, read back pixel by pixel: solid fill, a gradient fill, an RGBA
/// image blit, an even-odd fill (the hole stays clear), a dashed stroke,
/// a persistent layer, a transform, and a clip.
#[wasm_bindgen_test]
async fn scene_ops_paint_the_expected_pixels() {
    let host = fresh_host();
    backend_web::newcore::start_in("#app", canvas_native::register, || {
        Canvas(CanvasProps {
            draw: draw(|s| {
                s.fill_path(Path::rect(0.0, 0.0, 50.0, 50.0), RED);
                s.fill_path(
                    Path::rect(50.0, 0.0, 50.0, 50.0),
                    Paint::linear(
                        50.0,
                        0.0,
                        100.0,
                        0.0,
                        vec![GradientStop::new(0.0, GREEN), GradientStop::new(1.0, GREEN)],
                    ),
                );
                // 2×2 opaque blue, scaled up into the bottom-left cell.
                let blue = ImageSource::from_rgba8(7, 2, 2, [0u8, 0, 255, 255].repeat(4));
                s.draw_image(Arc::new(blue), Rect::new(0.0, 50.0, 50.0, 50.0));
                // Even-odd: an outer square with an inner square hole.
                s.path()
                    .add_path(Path::rect(50.0, 50.0, 50.0, 50.0))
                    .add_path(Path::rect(65.0, 65.0, 20.0, 20.0));
                s.fill_rule(YELLOW, FillRule::EvenOdd);
                // A dashed stroke across the top edge (exercises setLineDash
                // and its reset; the pixels it covers are already red/green).
                s.stroke_path(
                    Path::new().move_to(0.0, 1.0).line_to(100.0, 1.0),
                    RED,
                    Stroke::width(1.0).dash(vec![4.0, 2.0], 0.0),
                );
                // A persistent layer holding magenta at the bottom-left of the
                // last row, then cyan through a transform, clipped to its cell.
                s.layer(1, true, |l| {
                    l.fill_path(Path::rect(0.0, 100.0, 50.0, 20.0), MAGENTA);
                });
                s.save();
                s.push_op(canvas_core::DrawOp::Clip {
                    path: Path::rect(50.0, 100.0, 50.0, 20.0),
                    fill_rule: FillRule::NonZero,
                });
                s.translate(50.0, 0.0);
                s.fill_path(Path::rect(0.0, 100.0, 80.0, 20.0), CYAN);
                s.restore();
            }),
            ..Default::default()
        })
        .with_style(sized(100.0, 120.0))
        .into_element()
    });
    sleep(80).await;

    let canvas = mounted_canvas(&host);
    let px = |x, y| pixel(&canvas, x, y);
    assert_eq!(px(25.0, 25.0), [255, 0, 0, 255], "solid fill");
    assert_eq!(px(75.0, 25.0), [0, 255, 0, 255], "gradient fill");
    assert_eq!(px(25.0, 75.0), [0, 0, 255, 255], "RGBA image blit");
    assert_eq!(px(55.0, 55.0), [255, 255, 0, 255], "even-odd ring");
    assert_eq!(px(75.0, 75.0), [0, 0, 0, 0], "even-odd hole stays clear");
    assert_eq!(px(25.0, 110.0), [255, 0, 255, 255], "persistent layer composited");
    assert_eq!(px(75.0, 110.0), [0, 255, 255, 255], "translated fill");
    backend_web::newcore::stop();
}

/// Texture layers: a static image layer and a live stream layer (another
/// canvas's `captureStream`, published as a `web_sys::MediaStream` native
/// source) both composite over the scene; and a `capture` sink receives the
/// canvas's own `captureStream` as a `web_sys::MediaStream` native source.
#[wasm_bindgen_test]
async fn texture_layers_and_self_capture() {
    let host = fresh_host();
    let doc = window().unwrap().document().unwrap();

    // The live source: a small orange canvas, captured.
    let src: HtmlCanvasElement = doc.create_element("canvas").unwrap().dyn_into().unwrap();
    src.set_width(16);
    src.set_height(16);
    let sctx = src.get_context("2d").unwrap().unwrap();
    sctx.set("fillStyle", &JsValue::from_str("rgb(255,128,0)")).unwrap();
    sctx.call_method(
        "fillRect",
        &[&JsValue::from_f64(0.0), &JsValue::from_f64(0.0), &JsValue::from_f64(16.0), &JsValue::from_f64(16.0)],
    )
    .unwrap();
    let live = src.as_js().call_method("captureStream", &[&JsValue::from_f64(30.0)]).unwrap();
    let native: web_sys::MediaStream =
        wasm_bindgen::JsCast::unchecked_into(web_glue::bridge::to_bindgen(&live));
    let (layer_stream, _layer_writer) = media_stream::MediaStream::new();
    layer_stream.set_native_source(Rc::new(native));

    let (captured, writer) = media_stream::MediaStream::with_surface_capture();
    let bump = Rc::new(Cell::new(None));
    let bump_app = bump.clone();
    let writer = Rc::new(Cell::new(Some(writer)));
    backend_web::newcore::start_in("#app", canvas_native::register, move || {
        let version = signal(0u32);
        bump_app.set(Some(version));
        let ls = layer_stream.clone();
        let white = ImageSource::from_rgba8(9, 1, 1, vec![255, 255, 255, 255]);
        let white = Arc::new(white);
        Canvas(CanvasProps {
            draw: draw(move |s| {
                let _ = version.get(); // repaint on bump, so the layer video's frames land
                s.fill_path(Path::rect(0.0, 0.0, 100.0, 50.0), Color::new(0, 0, 0, 255));
            }),
            capture: writer.take(),
            layers: vec![
                TextureLayer::image(Rc::new(move || Some(white.clone())), Rc::new(|| (0.0, 0.0, 40.0, 40.0))),
                TextureLayer::new(Rc::new(move || Some(ls.clone())), Rc::new(|| (50.0, 0.0, 40.0, 40.0))),
            ],
        })
        .with_style(sized(100.0, 50.0))
        .into_element()
    });
    sleep(60).await;
    let canvas = mounted_canvas(&host);
    assert_eq!(pixel(&canvas, 20.0, 20.0), [255, 255, 255, 255], "image layer");

    // The hidden player needs a decoded frame; repaint until it lands.
    let version = bump.get().expect("signal created");
    let mut orange = false;
    for i in 1..=60 {
        version.set(i);
        backend_web::newcore::flush_sync();
        sleep(50).await;
        let [r, g, b, a] = pixel(&canvas, 70.0, 20.0);
        if r > 200 && (100..=160).contains(&g) && b < 60 && a == 255 {
            orange = true;
            break;
        }
    }
    assert!(orange, "stream layer composited: {:?}", pixel(&canvas, 70.0, 20.0));

    // Self-capture: the canvas published its captureStream as the stream's
    // native source, as the web-sys type the media consumers downcast.
    let src = captured.native_source().expect("capture published a native source");
    let ms = src.downcast_ref::<web_sys::MediaStream>().expect("a web_sys::MediaStream");
    assert!(!ms.id().is_empty());
    backend_web::newcore::stop();
}

/// canvas-vello's entry point: `make_2d_rasterizer` takes a
/// `web_sys::HtmlCanvasElement` and must draw into that very element.
#[wasm_bindgen_test]
fn public_rasterizer_draws_into_a_web_sys_canvas() {
    let doc = window().unwrap().document().unwrap();
    let el: HtmlCanvasElement = doc.create_element("canvas").unwrap().dyn_into().unwrap();
    el.set_attribute("style", "width: 20px; height: 20px").unwrap();
    doc.body().unwrap().append_child(&el).unwrap();
    let web_sys_canvas: web_sys::HtmlCanvasElement =
        wasm_bindgen::JsCast::unchecked_into(web_glue::bridge::to_bindgen(&el));
    let props = Rc::new(CanvasProps::default());
    let mut raster = canvas_native::make_2d_rasterizer(web_sys_canvas, &props);
    let mut scene = canvas_core::Scene::new();
    scene.fill_path(Path::rect(0.0, 0.0, 20.0, 20.0), RED);
    raster(&scene);
    assert_eq!(pixel(&el, 10.0, 10.0), [255, 0, 0, 255]);
    el.remove();
}
