//! The robot's reads (and one write) of an element's scroller, keyboard
//! kind and resolved style on web — the `IntrospectionOps::scroll_port` /
//! `set_scroll_offset` / `keyboard_kind` / `computed_style` seam.
//!
//! Every value comes from the browser (`getComputedStyle`, the scroll
//! metrics, the input's own attributes), never from the rules the framework
//! asked for: CSS decides what applies here (state pseudo-classes, `@media`,
//! preminted classes, inheritance), so the browser is the only reliable
//! source.

use runtime_shared::primitives::portal::ViewportRect;
use runtime_shared::primitives::text_input::KeyboardKind;
use runtime_vocabulary::caps::{NativeInjection, ResolvedStyle, ScrollPort, ScrollTarget};
use web_glue::JsValue;

use crate::introspect::{first_font_family, parse_color, parse_f32, parse_font_weight, parse_px};

web_glue::import! {
    /// The scroller `scroll_port` means, as a fresh handle (0 = none).
    ///
    /// `host == 0`: `n` itself, when its computed `overflow` scrolls on
    /// either axis. `host == 1`: the nearest element above `n` whose
    /// `overflow` scrolls and whose content overflows it, else the page —
    /// `document.scrollingElement`, which scrolls when the app's root
    /// outgrows the window with nothing in between to scroll it.
    fn js_scroll_target(n: u32, host: u32) -> u32 =
        "(n, host) => { const e = G.get(n); if (!e || e.nodeType !== 1) return 0; \
           const sc = (el, ax) => { const o = getComputedStyle(el)[ax]; return o === 'auto' || o === 'scroll'; }; \
           if (!host) return sc(e, 'overflowX') || sc(e, 'overflowY') ? G.add(e) : 0; \
           const top = document.scrollingElement || document.documentElement; \
           for (let p = e.parentElement; p && p !== top && p !== document.body; p = p.parentElement) { \
             if ((sc(p, 'overflowY') && p.scrollHeight > p.clientHeight) || \
                 (sc(p, 'overflowX') && p.scrollWidth > p.clientWidth)) return G.add(p); } \
           return G.add(top); }";

    /// One scroll metric of scroller `t` (a `js_scroll_target` handle):
    /// `k` 0 `scrollLeft`, 1 `scrollTop`, 2 / 3 the range on x / y (0 on
    /// an axis whose `overflow` doesn't scroll — the page scrolls both),
    /// 4..=7 the viewport rect it shows content in (x, y, width, height —
    /// its padding box, without borders and scroll bars; the window for
    /// the page).
    fn js_scroll_metric(t: u32, k: u32) -> f64 =
        "(t, k) => { const e = G.get(t); \
           const page = e === (document.scrollingElement || document.documentElement); \
           const sc = (ax) => { if (page) return true; const o = getComputedStyle(e)[ax]; return o === 'auto' || o === 'scroll'; }; \
           switch (k) { \
             case 0: return e.scrollLeft; case 1: return e.scrollTop; \
             case 2: return sc('overflowX') ? Math.max(0, e.scrollWidth - e.clientWidth) : 0; \
             case 3: return sc('overflowY') ? Math.max(0, e.scrollHeight - e.clientHeight) : 0; } \
           if (page) return [0, 0, e.clientWidth, e.clientHeight][k - 4]; \
           const r = e.getBoundingClientRect(); \
           return [r.left + e.clientLeft, r.top + e.clientTop, e.clientWidth, e.clientHeight][k - 4]; }";

    /// Move scroller `t` to `(x, y)` at once — `behavior: 'instant'`, so an
    /// author's `scroll-behavior: smooth` can't leave it mid-animation
    /// when the robot reads the frames back.
    fn js_scroll_to(t: u32, x: f64, y: f64) =
        "(t, x, y) => { G.get(t).scrollTo({ left: x, top: y, behavior: 'instant' }); }";
}

fn which(target: ScrollTarget) -> u32 {
    match target {
        ScrollTarget::Node => 0,
        ScrollTarget::Host => 1,
    }
}

/// The scroller handle for `node` / `target`, released on drop.
fn scroll_target(node: &web_glue::dom::Node, target: ScrollTarget) -> Option<JsValue> {
    let raw = AsRef::<JsValue>::as_ref(node).raw();
    // SAFETY: `js_scroll_target` returns a fresh `G.add` handle (or 0)
    // that nothing else owns.
    match unsafe { js_scroll_target(raw, which(target)) } {
        0 => None,
        h => Some(unsafe { JsValue::from_raw(h) }),
    }
}

/// `IntrospectionOps::scroll_port` on web.
pub(crate) fn scroll_port(node: &web_glue::dom::Node, target: ScrollTarget) -> Option<ScrollPort> {
    let t = scroll_target(node, target)?;
    let m = |k| unsafe { js_scroll_metric(t.raw(), k) } as f32;
    Some(ScrollPort {
        x: m(0),
        y: m(1),
        max_x: m(2),
        max_y: m(3),
        port: Some(ViewportRect { x: m(4), y: m(5), width: m(6), height: m(7) }),
    })
}

