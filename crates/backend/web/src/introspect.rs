//! Platform-native render introspection for the web backend.
//!
//! Reads the **browser's resolved state** — `getComputedStyle` (the *used*
//! CSS values, after the cascade and the engine's own resolution) and
//! `getBoundingClientRect` (the laid-out geometry) — and normalizes it to the
//! canonical [`runtime_shared::introspect`] schema. This is the deliberate
//! "don't trust our inline style" path: the values come from the layout/style
//! engine, not from the `StyleRules` the framework asked for, so a diff
//! against the macOS backend catches real divergence.
//!
//! NOT gated: this module is compiled into every web build, including release.
//! (It claimed a `debug-stats` gate for a long time and never had one.) The
//! runtime cost is nil — the only caller is the Robot bridge, which a release
//! build never starts — but the code is carried. `backend-linux` gates its
//! equivalent behind its `robot` feature; doing the same here means deciding
//! what a wasm release build should pay for a diagnostic it cannot serve.

use web_glue::JsCast;
use web_glue::dom::Element;

use runtime_shared::introspect::{keys, collect_native_tree, NativeNode, NativeRect, NativeValue};

use crate::WebBackend;

impl WebBackend {
    /// Record a framework primitive root for the introspection boundary walk.
    /// Backs `IntrospectionOps::note_introspection_root`.
    pub(crate) fn note_introspection_root_impl(&self, node: &web_glue::dom::Node) {
        self.introspection_roots.add(node.as_ref());
    }

    /// Read the native render tree for `node`. Backs
    /// `IntrospectionOps::introspect_native`.
    pub(crate) fn introspect_native_impl(&self, node: &web_glue::dom::Node) -> Option<NativeNode> {
        let _t = crate::phase_timer::PhaseTimer::start("introspect_native");
        let root: Element = node.clone().dyn_into::<Element>().ok()?;
        // Not connected = not laid out; surface as "no data yet" (bridge → null).
        if !root.is_connected() {
            return None;
        }
        Some(collect_native_tree(
            &root,
            &read_element,
            &child_elements,
            &|el| self.is_framework_root(el),
        ))
    }

    /// A descendant element is a **framework element boundary** when it's a
    /// registered primitive root (identity-present in `introspection_roots`).
    /// The walk stops there — that element is introspected as its own node.
    fn is_framework_root(&self, el: &Element) -> bool {
        self.introspection_roots.has(el.as_ref())
    }
}

/// Direct **element** children (skips text/comment nodes — those surface as
/// the parent's `text` prop instead). Walks the element-sibling chain rather
/// than `children()` to stay within the crate's enabled `web-sys` surface.
fn child_elements(el: &Element) -> Vec<Element> {
    let mut out = Vec::new();
    let mut cur = el.first_element_child();
    while let Some(c) = cur {
        cur = c.next_element_sibling();
        out.push(c);
    }
    out
}

