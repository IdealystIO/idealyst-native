//! DOM screenshot for the web Robot transport — the web peer of the native
//! `capture_screenshot` (AppKit/UIKit/Android), so `robot screenshot` works the
//! same on every platform.
//!
//! The browser has no synchronous DOM-rasterize API, so we use the standard SVG
//! `<foreignObject>` technique:
//!   1. serialize the `#app` subtree to XHTML (keeping its class names) — via a
//!      deep CLONE whose input/textarea/option attributes are synced from the
//!      live DOM properties first (`.checked`, `.value`, `.selected`), because
//!      XMLSerializer reads attributes and would otherwise snapshot stale
//!      pre-interaction state,
//!   2. embed every `<style>` sheet's CSSOM rules inline (idealyst styles via
//!      hashed CSS classes, not inline `style=` — without this the snapshot
//!      comes out unstyled),
//!   3. inline every `url(...)` asset (notably `@font-face` fonts) as a `data:`
//!      URL — the SVG renders in an isolated context with neither the page's
//!      loaded web fonts nor network access, so without this the text falls
//!      back to a default (e.g. Times for a missing Inter),
//!   4. wrap it in an SVG sized to the element, render that into an `<img>`,
//!   5. draw the image to a `<canvas>` and export PNG.
//!
//! Step 4 is async (image load), so this reports via a callback; the robot
//! transport sends the bridge response when it fires.
//!
//! **Fidelity caveat:** this is DOM rasterization, not the browser compositor's
//! output. A *cross-origin* image (no CORS) taints the canvas (→ an error
//! response); same-origin assets and fonts are inlined and render fine. For
//! pixel-perfect web capture use Playwright/CDP; this is the *uniform
//! cross-platform* robot path.

use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{
    CanvasRenderingContext2d, CssStyleSheet, HtmlCanvasElement, HtmlImageElement,
    HtmlInputElement, HtmlOptionElement, HtmlStyleElement, HtmlTextAreaElement,
};

/// Result handed to the caller: `(png_base64, width_px, height_px)`.
pub type ShotResult = Result<(String, u32, u32), String>;

/// Capture the current page (`#app`, else `<body>`) to a PNG and call `done`
/// with the base64 PNG + pixel dimensions, or an error. Async — `done` fires
/// after the snapshot image loads.
pub fn capture(done: Box<dyn FnOnce(ShotResult)>) {
    match build_svg_data_url() {
        Ok(prep) => render_to_png(prep, done),
        Err(e) => done(Err(e)),
    }
}

struct Prep {
    /// The SVG as a `data:` URL. NB: a `blob:` URL taints the canvas on the
    /// `foreignObject` draw in Chromium (opaque origin); a `data:` URL does not.
    url: String,
    /// CSS-pixel size of the captured element.
    css_w: f64,
    css_h: f64,
    /// Device-pixel-ratio scale, so the PNG is crisp on retina and the reported
    /// dimensions match the native backends (which return device pixels).
    dpr: f64,
}

