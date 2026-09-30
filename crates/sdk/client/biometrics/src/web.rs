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
//! Every browser call is a web-glue binding declared here (own-web-bindings
//! phase 3). The options dictionary is built inside the one binding that
//! calls `navigator.credentials.get` — its shape maps 1:1 to the WebAuthn
//! `PublicKeyCredentialRequestOptions` spec — and the assertion's
//! `ArrayBuffer`s are copied out into `Vec<u8>`s (length, then one copy
//! into a buffer Rust allocated first, so no view outlives a wasm call).

use web_glue::{cast, string, JsError, JsFuture, JsValue};

use crate::{
    AuthFuture, AuthRequest, Authentication, BioError, Biometry, BiometricAuthenticator,
    WebAuthnAssertion, WebAuthnRequest,
};

web_glue::import! {
    fn js_webauthn_supported() -> u32 =
        "() => typeof window !== 'undefined' && 'PublicKeyCredential' in window ? 1 : 0";
    fn js_has_window() -> u32 = "() => typeof window === 'undefined' ? 0 : 1";
    // `navigator.credentials.get({ publicKey })` → its Promise.
    //   challenge: `cl` bytes at `cp` (copied — `slice`, never a view);
    //   rpId: set when `has & 1`; timeout: set when `has & 2`;
    //   allowCredentials: `n` ids whose u32 byte lengths are at `lens`,
    //   concatenated at `ids`.
    // Throws (→ Err) where `navigator.credentials` is absent (an insecure
    // context) or the options are rejected synchronously.
    #[catch]
    fn js_credentials_get(
        cp: usize, cl: usize,
        has: u32, rp: usize, rl: usize, timeout: f64,
        ids: usize, lens: usize, n: usize
    ) -> u32 =
        "(cp, cl, has, rp, rl, timeout, ids, lens, n) => { \
           const u8 = G.u8(); const bytes = (p, l) => u8.slice(p >>> 0, (p >>> 0) + (l >>> 0)); \
           const pk = { challenge: bytes(cp, cl), userVerification: 'required' }; \
           if (has & 1) pk.rpId = G.str(rp, rl); \
           if (has & 2) pk.timeout = timeout; \
           if (n > 0) { const w = G.u32(); const lb = (lens >>> 0) >>> 2; let at = ids >>> 0; \
             pk.allowCredentials = []; \
             for (let k = 0; k < (n >>> 0); k++) { const l = w[lb + k]; \
               pk.allowCredentials.push({ type: 'public-key', id: bytes(at, l) }); at += l; } } \
           return G.add(window.navigator.credentials.get({ publicKey: pk })); }";
    // The assertion's buffers: `which` 0 rawId, 1 authenticatorData,
    // 2 clientDataJSON, 3 signature, 4 userHandle (0 when null).
    fn js_assertion_buffer(cred: u32, which: u32) -> u32 =
        "(c, k) => { const v = G.get(c); const r = v.response; \
           const b = k === 0 ? v.rawId : k === 1 ? r.authenticatorData : k === 2 ? r.clientDataJSON \
                   : k === 3 ? r.signature : r.userHandle; \
           return b == null ? 0 : G.add(b); }";
    fn js_response(cred: u32) -> u32 =
        "(c) => { const r = G.get(c).response; return r == null ? 0 : G.add(r); }";
    fn js_byte_length(b: u32) -> u32 = "(b) => G.get(b).byteLength";
    // Copy an ArrayBuffer into `len` bytes Rust already allocated at `p`.
    fn js_copy_bytes(b: u32, p: usize) = "(b, p) => { G.u8().set(new Uint8Array(G.get(b)), p >>> 0); }";
}

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
    unsafe { js_webauthn_supported() != 0 }
}

