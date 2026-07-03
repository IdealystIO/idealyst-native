//! Web biometric auth via **WebAuthn** — `navigator.credentials.get` with
//! `userVerification: "required"`.
//!
//! There is no local "is the device owner present" API in a browser. The
//! only biometric path is WebAuthn, where the platform authenticator signs
//! a **server-issued challenge** with a passkey and the resulting assertion
//! is verified by a **relying-party server**. This backend therefore:
//!
//! - requires [`AuthRequest::web_authn`] (the challenge + rp parameters);
//!   without it, [`authenticate`](WebAuthn::authenticate) returns
//!   [`BioError::Unsupported`] explaining what's missing, and
//! - returns the raw [`WebAuthnAssertion`] in
//!   [`Authentication::assertion`] for the caller to POST to its server.
//!   This crate cannot verify the signature locally and does not pretend to.
//!
//! The options dictionary is built with `js_sys::Reflect` rather than a
//! typed web-sys builder — fewer optional web-sys features, and the shape
//! maps 1:1 to the WebAuthn `PublicKeyCredentialRequestOptions` spec.

use js_sys::{Array, Object, Reflect, Uint8Array};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    AuthenticatorAssertionResponse, CredentialRequestOptions, PublicKeyCredential,
};

use crate::{
    AuthFuture, AuthRequest, Authentication, BioError, Biometry, BiometricAuthenticator,
    WebAuthnAssertion, WebAuthnRequest,
};

/// Guidance returned when a web `authenticate` call arrives without a
/// WebAuthn challenge — the one thing the browser path can't synthesize.
const NO_CHALLENGE: &str = "web biometric authentication uses WebAuthn, which needs a \
    server-issued challenge. Attach `AuthRequest::web_authn(WebAuthnRequest { challenge, .. })` \
    sourced from your relying-party server, then verify the returned assertion server-side — \
    that verification is the authentication.";

/// Biometric auth over WebAuthn (`navigator.credentials.get`).
#[derive(Default)]
pub struct WebAuthn {
    _private: (),
}

impl WebAuthn {
    /// Create a WebAuthn-backed authenticator.
    pub fn new() -> Self {
        Self::default()
    }
}

impl BiometricAuthenticator for WebAuthn {
    fn availability(&self) -> Biometry {
        // Coarse by necessity: the precise probe
        // (`isUserVerifyingPlatformAuthenticatorAvailable`) is async, but
        // this query is sync. Report `Unknown` (a usable authenticator may
        // exist; modality is never exposed on the web) when the WebAuthn API
        // is present, `None` when the browser lacks it entirely.
        if webauthn_supported() {
            Biometry::Unknown
        } else {
            Biometry::None
        }
    }

    fn authenticate(&self, request: AuthRequest) -> AuthFuture {
        Box::pin(async move {
            let Some(web) = request.web_authn else {
                return Err(BioError::Unsupported(NO_CHALLENGE.into()));
            };
            run_ceremony(web).await
        })
    }
}

fn webauthn_supported() -> bool {
    web_sys::window()
        .map(|w| {
            Reflect::has(&w, &JsValue::from_str("PublicKeyCredential")).unwrap_or(false)
        })
        .unwrap_or(false)
}

/// Build the `PublicKeyCredentialRequestOptions`, run
/// `navigator.credentials.get`, and unpack the assertion.
async fn run_ceremony(req: WebAuthnRequest) -> Result<Authentication, BioError> {
    let window = web_sys::window().ok_or_else(|| BioError::Backend("no window".into()))?;
    let credentials = window.navigator().credentials();

    let public_key = build_request_options(&req).map_err(js_to_backend)?;
    let options = Object::new();
    Reflect::set(&options, &"publicKey".into(), &public_key).map_err(js_to_backend)?;
    let options: CredentialRequestOptions = options.unchecked_into();

    let promise = credentials
        .get_with_options(&options)
        .map_err(js_to_backend)?;
    let credential = JsFuture::from(promise).await.map_err(map_get_error)?;

    let pkc: PublicKeyCredential = credential
        .dyn_into()
        .map_err(|_| BioError::Backend("credential was not a PublicKeyCredential".into()))?;
    let response: AuthenticatorAssertionResponse = pkc
        .response()
        .dyn_into()
        .map_err(|_| BioError::Backend("response was not an assertion".into()))?;

    let assertion = WebAuthnAssertion {
        credential_id: buffer_to_vec(&pkc.raw_id()),
        authenticator_data: buffer_to_vec(&response.authenticator_data()),
        client_data_json: buffer_to_vec(&response.client_data_json()),
        signature: buffer_to_vec(&response.signature()),
        user_handle: response.user_handle().map(|b| buffer_to_vec(&b)),
    };

    Ok(Authentication {
        assertion: Some(assertion),
    })
}

/// Construct the WebAuthn request options object (the `publicKey` member).
fn build_request_options(req: &WebAuthnRequest) -> Result<Object, JsValue> {
    let pk = Object::new();
    Reflect::set(
        &pk,
        &"challenge".into(),
        Uint8Array::from(req.challenge.as_slice()).as_ref(),
    )?;
    Reflect::set(&pk, &"userVerification".into(), &"required".into())?;

    if let Some(rp_id) = &req.rp_id {
        Reflect::set(&pk, &"rpId".into(), &JsValue::from_str(rp_id))?;
    }
    if let Some(timeout) = req.timeout_ms {
        Reflect::set(&pk, &"timeout".into(), &JsValue::from_f64(timeout as f64))?;
    }
    if !req.allow_credentials.is_empty() {
        let list = Array::new();
        for id in &req.allow_credentials {
            let desc = Object::new();
            Reflect::set(&desc, &"type".into(), &"public-key".into())?;
            Reflect::set(&desc, &"id".into(), Uint8Array::from(id.as_slice()).as_ref())?;
            list.push(&desc);
        }
        Reflect::set(&pk, &"allowCredentials".into(), &list)?;
    }
    Ok(pk)
}

/// Copy an `ArrayBuffer` (as returned by WebAuthn fields) into a `Vec<u8>`.
fn buffer_to_vec(buffer: &js_sys::ArrayBuffer) -> Vec<u8> {
    Uint8Array::new(buffer).to_vec()
}

/// Map a rejected `navigator.credentials.get` promise to a typed error. A
/// `NotAllowedError`/`AbortError` DOMException is the browser's signal for a
/// user cancellation or ceremony timeout.
fn map_get_error(err: JsValue) -> BioError {
    let name = Reflect::get(&err, &JsValue::from_str("name"))
        .ok()
        .and_then(|v| v.as_string())
        .unwrap_or_default();
    match name.as_str() {
        "NotAllowedError" | "AbortError" => BioError::Cancelled,
        _ => BioError::Backend(format!("WebAuthn get() failed: {}", describe(&err))),
    }
}

fn js_to_backend(err: JsValue) -> BioError {
    BioError::Backend(describe(&err))
}

/// Best-effort human description of a JS error value.
fn describe(err: &JsValue) -> String {
    Reflect::get(err, &JsValue::from_str("message"))
        .ok()
        .and_then(|v| v.as_string())
        .or_else(|| err.as_string())
        .unwrap_or_else(|| "unknown JS error".into())
}
