//! Platform-native render introspection for the web backend.
//!
//! Reads the **browser's resolved state** — `getComputedStyle` (the *used*
//! CSS values, after the cascade and the engine's own resolution) and
//! `getBoundingClientRect` (the laid-out geometry) — and normalizes it to the
//! canonical [`runtime_core::introspect`] schema. This is the deliberate
//! "don't trust our inline style" path: the values come from the layout/style
//! engine, not from the `StyleRules` the framework asked for, so a diff
//! against the macOS backend catches real divergence.
//!
//! Gated on `debug-stats` (a diagnostic surface; compiled out of production).

use wasm_bindgen::JsCast;
use web_sys::Element;

use runtime_core::introspect::{keys, collect_native_tree, NativeNode, NativeRect, NativeValue};

use crate::WebBackend;

impl WebBackend {
    /// Record a framework primitive root for the introspection boundary walk.
    /// Backs `Backend::note_introspection_root`.
    pub(crate) fn note_introspection_root_impl(&self, node: &web_sys::Node) {
        self.introspection_roots.add(node.as_ref());
    }

    /// Read the native render tree for `node`. Backs
    /// `Backend::introspect_native`.
    pub(crate) fn introspect_native_impl(&self, node: &web_sys::Node) -> Option<NativeNode> {
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
            .dyn_into::<web_sys::HtmlInputElement>()
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

    let style = match web_sys::window()
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
    if let Some(input) = el.dyn_ref::<web_sys::HtmlInputElement>() {
        node.set(keys::TEXT, Some(NativeValue::Text(input.value())));
        node.role = Some("text_input".to_string());
    } else if let Some(ta) = el.dyn_ref::<web_sys::HtmlTextAreaElement>() {
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
