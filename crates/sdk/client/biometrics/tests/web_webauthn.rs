//! Browser tests for the web biometrics backend (WebAuthn
//! `navigator.credentials.get`).
//!
//! A headless page has no platform authenticator, so
//! `navigator.credentials` is shadowed per test with a JS stand-in that
//! records the options it was handed and resolves with an object built on
//! the real `PublicKeyCredential` / `AuthenticatorAssertionResponse`
//! prototypes. What's under test is the SDK's binding: the options it
//! builds from the request's bytes, the assertion's bytes coming back, and
//! the error mapping.
//!
//! Run with `cargo test -p biometrics --target wasm32-unknown-unknown` (the
//! workspace runner supplies web-glue's JS; `wasm-pack test` cannot).

#![cfg(target_arch = "wasm32")]

use biometrics::{
    AuthRequest, BioError, Biometry, BiometricAuthenticator, WebAuthn, WebAuthnRequest,
};
use wasm_bindgen_test::*;
use web_glue::JsValue;

wasm_bindgen_test_configure!(run_in_browser);

fn eval(body: &str) {
    let f = JsValue::global()
        .get("Function")
        .unwrap()
        .construct(&[&JsValue::from_str(body)])
        .unwrap();
    f.call(&JsValue::undefined(), &[]).unwrap();
}

/// Shadow `navigator.credentials` with a JS expression until the guard
/// drops.
struct CredentialsOverride;

impl CredentialsOverride {
    fn install(js_expr: &str) -> CredentialsOverride {
        eval(&format!(
            "Object.defineProperty(navigator, 'credentials', {{ value: {js_expr}, configurable: true }});"
        ));
        CredentialsOverride
    }
}

impl Drop for CredentialsOverride {
    fn drop(&mut self) {
        eval("delete navigator.credentials; delete globalThis.__opts;");
    }
}

/// A stand-in whose `get` records its options (bytes as arrays) and
/// resolves with an assertion; `user_handle` is a JS expression.
fn resolving_credentials(user_handle: &str) -> String {
    format!(
        "{{ get: (o) => {{ \
            const pk = o.publicKey; \
            globalThis.__opts = JSON.stringify({{ \
              challenge: Array.from(pk.challenge), rpId: pk.rpId, timeout: pk.timeout, \
              userVerification: pk.userVerification, \
              allow: (pk.allowCredentials || []).map(c => [c.type, Array.from(c.id)]) }}); \
            const buf = (a) => new Uint8Array(a).buffer; \
            const response = Object.create(AuthenticatorAssertionResponse.prototype, {{ \
              authenticatorData: {{ value: buf([1, 2, 3]) }}, \
              clientDataJSON: {{ value: buf([123, 125]) }}, \
              signature: {{ value: buf([9, 8, 7, 6]) }}, \
              userHandle: {{ value: {user_handle} }} }}); \
            return Promise.resolve(Object.create(PublicKeyCredential.prototype, {{ \
              rawId: {{ value: buf([0xAA, 0xBB]) }}, response: {{ value: response }} }})); }} }}"
    )
}

fn request() -> AuthRequest {
    AuthRequest::new("Sign in").web_authn(WebAuthnRequest {
        rp_id: Some("localhost".into()),
        challenge: vec![0, 1, 2, 255],
        allow_credentials: vec![vec![5, 6], vec![], vec![7]],
        timeout_ms: Some(60_000),
    })
}

#[wasm_bindgen_test]
async fn the_ceremony_sends_the_request_and_returns_the_assertion_bytes() {
    let _c = CredentialsOverride::install(&resolving_credentials("new Uint8Array([42]).buffer"));
    let auth = WebAuthn::new().authenticate(request()).await.expect("assertion");

    let opts = JsValue::global().get("__opts").unwrap().as_string().unwrap();
    assert_eq!(
        opts,
        r#"{"challenge":[0,1,2,255],"rpId":"localhost","timeout":60000,"userVerification":"required","allow":[["public-key",[5,6]],["public-key",[]],["public-key",[7]]]}"#
    );

    let a = auth.assertion.expect("web returns the assertion");
    assert_eq!(a.credential_id, vec![0xAA, 0xBB]);
    assert_eq!(a.authenticator_data, vec![1, 2, 3]);
    assert_eq!(a.client_data_json, b"{}".to_vec());
    assert_eq!(a.signature, vec![9, 8, 7, 6]);
    assert_eq!(a.user_handle, Some(vec![42]));
}

#[wasm_bindgen_test]
async fn optional_members_are_left_off_and_a_null_user_handle_is_none() {
    let _c = CredentialsOverride::install(&resolving_credentials("null"));
    let req = AuthRequest::new("Sign in").web_authn(WebAuthnRequest {
        challenge: vec![7],
        ..Default::default()
    });
    let auth = WebAuthn::new().authenticate(req).await.expect("assertion");
    let opts = JsValue::global().get("__opts").unwrap().as_string().unwrap();
    assert_eq!(opts, r#"{"challenge":[7],"userVerification":"required","allow":[]}"#);
    assert_eq!(auth.assertion.unwrap().user_handle, None);
}

#[wasm_bindgen_test]
async fn cancellation_and_failures_map_to_typed_errors() {
    {
        let _c = CredentialsOverride::install(
            "{ get: () => Promise.reject(new DOMException('cancelled', 'NotAllowedError')) }",
        );
        assert_eq!(WebAuthn::new().authenticate(request()).await.err(), Some(BioError::Cancelled));
    }
    {
        let _c = CredentialsOverride::install(
            "{ get: () => Promise.reject(new DOMException('bad rp', 'SecurityError')) }",
        );
        assert_eq!(
            WebAuthn::new().authenticate(request()).await.err(),
            Some(BioError::Backend("WebAuthn get() failed: bad rp".into()))
        );
    }
    {
        // Resolved with something that isn't a PublicKeyCredential.
        let _c = CredentialsOverride::install("{ get: () => Promise.resolve({}) }");
        assert!(matches!(
            WebAuthn::new().authenticate(request()).await,
            Err(BioError::Backend(_))
        ));
    }
    {
        // An insecure context has no `navigator.credentials` at all.
        let _c = CredentialsOverride::install("undefined");
        assert!(matches!(
            WebAuthn::new().authenticate(request()).await,
            Err(BioError::Backend(_))
        ));
    }
}

#[wasm_bindgen_test]
async fn without_a_challenge_web_is_unsupported() {
    let err = WebAuthn::new().authenticate(AuthRequest::new("Sign in")).await.err();
    assert!(matches!(err, Some(BioError::Unsupported(_))));
}

#[wasm_bindgen_test]
fn availability_reflects_the_webauthn_api() {
    // Chrome exposes PublicKeyCredential on a secure (localhost) page.
    assert_eq!(WebAuthn::new().availability(), Biometry::Unknown);
}
