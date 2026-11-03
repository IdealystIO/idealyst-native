//! Keyboard focus-within delivery for the web backend
//! (`InputOps::install_focus_handler`, `view` / `pressable` `on_focus`).
//!
//! `focusin` / `focusout` on the element itself: unlike `focus` / `blur`
//! they bubble, so the element hears focus landing on any descendant —
//! the focus-within contract.
//!
//! - `focusin` reports whether the newly focused element matches
//!   `:focus-visible` — the same rule the `FOCUSED` style state lowers to
//!   (`css::state_pseudo`). A click that focuses a `<button>` reports
//!   `false` (no ring, no focus-driven UI); Tab and programmatic focus
//!   after keyboard use report `true`; a text field is always
//!   focus-visible.
//! - `focusout` whose `relatedTarget` (the element gaining focus) is still
//!   inside this element is focus moving WITHIN the subtree: skipped, the
//!   paired `focusin` reports the new state. Otherwise focus left: `false`.
//!
//! Repeats (`false` while unfocused) are deduplicated by the vocabulary.

use runtime_shared::FocusWithinHandler;
use web_glue::JsCast;
use web_glue::dom::{Element, Node};

/// Install `focusin` / `focusout` listeners on `node` that drive `handler`.
pub(crate) fn install(node: &Node, handler: FocusWithinHandler) {
    let element: Element = match node.clone().dyn_into::<Element>() {
        Ok(e) => e,
        Err(_) => return,
    };
    {
        let handler = handler.clone();
        crate::glue_dom::listen_for_element_lifetime(&element, "focusin", Default::default(), move |ev| {
            let visible = ev
                .target()
                .and_then(|t| t.dyn_into::<Element>().ok())
                .is_some_and(|t| t.matches(":focus-visible").unwrap_or(false));
            (handler)(visible);
        });
    }
    {
        let this: Node = element.clone().unchecked_into();
        crate::glue_dom::listen_for_element_lifetime(&element, "focusout", Default::default(), move |ev| {
            let ev: web_glue::dom::FocusEvent = ev.unchecked_into();
            let gaining: Option<Node> = ev.related_target().map(|t| t.unchecked_into());
            if this.contains(gaining.as_ref()) {
                return;
            }
            (handler)(false);
        });
    }
}