fn build_svg_data_url() -> Result<Prep, String> {
    let window = web_sys::window().ok_or("no window")?;
    let document = window.document().ok_or("no document")?;
    let target = document
        .query_selector("#app")
        .ok()
        .flatten()
        .or_else(|| document.body().map(Into::into))
        .ok_or("no #app or <body> to capture")?;

    let rect = target.get_bounding_client_rect();
    let css_w = rect.width().max(1.0);
    let css_h = rect.height().max(1.0);
    let dpr = window.device_pixel_ratio().max(1.0);

    // idealyst styles via hashed CSS classes in shared <style> sheets — embed
    // their CSSOM rules so the serialized class names resolve inside the SVG.
    let mut css = String::new();
    if let Ok(styles) = document.query_selector_all("style") {
        for i in 0..styles.length() {
            let Some(node) = styles.item(i) else { continue };
            let Ok(style_el) = node.dyn_into::<HtmlStyleElement>() else {
                continue;
            };
            let Some(sheet) = style_el.sheet() else { continue };
            let Ok(sheet) = sheet.dyn_into::<CssStyleSheet>() else {
                continue;
            };
            if let Ok(rules) = sheet.css_rules() {
                for j in 0..rules.length() {
                    if let Some(rule) = rules.item(j) {
                        css.push_str(&rule.css_text());
                        css.push('\n');
                    }
                }
            }
        }
    }

    // Inline @font-face fonts (and any url() assets) so the isolated SVG render
    // uses the real web fonts instead of a default fallback.
    let css = inline_resources(css);

    let xhtml = serialize_with_live_input_state(&target)?;

    // The SVG is sized in CSS pixels; the canvas scales by dpr for crispness.
    // The wrapper div is given an EXPLICIT pixel size and `#app` is forced to
    // fill it — without this, `#app`'s percentage-sized children resolve against
    // an auto-height root inside the foreignObject and collapse to 0 (blank).
    let svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{css_w}\" height=\"{css_h}\">\
           <foreignObject x=\"0\" y=\"0\" width=\"100%\" height=\"100%\">\
             <div xmlns=\"http://www.w3.org/1999/xhtml\" style=\"width:{css_w}px;height:{css_h}px\">\
               <style>{css}\n#app{{width:100%;height:100%}}</style>{xhtml}\
             </div>\
           </foreignObject>\
         </svg>"
    );

    // Percent-encode into a `data:` URL. (A `blob:` URL would taint the canvas
    // on the foreignObject draw — see the doc on `Prep::url`.)
    let encoded = String::from(js_sys::encode_uri_component(&svg));
    let url = format!("data:image/svg+xml;charset=utf-8,{encoded}");

    Ok(Prep {
        url,
        css_w,
        css_h,
        dpr,
    })
}

/// Serialize `target` to XHTML with the LIVE input state baked in.
///
/// XMLSerializer reads ATTRIBUTES, but the state a user (or the Robot) has
/// interacted with lives in DOM PROPERTIES — `input.checked`, `input.value`,
/// `option.selected` — which stop tracking their reflected attributes the
/// moment they're dirtied (the HTML "dirty value/checkedness" flags).
/// Serializing the live tree therefore produced a stale PNG: a toggled
/// checkbox rendered unchecked, typed text rendered as the initial value.
///
/// Fix at the root: deep-clone the subtree, mirror the live properties into
/// the CLONE's attributes, and serialize the clone — the user's live DOM is
/// never mutated. `cloneNode(true)` copies attributes only (properties reset
/// to attribute-derived state), so the mirroring must read from the live tree.
fn serialize_with_live_input_state(target: &web_sys::Element) -> Result<String, String> {
    let clone: web_sys::Element = target
        .clone_node_with_deep(true)
        .map_err(|_| "cloning the capture subtree failed".to_string())?
        .dyn_into()
        .map_err(|_| "cloned capture subtree is not an element".to_string())?;
    mirror_input_props_into_attributes(target, &clone);
    let serializer =
        web_sys::XmlSerializer::new().map_err(|_| "XMLSerializer unavailable".to_string())?;
    serializer
        .serialize_to_string(&clone)
        .map_err(|_| "serializing the DOM subtree failed".to_string())
}

