//! The `image` primitive — an `<img>` with reactive `src`.

use crate::WebBackend;
use runtime_shared::image_src::{self, ImageSrc, RejectReason};
use runtime_shared::{AssetId, ImageErrorHandler, ImageLoadEvent, ImageLoadHandler};
use web_glue::JsCast;
use web_glue::dom::{HtmlImageElement, Node};

/// Why an `image()` `src` produced no URL for the `<img>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Unresolved {
    /// `image_src::classify` rejected it (relative, `//host`, `blob:`, …).
    Rejected(RejectReason),
    /// `asset://<id>` with an id nobody registered.
    UnregisteredAsset(u64),
}

/// The `docs/image-src.md` contract, for web: the URL to put in
/// `<img src>`, or why there is none.
///
/// `http(s)` and `data:` pass through unchanged; `asset://<id>` becomes
/// the registered asset's URL (`asset_url`, the backend's `asset_urls`
/// table). Everything else is an error *before* the browser sees it: web
/// could resolve a relative path or `blob:` against the page, but no
/// native backend can, so letting the browser try is what made an app
/// "work on web" and draw a blank tile on iOS.
pub(crate) fn resolve_img_src<'a>(
    src: &'a str,
    asset_url: impl FnOnce(AssetId) -> Option<&'a str>,
) -> Result<&'a str, Unresolved> {
    match image_src::classify(src) {
        ImageSrc::Remote(url) | ImageSrc::Data(url) => Ok(url),
        ImageSrc::Asset(id) => asset_url(AssetId(id)).ok_or(Unresolved::UnregisteredAsset(id)),
        ImageSrc::Rejected(reason) => Err(Unresolved::Rejected(reason)),
    }
}

/// Marks an `<img>` whose `src` failed before reaching the browser:
/// `pending:<src>` until the synthetic `error` event is dispatched,
/// `fired:<src>` after. The two states keep `on_error` at exactly once:
/// a handler installed while `pending` hears the dispatch, one installed
/// after `fired` is replayed (the same late-install rule `install_error`
/// applies to a real `complete && naturalWidth == 0` failure). Living on
/// the element, the state is dropped with it.
const SRC_ERROR_ATTR: &str = "data-src-error";
const PENDING: &str = "pending:";
const FIRED: &str = "fired:";

