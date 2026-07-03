//! Regression tests for the native (reqwest) transport.
//!
//! Each test is named after the behaviour being pinned, not the function
//! exercising it — per CLAUDE.md rule 8. The harness spins up a fresh
//! hyper server per test so they're independent and can run in parallel.

mod common;

use std::time::Duration;

use net::{Client, Error, IntoBody, Json, Method};
use serde::{Deserialize, Serialize};

use common::{serve, serve_with, Canned};

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct Echo {
    name: String,
    n: i32,
}

#[tokio::test]
async fn regression_get_decodes_json_response() {
    let server = serve(Canned::json(r#"{"name":"Alice","n":42}"#)).await;
    let client = Client::new();
    let body: Echo = client
        .get(format!("{}/users/1", server.base_url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        body,
        Echo {
            name: "Alice".into(),
            n: 42
        }
    );
}

#[tokio::test]
async fn regression_json_body_sets_content_type() {
    let server = serve(Canned::ok("{}")).await;
    let client = Client::new();
    let payload = Echo {
        name: "Bob".into(),
        n: 7,
    };
    client
        .post(format!("{}/echo", server.base_url))
        .json(&payload)
        .send()
        .await
        .unwrap();

    let cap = &server.captured()[0];
    assert_eq!(cap.method, "POST");
    let ct = cap
        .headers
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case("content-type"))
        .map(|(_, v)| v.as_str());
    assert_eq!(ct, Some("application/json"));
    let body: Echo = serde_json::from_slice(&cap.body).unwrap();
    assert_eq!(body, payload);
}

#[tokio::test]
async fn regression_explicit_content_type_overrides_body_default() {
    let server = serve(Canned::ok("{}")).await;
    let client = Client::new();
    client
        .post(format!("{}/echo", server.base_url))
        .header("Content-Type", "application/vnd.custom+json")
        .json(&Echo {
            name: "C".into(),
            n: 1,
        })
        .send()
        .await
        .unwrap();

    let cap = &server.captured()[0];
    let ct = cap
        .headers
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case("content-type"))
        .map(|(_, v)| v.as_str());
    assert_eq!(ct, Some("application/vnd.custom+json"));
}

#[tokio::test]
async fn regression_form_body_encodes_urlencoded() {
    let server = serve(Canned::ok("")).await;
    let client = Client::new();

    #[derive(Serialize)]
    struct Login {
        user: String,
        pass: String,
    }

    client
        .post(format!("{}/login", server.base_url))
        .form(&Login {
            user: "alice".into(),
            pass: "p@ss w/ space".into(),
        })
        .send()
        .await
        .unwrap();

    let cap = &server.captured()[0];
    let ct = cap
        .headers
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case("content-type"))
        .map(|(_, v)| v.as_str());
    assert_eq!(ct, Some("application/x-www-form-urlencoded"));
    assert_eq!(
        std::str::from_utf8(&cap.body).unwrap(),
        "user=alice&pass=p%40ss+w%2F+space"
    );
}

#[tokio::test]
async fn regression_query_appends_to_url() {
    let server = serve(Canned::ok("")).await;
    let client = Client::new();

    #[derive(Serialize)]
    struct Filter<'a> {
        q: &'a str,
        page: u32,
    }

    client
        .get(format!("{}/search", server.base_url))
        .query(&Filter {
            q: "hello world",
            page: 2,
        })
        .send()
        .await
        .unwrap();

    let cap = &server.captured()[0];
    assert_eq!(cap.path, "/search");
    assert_eq!(cap.query.as_deref(), Some("q=hello+world&page=2"));
}

#[tokio::test]
async fn regression_error_for_status_maps_4xx_to_status_error() {
    let server = serve(Canned::status(404)).await;
    let client = Client::new();
    let result = client
        .get(format!("{}/missing", server.base_url))
        .send()
        .await
        .unwrap()
        .error_for_status();
    match result {
        Err(Error::Status { code, body }) => {
            assert_eq!(code, 404);
            assert_eq!(body.as_deref(), Some("status 404"));
        }
        other => panic!("expected Error::Status, got {other:?}"),
    }
}

#[tokio::test]
async fn regression_2xx_passes_through_error_for_status() {
    let server = serve(Canned::ok("ok")).await;
    let client = Client::new();
    let resp = client
        .get(format!("{}/", server.base_url))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn regression_base_url_resolves_relative_paths() {
    let server = serve(Canned::ok("ok")).await;
    let client = Client::builder().base_url(&server.base_url).build();
    client.get("/api/v1/ping").send().await.unwrap();
    let cap = &server.captured()[0];
    assert_eq!(cap.path, "/api/v1/ping");
}

#[tokio::test]
async fn regression_absolute_url_ignores_base_url() {
    let server = serve(Canned::ok("ok")).await;
    let client = Client::builder()
        .base_url("https://wrong.example.com")
        .build();
    client.get(&server.base_url).send().await.unwrap();
    assert_eq!(server.captured().len(), 1);
}

#[tokio::test]
async fn regression_default_headers_apply_to_every_request() {
    let server = serve(Canned::ok("ok")).await;
    let client = Client::builder()
        .default_header("Authorization", "Bearer t0k3n")
        .build();
    client
        .get(format!("{}/a", server.base_url))
        .send()
        .await
        .unwrap();
    client
        .get(format!("{}/b", server.base_url))
        .send()
        .await
        .unwrap();
    for cap in server.captured() {
        let auth = cap
            .headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case("authorization"))
            .map(|(_, v)| v.as_str());
        assert_eq!(auth, Some("Bearer t0k3n"), "missing auth on {}", cap.path);
    }
}

