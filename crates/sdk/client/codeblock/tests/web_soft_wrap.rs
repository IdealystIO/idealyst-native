//! Browser test: `code_editor(..).soft_wrap(true)` breaks lines at the
//! same places in both layers.
//!
//! The host-mock suite (`tests/scene.rs`) pins that the handler TELLS
//! both layers to wrap under the same rules; only a real layout engine
//! can show that they then agree. The measurement: the decorated `<pre>`
//! is in flow and sizes the box, the editing `<textarea>` is stretched
//! over it. If the two wrap identically, the textarea's own content
//! height (`scrollHeight`) is exactly the box — it neither overflows
//! (it wrapped into MORE rows than the `<pre>`) nor leaves rows empty
//! (fewer). A border on the editing layer alone, or a `<pre>` still at
//! `white-space: pre`, breaks that.
//!
//! Run with `wasm-pack test --headless --chrome --package codeblock`.

#![cfg(target_arch = "wasm32")]

use codeblock::code_editor;
use runtime_vocabulary::glue::IntoElement;
use runtime_world::signal;
use web_sys::wasm_bindgen::JsCast;
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

/// Long enough to wrap many times in a 220px column, with long words
/// (break-word territory) and spaces (ordinary soft breaks) both.
const LONG: &str = "IF {quantity_delivered} > 100 AND {site_code} = \"NORTH-DRIFT-100L\" \
     THEN {quantity_delivered} * 0.9 ELSE {averylongcolumnreferencewithnobreakopportunityatall} END";

async fn next_frames() {
    // Two timer turns: the mount's microtask flush, then layout.
    for _ in 0..2 {
        let promise = js_sys::Promise::new(&mut |resolve, _| {
            web_sys::window()
                .unwrap()
                .set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, 30)
                .unwrap();
        });
        wasm_bindgen_futures::JsFuture::from(promise).await.unwrap();
    }
}

#[wasm_bindgen_test]
async fn soft_wrap_layers_break_at_the_same_places() {
    let doc = web_sys::window().unwrap().document().unwrap();
    if let Some(old) = doc.get_element_by_id("app") {
        old.remove();
    }
    let host = doc.create_element("div").unwrap();
    host.set_id("app");
    host.set_attribute("style", "width: 220px").unwrap();
    doc.body().unwrap().append_child(&host).unwrap();

    backend_web::newcore::start_in("#app", codeblock::register, || {
        let src = signal(String::from(LONG));
        code_editor(src, move |next| src.set(next))
            .soft_wrap(true)
            .into_element()
    });
    next_frames().await;

    let ta: web_sys::HtmlTextAreaElement = doc
        .query_selector("#app textarea")
        .unwrap()
        .expect("the editing layer")
        .dyn_into()
        .unwrap();
    let pre: web_sys::HtmlElement = doc
        .query_selector("#app pre")
        .unwrap()
        .expect("the decorated layer")
        .dyn_into()
        .unwrap();
    let win = web_sys::window().unwrap();
    let css = |el: &web_sys::Element, prop: &str| {
        win.get_computed_style(el)
            .unwrap()
            .unwrap()
            .get_property_value(prop)
            .unwrap()
    };

    // Same wrapping rules on both layers.
    for prop in ["white-space", "overflow-wrap", "word-break", "font-family", "font-size", "line-height", "padding-left", "padding-right"] {
        assert_eq!(css(&ta, prop), css(&pre, prop), "layers disagree on `{prop}`");
    }
    assert_eq!(css(&pre, "white-space"), "pre-wrap");
    for side in ["top", "right", "bottom", "left"] {
        assert_eq!(css(&ta, &format!("border-{side}-width")), "0px", "editing layer border-{side}");
    }

    // It really wrapped: many rows, not one.
    let line_height: f64 = css(&pre, "line-height").trim_end_matches("px").parse().unwrap();
    let pre_rect = pre.get_bounding_client_rect();
    assert!(
        pre_rect.height() > 4.0 * line_height,
        "the decorated layer must wrap in a 220px column (height {})",
        pre_rect.height()
    );

    // Same box…
    let ta_rect = ta.get_bounding_client_rect();
    assert!((ta_rect.width() - pre_rect.width()).abs() < 0.5, "widths {} vs {}", ta_rect.width(), pre_rect.width());
    assert!((ta_rect.height() - pre_rect.height()).abs() < 0.5, "heights {} vs {}", ta_rect.height(), pre_rect.height());
    // …and the same rows: the textarea's wrapped content fills that box
    // exactly — more rows would overflow it, fewer would leave it short
    // by at least one line.
    let content = ta.scroll_height() as f64;
    let box_h = ta.client_height() as f64;
    assert!(
        (content - box_h).abs() < line_height / 2.0,
        "editing layer wrapped to {content}px of content in a {box_h}px box — its rows differ from the decorated layer's"
    );
    assert_eq!(ta.scroll_width(), ta.client_width(), "the editing layer must not scroll sideways");
}
