//! Browser tests for the web `WebView` handler: the `<iframe>`, its
//! author-callback listeners (message filtered to this frame, load), the
//! `WebViewHandle` ops (`post_message`, `execute_js`), and the listeners'
//! teardown.
//!
//! Run with `cargo test -p webview --target wasm32-unknown-unknown` (the
//! workspace runner supplies web-glue's JS; `wasm-pack test` cannot).

#![cfg(target_arch = "wasm32")]

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use runtime_shared::Ref;
use runtime_vocabulary::glue::IntoElement;
use wasm_bindgen_test::*;
use web_glue::dom::{window, Element};
use web_glue::js::Promise;
use web_glue::{Closure, JsFuture, JsValue};
use webview::prelude::*;

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

fn run(body: &str) {
    let f = JsValue::global()
        .get("Function")
        .unwrap()
        .construct(&[&JsValue::from_str(body)])
        .unwrap();
    f.call(&JsValue::undefined(), &[]).unwrap();
}

struct Mounted {
    loads: Rc<Cell<u32>>,
    messages: Rc<RefCell<Vec<String>>>,
    handle: Ref<WebViewHandle>,
}

async fn mount(url_str: &'static str) -> Mounted {
    let loads = Rc::new(Cell::new(0u32));
    let messages = Rc::new(RefCell::new(Vec::new()));
    let handle: Ref<WebViewHandle> = Ref::new();
    let (l, m, h) = (loads.clone(), messages.clone(), handle.clone());
    backend_web::newcore::start_in("#app", webview::register, move || {
        let (l, m) = (l.clone(), m.clone());
        WebView(WebViewProps {
            url: url(url_str),
            on_load: Some(Rc::new(move || l.set(l.get() + 1))),
            on_message: Some(Rc::new(move |s| m.borrow_mut().push(s))),
            ..Default::default()
        })
        .bind(h.clone())
        .into_element()
    });
    for _ in 0..20 {
        sleep(20).await;
        if loads.get() > 0 {
            break;
        }
    }
    Mounted { loads, messages, handle }
}

#[wasm_bindgen_test]
async fn load_message_and_ops_reach_the_mounted_iframe() {
    let host = fresh_host();
    let m = mount("about:blank").await;
    let iframe = host
        .query_selector("iframe[data-external-kind='webview::WebViewProps']")
        .unwrap()
        .expect("the iframe is mounted");
    assert_eq!(iframe.get_attribute("style").as_deref(), Some("border: 0"));
    assert!(m.loads.get() >= 1, "on_load fired");

    let exec = |code: &str| m.handle.with(|h| h.execute_js(code)).expect("ref filled");
    assert_eq!(exec("1 + 2"), Ok("3".to_string()));
    assert_eq!(exec("undefined"), Ok(String::new()));
    assert_eq!(exec("({ a: [1, 'x'] })"), Ok(r#"{"a":[1,"x"]}"#.to_string()));
    assert_eq!(exec("throw { code: 7 }"), Err(r#"{"code":7}"#.to_string()));
    assert_eq!(exec("(function () {})"), Err("result is not JSON-stringifiable".to_string()));

    // Messages from the frame reach on_message (objects JSON-stringified);
    // one the top window posts to itself is filtered out by source.
    exec("parent.postMessage('hello', '*'); parent.postMessage({ n: 1 }, '*'); 0").unwrap();
    run("window.postMessage('from-top', '*');");
    sleep(50).await;
    assert_eq!(*m.messages.borrow(), vec!["hello".to_string(), r#"{"n":1}"#.to_string()]);

    // post_message reaches the frame's window.
    exec("window.addEventListener('message', (e) => { window.__got = e.data; }); 0").unwrap();
    m.handle.with(|h| h.post_message("ping")).unwrap();
    sleep(50).await;
    assert_eq!(exec("window.__got"), Ok(r#""ping""#.to_string()));
    backend_web::newcore::stop();
}

#[wasm_bindgen_test]
async fn execute_js_on_a_cross_origin_frame_is_an_error() {
    fresh_host();
    // A data: URL document has an opaque origin: reading its `eval` throws.
    let m = mount("data:text/html,<p>x</p>").await;
    assert_eq!(
        m.handle.with(|h| h.execute_js("1")).unwrap(),
        Err("iframe is cross-origin; eval is inaccessible".to_string())
    );
    backend_web::newcore::stop();
}

/// Regression: the listener closures were parked behind an `Rc::into_raw`
/// number stored on the iframe and never reclaimed, so every mounted
/// webview leaked its listeners — including the `message` listener on
/// `window`, which kept running for the rest of the page's life. Teardown
/// now detaches and frees them.
#[wasm_bindgen_test]
async fn regression_unmount_detaches_and_frees_the_listeners() {
    // Warm-up boot: the backend's page-lifetime closures, then a webview
    // without callbacks, fix the baseline.
    fresh_host();
    backend_web::newcore::start_in("#app", webview::register, || {
        WebView(WebViewProps { url: url("about:blank"), ..Default::default() }).into_element()
    });
    sleep(30).await;
    backend_web::newcore::stop();
    sleep(30).await;
    let baseline = Closure::live_count();

    let m = mount("about:blank").await;
    assert!(Closure::live_count() > baseline, "listeners are live while mounted");
    backend_web::newcore::stop();
    sleep(30).await;
    assert_eq!(Closure::live_count(), baseline, "teardown freed every listener");
    run("window.postMessage('after', '*');");
    sleep(30).await;
    assert!(m.messages.borrow().is_empty(), "nothing reaches on_message after unmount");
}
