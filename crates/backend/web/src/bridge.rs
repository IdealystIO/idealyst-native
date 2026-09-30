//! HYBRID-BRIDGE — the backend's crossing to web-sys for SDKs that have not
//! been ported off it yet (own-web-bindings phase 3 ports them and deletes
//! this module, and with it backend-web's `web-sys` / `wasm-bindgen`
//! dependencies).
//!
//! Since phase 2b the backend's nodes are `web_glue::dom` handles
//! (`Host::Node` is `web_glue::dom::Node`). An SDK whose internals still
//! use web-sys crosses here, at its seams only:
//!
//! * a mount handler builds its element with web-sys and hands it to the
//!   host with [`node_from_web_sys`];
//! * an ops impl that receives the host node as `&dyn Any` recovers a
//!   web-sys handle with [`node_to_web_sys`].
//!
//! Each crossing is one JS call (`web_glue::bridge`); nothing is copied.

use std::any::Any;

/// A web-sys node as the backend's node type (a new glue handle to the same
/// DOM object).
pub fn node_from_web_sys(node: &web_sys::Node) -> web_glue::dom::Node {
    web_glue::bridge::from_bindgen(node.as_ref())
}

/// The backend node an SDK receives as `&dyn Any` (a `web_glue::dom::Node`)
/// as a web-sys node. `None` if `node` is not a backend node.
pub fn node_to_web_sys(node: &dyn Any) -> Option<web_sys::Node> {
    let node = node.downcast_ref::<web_glue::dom::Node>()?;
    Some(wasm_bindgen::JsCast::unchecked_into(web_glue::bridge::to_bindgen(node)))
}

/// Any glue handle as a web-sys value of type `T` (unchecked: the caller
/// knows what the value is).
pub fn to_web_sys<T: wasm_bindgen::JsCast>(v: &impl web_glue::JsCast) -> T {
    wasm_bindgen::JsCast::unchecked_into(web_glue::bridge::to_bindgen(v))
}

/// Any web-sys value as a glue handle of type `T` (unchecked).
pub fn from_web_sys<T: web_glue::JsCast>(v: &impl AsRef<wasm_bindgen::JsValue>) -> T {
    web_glue::bridge::from_bindgen(v.as_ref())
}

/// The backend's node type (`Host::Node`), for SDK mount-handler
/// signatures.
pub use web_glue::dom::Node as HostNode;