/// Walk the live tree and its deep clone in parallel and write each live
/// input property into the clone's serializable attribute form.
///
/// The clone is a structural copy, so both trees have identical document
/// order — `querySelectorAll` with the same selector yields index-aligned
/// lists to zip. Handled state:
/// - `<input>`: `checked` property → set/remove the `checked` attribute
///   (checkbox/radio; harmless elsewhere), `value` property → `value` attr;
/// - `<textarea>`: `value` property → child text (a textarea renders its
///   text content — it has no `value` attribute);
/// - `<option>`: `selected` property → set/remove the `selected` attribute.
fn mirror_input_props_into_attributes(live_root: &web_sys::Element, clone_root: &web_sys::Element) {
    const SELECTOR: &str = "input, textarea, option";
    let (Ok(live), Ok(cloned)) = (
        live_root.query_selector_all(SELECTOR),
        clone_root.query_selector_all(SELECTOR),
    ) else {
        return;
    };
    let n = live.length().min(cloned.length());
    for i in 0..n {
        let (Some(l), Some(c)) = (live.item(i), cloned.item(i)) else { continue };
        if let (Some(li), Some(ci)) =
            (l.dyn_ref::<HtmlInputElement>(), c.dyn_ref::<HtmlInputElement>())
        {
            if li.checked() {
                let _ = ci.set_attribute("checked", "");
            } else {
                let _ = ci.remove_attribute("checked");
            }
            let _ = ci.set_attribute("value", &li.value());
        } else if let (Some(lt), Some(ct)) =
            (l.dyn_ref::<HtmlTextAreaElement>(), c.dyn_ref::<HtmlTextAreaElement>())
        {
            ct.set_text_content(Some(&lt.value()));
        } else if let (Some(lo), Some(co)) =
            (l.dyn_ref::<HtmlOptionElement>(), c.dyn_ref::<HtmlOptionElement>())
        {
            if lo.selected() {
                let _ = co.set_attribute("selected", "");
            } else {
                let _ = co.remove_attribute("selected");
            }
        }
    }
}

fn render_to_png(prep: Prep, done: Box<dyn FnOnce(ShotResult)>) {
    let img = match HtmlImageElement::new() {
        Ok(i) => i,
        Err(_) => {
            done(Err("could not create <img>".into()));
            return;
        }
    };

    // Shared one-shot sink: whichever of load/error fires first takes `done`.
    let sink = Rc::new(RefCell::new(Some(done)));

    let img_for_load = img.clone();
    let sink_load = sink.clone();
    let on_load = Closure::once_into_js(move || {
        let result = draw_and_export(&img_for_load, prep.css_w, prep.css_h, prep.dpr);
        if let Some(cb) = sink_load.borrow_mut().take() {
            cb(result);
        }
    });
    img.set_onload(Some(on_load.unchecked_ref()));

    let sink_err = sink.clone();
    let on_error = Closure::once_into_js(move |_e: JsValue| {
        if let Some(cb) = sink_err.borrow_mut().take() {
            cb(Err("the snapshot SVG failed to load (malformed markup?)".into()));
        }
    });
    img.set_onerror(Some(on_error.unchecked_ref()));

    // `once_into_js` hands ownership to JS, so the closures live until fired.
    img.set_src(&prep.url);
}

fn draw_and_export(img: &HtmlImageElement, css_w: f64, css_h: f64, dpr: f64) -> ShotResult {
    let document = web_sys::window()
        .and_then(|w| w.document())
        .ok_or("no document")?;
    let canvas: HtmlCanvasElement = document
        .create_element("canvas")
        .map_err(|_| "create canvas")?
        .dyn_into()
        .map_err(|_| "canvas cast")?;
    let px_w = (css_w * dpr).round() as u32;
    let px_h = (css_h * dpr).round() as u32;
    canvas.set_width(px_w);
    canvas.set_height(px_h);

    let ctx: CanvasRenderingContext2d = canvas
        .get_context("2d")
        .map_err(|_| "get 2d context")?
        .ok_or("no 2d context")?
        .dyn_into()
        .map_err(|_| "context cast")?;
    let _ = ctx.scale(dpr, dpr);
    ctx.draw_image_with_html_image_element(img, 0.0, 0.0)
        .map_err(|_| "drawImage failed")?;

    // `toDataURL` throws SecurityError if the canvas was tainted (cross-origin).
    let data_url = canvas
        .to_data_url_with_type("image/png")
        .map_err(|_| "toDataURL failed — canvas tainted by cross-origin content".to_string())?;
    let b64 = data_url
        .split_once(',')
        .map(|(_, b)| b.to_string())
        .ok_or("malformed data URL")?;
    Ok((b64, px_w, px_h))
}

