//! Browser tests for the web notifications backend (`new Notification`).
//!
//! A headless page can't be granted the notification permission, so the
//! `Notification` constructor is shadowed per test with a JS stand-in that
//! records its arguments; what's under test is the SDK's binding.
//!
//! Run with `cargo test -p notifications --target wasm32-unknown-unknown`
//! (the workspace runner supplies web-glue's JS; `wasm-pack test` cannot).

#![cfg(target_arch = "wasm32")]

use std::time::Duration;

use notifications::{notify, push_token, schedule, Notification, NotificationId, NotifyError};
use wasm_bindgen_test::*;
use web_glue::JsValue;

wasm_bindgen_test_configure!(run_in_browser);

fn run(body: &str) {
    let f = JsValue::global()
        .get("Function")
        .unwrap()
        .construct(&[&JsValue::from_str(body)])
        .unwrap();
    f.call(&JsValue::undefined(), &[]).unwrap();
}

/// Replace `globalThis.Notification` with a JS expression until the guard
/// drops.
struct NotificationOverride;

impl NotificationOverride {
    fn install(js_expr: &str) -> NotificationOverride {
        run(&format!(
            "globalThis.__savedNotification = globalThis.Notification; globalThis.Notification = {js_expr};"
        ));
        NotificationOverride
    }
}

impl Drop for NotificationOverride {
    fn drop(&mut self) {
        run("globalThis.Notification = globalThis.__savedNotification; delete globalThis.__posted;");
    }
}

#[wasm_bindgen_test]
async fn notify_posts_title_body_and_the_id_as_tag() {
    let _stub = NotificationOverride::install(
        "class { constructor(t, o) { globalThis.__posted = JSON.stringify([t, o]); } }",
    );
    let id = notify(Notification::new("Tïtle", "body").subtitle("sub").id("order-7"))
        .await
        .unwrap();
    assert_eq!(id, NotificationId::from("order-7"));
    let posted = JsValue::global().get("__posted").unwrap().as_string().unwrap();
    // The subtitle is folded onto the body (web has no subtitle field).
    assert_eq!(posted, r#"["Tïtle",{"body":"sub\nbody","tag":"order-7"}]"#);
}

#[wasm_bindgen_test]
async fn a_throwing_constructor_is_a_backend_error() {
    let _stub = NotificationOverride::install(
        "class { constructor() { throw new TypeError('Illegal constructor'); } }",
    );
    match notify(Notification::new("t", "b")).await {
        Err(NotifyError::Backend(msg)) => assert!(msg.contains("Illegal constructor"), "{msg}"),
        other => panic!("expected a Backend error, got {other:?}"),
    }
}

#[wasm_bindgen_test]
async fn no_notification_api_is_a_backend_error() {
    let _gone = NotificationOverride::install("undefined");
    assert!(matches!(notify(Notification::new("t", "b")).await, Err(NotifyError::Backend(_))));
}

#[wasm_bindgen_test]
async fn schedule_and_push_token_are_not_supported_on_web() {
    assert_eq!(
        schedule(Notification::new("t", "b"), Duration::from_secs(1)).await,
        Err(NotifyError::NotSupported)
    );
    assert!(matches!(push_token().await, Err(NotifyError::NotSupported)));
}