/// `IntrospectionOps::set_scroll_offset` on web. The target is resolved
/// when the injection runs, so it moves whatever scrolls then.
pub(crate) fn set_scroll_offset(
    node: &web_glue::dom::Node,
    target: ScrollTarget,
    x: f32,
    y: f32,
) -> Option<NativeInjection> {
    scroll_target(node, target)?;
    let node = node.clone();
    Some(Box::new(move || {
        let t = scroll_target(&node, target).ok_or("the element no longer scrolls")?;
        unsafe { js_scroll_to(t.raw(), x as f64, y as f64) };
        Ok(())
    }))
}

/// `IntrospectionOps::keyboard_kind` on web: the input's `inputmode`, which
/// is what picks a phone browser's soft keyboard (`TextInputOps`'s
/// `update_text_input_keyboard` writes it).
pub(crate) fn keyboard_kind(node: &web_glue::dom::Node) -> Option<KeyboardKind> {
    use web_glue::JsCast;
    let el = node.dyn_ref::<web_glue::dom::Element>()?;
    Some(keyboard_from_inputmode(el.get_attribute("inputmode").as_deref()))
}

/// The keyboard kind an `inputmode` value asks for.
fn keyboard_from_inputmode(mode: Option<&str>) -> KeyboardKind {
    match mode.map(str::trim) {
        Some("email") => KeyboardKind::Email,
        Some("tel") => KeyboardKind::Phone,
        Some("numeric") => KeyboardKind::Number,
        Some("decimal") => KeyboardKind::Decimal,
        Some("url") => KeyboardKind::Url,
        _ => KeyboardKind::Default,
    }
}

/// `IntrospectionOps::computed_style` on web: the element's
/// `getComputedStyle`.
pub(crate) fn computed_style(node: &web_glue::dom::Node) -> Option<ResolvedStyle> {
    use web_glue::JsCast;
    let el = node.dyn_ref::<web_glue::dom::Element>()?;
    let style = web_glue::dom::window()?.get_computed_style(el).ok().flatten()?;
    let get = |p: &str| style.get_property_value(p).unwrap_or_default();
    Some(resolved_from_computed(&get))
}