/// Shallow read of one element: tag, frame, and canonical resolved props from
/// `getComputedStyle`.
fn read_element(el: &Element) -> NativeNode {
    // Tag name, lowercased to match author/CSS convention (`div`, not `DIV`),
    // with the input subtype appended so `<input type=text>` is distinct.
    let mut class = el.tag_name().to_lowercase();
    if class == "input" {
        if let Ok(t) = el
            .clone()
            .dyn_into::<web_glue::dom::HtmlInputElement>()
            .map(|i| i.type_())
        {
            class = format!("input[{t}]");
        }
    }
    let r = el.get_bounding_client_rect();
    let frame = NativeRect {
        x: r.x() as f32,
        y: r.y() as f32,
        width: r.width() as f32,
        height: r.height() as f32,
    };
    let mut node = NativeNode::leaf(class, frame);

    let style = match web_glue::dom::window()
        .and_then(|w| w.get_computed_style(el).ok().flatten())
    {
        Some(s) => s,
        None => return node,
    };
    let get = |p: &str| style.get_property_value(p).unwrap_or_default();

    node.set(keys::BACKGROUND_COLOR, parse_color(&get("background-color")).map(NativeValue::Color));
    node.set(keys::TEXT_COLOR, parse_color(&get("color")).map(NativeValue::Color));

    if let Some(o) = parse_f32(&get("opacity")) {
        node.set(keys::OPACITY, Some(NativeValue::Number(o)));
    }
    if let Some(radius) = parse_px(&get("border-top-left-radius")) {
        if radius > 0.0 {
            node.set(keys::CORNER_RADIUS, Some(NativeValue::Length(radius)));
        }
    }
    if let Some(bw) = parse_px(&get("border-top-width")) {
        if bw > 0.0 {
            node.set(keys::BORDER_WIDTH, Some(NativeValue::Length(bw)));
            node.set(keys::BORDER_COLOR, parse_color(&get("border-top-color")).map(NativeValue::Color));
            let style = get("border-top-style");
            if !style.is_empty() {
                node.set(keys::BORDER_STYLE, Some(NativeValue::Text(style)));
            }
        }
    }

    // Hidden: the engine's own visibility verdict, not our style intent.
    let hidden = get("display") == "none" || get("visibility") == "hidden";
    node.set(keys::HIDDEN, Some(NativeValue::Flag(hidden)));

    // Font longhands (computed → numeric/explicit).
    let family = first_font_family(&get("font-family"));
    if !family.is_empty() {
        node.set(keys::FONT_FAMILY, Some(NativeValue::Text(family)));
    }
    if let Some(fs) = parse_px(&get("font-size")) {
        node.set(keys::FONT_SIZE, Some(NativeValue::Length(fs)));
    }
    node.set(keys::FONT_WEIGHT, Some(NativeValue::Number(parse_font_weight(&get("font-weight")))));

    // Shadow (approximate: first length = blur radius, trailing color).
    let shadow = get("box-shadow");
    if !shadow.is_empty() && shadow != "none" {
        if let Some(c) = parse_color(&shadow) {
            node.set(keys::SHADOW_COLOR, Some(NativeValue::Color(c)));
        }
        if let Some(radius) = shadow_blur_radius(&shadow) {
            node.set(keys::SHADOW_RADIUS, Some(NativeValue::Length(radius)));
        }
    }

    // Displayed text. Inputs/textareas report `.value`; otherwise, only leaf
    // elements (no element children) report `textContent` so we don't repeat
    // a container's text on every ancestor.
    if let Some(input) = el.dyn_ref::<web_glue::dom::HtmlInputElement>() {
        node.set(keys::TEXT, Some(NativeValue::Text(input.value())));
        node.role = Some("text_input".to_string());
    } else if let Some(ta) = el.dyn_ref::<web_glue::dom::HtmlTextAreaElement>() {
        node.set(keys::TEXT, Some(NativeValue::Text(ta.value())));
        node.role = Some("text_input".to_string());
    } else if el.first_element_child().is_none() {
        if let Some(t) = el.text_content() {
            let t = t.trim();
            if !t.is_empty() {
                node.set(keys::TEXT, Some(NativeValue::Text(t.to_string())));
            }
        }
    }

    node
}

/// Parse a computed CSS color (`rgb(r, g, b)` / `rgba(r, g, b, a)`) into
/// straight sRGB RGBA `0..1`. Returns `None` for `transparent`, keyword
/// `none`, or anything not in the computed `rgb[a]()` form (e.g. a gradient,
/// which `background-color` never is). The browser always normalizes a
/// resolved color to this form, so we don't reimplement the CSS color parser.
fn parse_color(s: &str) -> Option<[f32; 4]> {
    let s = s.trim();
    let inner = s.strip_prefix("rgba(").or_else(|| s.strip_prefix("rgb("))?;
    let inner = inner.strip_suffix(')')?;
    // Components are comma- (legacy) or space/slash- (modern) separated.
    let parts: Vec<f32> = inner
        .split(|c| c == ',' || c == '/' || c == ' ')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .filter_map(|p| p.trim_end_matches('%').parse::<f32>().ok())
        .collect();
    if parts.len() < 3 {
        return None;
    }
    let a = parts.get(3).copied().unwrap_or(1.0);
    Some([parts[0] / 255.0, parts[1] / 255.0, parts[2] / 255.0, a])
}

/// Parse a `<length>` in px (computed styles resolve all lengths to px).
fn parse_px(s: &str) -> Option<f32> {
    s.trim().strip_suffix("px")?.trim().parse::<f32>().ok()
}

fn parse_f32(s: &str) -> Option<f32> {
    s.trim().parse::<f32>().ok()
}

/// Computed `font-weight` is numeric (`"400"`), but guard the keyword forms.
fn parse_font_weight(s: &str) -> f32 {
    match s.trim() {
        "normal" => 400.0,
        "bold" => 700.0,
        other => other.parse::<f32>().unwrap_or(400.0),
    }
}

