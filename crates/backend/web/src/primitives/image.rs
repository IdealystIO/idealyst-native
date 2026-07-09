//! `Element::Image` — an `<img>` with reactive `src`.

use crate::WebBackend;
use runtime_core::{AssetId, ImageErrorHandler, ImageLoadEvent, ImageLoadHandler};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use web_sys::{Event, HtmlImageElement, Node};

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
    // Hydration adoption — see `text_input::create` for the rationale.
    let img = if let Some(el) = b.hydrate_next("img") {
        el
    } else {
        let fresh = b
            .doc
            .create_element("img")
            .expect("create_element img failed");
        let node: Node = fresh.clone().unchecked_into();
        b.hydrate_note_fresh(&node);
        fresh
    };
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

/// Install an `on_load` handler: fire the framework's [`ImageLoadHandler`]
/// with the decoded bitmap's natural dimensions.
///
/// The `<img>` may already be complete by the time the walker installs
/// this (a cached image decodes synchronously, and the handler is wired
/// after `create`), so we check `HtmlImageElement::complete()` up front
/// and fire immediately — otherwise the `load` event already fired and we
/// would drop it. `complete && naturalWidth > 0` distinguishes a decoded
/// image from a still-loading or errored one. Closures are parked in the
/// backend keepalive vec (`_touch_closures`) so JS keeps them alive for
/// the element's lifetime — same pattern as touch/hover.
pub(crate) fn install_load(b: &mut WebBackend, node: &Node, handler: ImageLoadHandler) {
    let Ok(img) = node.clone().dyn_into::<HtmlImageElement>() else {
        return;
    };
    // Already decoded before we could attach — fire now with its size.
    if img.complete() && img.natural_width() > 0 {
        handler(&ImageLoadEvent {
            width: img.natural_width() as f32,
            height: img.natural_height() as f32,
        });
        // Still attach the listener: a reactive `src` swap loads a new
        // bitmap and re-fires `load`, which should re-notify.
    }
    let img_for_cb = img.clone();
    let closure = Closure::<dyn FnMut(Event)>::new(move |_ev: Event| {
        handler(&ImageLoadEvent {
            width: img_for_cb.natural_width() as f32,
            height: img_for_cb.natural_height() as f32,
        });
    });
    let _ = img.add_event_listener_with_callback("load", closure.as_ref().unchecked_ref());
    b._touch_closures.push(closure.into_js_value().unchecked_into());
}

/// Install an `on_error` handler: fire the framework's
/// [`ImageErrorHandler`] when the `<img>` fails to load/decode. Mirrors
/// [`install_load`], including the already-settled check — a `complete`
/// image with `naturalWidth == 0` has already errored, so fire now.
pub(crate) fn install_error(b: &mut WebBackend, node: &Node, handler: ImageErrorHandler) {
    let Ok(img) = node.clone().dyn_into::<HtmlImageElement>() else {
        return;
    };
    // Already errored before we could attach (complete but no bitmap).
    if img.complete() && img.natural_width() == 0 && !img.src().is_empty() {
        handler();
    }
    let closure = Closure::<dyn FnMut(Event)>::new(move |_ev: Event| {
        handler();
    });
    let _ = img.add_event_listener_with_callback("error", closure.as_ref().unchecked_ref());
    b._touch_closures.push(closure.into_js_value().unchecked_into());
}

/// Swap the `<img alt>` in place when a reactive `alt` source fires.
/// `None` removes the attribute. Mirrors `create`'s alt handling — the
/// alt attribute IS the accessibility text for an image on web.
pub(crate) fn update_alt(node: &Node, alt: Option<&str>) {
    if let Ok(el) = node.clone().dyn_into::<web_sys::Element>() {
        match alt {
            Some(a) => {
                let _ = el.set_attribute("alt", a);
            }
            None => {
                let _ = el.remove_attribute("alt");
            }
        }
    }
}