#[tokio::test]
async fn regression_response_headers_round_trip() {
    let server = serve(Canned {
        status: 201,
        headers: vec![
            ("x-request-id", "abc-123"),
            ("location", "/created/42"),
        ],
        body: b"created".to_vec(),
    })
    .await;
    let client = Client::new();
    let resp = client
        .post(format!("{}/things", server.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);
    assert_eq!(resp.header("x-request-id"), Some("abc-123"));
    assert_eq!(resp.header("X-Request-Id"), Some("abc-123")); // case-insensitive lookup
    assert_eq!(resp.header("location"), Some("/created/42"));
}

#[tokio::test]
async fn regression_method_dispatch_uses_correct_verb() {
    let server = serve(Canned::ok("")).await;
    let client = Client::new();
    for (m, expected) in [
        (Method::Get, "GET"),
        (Method::Post, "POST"),
        (Method::Put, "PUT"),
        (Method::Patch, "PATCH"),
        (Method::Delete, "DELETE"),
    ] {
        client
            .request(m, format!("{}/", server.base_url))
            .send()
            .await
            .unwrap();
        let last = server.captured().pop().unwrap();
        assert_eq!(last.method, expected);
    }
}

#[tokio::test]
async fn regression_json_wrapper_works_as_into_body_and_from_body() {
    // Exercises the canonical IntoBody/FromBody path that downstream
    // server-fn will use — `.body(Json(...))` and `.body::<Json<T>>()`.
    let server = serve_with(|cap| {
        // Echo the JSON request body back.
        Canned {
            status: 200,
            headers: vec![("content-type", "application/json")],
            body: cap.body.clone(),
        }
    })
    .await;
    let client = Client::new();
    let payload = Echo {
        name: "round-trip".into(),
        n: 99,
    };
    let resp = client
        .post(format!("{}/echo", server.base_url))
        .body(Json(&payload))
        .send()
        .await
        .unwrap();
    let Json(decoded): Json<Echo> = resp.body().await.unwrap();
    assert_eq!(decoded, payload);
}

#[tokio::test]
async fn regression_text_and_bytes_codecs() {
    let server = serve(Canned::ok("hello, world")).await;
    let client = Client::new();
    let txt = client
        .get(format!("{}/", server.base_url))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(txt, "hello, world");

    let server = serve(Canned::ok(vec![1u8, 2, 3, 4, 5])).await;
    let bytes = client
        .get(format!("{}/", server.base_url))
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    assert_eq!(bytes, vec![1, 2, 3, 4, 5]);
}

#[tokio::test]
async fn regression_unit_body_yields_empty_request_body() {
    let server = serve(Canned::ok("")).await;
    let client = Client::new();
    client
        .post(format!("{}/", server.base_url))
        .body(())
        .send()
        .await
        .unwrap();
    let cap = &server.captured()[0];
    assert!(cap.body.is_empty());
    // No Content-Type should have been auto-set for `()`.
    let has_ct = cap
        .headers
        .iter()
        .any(|(n, _)| n.eq_ignore_ascii_case("content-type"));
    assert!(!has_ct, "() body must not synthesise a content-type");
}

#[tokio::test]
async fn regression_relative_url_without_base_errors() {
    let client = Client::new();
    let err = client.get("/no-base").send().await.unwrap_err();
    match err {
        Error::InvalidUrl(_) => {}
        other => panic!("expected InvalidUrl, got {other:?}"),
    }
}

#[tokio::test]
async fn regression_connection_refused_maps_to_network_error() {
    let client = Client::new();
    // Reserved-for-discard port; nothing listens here.
    let err = client
        .get("http://127.0.0.1:1")
        .timeout(Duration::from_millis(500))
        .send()
        .await
        .unwrap_err();
    assert!(
        matches!(err, Error::Network(_) | Error::Timeout),
        "got {err:?}"
    );
}

#[tokio::test]
async fn regression_custom_into_body_impl_drives_full_request() {
    // Proves the public IntoBody trait is enough for a downstream
    // wrapper to plug in a new wire format without touching `net`.
    struct CustomWire(Vec<u8>);
    impl IntoBody for CustomWire {
        fn into_body(self) -> Result<(Vec<u8>, Option<&'static str>), Error> {
            Ok((self.0, Some("application/x-custom-wire")))
        }
    }

    let server = serve(Canned::ok("")).await;
    let client = Client::new();
    client
        .post(format!("{}/wire", server.base_url))
        .body(CustomWire(vec![0xCA, 0xFE, 0xBA, 0xBE]))
        .send()
        .await
        .unwrap();
    let cap = &server.captured()[0];
    assert_eq!(cap.body, vec![0xCA, 0xFE, 0xBA, 0xBE]);
    let ct = cap
        .headers
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case("content-type"))
        .map(|(_, v)| v.as_str());
    assert_eq!(ct, Some("application/x-custom-wire"));
}
