//! Browser tests for the web `Svg` handler: the wrapper `<div>` whose
//! `innerHTML` is the markup, the reactive markup effect, and the
//! `intrinsic_size` op reading the mounted `<svg>` through the host node.
//!
//! Run with `cargo test -p svg --target wasm32-unknown-unknown` (the
//! workspace runner supplies web-glue's JS; `wasm-pack test` cannot).

#![cfg(target_arch = "wasm32")]

use std::cell::Cell;
use std::rc::Rc;

use runtime_shared::Ref;
use runtime_vocabulary::glue::IntoElement;
use runtime_world::signal;
use svg::prelude::*;
use wasm_bindgen_test::*;
use web_glue::dom::{window, Element};
use web_glue::js::Promise;
use web_glue::JsFuture;

wasm_bindgen_test_configure!(run_in_browser);

async fn next_frames() {
    // Two timer turns: the mount's microtask flush, then layout.
    for _ in 0..2 {
        let promise = Promise::new(&mut |resolve, _| {
            window()
                .unwrap()
                .set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, 30)
                .unwrap();
        });
        JsFuture::from(web_glue::JsValue::from(promise)).await.unwrap();
    }
}

fn fresh_host() -> Element {
    let doc = window().unwrap().document().unwrap();
    backend_web::newcore::stop();
    if let Some(old) = doc.get_element_by_id("app") {
        old.remove();
    }
    let host = doc.create_element("div").unwrap();
    host.set_id("app");
    doc.body().unwrap().append_child(&host).unwrap();
    host
}

const SQUARE: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 48 32"><rect width="48" height="32" fill="red"/></svg>"#;
const WIDE: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="120px" height="20px"><rect width="120" height="20"/></svg>"#;

/// The mounted wrapper carries the markup and the external-kind marker,
/// and the bound handle's `intrinsic_size` reads the `<svg>` behind the
/// host node. The host node is a `web_glue::dom::Node` (own-web-bindings
/// phase 2b); an op that downcast it to any other type would get `None`
/// and silently report no size.
#[wasm_bindgen_test]
async fn mounts_markup_and_reads_intrinsic_size_through_the_host_node() {
    let host = fresh_host();
    let r: Ref<SvgHandle> = Ref::new();
    let r_app = r.clone();
    backend_web::newcore::start_in("#app", svg::register, move || {
        Svg(SvgProps { markup: markup(SQUARE), ..Default::default() })
            .bind(r_app.clone())
            .into_element()
    });
    next_frames().await;

    let wrapper = host
        .query_selector("[data-external-kind='svg::SvgProps']")
        .unwrap()
        .expect("the svg wrapper is mounted");
    assert!(wrapper.query_selector("svg rect").unwrap().is_some(), "markup became DOM");
    let size = r.with(|h| h.intrinsic_size()).expect("ref filled at mount");
    assert_eq!(size, Some((48.0, 32.0)), "viewBox extents");
    backend_web::newcore::stop();
}

/// Reactive markup: a new string replaces the DOM and fires `on_load`; an
/// identical string is skipped (re-setting `innerHTML` would restart
/// the SVG's animations), and width/height attributes are the fallback
/// intrinsic size.
#[wasm_bindgen_test]
async fn reactive_markup_updates_and_skips_identical_markup() {
    let host = fresh_host();
    let loads = Rc::new(Cell::new(0u32));
    let r: Ref<SvgHandle> = Ref::new();
    let src = Rc::new(Cell::new(None));
    let (loads_app, r_app, src_app) = (loads.clone(), r.clone(), src.clone());
    backend_web::newcore::start_in("#app", svg::register, move || {
        let m = signal(SQUARE.to_string());
        src_app.set(Some(m));
        let loads = loads_app.clone();
        Svg(SvgProps {
            markup: markup(move || m.get()),
            on_load: Some(Rc::new(move || loads.set(loads.get() + 1))),
            ..Default::default()
        })
        .bind(r_app.clone())
        .into_element()
    });
    next_frames().await;
    assert_eq!(loads.get(), 1, "first render fires on_load once");
    let first_svg = host.query_selector("svg").unwrap().expect("svg");

    let m = src.get().expect("signal created");
    m.set(SQUARE.to_string());
    backend_web::newcore::flush_sync();
    next_frames().await;
    assert_eq!(loads.get(), 1, "identical markup is not re-applied");
    let same_svg = host.query_selector("svg").unwrap().expect("svg");
    assert!(same_svg.is_same_node(Some(&first_svg)), "the DOM was not rebuilt");

    m.set(WIDE.to_string());
    backend_web::newcore::flush_sync();
    next_frames().await;
    assert_eq!(loads.get(), 2, "new markup fires on_load again");
    assert_eq!(
        r.with(|h| h.intrinsic_size()).flatten(),
        Some((120.0, 20.0)),
        "width/height attributes (units stripped) when there is no viewBox"
    );
    backend_web::newcore::stop();
}
