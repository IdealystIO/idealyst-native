//! Web backend accessibility translation — `AccessibilityProps` →
//! ARIA / role / aria-live attributes on the DOM.
//!
//! Strategy: a single [`apply`] function takes a DOM `Element` and the
//! resolved `AccessibilityProps` and writes (or clears) each attribute.
//! All `create_*` paths call it after constructing the node; the
//! dynamic `A11yOps::update_accessibility`
//! path reuses it identically. Removing-on-`None` is intentional —
//! `update_accessibility` must be able to clear an attribute that was
//! previously set, otherwise reactive a11y prop changes leak stale
//! values on the DOM.
//!
//! Native AX trees (the browser's accessibility walker) read ARIA
//! attributes directly off the DOM, so the web backend doesn't need
//! a parallel semantics tree — `dump_accessibility_tree` stays `None`.

use runtime_shared::accessibility::{AccessibilityProps, LiveRegionPriority, Role};
use web_glue::JsCast;
use web_glue::dom::{Element, Node};

/// Apply / refresh every ARIA attribute on `node` from `props`, per the
/// table in [`AccessibilityProps::dom_attributes`].
///
/// `inferred_role` is the primitive's default role (see
/// [`runtime_shared::accessibility::default_role`]); an author-supplied
/// `props.role` always wins.
///
/// Idempotent: every attribute is either written or explicitly
/// removed. Calling twice with the same `props` produces the same DOM
/// state; calling with different `props` always converges.
pub(crate) fn apply(node: &Node, props: &AccessibilityProps, inferred_role: Option<Role>) {
    let Some(elem) = node.dyn_ref::<Element>() else {
        // Text nodes etc. — no attributes, nothing to do.
        return;
    };
    for (attr, value) in props.dom_attributes(inferred_role) {
        match value {
            Some(v) => {
                let _ = elem.set_attribute(attr, v);
            }
            None => {
                let _ = elem.remove_attribute(attr);
            }
        }
    }
}

fn live_region_attr(p: LiveRegionPriority) -> &'static str {
    match p {
        LiveRegionPriority::Polite => "polite",
        LiveRegionPriority::Assertive => "assertive",
    }
}

// ---------------------------------------------------------------------------
// announce_for_accessibility — lazy hidden live-region pair on <body>.
// ---------------------------------------------------------------------------

/// Post a one-shot accessibility announcement. The web pattern is a
/// hidden element with `aria-live` set; assistive tech (NVDA, JAWS,
/// VoiceOver) observes mutations to it and reads the new text.
///
/// We keep one polite + one assertive live-region pair on
/// `document.body`, lazily created on first use. Each call:
///
/// 1. Writes `msg` into the matching element's `textContent`.
/// 2. Schedules a clear after a short delay so re-announcing the
///    same string is observed as a fresh mutation (screen readers
///    suppress identical consecutive text).
///
/// Idempotent across calls; safe to invoke from any
/// `A11yOps::announce_for_accessibility` site.
pub(crate) fn announce(msg: &str, priority: LiveRegionPriority) {
    let Some(window) = web_glue::dom::window() else { return };
    let Some(document) = window.document() else { return };
    let Some(body) = document.body() else { return };

    let id = match priority {
        LiveRegionPriority::Polite => "__idealyst_ax_announce_polite",
        LiveRegionPriority::Assertive => "__idealyst_ax_announce_assertive",
    };
    let live_attr = live_region_attr(priority);

    // Find existing or create.
    let region = if let Some(existing) = document.get_element_by_id(id) {
        existing
    } else {
        let elem = match document.create_element("div") {
            Ok(e) => e,
            Err(_) => return,
        };
        let _ = elem.set_attribute("id", id);
        let _ = elem.set_attribute("aria-live", live_attr);
        let _ = elem.set_attribute("aria-atomic", "true");
        // Visually-hidden styles. Position + clip so the element
        // doesn't take any visible space but stays in the AX tree.
        let _ = elem.set_attribute(
            "style",
            "position:absolute;left:-10000px;top:auto;width:1px;height:1px;\
             overflow:hidden;clip:rect(1px,1px,1px,1px);white-space:nowrap;",
        );
        let _ = body.append_child(&elem);
        elem
    };

    // Clear-then-set so consecutive identical announcements still
    // produce a mutation event. Some readers (NVDA) suppress text
    // changes that don't actually change the textContent.
    region.set_text_content(Some(""));
    region.set_text_content(Some(msg));
}
