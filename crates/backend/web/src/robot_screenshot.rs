//! DOM screenshot for the web Robot transport — the web peer of the native
//! `capture_screenshot` (AppKit/UIKit/Android), so `robot screenshot` works the
//! same on every platform.
//!
//! The browser has no synchronous DOM-rasterize API, so we use the standard SVG
//! `<foreignObject>` technique:
//!   1. serialize the live `#app` subtree to XHTML (keeping its class names),
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
    CanvasRenderingContext2d, CssStyleSheet, HtmlCanvasElement, HtmlImageElement, HtmlStyleElement,
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

    let serializer =
        web_sys::XmlSerializer::new().map_err(|_| "XMLSerializer unavailable".to_string())?;
    let xhtml = serializer
        .serialize_to_string(&target)
        .map_err(|_| "serializing the DOM subtree failed".to_string())?;

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
