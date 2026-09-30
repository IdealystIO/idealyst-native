//! Browser test for the web `MapView` handler: the OpenStreetMap embed
//! `<iframe>` built by the `maps-web` leaf is mounted as the host node,
//! with the author style on it.
//!
//! Run with `cargo test -p maps --target wasm32-unknown-unknown` (the
//! workspace runner supplies web-glue's JS; `wasm-pack test` cannot).

#![cfg(target_arch = "wasm32")]

use maps::{MapView, MapViewProps};
use runtime_shared::{StyleRules, Tokenized};
use runtime_vocabulary::glue::IntoElement;
use wasm_bindgen_test::*;
use web_glue::dom::window;
use web_glue::js::Promise;
use web_glue::JsFuture;

wasm_bindgen_test_configure!(run_in_browser);

async fn next_frames() {
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

/// Also the regression test for the author's size being ignored on web: the
/// leaf pinned `width: 100%; height: 100%` in the iframe's inline style,
/// which beats the style classes `with_style` produces.
#[wasm_bindgen_test]
async fn regression_author_size_reaches_the_map_iframe() {
    let doc = window().unwrap().document().unwrap();
    if let Some(old) = doc.get_element_by_id("app") {
        old.remove();
    }
    let host = doc.create_element("div").unwrap();
    host.set_id("app");
    doc.body().unwrap().append_child(&host).unwrap();

    backend_web::newcore::start_in("#app", maps::register, || {
        let mut style = StyleRules::default();
        style.width = Some(Tokenized::Literal(runtime_shared::Length::Px(320.0)));
        style.height = Some(Tokenized::Literal(runtime_shared::Length::Px(180.0)));
        MapView(MapViewProps { lat: 37.7749, lon: -122.4194, zoom: 12.0 })
            .with_style(style)
            .into_element()
    });
    next_frames().await;

    let iframe = host
        .query_selector("iframe[data-external-kind='maps_core::MapViewProps']")
        .unwrap()
        .expect("the map iframe is mounted under the app root");
    let src = iframe.get_attribute("src").unwrap_or_default();
    assert!(
        src.starts_with("https://www.openstreetmap.org/export/embed.html?bbox="),
        "{src}"
    );
    assert!(src.ends_with("&marker=37.7749,-122.4194"), "{src}");
    assert_eq!(iframe.get_attribute("loading").as_deref(), Some("lazy"));
    // The author style reached the element the handler returned.
    let rect = iframe.get_bounding_client_rect();
    assert_eq!(rect.width(), 320.0, "author width applies to the iframe");
    assert_eq!(rect.height(), 180.0, "author height applies to the iframe");
    backend_web::newcore::stop();
}