/// Fetch every `url(...)` resource in the CSS (fonts, small images) and inline
/// it as a `data:` URL. A foreignObject SVG renders in an isolated context with
/// neither the page's loaded `@font-face` fonts nor network access, so without
/// this the text falls back to a default font (e.g. Times for a missing Inter).
/// Synchronous XHR (same-origin dev assets) keeps this inside the non-async
/// capture path.
fn inline_resources(mut css: String) -> String {
    for url in extract_urls(&css) {
        if let Some(data_url) = fetch_as_data_url(&url) {
            css = css.replace(&url, &data_url);
        }
    }
    css
}

/// Pull the contents of each `url(...)` (deduped), skipping already-inlined
/// `data:` URLs.
fn extract_urls(css: &str) -> Vec<String> {
    let mut urls = Vec::new();
    let mut rest = css;
    while let Some(i) = rest.find("url(") {
        rest = &rest[i + 4..];
        let Some(j) = rest.find(')') else { break };
        let inner = rest[..j].trim().trim_matches(|c| c == '"' || c == '\'');
        if !inner.is_empty() && !inner.starts_with("data:") {
            urls.push(inner.to_string());
        }
        rest = &rest[j + 1..];
    }
    urls.sort();
    urls.dedup();
    urls
}

/// Synchronous GET of `url` → a `data:<mime>;base64,...` URL, or `None` on any
/// failure. The `x-user-defined` charset makes each response byte readable as a
/// char in `0x00..=0xFF`, which `btoa` then base64-encodes.
fn fetch_as_data_url(url: &str) -> Option<String> {
    let xhr = web_sys::XmlHttpRequest::new().ok()?;
    xhr.open_with_async("GET", url, false).ok()?;
    let _ = xhr.override_mime_type("text/plain; charset=x-user-defined");
    xhr.send().ok()?;
    if xhr.status().ok()? != 200 {
        return None;
    }
    let text = xhr.response_text().ok()??;
    let bytes: String = text
        .chars()
        .map(|c| char::from_u32((c as u32) & 0xFF).unwrap_or('\u{0}'))
        .collect();
    let b64 = web_sys::window()?.btoa(&bytes).ok()?;
    let mime = match url.rsplit('.').next() {
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        Some("ttf") => "font/ttf",
        Some("otf") => "font/otf",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("svg") => "image/svg+xml",
        _ => "application/octet-stream",
    };
    Some(format!("data:{mime};base64,{b64}"))
}

// Browser-side tests (this module needs a real DOM — XMLSerializer, cloneNode,
// property/attribute divergence). Run with the `robot` feature on:
//
// ```sh
// cd crates/backend/web
// wasm-pack test --headless --chrome --release -- --features robot
// ```
#[cfg(test)]
mod tests {
    use super::*;
    use wasm_bindgen_test::*;

    fn doc() -> web_sys::Document {
        web_sys::window().unwrap().document().unwrap()
    }

    /// Build a detached-from-`#app` scratch root attached to `<body>` (the
    /// serializer needs a connected tree for `querySelectorAll` parity with
    /// the real capture path) and clean it up via the returned guard.
    fn scratch_root() -> web_sys::Element {
        let d = doc();
        let root = d.create_element("div").unwrap();
        d.body().unwrap().append_child(&root).unwrap();
        root
    }

