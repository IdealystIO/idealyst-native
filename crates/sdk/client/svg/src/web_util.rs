//! Pure wasm32 DOM helpers for the web leg: intrinsic-size
//! introspection off the mounted wrapper. Kept as its own module so the DOM introspection
//! stays framework-free (pure `web_sys`, no scene/world types).

use std::any::Any;
use wasm_bindgen::JsCast;

/// Walk from the type-erased backend node (a `web_sys::Node` wrapper
/// `<div>`) to the first `<svg>` descendant and read its intrinsic
/// size. This is the whole body of `SvgOps::intrinsic_size` on web,
/// shared by the crate's web ops impls.
pub(crate) fn intrinsic_size_of_node(node: &dyn Any) -> Option<(f32, f32)> {
    let wrapper = node.downcast_ref::<web_sys::Node>()?;
    let wrapper_el: &web_sys::Element = wrapper.dyn_ref::<web_sys::Element>()?;
    // `querySelector` is enabled by web-sys's base `Element`
    // feature. Iterating `children()` would also work but needs
    // the `HtmlCollection` feature — overkill for "find the first
    // svg descendant".
    let svg = wrapper_el.query_selector("svg").ok().flatten()?;
    parse_svg_intrinsic_size(&svg)
}

/// Pull intrinsic dimensions off an `<svg>` element. Prefers viewBox
/// (the conventional way to declare logical extents) and falls back
/// to width/height attributes for simpler markup.
pub(crate) fn parse_svg_intrinsic_size(svg: &web_sys::Element) -> Option<(f32, f32)> {
    if let Some(vb) = svg.get_attribute("viewBox") {
        // `viewBox = "minX minY width height"`. Comma- or space-
        // separated per the SVG spec.
        let parts: Vec<&str> = vb
            .split(|c: char| c.is_whitespace() || c == ',')
            .filter(|s| !s.is_empty())
            .collect();
        if parts.len() == 4 {
            let w: f32 = parts[2].parse().ok()?;
            let h: f32 = parts[3].parse().ok()?;
            return Some((w, h));
        }
    }
    let w_attr = svg.get_attribute("width")?;
    let h_attr = svg.get_attribute("height")?;
    let w: f32 = strip_unit(&w_attr).parse().ok()?;
    let h: f32 = strip_unit(&h_attr).parse().ok()?;
    Some((w, h))
}

/// Drop a trailing unit suffix (`px`, `pt`, `%`, etc.) from an SVG
/// dimension attribute. Returns the leading numeric portion.
fn strip_unit(s: &str) -> &str {
    let end = s
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == '+'))
        .unwrap_or(s.len());
    &s[..end]
}
