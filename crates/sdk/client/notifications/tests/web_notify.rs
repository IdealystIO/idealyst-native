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

use std::cell::RefCell;
use std::rc::Rc;

use notifications::{
    cancel, cancel_all, notify, on_response, push_token, schedule, Notification, NotificationId, NotificationResponse,
    NotifyError, Priority, ResponseAction, Sound,
};
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
        run("globalThis.Notification = globalThis.__savedNotification; delete globalThis.__posted; \
             delete globalThis.__shown; delete globalThis.__log; globalThis.__idealystNotes?.clear();");
    }
}

/// A stand-in `Notification` that records each instance in `__shown`, with
/// a `close()` that marks it closed.
const RECORDING: &str = "class { constructor(t, o) { this.title = t; this.options = o; this.tag = o.tag; \
     this.closed = false; (globalThis.__shown ??= []).push(this); (globalThis.__log ??= []).push('show:' + t); } \
     close() { this.closed = true; (globalThis.__log ??= []).push('close:' + this.title); } }";

/// `options` of the `i`-th shown notification as JSON, with its tag cut
/// back to the id it was posted for (every post gets a fresh `<id>#…`).
fn shown_options(i: usize) -> String {
    eval(&format!(
        "JSON.stringify(((o) => ({{ ...o, tag: o.tag.split('#')[0] }}))(globalThis.__shown[{i}].options))"
    ))
}

/// Evaluate a JS expression in the page, as a string.
fn eval(expr: &str) -> String {
    run(&format!("globalThis.__out = String({expr});"));
    JsValue::global().get("__out").unwrap().as_string().unwrap()
}