    /// Regression: the web `robot screenshot` rendered a toggled checkbox as
    /// UNCHECKED (stale) because XMLSerializer serializes ATTRIBUTES while the
    /// toggle lives in the `.checked` PROPERTY (dirty-checkedness flag). The
    /// serialized copy must mirror the live property both ways, and the live
    /// DOM must never be mutated by the capture.
    #[wasm_bindgen_test]
    fn regression_screenshot_reflects_checkbox_property_state() {
        let root = scratch_root();
        let input = doc().create_element("input").unwrap();
        input.set_attribute("type", "checkbox").unwrap();
        root.append_child(&input).unwrap();

        // Property-only toggle — exactly what a user click / robot `click`
        // produces: `.checked == true`, no `checked` attribute.
        let cb: HtmlInputElement = input.clone().dyn_into().unwrap();
        cb.set_checked(true);

        // NB: serializers normalize the attribute VALUE (`checked=""` vs
        // Firefox's `checked="checked"`) — assert on the attribute NAME
        // only. `type="checkbox"` does not contain the substring `checked=`.
        let xhtml = serialize_with_live_input_state(&root).unwrap();
        assert!(
            xhtml.contains("checked="),
            "serialized copy must carry the live checked property as an attribute: {xhtml}"
        );
        // The capture must not touch the live DOM (we serialized a clone).
        assert!(
            !input.has_attribute("checked"),
            "live DOM was mutated by the capture"
        );

        // Inverse direction: markup says checked, live property says not —
        // the stale attribute must be REMOVED from the serialized copy.
        // (Setting the attribute after a script `.checked` write doesn't
        // flip the property back: the dirty-checkedness flag is set.)
        cb.set_checked(false);
        input.set_attribute("checked", "").unwrap();
        let xhtml = serialize_with_live_input_state(&root).unwrap();
        assert!(
            !xhtml.contains("checked="),
            "stale checked attribute must be dropped when the property is false: {xhtml}"
        );

        root.remove();
    }

    /// Same staleness bug, other property-backed widgets: typed text
    /// (`input.value` / `textarea.value`) and `<select>` selection
    /// (`option.selected`) must reach the serialized copy.
    #[wasm_bindgen_test]
    fn regression_screenshot_reflects_text_and_select_property_state() {
        let root = scratch_root();
        let d = doc();

        let text = d.create_element("input").unwrap();
        text.set_attribute("value", "initial").unwrap();
        root.append_child(&text).unwrap();
        let area = d.create_element("textarea").unwrap();
        root.append_child(&area).unwrap();
        let select = d.create_element("select").unwrap();
        let opt_a = d.create_element("option").unwrap();
        opt_a.set_attribute("value", "a").unwrap();
        opt_a.set_attribute("selected", "").unwrap();
        let opt_b = d.create_element("option").unwrap();
        opt_b.set_attribute("value", "b").unwrap();
        select.append_child(&opt_a).unwrap();
        select.append_child(&opt_b).unwrap();
        root.append_child(&select).unwrap();

        // Live edits: properties diverge from the serialized attributes.
        text.clone()
            .dyn_into::<HtmlInputElement>()
            .unwrap()
            .set_value("typed text");
        area.clone()
            .dyn_into::<HtmlTextAreaElement>()
            .unwrap()
            .set_value("typed area");
        opt_b.clone().dyn_into::<HtmlOptionElement>().unwrap().set_selected(true);

        let xhtml = serialize_with_live_input_state(&root).unwrap();
        assert!(xhtml.contains("value=\"typed text\""), "input value stale: {xhtml}");
        assert!(!xhtml.contains("value=\"initial\""), "stale initial value kept: {xhtml}");
        assert!(xhtml.contains("typed area"), "textarea text stale: {xhtml}");
        // Picking b deselects a (single-select): the serialized copy must
        // move the `selected` attribute from a to b. Match per-`<option`
        // fragment so attribute order / serializer value normalization
        // (`selected=""` vs `selected="selected"`) don't matter.
        let opt_fragment = |needle: &str| {
            xhtml
                .split("<option")
                .find(|frag| frag.contains(needle))
                .unwrap_or_else(|| panic!("no <option {needle}> in: {xhtml}"))
                .to_string()
        };
        assert!(
            opt_fragment("value=\"b\"").contains("selected="),
            "selected option stale: {xhtml}"
        );
        assert!(
            !opt_fragment("value=\"a\"").contains("selected="),
            "deselected option kept its selected attribute: {xhtml}"
        );
        // Live DOM untouched.
        assert!(!opt_b.has_attribute("selected"), "live DOM was mutated by the capture");

        root.remove();
    }
}
