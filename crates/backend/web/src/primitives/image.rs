//! `Primitive::Image` — an `<img>` with reactive `src`.

use crate::WebBackend;
use runtime_core::AssetId;
use wasm_bindgen::JsCast;
use web_sys::Node;

/// Sentinel URL the framework emits for asset-backed images.
/// The shape is `asset://<u64-id>` — see `runtime_core::image_asset`.
const ASSET_URL_PREFIX: &str = "asset://";

/// If `src` is an `asset://{id}` sentinel, resolve it through the
/// backend's `asset_urls` table; otherwise return the URL as-is.
/// Falls back to the sentinel string verbatim if the id has not been
/// registered (the browser will render a broken image — visible
/// failure beats silent stripping).
fn resolve_src<'a>(b: &'a WebBackend, src: &'a str) -> std::borrow::Cow<'a, str> {
    let Some(rest) = src.strip_prefix(ASSET_URL_PREFIX) else {
        return std::borrow::Cow::Borrowed(src);
    };
    let Ok(id_value) = rest.parse::<u64>() else {
        return std::borrow::Cow::Borrowed(src);
    };
    match b.asset_urls.get(&AssetId(id_value)) {
        Some(url) => std::borrow::Cow::Owned(url.clone()),
        None => std::borrow::Cow::Borrowed(src),
    }
}

pub(crate) fn create(b: &mut WebBackend, src: &str, alt: Option<&str>) -> Node {
    let img = b
        .doc
        .create_element("img")
        .expect("create_element img failed");
    let resolved = resolve_src(b, src);
    let _ = img.set_attribute("src", &resolved);
    if let Some(a) = alt {
        let _ = img.set_attribute("alt", a);
    }
    img.unchecked_into::<Node>()
}

pub(crate) fn update_src(b: &WebBackend, node: &Node, src: &str) {
    if let Ok(el) = node.clone().dyn_into::<web_sys::Element>() {
        let resolved = resolve_src(b, src);
        let _ = el.set_attribute("src", &resolved);
    }
}