/// First family in a computed `font-family` list, quotes stripped.
fn first_font_family(s: &str) -> String {
    s.split(',')
        .next()
        .unwrap_or("")
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .to_string()
}

/// Pull the blur radius out of a `box-shadow` string. The radius is the third
/// length token (offset-x, offset-y, blur, [spread]); approximate — multiple
/// shadows just read the first.
fn shadow_blur_radius(s: &str) -> Option<f32> {
    let lengths: Vec<f32> = s
        .split_whitespace()
        .filter_map(parse_px)
        .collect();
    lengths.get(2).copied()
}


web_glue::import! {
    /// 1 shown / 0 not / 2 can't tell (not an element). `checkVisibility`
    /// is the browser's own answer — `display: none` on it or an ancestor,
    /// `content-visibility`, and (asked for) `visibility` and `opacity: 0`;
    /// browsers without it fall back on "renders any box".
    fn js_is_shown(n: u32) -> u32 =
        "(n) => { const e = G.get(n); if (!e.isConnected) return 0; if (e.nodeType !== 1) return 2; \
           if (typeof e.checkVisibility === 'function') \
             return e.checkVisibility({ opacityProperty: true, visibilityProperty: true }) ? 1 : 0; \
           return e.getClientRects().length ? 1 : 0; }";
}

/// Whether the browser shows `node` (see `IntrospectionOps::is_shown`):
/// connected to the document, and not hidden by its own or an ancestor's
/// `display` / `visibility` / `opacity`.
pub(crate) fn is_shown(node: &web_glue::dom::Node) -> Option<bool> {
    let raw = AsRef::<web_glue::JsValue>::as_ref(node).raw();
    match unsafe { js_is_shown(raw) } {
        1 => Some(true),
        0 => Some(false),
        _ => None,
    }
}

/// Whether `node` is the document's focused element (see
/// `IntrospectionOps::is_focused`). Read from `document.activeElement`, not
/// from `focus`/`blur` events: Chrome moves `activeElement` on
/// `element.focus()` in a window that lacks OS focus (headless
/// `idealyst test --web`, a background window) but defers the `focus` event
/// until the window is focused — which a headless run never is.
pub(crate) fn is_focused(doc: &web_glue::dom::Document, node: &web_glue::dom::Node) -> bool {
    doc.active_element().is_some_and(|a| node.is_same_node(Some(a.as_ref())))
}

web_glue::import! {
    /// The computed `overflow` of `n`: 0 visible, 1 cut (`hidden` /
    /// `clip`), 2 scrolls (`auto` / `scroll` on either axis — CSS turns the
    /// other axis's `visible` into `auto` then), 3 can't tell (not an
    /// element).
    fn js_content_clip(n: u32) -> u32 =
        "(n) => { const e = G.get(n); if (!e || e.nodeType !== 1) return 3; \
           const s = getComputedStyle(e); const v = [s.overflowX, s.overflowY]; \
           if (v.some(o => o === 'auto' || o === 'scroll')) return 2; \
           return v.some(o => o === 'hidden' || o === 'clip') ? 1 : 0; }";
}

/// How the browser clips `node`'s descendants (see
/// `IntrospectionOps::content_clip`) — its computed `overflow`.
pub(crate) fn content_clip(node: &web_glue::dom::Node) -> Option<runtime_vocabulary::caps::ContentClip> {
    use runtime_vocabulary::caps::ContentClip;
    let raw = AsRef::<web_glue::JsValue>::as_ref(node).raw();
    match unsafe { js_content_clip(raw) } {
        0 => Some(ContentClip::Visible),
        1 => Some(ContentClip::Hidden),
        2 => Some(ContentClip::Scroll),
        _ => None,
    }
}

