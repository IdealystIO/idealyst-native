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
//!   4. wrap it in an SVG sized to the VIEWPORT, with the element placed at
//!      its live viewport position, render that into an `<img>`,
//!   5. draw the visible part of the element to a `<canvas>` and export PNG.
//!
//! The capture is "what the user sees": the serialized copy carries no
//! scroll state (a clone of a scrolled `overflow: auto` box renders scrolled
//! to the top), so every scrolled container's offset is baked into the
//! clone's layout ([`bake_scroll_offsets`]), and window scroll is replayed by
//! the viewport-sized SVG — `#app` sits at its live (possibly negative)
//! offset, and the PNG is cropped to the part of it inside the viewport.
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
use web_glue::JsCast;
use web_glue::dom::{
    CanvasRenderingContext2d, CssStyleDeclaration, CssStyleSheet, Element, HtmlCanvasElement,
    HtmlElement, HtmlImageElement, HtmlInputElement, HtmlOptionElement, HtmlStyleElement,
    HtmlTextAreaElement, SvgElement, Window,
};

/// Result handed to the caller: `(png_base64, width_px, height_px)`.
pub type ShotResult = Result<(String, u32, u32), String>;

/// Capture the current page (`#app`, else `<body>`) to a PNG and call `done`
/// with the base64 PNG + pixel dimensions, or an error. Async — `done` fires
/// after the snapshot image loads.
pub fn capture(done: Box<dyn FnOnce(ShotResult)>) {
    let target = web_glue::dom::window()
        .and_then(|w| w.document())
        .and_then(|d| {
            d.query_selector("#app")
                .ok()
                .flatten()
                .or_else(|| d.body().map(Into::into))
        });
    match target {
        Some(target) => capture_element(&target, done),
        None => done(Err("no #app or <body> to capture".into())),
    }
}

/// [`capture`] for an explicit element — the visible part of `target`.
fn capture_element(target: &Element, done: Box<dyn FnOnce(ShotResult)>) {
    match build_svg_data_url(target) {
        Ok(prep) => render_to_png(prep, done),
        Err(e) => done(Err(e)),
    }
}

struct Prep {
    /// The SVG as a `data:` URL. NB: a `blob:` URL taints the canvas on the
    /// `foreignObject` draw in Chromium (opaque origin); a `data:` URL does not.
    url: String,
    /// The visible part of the captured element, in viewport CSS pixels —
    /// the region of the viewport-sized SVG the PNG is cropped to.
    crop: Crop,
    /// Device-pixel-ratio scale, so the PNG is crisp on retina and the reported
    /// dimensions match the native backends (which return device pixels).
    dpr: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Crop {
    left: f64,
    top: f64,
    width: f64,
    height: f64,
}

/// The part of `rect` (viewport coordinates) inside a `vw × vh` viewport —
/// what the user can actually see of the captured element. At least 1×1 so
/// the canvas is never empty (an element scrolled fully out of view yields a
/// 1-pixel capture rather than an error).
fn visible_crop(left: f64, top: f64, right: f64, bottom: f64, vw: f64, vh: f64) -> Crop {
    let l = left.max(0.0);
    let t = top.max(0.0);
    Crop {
        left: l,
        top: t,
        width: (right.min(vw) - l).max(1.0),
        height: (bottom.min(vh) - t).max(1.0),
    }
}

fn build_svg_data_url(target: &Element) -> Result<Prep, String> {
    let window = web_glue::dom::window().ok_or("no window")?;
    let document = window.document().ok_or("no document")?;

    let rect = target.get_bounding_client_rect();
    let css_w = rect.width().max(1.0);
    let css_h = rect.height().max(1.0);
    let dpr = window.device_pixel_ratio().max(1.0);
    // The layout viewport, scrollbars excluded — the box `position: fixed`
    // resolves against live, which the SVG's foreignObject stands in for.
    let viewport = document.document_element().ok_or("no documentElement")?;
    let vw = f64::from(viewport.client_width()).max(1.0);
    let vh = f64::from(viewport.client_height()).max(1.0);
    let crop = visible_crop(rect.left(), rect.top(), rect.right(), rect.bottom(), vw, vh);

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

    let xhtml = serialize_live_state(&window, target)?;

    // The SVG is the VIEWPORT, in CSS pixels; the canvas scales by dpr for
    // crispness. The wrapper div sits at the element's live viewport offset
    // (negative once the window is scrolled), so window scroll is replayed and
    // `position: fixed` descendants — which resolve against the foreignObject
    // here — land where they are on screen. The wrapper is given an EXPLICIT
    // pixel size and `#app` is forced to fill it — without this, `#app`'s
    // percentage-sized children resolve against an auto-height root inside
    // the foreignObject and collapse to 0 (blank).
    let (left, top) = (rect.left(), rect.top());
    let svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{vw}\" height=\"{vh}\">\
           <foreignObject x=\"0\" y=\"0\" width=\"100%\" height=\"100%\">\
             <div xmlns=\"http://www.w3.org/1999/xhtml\" \
                  style=\"position:absolute;left:{left}px;top:{top}px;width:{css_w}px;height:{css_h}px\">\
               <style>{css}\n#app{{width:100%;height:100%}}</style>{xhtml}\
             </div>\
           </foreignObject>\
         </svg>"
    );

