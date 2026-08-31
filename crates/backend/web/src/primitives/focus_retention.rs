//! `Backend::mark_preserves_focus` — the web half of the framework's
//! focus-preserving press region (see `runtime_shared::backend`).
//!
//! In browsers, moving keyboard focus is the *default action* of
//! `mousedown` — and per the Pointer Events spec, canceling `pointerdown`
//! suppresses the compatibility mouse events (and with them the focus
//! move), while `click` still fires. So one capture-phase `pointerdown`
//! listener calling `preventDefault()` makes every press inside the
//! marked subtree focus-neutral: a click on an option row in a combobox
//! menu commits without blurring the input that anchors the menu.
//!
//! CAPTURE phase is load-bearing: pressables install a bubble-phase
//! `pointerdown` listener that calls `stopPropagation()` (the
//! ancestor-touch swallow, see `pressable.rs` / `touch.rs`). A bubble
//! listener on the marked ancestor would never see a press that starts
//! on a row; the capture listener runs before any bubble-phase
//! propagation control can cancel it.
//!
//! Side effects of the canceled default inside the marked subtree —
//! no text selection by drag, no `:active` via compat mouse events —
//! are acceptable for the popover/adornment surfaces this is meant for
//! (their press feedback is signal-driven, not UA-driven).
//!
//! One press is exempt: a press that lands on a TEXT-ENTRY control inside
//! the region. A menu panel with a pinned `header`/`footer` slot (see
//! idea-ui's `Menu` / `SubMenu`) is marked so its ROWS don't blur the slot's
//! search field — but with a blanket cancel that field can never be focused
//! by clicking it either, which is the one thing it exists for. AppKit's
//! half of this mark already behaves that way (it only skips the
//! outside-click resign in `FlippedView::mouse_down`; a press on an
//! `NSTextField` is delivered to the field and focuses it), so the exemption
//! is what makes the two backends observably the same.
//!
//! The exemption is deliberately NARROW — form controls that own text
//! entry, not "anything focusable". A Pressable renders as
//! `div[tabindex=0]`, so a generic focusability test would hand focus to
//! every combobox option row and blur the input this mark exists to
//! protect.

use crate::WebBackend;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use web_sys::Node;

/// Controls whose own focus-on-press must survive the region's cancel.
const TEXT_ENTRY: &str = "input,textarea,select,[contenteditable=''],[contenteditable='true']";

/// Did this press land on (or inside) a text-entry control? `closest` walks
/// up from the target, so a press on a control's inner chrome counts too.
pub(crate) fn lands_on_text_entry(ev: &web_sys::Event) -> bool {
    let Some(target) = ev.target() else {
        return false;
    };
    let el: web_sys::Element = match target.dyn_into() {
        Ok(e) => e,
        Err(_) => return false,
    };
    matches!(el.closest(TEXT_ENTRY), Ok(Some(_)))
}

pub(crate) fn mark(b: &mut WebBackend, node: &Node) {
    let el: web_sys::Element = match node.clone().dyn_into() {
        Ok(e) => e,
        Err(_) => return,
    };
    let id = b.node_id(node);
    let closure = Closure::<dyn FnMut(web_sys::Event)>::new(move |ev: web_sys::Event| {
        if lands_on_text_entry(&ev) {
            return;
        }
        ev.prevent_default();
    });
    // `capture = true`: pressables install a bubble-phase `pointerdown`
    // listener that calls `stopPropagation`, so a bubble listener here would
    // never see a press that starts on a row.
    b.track_listener(id, &el, "pointerdown", true, closure);
}