/// Build the `PublicKeyCredentialRequestOptions`, run
/// `navigator.credentials.get`, and unpack the assertion.
async fn run_ceremony(req: WebAuthnRequest) -> Result<Authentication, BioError> {
    if unsafe { js_has_window() } == 0 {
        return Err(BioError::Backend("no window".into()));
    }
    let promise = credentials_get(&req).map_err(js_to_backend)?;
    let credential = JsFuture::new(&promise).await.map_err(map_get_error)?;

    if !cast::instance_of(&credential, "PublicKeyCredential") {
        return Err(BioError::Backend("credential was not a PublicKeyCredential".into()));
    }
    // SAFETY (both): fresh `G.add` slots the snippets minted for us.
    let response = match unsafe { js_response(credential.raw()) } {
        0 => JsValue::undefined(),
        h => unsafe { JsValue::from_raw(h) },
    };
    if !cast::instance_of(&response, "AuthenticatorAssertionResponse") {
        return Err(BioError::Backend("response was not an assertion".into()));
    }

    let field = |which: u32| match unsafe { js_assertion_buffer(credential.raw(), which) } {
        0 => None,
        h => Some(buffer_to_vec(&unsafe { JsValue::from_raw(h) })),
    };
    let assertion = WebAuthnAssertion {
        credential_id: field(0).unwrap_or_default(),
        authenticator_data: field(1).unwrap_or_default(),
        client_data_json: field(2).unwrap_or_default(),
        signature: field(3).unwrap_or_default(),
        user_handle: field(4),
    };

    Ok(Authentication {
        assertion: Some(assertion),
    })
}

/// Start the ceremony: the WebAuthn request options (the `publicKey`
/// member) are assembled inside the binding from the request's bytes.
fn credentials_get(req: &WebAuthnRequest) -> Result<JsValue, JsError> {
    let mut has = 0;
    let (rp, rl) = match &req.rp_id {
        Some(id) => {
            has |= 1;
            string::abi(id)
        }
        None => (0, 0),
    };
    let timeout = match req.timeout_ms {
        Some(ms) => {
            has |= 2;
            f64::from(ms)
        }
        None => 0.0,
    };
    let ids: Vec<u8> = req.allow_credentials.concat();
    let lens: Vec<u32> = req.allow_credentials.iter().map(|id| id.len() as u32).collect();
    let h = unsafe {
        js_credentials_get(
            req.challenge.as_ptr() as usize,
            req.challenge.len(),
            has,
            rp,
            rl,
            timeout,
            ids.as_ptr() as usize,
            lens.as_ptr() as usize,
            lens.len(),
        )
    }?;
    // SAFETY: a fresh `G.add` slot the snippet minted for us.
    Ok(unsafe { JsValue::from_raw(h) })
}

/// Copy an `ArrayBuffer` (as returned by WebAuthn fields) into a `Vec<u8>`.
fn buffer_to_vec(buffer: &JsValue) -> Vec<u8> {
    let len = unsafe { js_byte_length(buffer.raw()) } as usize;
    // Allocated BEFORE the copy crossing: the snippet takes its memory view
    // after nothing else can grow memory.
    let mut out = vec![0u8; len];
    if len != 0 {
        unsafe { js_copy_bytes(buffer.raw(), out.as_mut_ptr() as usize) }
    }
    out
}

/// Map a rejected `navigator.credentials.get` promise to a typed error. A
/// `NotAllowedError`/`AbortError` DOMException is the browser's signal for a
/// user cancellation or ceremony timeout.
fn map_get_error(err: JsError) -> BioError {
    let name = err
        .value()
        .get("name")
        .ok()
        .and_then(|v| v.as_string())
        .unwrap_or_default();
    match name.as_str() {
        "NotAllowedError" | "AbortError" => BioError::Cancelled,
        _ => BioError::Backend(format!("WebAuthn get() failed: {}", describe(&err))),
    }
}

fn js_to_backend(err: JsError) -> BioError {
    BioError::Backend(describe(&err))
}

/// Best-effort human description of a JS error value.
fn describe(err: &JsError) -> String {
    let v = err.value();
    v.get("message")
        .ok()
        .and_then(|m| m.as_string())
        .or_else(|| v.as_string())
        .unwrap_or_else(|| "unknown JS error".into())
}
