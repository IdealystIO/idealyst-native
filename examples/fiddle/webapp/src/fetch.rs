//! Browser-side `POST /compile` helper. Raw web-sys instead of
//! `gloo-net` to avoid a dep — the request is shaped simply enough
//! that the boilerplate is contained.

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;
use web_sys::{Headers, Request, RequestInit, Response};

/// Output mode the user picked in the editor. Maps to the
/// `mode` field on the wire and the cargo feature the server
/// hands to wasm-pack.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    Simulator,
    Web,
}

impl Mode {
    fn wire(self) -> &'static str {
        match self {
            Mode::Simulator => "simulator",
            Mode::Web => "web",
        }
    }
}

/// POST the project tree to `/compile`. `files` is the user's
/// `<relative path>` → contents map; the server treats each path
/// as relative to the snippet's logical `src/` directory. Returns
/// the cache hash on success, or a human-readable error string on
/// failure (the raw rustc / wasm-pack stderr is included when the
/// failure is a compile failure).
pub async fn compile(
    files: &std::collections::BTreeMap<String, String>,
    mode: Mode,
) -> Result<String, String> {
    let body = serialize_body(files, mode).map_err(|e| format!("serialize body: {e}"))?;

    let headers = Headers::new().map_err(|e| jserr("Headers::new", e))?;
    headers
        .set("content-type", "application/json")
        .map_err(|e| jserr("Headers::set content-type", e))?;

    let opts = RequestInit::new();
    opts.set_method("POST");
    opts.set_headers(&headers);
    opts.set_body(&JsValue::from_str(&body));

    let request = Request::new_with_str_and_init("/compile", &opts)
        .map_err(|e| jserr("Request::new", e))?;
    let window = web_sys::window().ok_or_else(|| "no window".to_string())?;
    let resp_value = JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(|e| jserr("fetch", e))?;
    let resp: Response = resp_value
        .dyn_into()
        .map_err(|_| "response wasn't a Response".to_string())?;

    let text_promise = resp.text().map_err(|e| jserr("Response::text", e))?;
    let text_js = JsFuture::from(text_promise)
        .await
        .map_err(|e| jserr("read body", e))?;
    let body_text = text_js
        .as_string()
        .ok_or_else(|| "body wasn't a string".to_string())?;

    parse_response(resp.status(), &body_text)
}

/// Build the request body without pulling in `serde`. Shape is
/// `{ "files": { "<path>": "<contents>", ... }, "mode": "..." }`;
/// `js_sys::JSON::stringify` handles all escaping in one call so
/// embedded `"`, newlines, control chars, etc. in user source land
/// in the JSON correctly.
fn serialize_body(
    files: &std::collections::BTreeMap<String, String>,
    mode: Mode,
) -> Result<String, String> {
    let outer = js_sys::Object::new();
    let files_obj = js_sys::Object::new();
    for (path, contents) in files {
        js_sys::Reflect::set(
            &files_obj,
            &JsValue::from_str(path),
            &JsValue::from_str(contents),
        )
        .map_err(|e| jserr("Reflect::set file entry", e))?;
    }
    js_sys::Reflect::set(&outer, &"files".into(), &files_obj)
        .map_err(|e| jserr("Reflect::set files", e))?;
    js_sys::Reflect::set(&outer, &"mode".into(), &JsValue::from_str(mode.wire()))
        .map_err(|e| jserr("Reflect::set mode", e))?;
    let s = js_sys::JSON::stringify(&outer).map_err(|e| jserr("JSON.stringify", e))?;
    s.as_string()
        .ok_or_else(|| "stringify returned non-string".to_string())
}

/// Server returns `{ "hash": "..." }` on success, `{ "error": "..." }`
/// on failure. Status code is 200 / 4xx / 5xx accordingly. We parse
/// the body either way so the caller surfaces a useful message
/// regardless of which path fired.
fn parse_response(status: u16, body: &str) -> Result<String, String> {
    let parsed: JsValue = js_sys::JSON::parse(body).map_err(|e| jserr("JSON.parse", e))?;
    if status >= 200 && status < 300 {
        let hash = js_sys::Reflect::get(&parsed, &"hash".into())
            .map_err(|e| jserr("Reflect::get hash", e))?;
        hash.as_string()
            .ok_or_else(|| "response missing `hash`".to_string())
    } else {
        let err = js_sys::Reflect::get(&parsed, &"error".into())
            .map_err(|e| jserr("Reflect::get error", e))?;
        Err(err
            .as_string()
            .unwrap_or_else(|| format!("HTTP {status}")))
    }
}

fn jserr(ctx: &str, e: JsValue) -> String {
    let dbg = e
        .as_string()
        .or_else(|| e.dyn_ref::<js_sys::Error>().map(|e| e.message().into()))
        .unwrap_or_else(|| format!("{e:?}"));
    format!("{ctx}: {dbg}")
}