fn apply_src(b: &WebBackend, img: &web_glue::dom::Element, src: &str) {
    match resolve_img_src(src, |id| b.asset_urls.get(&id).map(String::as_str)) {
        Ok(url) => {
            let _ = img.remove_attribute(SRC_ERROR_ATTR);
            // Guard the write: the mount path calls `create_image` and then
            // `update_image_src` with the same src, and re-setting `src`
            // re-runs the browser's image update (a cached image can
            // re-fire `load`).
            if img.get_attribute("src").as_deref() != Some(url) {
                let _ = img.remove_attribute(REPLAYED_ATTR);
                let _ = img.set_attribute("src", url);
            }
        }
        Err(why) => {
            if let Some(state) = img.get_attribute(SRC_ERROR_ATTR) {
                let same = state.strip_prefix(PENDING).or_else(|| state.strip_prefix(FIRED));
                if same == Some(src) {
                    return; // already failed on exactly this src
                }
            }
            match why {
                Unresolved::Rejected(reason) => image_src::warn_rejected(src, reason),
                Unresolved::UnregisteredAsset(_id) => {
                    #[cfg(debug_assertions)]
                    runtime_shared::log_warn!(
                        "image(): asset id {_id} is not registered; use image_asset(..)"
                    );
                }
            }
            // Removing `src` aborts any in-flight load of the previous
            // src (so its `load`/`error` cannot land after this one) and,
            // with no `src` attribute, the browser queues no event of its
            // own (HTML "update the image data": selected source null).
            let _ = img.remove_attribute("src");
            let _ = img.remove_attribute(REPLAYED_ATTR);
            let _ = img.set_attribute(SRC_ERROR_ATTR, &format!("{PENDING}{src}"));
            // Dispatch the failure as the element's own `error` event, the
            // path a DOM load error takes, on a microtask (the browser's
            // error is a queued task too; and this runs inside the
            // backend's `borrow_mut`). A later `src` change that lands
            // first rewrites the marker, which cancels this dispatch.
            let img = img.clone();
            let src = src.to_string();
            runtime_shared::schedule_microtask(move || {
                let pending = format!("{PENDING}{src}");
                if img.get_attribute(SRC_ERROR_ATTR).as_deref() != Some(pending.as_str()) {
                    return; // superseded
                }
                let _ = img.set_attribute(SRC_ERROR_ATTR, &format!("{FIRED}{src}"));
                if let Ok(ev) = web_glue::dom::Event::new("error") {
                    let _ = img.dispatch_event(&ev);
                }
            });
        }
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
    apply_src(b, &img, src);
    if let Some(a) = alt {
        let _ = img.set_attribute("alt", a);
    }
    img.unchecked_into::<Node>()
}

pub(crate) fn update_src(b: &WebBackend, node: &Node, src: &str) {
    if let Ok(el) = node.clone().dyn_into::<web_glue::dom::Element>() {
        apply_src(b, &el, src);
    }
}

/// Install an `on_load` handler: fire the framework's [`ImageLoadHandler`]
/// with the decoded bitmap's natural dimensions.
///
/// The `<img>` may already be complete by the time the media handler
/// installs this (a hydrated SSR `<img>` loaded before the wasm booted; a cached
/// image is complete synchronously), so we check
/// `HtmlImageElement::complete()` up front and fire — if the `load` event
/// already fired we would otherwise drop it. If it is merely queued (the
/// cached case), [`take_replayed`] drops it so `on_load` fires once.
/// `complete && naturalWidth > 0` distinguishes a decoded image from
/// a still-loading or errored one. The `<img>` owns the closure (see
/// `glue_dom::listen_for_element_lifetime`) — same pattern as touch/hover.
///
/// The already-decoded fire is **deferred to a microtask** rather than
/// invoked inline. This install runs from inside the vocabulary's image
/// handler (`handlers/media.rs`) while the backend `RefCell` is
/// `borrow_mut`'d; a user `on_load` that writes a signal a reactive style
/// depends on would re-enter that same borrow through the style effect
/// and panic ("RefCell already borrowed"). Deferring also matches the browser: a cached
/// `<img>`'s real `load` event dispatches as a queued task, never inline.
pub(crate) fn install_load(node: &Node, handler: ImageLoadHandler) {
    let Ok(img) = node.clone().dyn_into::<HtmlImageElement>() else {
        return;
    };
    // Already decoded before we could attach — notify on a microtask
    // (out of the backend borrow, and consistent with async `load`).
    if img.complete() && img.natural_width() > 0 {
        let event = ImageLoadEvent {
            width: img.natural_width() as f32,
            height: img.natural_height() as f32,
        };
        mark_replayed(&img);
        let handler = handler.clone();
        runtime_shared::schedule_microtask(move || handler(&event));
        // Still attach the listener: a reactive `src` swap loads a new
        // bitmap and re-fires `load`, which should re-notify.
    }
    let img_for_cb = img.clone();
    crate::glue_dom::listen_for_element_lifetime(&img, "load", Default::default(), move |_| {
        if take_replayed(&img_for_cb) {
            return;
        }
        handler(&ImageLoadEvent {
            width: img_for_cb.natural_width() as f32,
            height: img_for_cb.natural_height() as f32,
        });
    });
}

/// Records that `install_load`/`install_error` already replayed the
/// current `src`'s outcome. `complete` can't tell "the event already
/// fired" from "the event is queued": an image in the browser's
/// list of available images (the same URL decoded earlier in the page,
/// e.g. the second row of a list showing one avatar) is `complete`
/// synchronously after `src` is set, yet its `load` is still a queued
/// task. Without this the replay AND that queued event both reached the
/// handler — `on_load` twice. The listener drops the one event whose
/// `src` matches the replay; any write to `src` clears the record.
const REPLAYED_ATTR: &str = "data-src-replayed";

fn mark_replayed(img: &HtmlImageElement) {
    if let Some(src) = img.get_attribute("src") {
        let _ = img.set_attribute(REPLAYED_ATTR, &src);
    }
}

fn take_replayed(img: &HtmlImageElement) -> bool {
    let Some(replayed) = img.get_attribute(REPLAYED_ATTR) else {
        return false;
    };
    let _ = img.remove_attribute(REPLAYED_ATTR);
    img.get_attribute("src").as_deref() == Some(replayed.as_str())
}

/// Install an `on_error` handler: fire the framework's
/// [`ImageErrorHandler`] when the `<img>` fails to load/decode. Mirrors
/// [`install_load`], including the already-settled check — a `complete`
/// image with `naturalWidth == 0` has already errored, so notify. The
/// already-errored fire is deferred to a microtask for the same reason
/// as `install_load`: this runs inside the backend `borrow_mut`, and a
/// user `on_error` writing a style-affecting signal would re-enter it.
pub(crate) fn install_error(node: &Node, handler: ImageErrorHandler) {
    let Ok(img) = node.clone().dyn_into::<HtmlImageElement>() else {
        return;
    };
    // Already errored before we could attach: the browser failed it
    // (complete but no bitmap), or `apply_src` failed it and already
    // dispatched its `error` (`fired:`). A `pending:` failure needs no
    // replay — its dispatch is still queued and reaches the listener below.
    let browser_failed = img.complete() && img.natural_width() == 0 && !img.src().is_empty();
    let src_failed = img
        .get_attribute(SRC_ERROR_ATTR)
        .is_some_and(|s| s.starts_with(FIRED));
    if browser_failed || src_failed {
        if browser_failed {
            mark_replayed(&img);
        }
        let handler = handler.clone();
        runtime_shared::schedule_microtask(move || handler());
    }
    let img_for_cb = img.clone();
    crate::glue_dom::listen_for_element_lifetime(&img, "error", Default::default(), move |_| {
        if take_replayed(&img_for_cb) {
            return;
        }
        handler();
    });
}

/// Swap the `<img alt>` in place when a reactive `alt` source fires.
/// `None` removes the attribute. Mirrors `create`'s alt handling — the
/// alt attribute IS the accessibility text for an image on web.
pub(crate) fn update_alt(node: &Node, alt: Option<&str>) {
    if let Ok(el) = node.clone().dyn_into::<web_glue::dom::Element>() {
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

#[cfg(test)]
mod tests {
    use super::*;
    use runtime_shared::image_src::vectors::{self, Expect};

    fn no_assets(_: AssetId) -> Option<&'static str> {
        None
    }

    /// The CrewForge kiosk's `/attachments/<id>` loaded on web (resolved
    /// against the page) and drew a blank tile on iOS. Web rejects it too,
    /// before the browser can resolve it.
    #[test]
    fn regression_relative_src_is_rejected_on_web_too() {
        for src in ["/attachments/abc", "logo.png", "./logo.png", "../x.png"] {
            assert_eq!(
                resolve_img_src(src, no_assets),
                Err(Unresolved::Rejected(RejectReason::Relative)),
                "{src}"
            );
        }
        // The other forms only a browser could read.
        assert_eq!(
            resolve_img_src("//cdn.example.com/x.png", no_assets),
            Err(Unresolved::Rejected(RejectReason::SchemeRelative))
        );
        for src in ["blob:https://example.com/0f2c", "file:///tmp/x.png", "ftp://a.io/x.png"] {
            assert_eq!(
                resolve_img_src(src, no_assets),
                Err(Unresolved::Rejected(RejectReason::UnsupportedScheme)),
                "{src}"
            );
        }
    }

    #[test]
    fn remote_and_data_pass_through() {
        assert_eq!(resolve_img_src("https://x.io/a.png", no_assets), Ok("https://x.io/a.png"));
        assert_eq!(resolve_img_src("HTTP://x.io/a.png", no_assets), Ok("HTTP://x.io/a.png"));
        assert_eq!(
            resolve_img_src(vectors::SVG_2X1_PERCENT, no_assets),
            Ok(vectors::SVG_2X1_PERCENT)
        );
    }

    /// `asset://` was matched case-sensitively and an unregistered id fell
    /// through to the browser as the literal sentinel.
    #[test]
    fn assets_resolve_case_insensitively_and_unregistered_ids_fail() {
        let table = |id: AssetId| (id == AssetId(7)).then_some("/assets/images/logo.png");
        assert_eq!(resolve_img_src("asset://7", table), Ok("/assets/images/logo.png"));
        assert_eq!(resolve_img_src("ASSET://7", table), Ok("/assets/images/logo.png"));
        assert_eq!(resolve_img_src("asset://8", table), Err(Unresolved::UnregisteredAsset(8)));
        assert_eq!(
            resolve_img_src("asset://x", table),
            Err(Unresolved::Rejected(RejectReason::BadAssetId))
        );
    }

    /// Every conformance vector: an `Errors` vector either never reaches
    /// the `<img>` or is a `data:` URI the browser fails to decode (that
    /// half is checked against a real browser in `tests.rs`); a `Loads`
    /// vector reaches it unchanged.
    #[test]
    fn conformance_vectors_resolve_per_contract() {
        for &(src, expect) in vectors::SYNC {
            match (resolve_img_src(src, no_assets), expect) {
                (Ok(url), Expect::Loads { .. }) => assert_eq!(url, src),
                (Ok(url), Expect::Errors) => {
                    assert!(url.to_ascii_lowercase().starts_with("data:"), "{src:?} reached <img>")
                }
                (Err(_), Expect::Errors) => {}
                (got, want) => panic!("{src:?}: {got:?}, want {want:?}"),
            }
        }
    }
}
