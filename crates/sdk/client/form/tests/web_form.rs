//! Browser tests for the web `Form` handler: a real `<form>` wrapping its
//! children, the `submit` listener (preventDefault + `on_submit`), the
//! `FormHandle::submit` op (`requestSubmit`), and the listener's teardown.
//!
//! Run with `cargo test -p form --target wasm32-unknown-unknown` (the
//! workspace runner supplies web-glue's JS; `wasm-pack test` cannot).

#![cfg(target_arch = "wasm32")]

use std::cell::Cell;
use std::rc::Rc;

use form::prelude::*;
use runtime_shared::Ref;
use runtime_vocabulary::builders;
use runtime_vocabulary::glue::IntoElement;
use wasm_bindgen_test::*;
use web_glue::dom::{window, Element, Listener, ListenerOptions};
use web_glue::js::Promise;
use web_glue::{Closure, JsFuture};

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

/// Mount a form with one text child, counting submits; returns the count,
/// the bound handle's ref, and the mounted `<form>`.
async fn mount_counting_form(host: &Element) -> (Rc<Cell<u32>>, Ref<FormHandle>, Element) {
    let submits = Rc::new(Cell::new(0u32));
    let r: Ref<FormHandle> = Ref::new();
    let (submits_app, r_app) = (submits.clone(), r.clone());
    backend_web::newcore::start_in("#app", form::register, move || {
        let submits = submits_app.clone();
        form(FormProps {
            on_submit: Some(Rc::new(move || submits.set(submits.get() + 1))),
            children: vec![builders::text().content("email").build()],
        })
        .bind(r_app.clone())
        .into_element()
    });
    next_frames().await;
    let el = host
        .query_selector("form[data-external-kind='form::FormProps']")
        .unwrap()
        .expect("the <form> is mounted");
    (submits, r, el)
}

#[wasm_bindgen_test]
async fn handle_submit_fires_on_submit_and_prevents_navigation() {
    let host = fresh_host();
    let (submits, r, el) = mount_counting_form(&host).await;
    assert!(
        el.text_content().unwrap_or_default().contains("email"),
        "children realize INTO the <form>"
    );

    // A bubble-phase listener on the document runs after the form's own
    // listener, so it sees whether the default navigation was cancelled.
    let prevented = Rc::new(Cell::new(None));
    let seen = prevented.clone();
    let doc = window().unwrap().document().unwrap();
    let _probe = Listener::new(doc.into(), "submit", ListenerOptions::default(), move |ev| {
        seen.set(Some(ev.default_prevented()))
    });

    r.with(|h| h.submit()).expect("ref filled at mount");
    assert_eq!(submits.get(), 1, "requestSubmit reached on_submit");
    assert_eq!(prevented.get(), Some(true), "the default GET/POST navigation is cancelled");
    backend_web::newcore::stop();
}

/// Regression: the submit listener's closure was parked behind an
/// `Rc::into_raw` number stored on the element and never reclaimed, so
/// every mounted form leaked its listener for the life of the page. It is
/// now owned by the mount and detached + freed at teardown.
#[wasm_bindgen_test]
async fn regression_unmount_detaches_and_frees_the_submit_listener() {
    // Warm-up: the first boot installs the backend's page-lifetime
    // closures (scheduler, viewport, ...); the same form without
    // `on_submit` then fixes the baseline every mount/unmount returns to.
    let host = fresh_host();
    backend_web::newcore::start_in("#app", form::register, || {
        form(FormProps { on_submit: None, children: Vec::new() }).into_element()
    });
    next_frames().await;
    backend_web::newcore::stop();
    next_frames().await;
    let baseline = Closure::live_count();

    let (submits, _r, el) = mount_counting_form(&host).await;
    assert!(Closure::live_count() > baseline, "the submit listener is live while mounted");

    backend_web::newcore::stop();
    next_frames().await;
    assert_eq!(Closure::live_count(), baseline, "teardown freed the submit listener");

    // The detached element no longer reaches the author callback (and a
    // dead closure is not invoked — that would throw).
    let ev = web_glue::dom::Event::new("submit").unwrap();
    el.dispatch_event(&ev).unwrap();
    assert_eq!(submits.get(), 0, "no submit after unmount");
}