/// The robot's style read from computed CSS values (`get(property)`).
fn resolved_from_computed(get: &dyn Fn(&str) -> String) -> ResolvedStyle {
    let decoration = get("text-decoration-line");
    let border_width = parse_px(&get("border-top-width"));
    let family = first_font_family(&get("font-family"));
    ResolvedStyle {
        color: parse_color(&get("color")),
        background: parse_color(&get("background-color")),
        opacity: parse_f32(&get("opacity")),
        font_size: parse_px(&get("font-size")),
        font_weight: Some(parse_font_weight(&get("font-weight")).round() as u16),
        italic: Some(matches!(get("font-style").trim(), "italic" | "oblique")),
        font_family: (!family.is_empty()).then_some(family),
        underline: Some(decoration.split_whitespace().any(|d| d == "underline")),
        line_through: Some(decoration.split_whitespace().any(|d| d == "line-through")),
        text_align: Some(
            match get("text-align").trim() {
                "start" | "left" | "-webkit-left" => "left",
                "end" | "right" | "-webkit-right" => "right",
                "center" | "-webkit-center" => "center",
                "justify" => "justify",
                _ => "left",
            }
            .to_string(),
        ),
        border_width,
        border_color: border_width.filter(|w| *w > 0.0).and_then(|_| parse_color(&get("border-top-color"))),
        corner_radius: parse_px(&get("border-top-left-radius")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `inputmode` back to the kind that wrote it, for every kind the
    /// backend writes (`update_text_input_keyboard`'s table).
    #[test]
    fn every_inputmode_reads_back_as_its_keyboard_kind() {
        assert_eq!(keyboard_from_inputmode(None), KeyboardKind::Default);
        assert_eq!(keyboard_from_inputmode(Some("email")), KeyboardKind::Email);
        assert_eq!(keyboard_from_inputmode(Some("tel")), KeyboardKind::Phone);
        assert_eq!(keyboard_from_inputmode(Some("numeric")), KeyboardKind::Number);
        assert_eq!(keyboard_from_inputmode(Some("decimal")), KeyboardKind::Decimal);
        assert_eq!(keyboard_from_inputmode(Some("url")), KeyboardKind::Url);
    }

    /// Computed CSS to the robot's style: the decoration line list, the
    /// numeric weight, `start` alignment, a border's color only with a
    /// border.
    #[test]
    fn computed_css_maps_to_the_robot_style() {
        let css = |p: &str| -> String {
            match p {
                "color" => "rgb(255, 0, 0)",
                "background-color" => "rgba(0, 0, 0, 0)",
                "opacity" => "0.5",
                "font-size" => "16px",
                "font-weight" => "700",
                "font-style" => "italic",
                "font-family" => "\"Inter\", sans-serif",
                "text-decoration-line" => "underline line-through",
                "text-align" => "start",
                "border-top-width" => "0px",
                "border-top-color" => "rgb(1, 2, 3)",
                "border-top-left-radius" => "4px",
                _ => "",
            }
            .to_string()
        };
        let s = resolved_from_computed(&css);
        assert_eq!(s.color, Some([1.0, 0.0, 0.0, 1.0]));
        assert_eq!(s.background, Some([0.0, 0.0, 0.0, 0.0]));
        assert_eq!(s.opacity, Some(0.5));
        assert_eq!(s.font_size, Some(16.0));
        assert_eq!(s.font_weight, Some(700));
        assert_eq!(s.italic, Some(true));
        assert_eq!(s.font_family.as_deref(), Some("Inter"));
        assert_eq!((s.underline, s.line_through), (Some(true), Some(true)));
        assert_eq!(s.text_align.as_deref(), Some("left"));
        assert_eq!(s.border_color, None, "no border, no border color");
        assert_eq!(s.corner_radius, Some(4.0));
    }
}

#[cfg(all(test, target_arch = "wasm32"))]
mod browser_tests {
    use super::*;
    use wasm_bindgen_test::*;

    fn div(parent: &web_glue::dom::Element, style: &str) -> web_glue::dom::Element {
        let doc = web_glue::dom::window().unwrap().document().unwrap();
        let e = doc.create_element("div").unwrap();
        e.set_attribute("style", style).unwrap();
        parent.append_child(&e).unwrap();
        e
    }

    fn node(e: &web_glue::dom::Element) -> &web_glue::dom::Node {
        e.as_ref()
    }

    /// An element whose CSS `overflow` scrolls is a scroller with a range;
    /// one that only overflows (`visible`) is not, and its host scroller is
    /// the nearest scrolling ancestor — which `set_scroll_offset` moves.
    #[wasm_bindgen_test]
    fn a_css_scroller_and_the_host_scroller_above_a_root() {
        let doc = web_glue::dom::window().unwrap().document().unwrap();
        // No scrollbar: a classic one takes its width out of the port, and
        // whether the browser draws one is a host setting.
        let outer = div(&doc.body().unwrap(), "height:100px;width:100px;overflow:auto;scrollbar-width:none");
        let root = div(&outer, "height:500px;width:50px");
        let inner = div(&root, "height:40px;width:50px;overflow-y:scroll");
        div(&inner, "height:240px;width:10px");

        let p = scroll_port(node(&inner), ScrollTarget::Node).expect("an overflow-y:scroll element scrolls");
        assert_eq!((p.x, p.y, p.max_x, p.max_y), (0.0, 0.0, 0.0, 200.0));
        assert_eq!(scroll_port(node(&root), ScrollTarget::Node), None, "overflow: visible is no scroller");

        let host = scroll_port(node(&root), ScrollTarget::Host).expect("the outer scroller holds the root");
        assert_eq!(host.max_y, 400.0);
        let port = host.port.unwrap();
        assert_eq!((port.width, port.height), (100.0, 100.0));
        set_scroll_offset(node(&root), ScrollTarget::Host, 0.0, 150.0).unwrap()().unwrap();
        assert_eq!(outer.scroll_top(), 150);
        outer.remove();
    }

    /// Off any scrolling ancestor, the host scroller is the page itself.
    #[wasm_bindgen_test]
    fn the_host_scroller_of_a_root_in_the_page_is_the_document() {
        let doc = web_glue::dom::window().unwrap().document().unwrap();
        let root = div(&doc.body().unwrap(), "height:10px");
        let host = scroll_port(node(&root), ScrollTarget::Host).expect("the page");
        let port = host.port.unwrap();
        assert_eq!((port.x, port.y), (0.0, 0.0));
        root.remove();
    }

    /// The keyboard kind reads back from what the backend wrote, and the
    /// computed style reports a struck-through, bold text.
    #[wasm_bindgen_test]
    fn keyboard_and_computed_style_read_back() {
        let doc = web_glue::dom::window().unwrap().document().unwrap();
        let input = doc.create_element("input").unwrap();
        doc.body().unwrap().append_child(&input).unwrap();
        crate::primitives::text_input::update_keyboard(node(&input), KeyboardKind::Email);
        assert_eq!(keyboard_kind(node(&input)), Some(KeyboardKind::Email));
        crate::primitives::text_input::update_keyboard(node(&input), KeyboardKind::Default);
        assert_eq!(keyboard_kind(node(&input)), Some(KeyboardKind::Default));
        input.remove();

        let t = div(&doc.body().unwrap(), "text-decoration:line-through;font-weight:700;color:rgb(0,0,255)");
        let s = computed_style(node(&t)).unwrap();
        assert_eq!((s.line_through, s.underline), (Some(true), Some(false)));
        assert_eq!(s.font_weight, Some(700));
        assert_eq!(s.color, Some([0.0, 0.0, 1.0, 1.0]));
        t.remove();
    }
}
