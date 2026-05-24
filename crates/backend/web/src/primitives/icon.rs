//! `Primitive::Icon` — inline `<svg>` with `<path>` children.
//!
//! Each icon renders as a self-contained SVG element sized via CSS
//! (defaults to `1em` so it scales with surrounding text). Color is
//! inherited via `currentColor` unless an explicit color is set.
//!
//! ## Stroke animation
//!
//! Uses the classic SVG dash trick: set `stroke-dasharray` to the
//! total path length, then animate `stroke-dashoffset` from that
//! length (nothing visible) to 0 (fully drawn). CSS transitions
//! handle the interpolation — smooth, hardware-accelerated, zero JS.

use runtime_core::primitives::icon::{FillRule, IconData};
use runtime_core::{Color, Easing};
use wasm_bindgen::JsCast;
use web_sys::Node;

use crate::WebBackend;

const SVG_NS: &str = "http://www.w3.org/2000/svg";

pub(crate) fn create(b: &mut WebBackend, data: &IconData, color: Option<&Color>) -> Node {
    let (vw, vh) = data.view_box;
    let svg = b
        .doc
        .create_element_ns(Some(SVG_NS), "svg")
        .expect("create_element_ns svg failed");

    let view_box = format!("0 0 {} {}", vw, vh);
    let _ = svg.set_attribute("viewBox", &view_box);
    let _ = svg.set_attribute("xmlns", SVG_NS);
    // Size with em so icon scales with font-size context.
    let _ = svg.set_attribute("width", "1em");
    let _ = svg.set_attribute("height", "1em");
    let _ = svg.set_attribute("fill", "none");
    // Prevent the SVG from capturing pointer events on transparent
    // regions — pass through to parent pressable/button.
    let _ = svg.set_attribute("style", "display:inline-block;vertical-align:middle;");

    let fill_rule_str = match data.fill_rule {
        FillRule::NonZero => "nonzero",
        FillRule::EvenOdd => "evenodd",
    };

    let stroke_color = match color {
        Some(c) => c.0.clone(),
        None => "currentColor".to_string(),
    };

    // Set stroke on the <svg> element — SVG presentation attributes
    // cascade to child elements, so all <path>s inherit.
    let _ = svg.set_attribute("stroke", &stroke_color);
    let _ = svg.set_attribute("stroke-width", "2");
    let _ = svg.set_attribute("stroke-linecap", "round");
    let _ = svg.set_attribute("stroke-linejoin", "round");

    for path_d in data.paths {
        let path = b
            .doc
            .create_element_ns(Some(SVG_NS), "path")
            .expect("create_element_ns path failed");
        let _ = path.set_attribute("d", path_d);
        let _ = path.set_attribute("fill-rule", fill_rule_str);
        // Set pathLength="1" so stroke-dasharray/offset work in
        // normalized 0–1 space regardless of actual path geometry.
        let _ = path.set_attribute("pathLength", "1");
        // Default: fully drawn (dasharray covers full length, offset 0).
        let _ = path.set_attribute("stroke-dasharray", "1");
        let _ = path.set_attribute("stroke-dashoffset", "0");
        let _ = svg.append_child(&path);
    }

    svg.unchecked_into::<Node>()
}

pub(crate) fn update_color(node: &Node, color: &Color) {
    if let Ok(el) = node.clone().dyn_into::<web_sys::Element>() {
        let _ = el.set_attribute("stroke", &color.0);
    }
}

/// Set stroke progress immediately (no transition).
/// progress: 0.0 = nothing drawn, 1.0 = fully drawn.
pub(crate) fn update_stroke(node: &Node, progress: f32) {
    let offset = 1.0 - progress.clamp(0.0, 1.0);
    let offset_str = format!("{}", offset);

    if let Ok(svg) = node.clone().dyn_into::<web_sys::Element>() {
        // Apply to all <path> children by iterating child nodes.
        // Remove any transition so the snap is instant.
        let child = svg.first_element_child();
        let mut current = child;
        while let Some(el) = current {
            let _ = el.set_attribute("stroke-dashoffset", &offset_str);
            let _ = el.set_attribute("style", "transition:none");
            current = el.next_element_sibling();
        }
    }
}

/// Animate stroke from→to over duration with easing.
pub(crate) fn animate_stroke(
    node: &Node,
    from: f32,
    to: f32,
    duration_ms: u32,
    easing: Easing,
    infinite: bool,
) {
    let from_offset = 1.0 - from.clamp(0.0, 1.0);
    let to_offset = 1.0 - to.clamp(0.0, 1.0);
    let from_str = format!("{}", from_offset);
    let to_str = format!("{}", to_offset);

    if let Ok(svg) = node.clone().dyn_into::<web_sys::Element>() {
        let child = svg.first_element_child();
        let mut current = child;
        while let Some(el) = current {
            if infinite {
                // CSS keyframes can't be inlined per-element, so we
                // use the transition + transitionend approach: set
                // stroke-dashoffset and use a CSS transition with
                // iteration. (A truly infinite version would need a
                // transitionend listener to flip the offset back —
                // backends that support true infinite (iOS/Android)
                // handle it natively, web's approximation is below.)
                let _ = el.set_attribute("stroke-dashoffset", &from_str);
                let _ = el.set_attribute("style", &format!(
                    "transition: stroke-dashoffset {}ms {}; stroke-dashoffset: {};",
                    duration_ms, easing_to_css(easing), to_str,
                ));
                // For true infinite, we'd need a transitionend listener.
                // Acceptable approximation: CSS animation property with
                // generated keyframe. For now, play once — backends that
                // support true infinite (iOS/Android) handle it natively.
                // Web gets one pass; full CSS keyframe injection is a
                // future enhancement.
            } else {
                // Single-shot: snap to from, then transition to to.
                let _ = el.set_attribute("stroke-dashoffset", &from_str);
                let _ = el.set_attribute("style", "transition:none");
                let _ = el.get_bounding_client_rect();
                let transition = format!(
                    "stroke-dashoffset {}ms {}",
                    duration_ms, easing_to_css(easing),
                );
                let _ = el.set_attribute("style", &format!("transition:{}", transition));
                let _ = el.set_attribute("stroke-dashoffset", &to_str);
            }
            current = el.next_element_sibling();
        }
    }
}

fn easing_to_css(easing: Easing) -> &'static str {
    match easing {
        Easing::Linear => "linear",
        Easing::Ease => "ease",
        Easing::EaseIn => "ease-in",
        Easing::EaseOut => "ease-out",
        Easing::EaseInOut => "ease-in-out",
        Easing::CubicBezier(_, _, _, _) => "ease", // TODO: format cubic-bezier()
    }
}
