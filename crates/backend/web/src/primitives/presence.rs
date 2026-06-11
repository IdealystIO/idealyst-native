//! `Element::Presence` — web-side apply_presence implementation.
//!
//! Maps a `PresenceState` (opacity + 2D translate + uniform scale)
//! to CSS via inline `style` properties on the node:
//!
//! - `opacity` → `style.opacity`
//! - `translate_x / translate_y / scale` → composed into a single
//!   `style.transform` string. We always emit a complete transform
//!   string so missing fields snap back to identity rather than
//!   inheriting an old value.
//!
//! Transitions are CSS-driven: when `apply_presence` is called with
//! `Some((duration_ms, easing))`, we set `style.transition` to a
//! shorthand covering `opacity` and `transform`. When the next
//! `apply_presence` snaps (None), we clear the transition first so
//! the snap is instant.
//!
//! This sits *parallel* to the regular style system — we touch
//! inline style props that the stylesheet system doesn't write
//! (`opacity`, `transform`, `transition`), so the two don't fight.
//! If an author also sets opacity or transform through a regular
//! stylesheet, the inline `style` value wins (matching CSS
//! specificity), so presence overrides cleanly.

use runtime_core::primitives::presence::PresenceState;
use runtime_core::Easing;
use wasm_bindgen::JsCast;
use web_sys::Node;

use crate::WebBackend;

pub(crate) fn apply(
    _b: &mut WebBackend,
    node: &Node,
    state: PresenceState,
    transition: Option<(u32, Easing)>,
) {
    let el = match node.dyn_ref::<web_sys::HtmlElement>() {
        Some(e) => e,
        None => return,
    };
    let style = el.style();

    // The transition property has to be set *before* the
    // opacity/transform writes for the browser to interpolate the
    // change. When `transition = None`, clear any prior transition
    // string so the next set snaps.
    let transition_str = match transition {
        Some((duration_ms, easing)) => {
            // Single shorthand covering both properties. Both
            // animate over the same duration with the same easing —
            // matching how authors usually want enter/exit to look.
            format!(
                "opacity {ms}ms {ease}, transform {ms}ms {ease}",
                ms = duration_ms,
                ease = easing_css(easing),
            )
        }
        None => String::new(),
    };
    let _ = style.set_property("transition", &transition_str);

    // Opacity: write only if the state declares one, else clear so
    // the node returns to its stylesheet-declared (or default 1.0)
    // opacity.
    match state.opacity {
        Some(v) => {
            let _ = style.set_property("opacity", &format!("{}", v));
        }
        None => {
            let _ = style.remove_property("opacity");
        }
    }

    // Transform: compose translate + scale into one transform
    // string. Identity components are omitted (so the resulting
    // string is shorter and closer to the rendered identity when
    // the state is `rest()`).
    let mut parts: Vec<String> = Vec::new();
    if let (Some(x), Some(y)) = (state.translate_x, state.translate_y) {
        if x != 0.0 || y != 0.0 {
            parts.push(format!("translate({}px, {}px)", x, y));
        }
    } else if let Some(x) = state.translate_x {
        if x != 0.0 {
            parts.push(format!("translateX({}px)", x));
        }
    } else if let Some(y) = state.translate_y {
        if y != 0.0 {
            parts.push(format!("translateY({}px)", y));
        }
    }
    if let Some(s) = state.scale {
        if s != 1.0 {
            parts.push(format!("scale({})", s));
        }
    }

    if parts.is_empty() {
        // Identity transform — clear the property so the element
        // falls back to whatever (if anything) its stylesheet set.
        let _ = style.remove_property("transform");
    } else {
        let _ = style.set_property("transform", &parts.join(" "));
    }
}

fn easing_css(e: Easing) -> &'static str {
    crate::style::easing_to_css(e)
}
