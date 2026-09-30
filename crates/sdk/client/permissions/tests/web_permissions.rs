//! Browser tests for the web permissions backend: `Notification.permission`
//! / `requestPermission()` and `navigator.permissions.query`.
//!
//! The real prompts can't be answered headless, so the APIs are shadowed
//! per test with JS stand-ins; what's under test is the SDK's binding and
//! its status mapping.
//!
//! Run with `cargo test -p permissions --target wasm32-unknown-unknown`
//! (the workspace runner supplies web-glue's JS; `wasm-pack test` cannot).

#![cfg(target_arch = "wasm32")]

use permissions::{request, status, Permission, PermissionStatus};
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

/// Shadow `<object>.<prop>` with a JS expression until the guard drops.
struct Override(&'static str, &'static str);

impl Override {
    fn install(object: &'static str, prop: &'static str, js_expr: &str) -> Override {
        run(&format!(
            "globalThis.__saved_{prop} = Object.getOwnPropertyDescriptor({object}, '{prop}'); \
             Object.defineProperty({object}, '{prop}', {{ value: {js_expr}, configurable: true, writable: true }});"
        ));
        Override(object, prop)
    }
}

impl Drop for Override {
    fn drop(&mut self) {
        let (object, prop) = (self.0, self.1);
        run(&format!(
            "const d = globalThis.__saved_{prop}; \
             if (d) Object.defineProperty({object}, '{prop}', d); else delete {object}.{prop};"
        ));
    }
}

/// Regression: without the Notification API (iOS Safari outside an
/// installed web app, older engines) the web-sys port's static
/// `Notification.permission` getter threw a ReferenceError through the
/// wasm frames. It is `Unsupported`, like the request path always was.
#[wasm_bindgen_test]
async fn regression_missing_notification_api_is_unsupported() {
    let _gone = Override::install("globalThis", "Notification", "undefined");
    assert_eq!(status(Permission::Notifications).await, PermissionStatus::Unsupported);
    assert_eq!(request(Permission::Notifications).await, PermissionStatus::Unsupported);
}

#[wasm_bindgen_test]
async fn notification_status_and_request_map_the_states() {
    let _n = Override::install(
        "globalThis",
        "Notification",
        "{ permission: 'default', requestPermission: () => Promise.resolve('granted') }",
    );
    assert_eq!(status(Permission::Notifications).await, PermissionStatus::Undetermined);
    assert_eq!(request(Permission::Notifications).await, PermissionStatus::Granted);
}

/// Older Safari implements only the callback form of `requestPermission`
/// and returns `undefined`: the request then re-reads the synchronous
/// status instead of failing.
#[wasm_bindgen_test]
async fn callback_only_request_permission_falls_back_to_the_status() {
    let _n = Override::install(
        "globalThis",
        "Notification",
        "{ permission: 'denied', requestPermission: () => undefined }",
    );
    assert_eq!(request(Permission::Notifications).await, PermissionStatus::Denied);
}

#[wasm_bindgen_test]
async fn permissions_query_maps_permission_status_states() {
    let _p = Override::install(
        "navigator",
        "permissions",
        "{ query: (d) => Promise.resolve(Object.create(PermissionStatus.prototype, { \
             state: { value: d.name === 'geolocation' ? 'granted' : 'prompt' } })) }",
    );
    assert_eq!(status(Permission::LocationWhenInUse).await, PermissionStatus::Granted);
    // Location has no explicit web request: `request` reports the status.
    assert_eq!(request(Permission::LocationAlways).await, PermissionStatus::Granted);
    assert_eq!(status(Permission::Camera).await, PermissionStatus::Undetermined);
}

#[wasm_bindgen_test]
async fn permissions_query_failures_are_unsupported() {
    {
        let _p = Override::install("navigator", "permissions", "undefined");
        assert_eq!(status(Permission::Microphone).await, PermissionStatus::Unsupported);
    }
    {
        // Firefox throws a TypeError for a descriptor it doesn't know.
        let _p = Override::install(
            "navigator",
            "permissions",
            "{ query: () => { throw new TypeError('unknown descriptor'); } }",
        );
        assert_eq!(status(Permission::Camera).await, PermissionStatus::Unsupported);
    }
    {
        let _p = Override::install(
            "navigator",
            "permissions",
            "{ query: () => Promise.reject(new TypeError('nope')) }",
        );
        assert_eq!(status(Permission::Camera).await, PermissionStatus::Unsupported);
    }
    {
        // Resolved with something that isn't a PermissionStatus.
        let _p = Override::install(
            "navigator",
            "permissions",
            "{ query: () => Promise.resolve({ state: 'granted' }) }",
        );
        assert_eq!(status(Permission::Camera).await, PermissionStatus::Unsupported);
    }
}