web_glue::import! {
    /// 1 when the topmost element at the viewport point `(x, y)` is `n`
    /// or inside it (`elementFromPoint` honors `pointer-events`), else 0;
    /// 2 can't tell (`n` is not an element).
    fn js_hit_contains(n: u32, x: f64, y: f64) -> u32 =
        "(n, x, y) => { const e = G.get(n); if (!e || e.nodeType !== 1) return 2; \
           const t = document.elementFromPoint(x, y); return t && (t === e || e.contains(t)) ? 1 : 0; }";

    /// One step of a synthesized primary-pointer gesture at the viewport
    /// point `(x, y)`: `phase` 0 down, 1 move, 2 up, 3 hover. Returns 0
    /// when there is nothing at the down (or hover) point.
    ///
    /// Dispatched as a browser dispatches a touch: `pointerdown` +
    /// `mousedown` on the topmost element there, moves and the up to that
    /// same element (a touch's implicit pointer capture — untrusted events
    /// can't take explicit capture), then `click` on the nearest common
    /// ancestor of the down and up targets. An untrusted `mousedown` runs
    /// no default action, so the focus a press gives a focusable target is
    /// given here, unless a listener prevented it.
    ///
    /// On a page with touch input (`navigator.maxTouchPoints > 0` — a phone,
    /// or `idealyst test --device`'s touch emulation) the gesture is also a
    /// touch, in the order a mobile browser sends one: `pointerdown` +
    /// `touchstart` at the down, `pointermove` + `touchmove` per move (no
    /// mouse events), then `pointerup` + `touchend`, and only after those the
    /// compatibility `mousedown`/focus/`mouseup` and the `click` — none of
    /// which fire when a `touchstart` or `touchend` listener prevented the
    /// default, as a browser suppresses them. Every touch event targets the
    /// down element, as a real touch stays with the element it started on.
    ///
    /// A hover is a mouse with no button held: `pointerout`/`mouseout` on
    /// the last hovered element, `pointerleave`/`mouseleave` on each of its
    /// ancestors the pointer left, `pointerover`/`mouseover` on the new
    /// target, `pointerenter`/`mouseenter` on each ancestor it entered,
    /// then `pointermove`/`mousemove` — the order a browser sends. Untrusted
    /// events do not move CSS `:hover`; only real input does.
    fn js_pointer_step(phase: u32, x: f64, y: f64) -> u32 =
        "(phase, x, y) => { \
           const s = (globalThis.__idealystNativePointer ||= { down: null }); \
           const o = { bubbles: true, cancelable: true, composed: true, clientX: x, clientY: y, \
                       button: 0, buttons: phase === 2 ? 0 : 1, pointerId: 1, pointerType: 'touch', \
                       isPrimary: true, view: window, detail: 1 }; \
           if (phase === 3) { \
             const t = document.elementFromPoint(x, y); if (!t) return 0; \
             const m = { ...o, buttons: 0, pointerType: 'mouse' }; \
             const prev = s.hover && s.hover.isConnected ? s.hover : null; s.hover = t; \
             const chain = (e) => { const a = []; for (; e; e = e.parentElement) a.push(e); return a; }; \
             const was = chain(prev), now = chain(t); \
             if (prev !== t) { \
               if (prev) { prev.dispatchEvent(new PointerEvent('pointerout', m)); prev.dispatchEvent(new MouseEvent('mouseout', m)); } \
               for (const e of was) if (!now.includes(e)) { e.dispatchEvent(new PointerEvent('pointerleave', { ...m, bubbles: false })); e.dispatchEvent(new MouseEvent('mouseleave', { ...m, bubbles: false })); } \
               t.dispatchEvent(new PointerEvent('pointerover', m)); t.dispatchEvent(new MouseEvent('mouseover', m)); \
               for (const e of now.slice().reverse()) if (!was.includes(e)) { e.dispatchEvent(new PointerEvent('pointerenter', { ...m, bubbles: false })); e.dispatchEvent(new MouseEvent('mouseenter', { ...m, bubbles: false })); } } \
             t.dispatchEvent(new PointerEvent('pointermove', m)); t.dispatchEvent(new MouseEvent('mousemove', m)); \
             return 1; } \
           const touch = navigator.maxTouchPoints > 0 && typeof Touch === 'function' && typeof TouchEvent === 'function'; \
           const press = (t) => { \
             if (t.dispatchEvent(new MouseEvent('mousedown', { ...o, buttons: 1 }))) { \
               const f = t.closest('input, textarea, select, button, a[href], [tabindex], [contenteditable]'); \
               if (f && typeof f.focus === 'function') f.focus(); } }; \
           const fire = (t, type, lifted) => { \
             const p = new Touch({ identifier: 1, target: t, clientX: x, clientY: y, pageX: x + scrollX, pageY: y + scrollY, \
                                   screenX: x, screenY: y, radiusX: 1, radiusY: 1, force: lifted ? 0 : 1 }); \
             const now = lifted ? [] : [p]; \
             return t.dispatchEvent(new TouchEvent(type, { bubbles: true, cancelable: true, composed: true, view: window, \
                                                            touches: now, targetTouches: now, changedTouches: [p] })); }; \
           if (phase === 0) { \
             const t = document.elementFromPoint(x, y); if (!t) return 0; s.down = t; s.cancelled = false; \
             t.dispatchEvent(new PointerEvent('pointerdown', o)); \
             if (touch) { s.cancelled = !fire(t, 'touchstart', false); return 1; } \
             press(t); \
             return 1; } \
           const t = s.down || document.elementFromPoint(x, y); if (!t) return 0; \
           if (phase === 1) { t.dispatchEvent(new PointerEvent('pointermove', o)); \
             if (touch) fire(t, 'touchmove', false); else t.dispatchEvent(new MouseEvent('mousemove', o)); return 1; } \
           t.dispatchEvent(new PointerEvent('pointerup', o)); \
           if (touch) { if (!fire(t, 'touchend', true)) s.cancelled = true; if (s.cancelled) { s.down = null; return 1; } press(t); } \
           t.dispatchEvent(new MouseEvent('mouseup', o)); \
           let c = t; const at = document.elementFromPoint(x, y); \
           while (c && at && !c.contains(at)) c = c.parentElement; \
           s.down = null; \
           if (c && at) c.dispatchEvent(new MouseEvent('click', o)); \
           return 1; }";
}

