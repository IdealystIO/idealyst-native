//! The web-sys + wasm-bindgen twin of `own-glue-demo`: the same page (title,
//! click counter, caught exception, awaited timer + rejection) and the
//! same `bench_*` workload, so the phase-1 comparison measures the binding
//! layer and nothing else. It has no counterpart to `own-glue-demo`'s
//! `selftest` module, which is why that one is compiled out
//! (`--no-default-features`) when sizes are compared.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use web_sys::{Document, Element};

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(catch, js_name = "eval")]
    fn js_eval(src: &str) -> Result<JsValue, JsValue>;
}

thread_local! {
    static BENCH_HOST: RefCell<Option<Element>> = const { RefCell::new(None) };
    static BENCH_ROWS: RefCell<Vec<Element>> = const { RefCell::new(Vec::new()) };
    /// Callbacks that live as long as the page.
    static KEEP: RefCell<Vec<Closure<dyn FnMut(web_sys::Event)>>> = const { RefCell::new(Vec::new()) };
}

fn document() -> Document {
    web_sys::window().unwrap().document().unwrap()
}

fn child(doc: &Document, parent: &Element, tag: &str, id: &str) -> Element {
    let el = doc.create_element(tag).unwrap();
    el.set_attribute("id", id).unwrap();
    parent.append_child(&el).unwrap();
    el
}

fn main() {
    let doc = document();
    let body: Element = doc.body().unwrap().into();
    let title = child(&doc, &body, "h1", "title");
    title.set_text_content(Some("web-sys demo ✓"));

    let button = child(&doc, &body, "button", "btn");
    button.set_text_content(Some("click me"));
    let count_el = child(&doc, &body, "span", "count");
    count_el.set_text_content(Some("0"));
    let count = Rc::new(Cell::new(0u32));
    let on_click = Closure::<dyn FnMut(web_sys::Event)>::new(move |event: web_sys::Event| {
        count.set(count.get() + 1);
        count_el.set_text_content(Some(&format!("{} ({})", count.get(), event.type_())));
    });
    button
        .add_event_listener_with_callback("click", on_click.as_ref().unchecked_ref())
        .unwrap();
    KEEP.with(|k| k.borrow_mut().push(on_click));

    let exc = child(&doc, &body, "div", "exception");
    match js_eval("throw new RangeError('bad value ✓')") {
        Ok(_) => exc.set_text_content(Some("no exception?")),
        Err(e) => {
            let msg = js_sys::Error::from(e).to_string();
            exc.set_text_content(Some(&format!("caught {}", String::from(msg))));
        }
    }

    let async_el = child(&doc, &body, "div", "async");
    async_el.set_text_content(Some("pending"));
    let status = child(&doc, &body, "div", "status");
    wasm_bindgen_futures::spawn_local(async move {
        let sleep = js_sys::Promise::new(&mut |resolve, _| {
            web_sys::window()
                .unwrap()
                .set_timeout_with_callback_and_timeout_and_arguments_1(&resolve, 30, &JsValue::from(30))
                .unwrap();
        });
        let slept = JsFuture::from(sleep).await.ok().and_then(|v| v.as_f64()).unwrap_or(-1.0);
        let rejection = match JsFuture::from(js_sys::Promise::reject(&JsValue::from_str("nope ✓"))).await {
            Ok(_) => "resolved?".to_string(),
            Err(e) => e.as_string().unwrap_or_default(),
        };
        async_el.set_text_content(Some(&format!("timer {slept}ms; rejection {rejection}")));
        status.set_text_content(Some("ready"));
    });

    BENCH_HOST.with(|h| *h.borrow_mut() = Some(child(&doc, &body, "div", "bench")));
}

#[wasm_bindgen]
pub fn bench_create(n: u32) {
    let doc = document();
    let host = BENCH_HOST.with(|h| h.borrow().clone().unwrap());
    let mut rows = Vec::with_capacity(n as usize);
    for i in 0..n {
        let el = doc.create_element("div").unwrap();
        el.set_attribute("class", "row").unwrap();
        el.set_attribute("data-index", &i.to_string()).unwrap();
        el.set_attribute("title", "benchmark row").unwrap();
        el.set_text_content(Some(&format!("row {i}")));
        host.append_child(&el).unwrap();
        rows.push(el);
    }
    BENCH_ROWS.with(|r| *r.borrow_mut() = rows);
}

#[wasm_bindgen]
pub fn bench_update() {
    BENCH_ROWS.with(|r| {
        for (i, el) in r.borrow().iter().enumerate() {
            el.set_text_content(Some(&format!("updated {i}")));
        }
    });
}

#[wasm_bindgen]
pub fn bench_clear() {
    BENCH_ROWS.with(|r| r.borrow_mut().clear());
    BENCH_HOST.with(|h| h.borrow().as_ref().unwrap().replace_children_with_node_0());
}