    // Percent-encode into a `data:` URL. (A `blob:` URL would taint the canvas
    // on the foreignObject draw — see the doc on `Prep::url`.)
    let encoded = String::from(web_glue::js::encode_uri_component(&svg));
    let url = format!("data:image/svg+xml;charset=utf-8,{encoded}");

    Ok(Prep { url, crop, dpr })
}

/// Serialize `target` to XHTML with the LIVE state baked in: input
/// properties (below) and scroll offsets ([`bake_scroll_offsets`]).
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
fn serialize_live_state(window: &Window, target: &Element) -> Result<String, String> {
    let clone: Element = target
        .clone_node_with_deep(true)
        .map_err(|_| "cloning the capture subtree failed".to_string())?
        .dyn_into()
        .map_err(|_| "cloned capture subtree is not an element".to_string())?;
    mirror_input_props_into_attributes(target, &clone);
    bake_scroll_offsets(window, target, &clone);
    let serializer =
        web_glue::dom::XmlSerializer::new().map_err(|_| "XMLSerializer unavailable".to_string())?;
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
fn mirror_input_props_into_attributes(live_root: &web_glue::dom::Element, clone_root: &web_glue::dom::Element) {
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

/// Replay every scrolled container's offset in the clone's LAYOUT.
///
/// The clone renders with every `scrollTop`/`scrollLeft` at 0 — scroll
/// position is DOM state, not markup — so a capture taken after the user
/// scrolled showed the top of each list, not what was on screen. The
/// snapshot is a static image, so the offset is replayed by shifting each
/// scroller's content by `(-scrollLeft, -scrollTop)`:
///
/// - **flex / grid scrollers** — every item gets the margin PAIR
///   `margin-top -= y; margin-bottom += y` (and left/right for `x`). Each
///   item's margin box keeps its size, so track sizing, wrapping, `gap`,
///   stretch and `justify-content` are unchanged, and every item lands `y`
///   higher. Margins, not `translate`: a transform makes the item a
///   containing block for its absolute/fixed descendants (re-anchoring
///   them), and a translated `position: sticky` header would scroll off
///   with the content. With margins the sticky item's FLOW position moves
///   up by `y`, so at the clone's scroll 0 it sticks exactly where the live
///   one is stuck at scroll `y`.
/// - **block scrollers** — margins collapse between block siblings, so the
///   pair would not cancel; the in-flow children move into one
///   `display: flow-root` wrapper (a BFC, like the scroller itself, so no
///   margin crosses its edge) that is shifted instead.
///
/// Absolutely-positioned children shift too when the scroller is their
/// containing block (they scroll with the content live); `position: fixed`
/// ones and absolute ones anchored above the scroller don't scroll, so they
/// are left alone. `display: contents` children (the web backend's reactive
/// anchors) generate no box, so their children are shifted instead.
///
/// The live and clone trees are structurally identical, so their
/// `querySelectorAll("*")` lists are index-aligned; every scrolled pair is
/// collected BEFORE any clone is restructured by a block wrap.
fn bake_scroll_offsets(window: &Window, live_root: &Element, clone_root: &Element) {
    let (Ok(live), Ok(cloned)) = (
        live_root.query_selector_all("*"),
        clone_root.query_selector_all("*"),
    ) else {
        return;
    };
    let as_el = |n: Option<web_glue::dom::Node>| n.and_then(|n| n.dyn_into::<Element>().ok());
    let mut scrolled = Vec::new();
    let pairs = std::iter::once((Some(live_root.clone()), Some(clone_root.clone())))
        .chain((0..live.length().min(cloned.length())).map(|i| (as_el(live.item(i)), as_el(cloned.item(i)))));
    for (l, c) in pairs {
        let (Some(l), Some(c)) = (l, c) else { continue };
        let (x, y) = (l.scroll_left_f64(), l.scroll_top_f64());
        if x != 0.0 || y != 0.0 {
            scrolled.push((l, c, x, y));
        }
    }
    for (live_scroller, clone_scroller, x, y) in scrolled {
        let Some(cs) = computed(window, &live_scroller) else { continue };
        let display = cs.get_property_value("display").unwrap_or_default();
        if display.contains("flex") || display.contains("grid") {
            shift_items(window, &live_scroller, &live_scroller, &clone_scroller, x, y);
        } else {
            wrap_and_shift_block(window, &live_scroller, &clone_scroller, &cs, x, y);
        }
    }
}

/// Margin-pair shift of every box-generating child of `live_parent` (see
/// [`bake_scroll_offsets`]), written onto the index-aligned clone child.
fn shift_items(
    window: &Window,
    scroller: &Element,
    live_parent: &Element,
    clone_parent: &Element,
    x: f64,
    y: f64,
) {
    let (lc, cc) = (live_parent.children(), clone_parent.children());
    for i in 0..lc.length().min(cc.length()) {
        let (Some(l), Some(c)) = (lc.item(i), cc.item(i)) else { continue };
        let Some(cs) = computed(window, &l) else { continue };
        match cs.get_property_value("display").unwrap_or_default().as_str() {
            "none" => continue,
            "contents" => {
                shift_items(window, scroller, &l, &c, x, y);
                continue;
            }
            _ => {}
        }
        if scrolls_with(scroller, &l, &cs) {
            shift_by_margins(&c, &cs, x, y);
        }
    }
}

/// Block-flow scroller: move the clone's in-flow children into a shifted
/// `flow-root` wrapper (see [`bake_scroll_offsets`]). Positioned
/// (absolute/fixed) children stay direct children of the scroller so their
/// containing block is unchanged; those that scroll get the margin pair.
fn wrap_and_shift_block(
    window: &Window,
    live_scroller: &Element,
    clone_scroller: &Element,
    scroller_cs: &CssStyleDeclaration,
    x: f64,
    y: f64,
) {
    let Some(doc) = window.document() else { return };
    let Ok(wrapper) = doc.create_element("div") else { return };
    // `min-height` = the scroller's content box, so content that relied on
    // filling the scrollport still does; auto `width` with the cancelling
    // right margin keeps the content box width.
    let content_h = f64::from(live_scroller.client_height())
        - px(&scroller_cs.get_property_value("padding-top").unwrap_or_default()).unwrap_or(0.0)
        - px(&scroller_cs.get_property_value("padding-bottom").unwrap_or_default()).unwrap_or(0.0);
    let _ = wrapper.set_attribute(
        "style",
        &format!(
            "display:flow-root;margin:{}px {x}px 0 {}px;min-height:{}px",
            -y,
            -x,
            content_h.max(0.0)
        ),
    );

    // Decide every child against the LIVE tree before moving anything:
    // the clone's child nodes are index-aligned with the live ones.
    let (live_nodes, clone_nodes) = (live_scroller.child_nodes(), clone_scroller.child_nodes());
    let mut to_wrap = Vec::new();
    for i in 0..live_nodes.length().min(clone_nodes.length()) {
        let (Some(l), Some(c)) = (live_nodes.item(i), clone_nodes.item(i)) else { continue };
        let positioned = l
            .dyn_ref::<Element>()
            .and_then(|el| computed(window, el).map(|cs| (el.clone(), cs)))
            .filter(|(_, cs)| {
                matches!(
                    cs.get_property_value("position").unwrap_or_default().as_str(),
                    "absolute" | "fixed"
                )
            });
        match positioned {
            Some((el, cs)) => {
                if scrolls_with(live_scroller, &el, &cs) {
                    if let Some(c) = c.dyn_ref::<Element>() {
                        shift_by_margins(c, &cs, x, y);
                    }
                }
            }
            None => to_wrap.push(c),
        }
    }
    let first = clone_scroller.first_child();
    let _ = clone_scroller.insert_before(&wrapper, first.as_ref());
    for node in to_wrap {
        let _ = wrapper.append_child(&node);
    }
}

/// Whether `child` moves when `scroller` scrolls: everything in flow does;
/// `fixed` never does; `absolute` only when the scroller is its containing
/// block (`offsetParent` names the nearest positioned ancestor).
fn scrolls_with(scroller: &Element, child: &Element, cs: &CssStyleDeclaration) -> bool {
    match cs.get_property_value("position").unwrap_or_default().as_str() {
        "fixed" => false,
        "absolute" => child
            .dyn_ref::<HtmlElement>()
            .and_then(|h| h.offset_parent())
            .is_some_and(|p| p.is_same_node(Some(scroller.as_ref()))),
        _ => true,
    }
}

/// The margin pair: `-d` on the leading edge, `+d` on the trailing edge, so
/// the box moves by `(-x, -y)` while its margin box keeps its size. Written
/// `!important` inline so a stylesheet rule can't outrank it. Computed
/// margins are used values (px) for rendered boxes; a non-px value leaves
/// the box unshifted rather than writing a broken declaration.
fn shift_by_margins(clone: &Element, cs: &CssStyleDeclaration, x: f64, y: f64) {
    let Some(style) = inline_style(clone) else { return };
    let margin = |side: &str| px(&cs.get_property_value(&format!("margin-{side}")).unwrap_or_default());
    let (Some(t), Some(b), Some(l), Some(r)) =
        (margin("top"), margin("bottom"), margin("left"), margin("right"))
    else {
        return;
    };
    for (prop, v) in [
        ("margin-top", t - y),
        ("margin-bottom", b + y),
        ("margin-left", l - x),
        ("margin-right", r + x),
    ] {
        let _ = style.set_property_with_priority(prop, &format!("{v}px"), "important");
    }
}

fn computed(window: &Window, el: &Element) -> Option<CssStyleDeclaration> {
    window.get_computed_style(el).ok().flatten()
}

fn inline_style(el: &Element) -> Option<CssStyleDeclaration> {
    if let Some(h) = el.dyn_ref::<HtmlElement>() {
        Some(h.style())
    } else {
        el.dyn_ref::<SvgElement>().map(SvgElement::style)
    }
}

/// `"12.5px"` → `12.5`; anything else → `None`.
fn px(v: &str) -> Option<f64> {
    v.trim().strip_suffix("px")?.trim().parse().ok()
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
    let on_load = web_glue::Closure::once_into_js(move |_| {
        let result = draw_and_export(&img_for_load, prep.crop, prep.dpr);
        if let Some(cb) = sink_load.borrow_mut().take() {
            cb(result);
        }
    });
    img.set_onload(Some(on_load.unchecked_ref()));

    let sink_err = sink.clone();
    let on_error = web_glue::Closure::once_into_js(move |_e| {
        if let Some(cb) = sink_err.borrow_mut().take() {
            cb(Err("the snapshot SVG failed to load (malformed markup?)".into()));
        }
    });
    img.set_onerror(Some(on_error.unchecked_ref()));

    // `once_into_js` hands ownership to JS, so the closures live until fired.
    img.set_src(&prep.url);
}

fn draw_and_export(img: &HtmlImageElement, crop: Crop, dpr: f64) -> ShotResult {
    let document = web_glue::dom::window()
        .and_then(|w| w.document())
        .ok_or("no document")?;
    let canvas: HtmlCanvasElement = document
        .create_element("canvas")
        .map_err(|_| "create canvas")?
        .dyn_into()
        .map_err(|_| "canvas cast")?;
    let px_w = (crop.width * dpr).round() as u32;
    let px_h = (crop.height * dpr).round() as u32;
    canvas.set_width(px_w);
    canvas.set_height(px_h);

    let ctx: CanvasRenderingContext2d = canvas
        .get_context("2d")
        .map_err(|_| "get 2d context")?
        .ok_or("no 2d context")?
        .dyn_into()
        .map_err(|_| "context cast")?;
    let _ = ctx.scale(dpr, dpr);
    // The image is the whole viewport; offsetting the draw by the crop origin
    // keeps only the visible part of the captured element.
    ctx.draw_image_with_html_image_element(img, -crop.left, -crop.top)
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
    let xhr = web_glue::dom::XmlHttpRequest::new().ok()?;
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
    let b64 = web_glue::dom::window()?.btoa(&bytes).ok()?;
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

    fn doc() -> web_glue::dom::Document {
        web_glue::dom::window().unwrap().document().unwrap()
    }

    /// Build a detached-from-`#app` scratch root attached to `<body>` (the
    /// serializer needs a connected tree for `querySelectorAll` parity with
    /// the real capture path) and clean it up via the returned guard.
    fn scratch_root() -> web_glue::dom::Element {
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
        let xhtml = serialize_live_state(&web_glue::dom::window().unwrap(), &root).unwrap();
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
        let xhtml = serialize_live_state(&web_glue::dom::window().unwrap(), &root).unwrap();
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

        let xhtml = serialize_live_state(&web_glue::dom::window().unwrap(), &root).unwrap();
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

    // ---- scroll position (the "see what I see" bug) ----------------------

    /// Run the real capture pipeline on `target` and read the PNG back:
    /// `(device-pixel width, device-pixel height, RGBA at each CSS point)`.
    async fn capture_pixels(target: &Element, points: &[(f64, f64)]) -> (u32, u32, Vec<[u8; 4]>) {
        use web_glue::js::{Function, Promise};
        let shot = Rc::new(RefCell::new(None));
        let promise = {
            let target = target.clone();
            let shot = shot.clone();
            Promise::new(&mut |resolve, _reject| {
                let shot = shot.clone();
                capture_element(
                    &target,
                    Box::new(move |res| {
                        *shot.borrow_mut() = Some(res);
                        let _ = resolve.call0(&web_glue::JsValue::NULL);
                    }),
                );
            })
        };
        web_glue::JsFuture::new(&promise).await.unwrap();
        let (b64, w, h) = shot.borrow_mut().take().unwrap().expect("capture failed");

        // Decode in the page: draw the PNG to a canvas and sample device
        // pixels at `point * devicePixelRatio`.
        let pts = points.iter().map(|(x, y)| format!("{x},{y}")).collect::<Vec<_>>().join(";");
        let decode = Function::new_with_args(
            "b64, pts",
            "return new Promise((ok, err) => {
                const img = new Image();
                img.onerror = () => err('png decode failed');
                img.onload = () => {
                    const c = document.createElement('canvas');
                    c.width = img.naturalWidth; c.height = img.naturalHeight;
                    const g = c.getContext('2d');
                    g.drawImage(img, 0, 0);
                    const dpr = Math.max(window.devicePixelRatio, 1);
                    ok(pts.split(';').map(p => {
                        const [x, y] = p.split(',').map(Number);
                        return Array.from(g.getImageData(Math.floor(x * dpr), Math.floor(y * dpr), 1, 1).data).join(',');
                    }).join(';'));
                };
                img.src = 'data:image/png;base64,' + b64;
            });",
        );
        let decoded: Promise = decode
            .call2(&web_glue::JsValue::NULL, &b64.as_str().into(), &pts.as_str().into())
            .unwrap()
            .unchecked_into();
        let out = web_glue::JsFuture::new(&decoded).await.unwrap().as_string().unwrap();
        let px = out
            .split(';')
            .map(|p| {
                let v: Vec<u8> = p.split(',').map(|n| n.parse().unwrap()).collect();
                [v[0], v[1], v[2], v[3]]
            })
            .collect();
        (w, h, px)
    }

    fn el(tag_style: &str) -> Element {
        let e = doc().create_element("div").unwrap();
        e.set_attribute("style", tag_style).unwrap();
        e
    }

    const GREEN: [u8; 4] = [0, 128, 0, 255];
    const BLUE: [u8; 4] = [0, 0, 255, 255];
    const ORANGE: [u8; 4] = [255, 165, 0, 255];
    const MAGENTA: [u8; 4] = [255, 0, 255, 255];
    const WHITE: [u8; 4] = [255, 255, 255, 255];

    /// Regression: after scrolling a list, `robot screenshot` on web showed
    /// the list scrolled to the TOP — the serialized clone carries no
    /// `scrollTop` — so "see what I see" handed the agent the wrong screen.
    /// A flex scroller (the shape most idealyst lists take) scrolled 500px
    /// must capture what is on screen, including a `position: sticky`
    /// header that is stuck at the top only BECAUSE of the scroll.
    #[wasm_bindgen_test]
    async fn regression_screenshot_keeps_flex_scroller_offset_and_sticky_header() {
        let root = el("position:fixed;left:0;top:0;width:100px;height:100px;z-index:2147483647");
        let scroller = el("height:100px;overflow:auto;display:flex;flex-direction:column");
        scroller.append_child(&el("flex:none;height:300px;background:rgb(255,0,0)")).unwrap();
        scroller
            .append_child(&el("flex:none;height:20px;position:sticky;top:0;background:rgb(0,0,255)"))
            .unwrap();
        scroller.append_child(&el("flex:none;height:1000px;background:rgb(0,128,0)")).unwrap();
        root.append_child(&scroller).unwrap();
        doc().body().unwrap().append_child(&root).unwrap();
        scroller.set_scroll_top(500);

        let (_, _, px) = capture_pixels(&root, &[(10.0, 5.0), (10.0, 50.0)]).await;
        assert_eq!(px[0], BLUE, "sticky header stuck at the top of the scrolled list");
        assert_eq!(px[1], GREEN, "content below it is the scrolled-to content, not the top (red)");
        assert_eq!(scroller.scroll_top(), 500, "capture must not touch the live scroll");
        root.remove();
    }

    /// Same bug, block-flow scroller — where the flex margin pair would be
    /// wrong, because adjacent block margins COLLAPSE (10px + 20px render as
    /// a 20px gap, not 30px). Also covers an absolutely-positioned child of
    /// the scroller, which scrolls with the content live.
    #[wasm_bindgen_test]
    async fn regression_screenshot_keeps_block_scroller_offset_with_collapsing_margins() {
        let root = el("position:fixed;left:0;top:0;width:100px;height:100px;z-index:2147483647");
        let scroller = el("position:relative;height:100px;overflow:auto;background:rgb(255,255,255)");
        scroller
            .append_child(&el("height:300px;margin-bottom:10px;background:rgb(255,0,0)"))
            .unwrap();
        scroller
            .append_child(&el("height:1000px;margin-top:20px;background:rgb(0,128,0)"))
            .unwrap();
        scroller
            .append_child(&el(
                "position:absolute;top:330px;left:50px;width:20px;height:20px;background:rgb(255,165,0)",
            ))
            .unwrap();
        root.append_child(&scroller).unwrap();
        doc().body().unwrap().append_child(&root).unwrap();
        // Green starts at 300 + 20 (collapsed) = 320 → 10px below the top.
        scroller.set_scroll_top(310);

        let (_, _, px) = capture_pixels(&root, &[(10.0, 5.0), (10.0, 15.0), (60.0, 30.0)]).await;
        assert_eq!(px[0], WHITE, "the collapsed 20px gap (red would mean scroll was lost)");
        assert_eq!(px[1], GREEN, "green begins 10px down, not 20px (margins not collapsed)");
        assert_eq!(px[2], ORANGE, "absolute child scrolled with the content");
        root.remove();
    }

    /// Same bug at the page level: with the WINDOW scrolled, the capture is
    /// the part of the element inside the viewport (not its top), and a
    /// `position: fixed` descendant appears where it is on screen.
    #[wasm_bindgen_test]
    async fn regression_screenshot_replays_window_scroll() {
        let root = el("position:absolute;left:0;top:0;width:100px;height:3000px;background:rgb(255,255,255)");
        root.append_child(&el(
            "position:absolute;left:0;top:1000px;width:20px;height:20px;background:rgb(0,0,255)",
        ))
        .unwrap();
        root.append_child(&el(
            "position:fixed;left:50px;top:10px;width:20px;height:20px;background:rgb(255,0,255)",
        ))
        .unwrap();
        doc().body().unwrap().append_child(&root).unwrap();
        let window = web_glue::dom::window().unwrap();
        let scroll_to = web_glue::js::Function::new_with_args("y", "window.scrollTo(0, y)");
        scroll_to.call1(&web_glue::JsValue::NULL, &1000.0.into()).unwrap();
        assert_eq!(window.scroll_y_f64(), 1000.0, "test page must be scrollable");

        let (_, h, px) = capture_pixels(&root, &[(5.0, 5.0), (60.0, 20.0)]).await;
        scroll_to.call1(&web_glue::JsValue::NULL, &0.0.into()).unwrap();
        root.remove();

        assert_eq!(px[0], BLUE, "the capture starts at the scrolled-to part of the element");
        assert_eq!(px[1], MAGENTA, "fixed element where it is on screen");
        let vh = f64::from(doc().document_element().unwrap().client_height());
        let dpr = window.device_pixel_ratio().max(1.0);
        assert!(
            f64::from(h) <= (vh * dpr).ceil(),
            "capture is the visible part, not the whole 3000px element: {h}"
        );
    }

    #[wasm_bindgen_test]
    fn visible_crop_clips_to_viewport() {
        // Window scrolled 1000px: element starts above the viewport.
        assert_eq!(
            visible_crop(0.0, -1000.0, 100.0, 2000.0, 800.0, 600.0),
            Crop { left: 0.0, top: 0.0, width: 100.0, height: 600.0 }
        );
        // Fully inside.
        assert_eq!(
            visible_crop(10.0, 20.0, 110.0, 70.0, 800.0, 600.0),
            Crop { left: 10.0, top: 20.0, width: 100.0, height: 50.0 }
        );
        // Scrolled fully out of view: never an empty canvas.
        assert_eq!(
            visible_crop(0.0, -500.0, 100.0, -100.0, 800.0, 600.0),
            Crop { left: 0.0, top: 0.0, width: 100.0, height: 1.0 }
        );
    }
}