/// See `IntrospectionOps::hit_contains`: the browser's `elementFromPoint`.
pub(crate) fn hit_contains(node: &web_glue::dom::Node, x: f32, y: f32) -> Option<bool> {
    let raw = AsRef::<web_glue::JsValue>::as_ref(node).raw();
    match unsafe { js_hit_contains(raw, x as f64, y as f64) } {
        1 => Some(true),
        0 => Some(false),
        _ => None,
    }
}

/// See `IntrospectionOps::native_pointer`: DOM pointer + mouse events,
/// dispatched synchronously when the injection runs.
pub(crate) fn native_pointer(
    steps: &[runtime_vocabulary::caps::PointerStep],
) -> runtime_vocabulary::caps::NativeInjection {
    use runtime_vocabulary::caps::PointerStepPhase;
    let steps = steps.to_vec();
    Box::new(move || {
        for s in steps {
            let phase = match s.phase {
                PointerStepPhase::Down => 0,
                PointerStepPhase::Move => 1,
                PointerStepPhase::Up => 2,
                PointerStepPhase::Hover => 3,
            };
            if unsafe { js_pointer_step(phase, s.x as f64, s.y as f64) } == 0 {
                return Err(format!("nothing in the page at ({}, {})", s.x, s.y));
            }
        }
        Ok(())
    })
}

#[cfg(all(test, target_arch = "wasm32"))]
mod is_shown_tests {
    use wasm_bindgen_test::*;

    fn el(parent: &web_glue::dom::Element, style: &str) -> web_glue::dom::Element {
        let doc = web_glue::dom::window().unwrap().document().unwrap();
        let e = doc.create_element("div").unwrap();
        e.set_attribute("style", &format!("width:20px;height:20px;{style}")).unwrap();
        parent.append_child(&e).unwrap();
        e
    }

    fn shown(e: &web_glue::dom::Element) -> Option<bool> {
        let node: &web_glue::dom::Node = e.as_ref();
        super::is_shown(node)
    }

    /// The robot's `visible` on web is the browser's own `checkVisibility`:
    /// hidden by the element's or an ancestor's `display` / `visibility` /
    /// `opacity`, or not in the document, is not shown.
    #[wasm_bindgen_test]
    fn is_shown_is_what_the_browser_renders() {
        let doc = web_glue::dom::window().unwrap().document().unwrap();
        let root = doc.create_element("div").unwrap();
        doc.body().unwrap().append_child(&root).unwrap();

        assert_eq!(shown(&el(&root, "")), Some(true));
        assert_eq!(shown(&el(&root, "display:none")), Some(false));
        assert_eq!(shown(&el(&root, "visibility:hidden")), Some(false));
        assert_eq!(shown(&el(&root, "opacity:0")), Some(false));
        let hidden_parent = el(&root, "display:none");
        assert_eq!(shown(&el(&hidden_parent, "")), Some(false), "an ancestor hides it");
        let detached = doc.create_element("div").unwrap();
        assert_eq!(shown(&detached), Some(false), "not in the document");
        root.remove();
    }

