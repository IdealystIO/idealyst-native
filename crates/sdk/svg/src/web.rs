//! Web (`target_arch = "wasm32"`) implementation of the SVG SDK.
//!
//! Inline-renders the markup into a wrapper `<div>` via `innerHTML`.
//! The browser is itself an SVG renderer — we lean on that rather
//! than carrying a Rust SVG renderer into the wasm bundle.
//!
//! # Why a wrapper div instead of returning an `<svg>` directly?
//!
//! We don't know the inner SVG's root tag arity ahead of time, and
//! re-assigning `outerHTML` of a root element loses its identity
//! (the framework keeps a reference to the original node for layout
//! + reactive ops). A stable wrapper `<div>` with `innerHTML` swapped
//! on every Effect tick keeps node identity stable and gives us a
//! single element to apply Taffy frames to.
//!
//! # Intrinsic size
//!
//! Read directly off the parsed root `<svg>` element after each
//! render — `getAttribute("viewBox")` if present, falling back to
//! `width`/`height` attributes. The handle ops walks the DOM at call
//! time, so the value is always current.

use crate::{SvgOps, SvgProps};
use backend_web::WebBackend;
use runtime_core::Effect;
use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::JsCast;

pub(crate) static OPS: &dyn SvgOps = &WebSvgOps;

/// Register the SVG handler against a `WebBackend`. One-line call
/// from app bootstrap.
pub fn register(backend: &mut WebBackend) {
    backend.register_external::<SvgProps, _>(|props, _backend| build_svg(props));
}

fn build_svg(props: &Rc<SvgProps>) -> web_sys::Element {
    let document = web_sys::window()
        .expect("no window")
        .document()
        .expect("no document");
    // A bare wrapper — author-provided style classes drive size. The
    // wrapper's purpose is to give the framework a stable element to
    // attach Taffy frames + classes to while we swap `innerHTML`
    // freely for reactive markup updates.
    let wrapper = document
        .create_element("div")
        .expect("create_element(div) failed");
    let _ = wrapper.set_attribute("data-external-kind", "svg::SvgProps");
    // Default: zero margin/padding so the inner SVG fills the wrapper.
    // `display: contents` would also work but reads as too clever;
    // `display: inline-block` keeps the SVG sized like an image.
    let _ = wrapper.set_attribute("style", "display: inline-block; line-height: 0");

    // Cache the last applied markup so the Effect skips no-op re-runs.
    // Reactive sources frequently re-emit identical values; setting
    // `innerHTML` to the same string still tears down + rebuilds DOM
    // children, which is wasteful and resets any browser animation
    // state inside the SVG.
    let last = Rc::new(RefCell::new(String::new()));

    let wrapper_for_effect = wrapper.clone();
    let props_for_effect = props.clone();
    let last_for_effect = last.clone();
    let _effect = Effect::new(move || {
        let markup = (props_for_effect.markup)();
        {
            let cached = last_for_effect.borrow();
            if *cached == markup {
                return;
            }
        }
        wrapper_for_effect.set_inner_html(&markup);
        *last_for_effect.borrow_mut() = markup;
        // `on_error` doesn't fire on web: `Element::set_inner_html`
        // for SVG fragments is silently recoverable in every modern
        // browser. Surfacing parse errors would need a JS-side
        // try/catch + DOMException decode, which doesn't justify its
        // weight for the failure modes that actually occur in
        // practice (none — browsers accept malformed SVG and render
        // partial trees).
        if let Some(cb) = &props_for_effect.on_load {
            cb();
        }
    });

    wrapper
}

// ============================================================================
// Imperative ops
// ============================================================================

struct WebSvgOps;

impl SvgOps for WebSvgOps {
    fn intrinsic_size(&self, node: &dyn Any) -> Option<(f32, f32)> {
        let wrapper = node.downcast_ref::<web_sys::Node>()?;
        let wrapper_el: &web_sys::Element = wrapper.dyn_ref::<web_sys::Element>()?;
        // `querySelector` is enabled by web-sys's base `Element`
        // feature. Iterating `children()` would also work but needs
        // the `HtmlCollection` feature — overkill for "find the first
        // svg descendant".
        let svg = wrapper_el.query_selector("svg").ok().flatten()?;
        parse_svg_intrinsic_size(&svg)
    }
}

/// Pull intrinsic dimensions off an `<svg>` element. Prefers viewBox
/// (the conventional way to declare logical extents) and falls back
/// to width/height attributes for simpler markup.
fn parse_svg_intrinsic_size(svg: &web_sys::Element) -> Option<(f32, f32)> {
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
