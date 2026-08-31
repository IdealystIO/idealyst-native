//! `Element::Slider` — an `<input type="range">`. We map our f32
//! value range to the browser's via `min`/`max`/`step` attributes;
//! `step="any"` for continuous values.

use crate::WebBackend;
// `css_num`, not `f32: Display`: a bare `{}` on an f32 is what pulls core's
// ~12-15 KB flt2dec float formatter into every bundle. 3-decimal precision
// is ample for range-input attributes and values.
use css::css_num;
use std::rc::Rc;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use web_sys::Node;

pub(crate) fn create(
    b: &mut WebBackend,
    initial_value: f32,
    min: f32,
    max: f32,
    step: Option<f32>,
    on_change: Rc<dyn Fn(f32)>,
) -> Node {
    let input: web_sys::HtmlInputElement = b
        .doc
        .create_element("input")
        .expect("create_element input failed")
        .unchecked_into();
    input.set_type("range");
    let _ = input.set_attribute("min", &css_num(min).to_string());
    let _ = input.set_attribute("max", &css_num(max).to_string());
    if let Some(s) = step {
        let _ = input.set_attribute("step", &css_num(s).to_string());
    } else {
        // "any" enables continuous values in the browser.
        let _ = input.set_attribute("step", "any");
    }
    input.set_value(&css_num(initial_value).to_string());

    // Fire on every `input` event (continuous drag).
    let input_clone = input.clone();
    let closure = Closure::<dyn FnMut(web_sys::Event)>::new(move |_e: web_sys::Event| {
        // Parse the string value back to f32; bail on parse error
        // (shouldn't happen with a range input).
        if let Ok(v) = input_clone.value().parse::<f32>() {
            on_change(v);
        }
    });
    let id = b.node_id(&input.clone().unchecked_into::<Node>());
    b.track_listener(id, &input, "input", false, closure);
    input.unchecked_into::<Node>()
}

pub(crate) fn update_value(node: &Node, value: f32) {
    if let Ok(input) = node.clone().dyn_into::<web_sys::HtmlInputElement>() {
        let s = css_num(value).to_string();
        if input.value() != s {
            input.set_value(&s);
        }
    }
}