    /// The robot's clipping read on web is the computed `overflow`: what
    /// cuts a descendant's visible box, and whether scrolling reveals it.
    #[wasm_bindgen_test]
    fn content_clip_is_the_computed_overflow() {
        use runtime_vocabulary::caps::ContentClip;
        let doc = web_glue::dom::window().unwrap().document().unwrap();
        let root = doc.create_element("div").unwrap();
        doc.body().unwrap().append_child(&root).unwrap();
        let clip = |e: &web_glue::dom::Element| super::content_clip(e.as_ref());

        assert_eq!(clip(&el(&root, "")), Some(ContentClip::Visible));
        assert_eq!(clip(&el(&root, "overflow:hidden")), Some(ContentClip::Hidden));
        assert_eq!(clip(&el(&root, "overflow:clip")), Some(ContentClip::Hidden));
        assert_eq!(clip(&el(&root, "overflow:auto")), Some(ContentClip::Scroll));
        assert_eq!(clip(&el(&root, "overflow-x:hidden;overflow-y:scroll")), Some(ContentClip::Scroll));
        root.remove();
    }

    web_glue::import! {
        fn js_is_active(n: u32) -> u32 = "(n) => document.activeElement === G.get(n) ? 1 : 0";
    }

    /// The robot's native tap on web: the page's own listeners receive
    /// pointerdown / pointerup / click at the point, a focusable target
    /// takes focus, and an element on top fails the hit test (and would
    /// take the events instead).
    #[wasm_bindgen_test]
    fn a_native_tap_reaches_the_page_s_listeners_and_respects_what_is_on_top() {
        use runtime_vocabulary::caps::{PointerStep, PointerStepPhase};
        use std::cell::RefCell;
        use std::rc::Rc;

        let doc = web_glue::dom::window().unwrap().document().unwrap();
        let root = doc.create_element("div").unwrap();
        root.set_attribute("style", "position:fixed;left:0;top:0;width:200px;height:200px").unwrap();
        doc.body().unwrap().append_child(&root).unwrap();
        let target = doc.create_element("input").unwrap();
        target.set_attribute("style", "position:absolute;left:10px;top:10px;width:80px;height:30px").unwrap();
        root.append_child(&target).unwrap();

        let seen: Rc<RefCell<Vec<&'static str>>> = Rc::default();
        let mut keep = Vec::new();
        for kind in ["pointerdown", "pointerup", "click"] {
            let seen = seen.clone();
            let cb = web_glue::Closure::new(move |_| seen.borrow_mut().push(kind));
            let t: &web_glue::dom::EventTarget = target.as_ref();
            t.add_event_listener(kind, &cb, web_glue::dom::ListenerOptions::default());
            keep.push(cb);
        }
        let node: &web_glue::dom::Node = target.as_ref();
        assert_eq!(super::hit_contains(node, 50.0, 25.0), Some(true));
        let tap = [
            PointerStep { phase: PointerStepPhase::Down, x: 50.0, y: 25.0 },
            PointerStep { phase: PointerStepPhase::Up, x: 50.0, y: 25.0 },
        ];
        super::native_pointer(&tap)().unwrap();
        assert_eq!(seen.borrow().as_slice(), ["pointerdown", "pointerup", "click"]);
        let raw = AsRef::<web_glue::JsValue>::as_ref(node).raw();
        assert_eq!(unsafe { js_is_active(raw) }, 1, "a tapped input takes focus");

        let cover = doc.create_element("div").unwrap();
        cover.set_attribute("style", "position:absolute;left:0;top:0;width:200px;height:200px").unwrap();
        root.append_child(&cover).unwrap();
        assert_eq!(super::hit_contains(node, 50.0, 25.0), Some(false), "the cover takes the point");
        cover.set_attribute("style", "position:absolute;left:0;top:0;width:200px;height:200px;pointer-events:none").unwrap();
        assert_eq!(super::hit_contains(node, 50.0, 25.0), Some(true), "pointer-events: none lets it through");
        root.remove();
    }

    web_glue::import! {
        /// Give the page `on` touch points (what `idealyst test --device`'s
        /// touch emulation reports); 0 removes the override.
        fn js_set_touch_points(on: u32) =
            "(on) => { if (on) Object.defineProperty(navigator, 'maxTouchPoints', { get: () => on, configurable: true }); \
               else delete navigator.maxTouchPoints; }";
    }

