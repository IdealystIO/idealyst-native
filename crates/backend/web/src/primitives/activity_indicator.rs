//! `Element::ActivityIndicator` — a `<span>` with a CSS-animated
//! ring. Keyframes are injected once on first creation by
//! `ensure_spinner_keyframes`.

use crate::WebBackend;
use runtime_core::primitives::activity_indicator::ActivityIndicatorSize;
use wasm_bindgen::JsCast;
use web_sys::Node;

pub(crate) fn create(
    b: &mut WebBackend,
    size: ActivityIndicatorSize,
    color: Option<&runtime_core::Color>,
) -> Node {
    // Inject the keyframes rule once. Subsequent creations reuse the
    // same rule by checking a flag on the backend.
    b.ensure_spinner_keyframes();

    // Hydration adoption — see `text_input::create` for the rationale.
    let span = if let Some(el) = b.hydrate_next("span") {
        el
    } else {
        let fresh = b
            .doc
            .create_element("span")
            .expect("create_element span failed");
        let node: Node = fresh.clone().unchecked_into();
        b.hydrate_note_fresh(&node);
        fresh
    };
    let diameter = match size {
        ActivityIndicatorSize::Small => 16,
        ActivityIndicatorSize::Large => 36,
    };
    let accent = color.map(|c| c.0.as_str()).unwrap_or("currentColor");
    // Inline style: thin ring, accent on top, animated rotation.
    // Authors can override via .with_style(...) — these are just
    // defaults so the spinner renders meaningfully without one.
    let style = format!(
        "display: inline-block; width: {d}px; height: {d}px; \
         border: 2px solid transparent; border-top-color: {c}; \
         border-radius: 50%; animation: ui-spin 0.8s linear infinite",
        d = diameter,
        c = accent
    );
    let _ = span.set_attribute("style", &style);
    span.unchecked_into::<Node>()
}
