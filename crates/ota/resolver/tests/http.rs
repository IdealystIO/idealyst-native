//! The service over HTTP: the router on a real port, the requests the app's
//! client sends, every status it answers with.

use std::sync::Arc;

use ota_index::Resolution;
use ota_publish::Target;
use ota_resolver::{router, Resolver, Settings};
use remote_bundle::Provides;

/// The router on a free port, on its own runtime; its base URL.
fn serve(target: Target) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    listener.set_nonblocking(true).unwrap();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async move {
            let listener = tokio::net::TcpListener::from_std(listener).unwrap();
            axum::serve(listener, router(Arc::new(Resolver::new(target, Settings::default())))).await.unwrap();
        });
    });
    url
}

#[test]
fn the_protocol_over_http() {
    let dir = tempfile::tempdir().unwrap();
    let url = serve(Target::Dir(dir.path().into()));
    let client = reqwest::blocking::Client::new();
    let post = |body: serde_json::Value| client.post(format!("{url}/v1/resolve")).json(&body).send().unwrap();

    let mut app = Provides { codec: 2, ..Default::default() };
    app.components.insert("ui::Card".into(), Default::default());
    let id = app.id();

    assert_eq!(client.get(format!("{url}/health")).send().unwrap().text().unwrap(), "ok");
    assert_eq!(post(serde_json::json!({ "manifest": id })).status(), 404, "unknown: send the manifest");
    let answer = post(serde_json::json!({ "manifest": id, "provides": app }));
    assert_eq!(answer.status(), 200);
    let answer = Resolution::parse(&answer.bytes().unwrap()).unwrap();
    assert!(answer.answers(&id));
    assert_eq!(post(serde_json::json!({ "manifest": id })).status(), 200, "known now");

    let forged = post(serde_json::json!({ "manifest": "abc", "provides": app }));
    assert_eq!(forged.status(), 400);
    assert!(forged.text().unwrap().contains("its content's is"));

    let huge = "x".repeat(2 << 20);
    assert_eq!(post(serde_json::json!({ "manifest": huge })).status(), 413, "a body past the limit");
}