    fn record(target: &web_glue::dom::Element, kinds: &[&'static str]) -> (std::rc::Rc<std::cell::RefCell<Vec<&'static str>>>, Vec<web_glue::Closure>) {
        let seen: std::rc::Rc<std::cell::RefCell<Vec<&'static str>>> = Default::default();
        let mut keep = Vec::new();
        for &kind in kinds {
            let seen = seen.clone();
            let cb = web_glue::Closure::new(move |_| seen.borrow_mut().push(kind));
            let t: &web_glue::dom::EventTarget = target.as_ref();
            t.add_event_listener(kind, &cb, web_glue::dom::ListenerOptions::default());
            keep.push(cb);
        }
        (seen, keep)
    }

    /// On a touch page (phone emulation in `idealyst test --web --device`)
    /// a native tap is a touch as a mobile browser delivers it: pointer and
    /// touch events first, the compatibility mouse events and the click
    /// after the finger lifts. It used to be mouse events only, so a suite
    /// at a phone size never exercised the app's touch path.
    #[wasm_bindgen_test]
    fn regression_a_native_tap_on_a_touch_page_sends_touch_events_then_the_click() {
        use runtime_vocabulary::caps::{PointerStep, PointerStepPhase};
        let doc = web_glue::dom::window().unwrap().document().unwrap();
        let root = doc.create_element("div").unwrap();
        root.set_attribute("style", "position:fixed;left:0;top:0;width:200px;height:200px").unwrap();
        doc.body().unwrap().append_child(&root).unwrap();
        let target = doc.create_element("div").unwrap();
        target.set_attribute("style", "position:absolute;left:10px;top:10px;width:80px;height:30px").unwrap();
        root.append_child(&target).unwrap();
        let kinds = ["pointerdown", "touchstart", "touchmove", "pointerup", "touchend", "mousedown", "mousemove", "mouseup", "click"];
        let (seen, _keep) = record(&target, &kinds);

        unsafe { js_set_touch_points(5) };
        let tap = [
            PointerStep { phase: PointerStepPhase::Down, x: 50.0, y: 25.0 },
            PointerStep { phase: PointerStepPhase::Move, x: 51.0, y: 25.0 },
            PointerStep { phase: PointerStepPhase::Up, x: 51.0, y: 25.0 },
        ];
        let r = super::native_pointer(&tap)();
        unsafe { js_set_touch_points(0) };
        r.unwrap();
        assert_eq!(
            seen.borrow().as_slice(),
            ["pointerdown", "touchstart", "touchmove", "pointerup", "touchend", "mousedown", "mouseup", "click"]
        );
        root.remove();
    }

    /// A `touchstart` listener that prevents the default suppresses the
    /// compatibility mouse events and the click, as in a mobile browser.
    #[wasm_bindgen_test]
    fn a_prevented_touchstart_suppresses_the_mouse_events_and_click() {
        use runtime_vocabulary::caps::{PointerStep, PointerStepPhase};
        let doc = web_glue::dom::window().unwrap().document().unwrap();
        let root = doc.create_element("div").unwrap();
        root.set_attribute("style", "position:fixed;left:0;top:0;width:200px;height:200px").unwrap();
        doc.body().unwrap().append_child(&root).unwrap();
        let target = doc.create_element("div").unwrap();
        target.set_attribute("style", "position:absolute;left:10px;top:10px;width:80px;height:30px").unwrap();
        root.append_child(&target).unwrap();
        let prevent = web_glue::Closure::new(|ev| {
            web_glue::JsCast::unchecked_into::<web_glue::dom::Event>(ev).prevent_default()
        });
        let t: &web_glue::dom::EventTarget = target.as_ref();
        t.add_event_listener("touchstart", &prevent, web_glue::dom::ListenerOptions::default());
        let (seen, _keep) = record(&target, &["touchend", "mousedown", "mouseup", "click"]);

        unsafe { js_set_touch_points(5) };
        let tap = [
            PointerStep { phase: PointerStepPhase::Down, x: 50.0, y: 25.0 },
            PointerStep { phase: PointerStepPhase::Up, x: 50.0, y: 25.0 },
        ];
        let r = super::native_pointer(&tap)();
        unsafe { js_set_touch_points(0) };
        r.unwrap();
        assert_eq!(seen.borrow().as_slice(), ["touchend"]);
        root.remove();
    }
}
