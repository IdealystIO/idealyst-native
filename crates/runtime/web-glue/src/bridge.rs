//! HYBRID-BRIDGE — the crossing between web-glue handles and wasm-bindgen
//! values, for a page that links both.
//!
//! One page can hold two handle spaces: web-glue's slab and wasm-bindgen's
//! heap. These two functions move a value across. They are the ONLY
//! sanctioned crossing — grep for `HYBRID-BRIDGE` / `bridge::` to find
//! every use.
//!
//! Phase 3 of `docs/proposals/own-web-bindings.md` ported the DOM-mounting
//! and the media / file SDKs; the media streams (`native_source`) and the
//! dropped file now cross between crates as web-glue handles
//! (`web_glue::dom::MediaStream`, `web_glue::dom::File`). What still crosses
//! here is wgpu's (phase 5's hybrid mode), each site marked
//! `HYBRID-BRIDGE: wgpu`:
//!
//! * canvas-native's public `make_2d_rasterizer` / `publish_capture_stream`
//!   take a `web_sys::HtmlCanvasElement`, because canvas-vello calls them with
//!   its wgpu graphics canvas;
//! * canvas-vello's texture-layer `<video>` is a `web_sys::HtmlVideoElement`
//!   (wgpu's `ExternalImageSource` takes one), so a layer's glue
//!   `MediaStream` crosses out to become its `srcObject`.
//!
//! How: the hybrid glue module (`pkg/__idealyst_glue.js`, written by
//! `wasm_carve::glue_js::hybrid_glue_js`) publishes
//! `globalThis.__idealystGlue = { add, get, … }`, and the two imports below
//! reach it through wasm-bindgen's `js_namespace`. Each crossing is one
//! extra JS call; nothing else is copied. Only hybrid builds define the
//! global — an own-mode page has no wasm-bindgen heap to cross into.

use wasm_bindgen::prelude::wasm_bindgen;

use crate::cast::JsCast;
use crate::JsValue;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = __idealystGlue, js_name = add)]
    fn bridge_add(v: &wasm_bindgen::JsValue) -> u32;
    #[wasm_bindgen(js_namespace = __idealystGlue, js_name = get)]
    fn bridge_get(h: u32) -> wasm_bindgen::JsValue;
}

/// A wasm-bindgen value as a web-glue handle (a new slab slot for the same
/// JS object). Unchecked: `T` is whatever the caller knows the value is.
pub fn from_bindgen<T: JsCast>(v: &wasm_bindgen::JsValue) -> T {
    T::unchecked_from_js(unsafe { JsValue::from_raw(bridge_add(v)) })
}

/// A web-glue handle as a wasm-bindgen value (a new wasm-bindgen reference
/// to the same JS object; the glue handle is untouched).
pub fn to_bindgen<T: JsCast>(v: &T) -> wasm_bindgen::JsValue {
    bridge_get(v.as_ref().raw())
}