#[wasm_bindgen_test]
async fn notify_posts_title_body_under_a_tag_for_the_id() {
    let _stub = NotificationOverride::install(RECORDING);
    let id = notify(Notification::new("Tïtle", "body").subtitle("sub").id("order-7"))
        .await
        .unwrap();
    assert_eq!(id, NotificationId::from("order-7"));
    assert_eq!(eval("globalThis.__shown[0].title"), "Tïtle");
    // The subtitle is folded onto the body (web has no subtitle field).
    assert_eq!(shown_options(0), r#"{"body":"sub\nbody","renotify":true,"tag":"order-7"}"#);
    assert!(eval("globalThis.__shown[0].tag").starts_with("order-7#"));
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

#[wasm_bindgen_test]
async fn shared_fields_map_onto_notification_options() {
    let _stub = NotificationOverride::install(RECORDING);
    notify(
        Notification::new("T", "B")
            .id("opts")
            .sound(Sound::Silent)
            .priority(Priority::Urgent)
            .image("https://example.com/a.png")
            .group("dropped on web")
            .action(notifications::Action::new("reply", "Reply")),
    )
    .await
    .unwrap();
    assert_eq!(
        shown_options(0),
        r#"{"body":"B","image":"https://example.com/a.png","renotify":true,"requireInteraction":true,"silent":true,"tag":"opts"}"#
    );
}

#[wasm_bindgen_test]
async fn the_web_hook_edits_the_options_last() {
    let _stub = NotificationOverride::install(RECORDING);
    let n = Notification::new("T", "B").id("hook").web(|o| {
        o.remove("renotify");
        o.insert("vibrate".into(), runtime_core::__serde_json::json!([100, 50, 100]));
        o.insert("body".into(), runtime_core::__serde_json::json!("from the hook"));
    });
    notify(n).await.unwrap();
    assert_eq!(shown_options(0), r#"{"body":"from the hook","tag":"hook","vibrate":[100,50,100]}"#);
}

/// A click reports `Open` with the notification's payload to the
/// `on_response` handlers, and closes the notification.
#[wasm_bindgen_test]
async fn a_click_reports_open_with_the_payload_and_closes_it() {
    let _stub = NotificationOverride::install(RECORDING);
    let seen: Rc<RefCell<Vec<NotificationResponse>>> = Rc::default();
    let sink = seen.clone();
    let _sub = on_response(move |r| sink.borrow_mut().push(r.clone()));

    notify(Notification::new("Sam", "Lunch?").id("chat-sam").with("chat", "sam")).await.unwrap();
    assert!(seen.borrow().is_empty(), "nothing until the user clicks");
    run("globalThis.__shown[0].onclick();");

    assert_eq!(
        *seen.borrow(),
        vec![NotificationResponse {
            id: NotificationId::from("chat-sam"),
            action: ResponseAction::Open,
            text: None,
            data: [("chat".to_string(), "sam".to_string())].into(),
        }]
    );
    assert_eq!(eval("globalThis.__shown[0].closed"), "true");
}

/// Re-posting an id closes the notification it replaces; `cancel` closes
/// one by id and `cancel_all` the rest. Without the page-wide map these
/// were no-ops on web.
#[wasm_bindgen_test]
async fn regression_cancel_and_replace_close_shown_notifications() {
    let _stub = NotificationOverride::install(RECORDING);
    notify(Notification::new("a", "1").id("a")).await.unwrap();
    notify(Notification::new("a", "2").id("a")).await.unwrap();
    notify(Notification::new("b", "1").id("b")).await.unwrap();
    notify(Notification::new("c", "1").id("c")).await.unwrap();
    assert_eq!(eval("globalThis.__shown.map(n => n.closed).join()"), "true,false,false,false");

    cancel(&NotificationId::from("b")).await;
    assert_eq!(eval("globalThis.__shown.map(n => n.closed).join()"), "true,false,true,false");

    cancel_all().await;
    assert_eq!(eval("globalThis.__shown.map(n => n.closed).join()"), "true,true,true,true");
}

/// The bug: re-posting an id on web did not alert. A same-tag replacement
/// is silent without `renotify`, and on macOS it stays silent even with it
/// once the first notification's banner was swiped into Notification
/// Center (the browser keys the entry by tag); and closing the swiped entry
/// just before posting held back the new banner as well. Every post must
/// arrive under a tag of its own, with `renotify`, closing the one it
/// replaces only after it is shown.
#[wasm_bindgen_test]
async fn regression_a_replacement_alerts_again_like_native() {
    let _stub = NotificationOverride::install(RECORDING);
    notify(Notification::new("Inbox", "1 new message").id("inbox")).await.unwrap();
    notify(Notification::new("Inbox", "2 new messages").id("inbox")).await.unwrap();
    assert_eq!(eval("globalThis.__shown.map(n => n.options.renotify).join()"), "true,true");
    assert_eq!(eval("globalThis.__shown.every(n => n.tag.startsWith('inbox#'))"), "true");
    assert_eq!(eval("globalThis.__shown[0].tag !== globalThis.__shown[1].tag"), "true", "a fresh tag per post");
    assert_eq!(eval("globalThis.__shown.map(n => n.closed).join()"), "true,false");
    assert_eq!(eval("globalThis.__log.join()"), "show:Inbox,show:Inbox,close:Inbox", "closed after the post");
}

/// `alert_once`: while one is shown for the id, the post takes its tag
/// with `renotify` off, so the browser swaps it in without alerting — and
/// it is not closed first, which would make the post a new one.
#[wasm_bindgen_test]
async fn alert_once_replaces_the_shown_one_quietly() {
    let _stub = NotificationOverride::install(RECORDING);
    notify(Notification::new("Upload", "10%").id("upload").alert_once()).await.unwrap();
    notify(Notification::new("Upload", "60%").id("upload").alert_once()).await.unwrap();
    assert_eq!(eval("globalThis.__shown.map(n => n.options.renotify).join()"), "true,false", "the first alerts");
    assert_eq!(eval("globalThis.__shown[0].tag === globalThis.__shown[1].tag"), "true");
    assert_eq!(eval("globalThis.__shown.map(n => n.closed).join()"), "false,false");
}

/// With nothing shown for the id any more (the user clicked it),
/// `alert_once` alerts like any post.
#[wasm_bindgen_test]
async fn alert_once_alerts_when_nothing_is_shown() {
    let _stub = NotificationOverride::install(RECORDING);
    // Claims the click's response, which is otherwise held for the next
    // test's handler.
    let _sub = on_response(|_| {});
    notify(Notification::new("Upload", "10%").id("upload2").alert_once()).await.unwrap();
    run("globalThis.__shown[0].onclick();");
    notify(Notification::new("Upload", "done").id("upload2").alert_once()).await.unwrap();
    assert_eq!(eval("globalThis.__shown[1].options.renotify"), "true");
    assert_eq!(eval("globalThis.__shown[0].tag !== globalThis.__shown[1].tag"), "true");
}
