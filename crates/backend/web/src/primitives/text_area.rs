//! `Primitive::TextArea` — a `<textarea>` with a controlled value
//! signal and a per-keystroke `on_change` callback. Mirrors the
//! shape of `text_input.rs`; the only difference is the element
//! tag (`<textarea>` instead of `<input type="text">`) and the
//! per-keystroke listener landing on the textarea's `value`
//! property rather than `<input>.value`.

use crate::WebBackend;
use runtime_core::primitives::key::{KeyDownHandler, KeyEvent, KeyOutcome};
use runtime_core::primitives::text_area::{TextAreaHandle, TextAreaOps};
use std::any::Any;
use std::rc::Rc;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use web_sys::Node;

pub(crate) fn create(
    b: &mut WebBackend,
    initial_value: &str,
    placeholder: Option<&str>,
    on_change: Rc<dyn Fn(String)>,
    on_key_down: Option<KeyDownHandler>,
) -> Node {
    let textarea: web_sys::HtmlTextAreaElement = b
        .doc
        .create_element("textarea")
        .expect("create_element textarea failed")
        .unchecked_into();
    textarea.set_value(initial_value);
    if let Some(p) = placeholder {
        textarea.set_placeholder(p);
    }
    // Neutralize the browser defaults that break the
    // `text_area`-over-`code_block` overlay pattern in the fiddle's
    // editor. The framework's stylesheet (applied via the element's
    // `class`) still owns font / padding / color / size; this inline
    // baseline locks down the alignment-critical bits so a class
    // can't accidentally re-introduce a margin or break the
    // `white-space` mode.
    //
    //   - `margin: 0` / `border: 0` / `outline: none` strip
    //     browser-default chrome that would shift the text origin
    //     vs. the overlay `<pre>`.
    //   - `resize: none` — the corner grab handle is noise here.
    //   - `box-sizing: border-box` — padding lives inside the
    //     declared height so the overlay's padding lines up.
    //   - `white-space: pre` — match the `<pre>`'s wrapping mode
    //     so long lines don't wrap in one layer but not the other.
    //     Combined with `wrap="off"` for the HTML-attribute form.
    //   - `overflow: auto` — long files scroll inside the
    //     textarea instead of overflowing the editor card.
    let _ = textarea.set_attribute(
        "style",
        "margin: 0; border: 0; outline: none; resize: none; \
         box-sizing: border-box; white-space: pre; overflow: auto; \
         tab-size: 4;",
    );
    // The `wrap` attribute is the historical, still-honored way to
    // tell `<textarea>` to keep lines unwrapped. Pairs with
    // `white-space: pre` above.
    let _ = textarea.set_attribute("wrap", "off");
    // Code editing: disable browser features that would mangle
    // source. `spellcheck` puts red squiggles under every ident;
    // `autocapitalize` / `autocorrect` flip keywords on iOS.
    let _ = textarea.set_attribute("spellcheck", "false");
    let _ = textarea.set_attribute("autocapitalize", "off");
    let _ = textarea.set_attribute("autocorrect", "off");
    let _ = textarea.set_attribute("autocomplete", "off");

    let textarea_clone = textarea.clone();
    let closure = Closure::<dyn FnMut(web_sys::Event)>::new(move |_e: web_sys::Event| {
        on_change(textarea_clone.value());
    });
    let _ = textarea.add_event_listener_with_callback(
        "input",
        closure.as_ref().unchecked_ref(),
    );
    let id = b.node_id(&textarea.clone().unchecked_into::<Node>());
    b.state_listeners.entry(id).or_default().push(closure);
    if let Some(handler) = on_key_down {
        attach_key_listener_textarea(&textarea, id, b, handler);
    }
    textarea.unchecked_into::<Node>()
}

/// Mirror of `text_input::attach_key_listener_input` for the
/// textarea-specific element type. See that function for the design
/// notes — the only difference is the DOM type we read selection from.
fn attach_key_listener_textarea(
    textarea: &web_sys::HtmlTextAreaElement,
    id: u32,
    b: &mut WebBackend,
    handler: KeyDownHandler,
) {
    let textarea_clone = textarea.clone();
    let closure = Closure::<dyn FnMut(web_sys::Event)>::new(move |e: web_sys::Event| {
        if let Ok(ke) = e.dyn_into::<web_sys::KeyboardEvent>() {
            let event = KeyEvent {
                key: ke.key(),
                shift: ke.shift_key(),
                ctrl: ke.ctrl_key(),
                alt: ke.alt_key(),
                meta: ke.meta_key(),
                selection_start: textarea_clone
                    .selection_start()
                    .ok()
                    .flatten()
                    .unwrap_or(0) as usize,
                selection_end: textarea_clone
                    .selection_end()
                    .ok()
                    .flatten()
                    .unwrap_or(0) as usize,
            };
            if handler(&event) == KeyOutcome::PreventDefault {
                ke.prevent_default();
            }
        }
    });
    let _ = textarea.add_event_listener_with_callback(
        "keydown",
        closure.as_ref().unchecked_ref(),
    );
    b.state_listeners.entry(id).or_default().push(closure);
}

pub(crate) fn update_value(node: &Node, value: &str) {
    if let Ok(textarea) = node.clone().dyn_into::<web_sys::HtmlTextAreaElement>() {
        // Same cursor-jump avoidance as `text_input::update_value`:
        // skip the write when the signal-driven update would set
        // back the same value we just read off the `input` event.
        if textarea.value() != value {
            textarea.set_value(value);
        }
    }
}

pub(crate) fn make_handle(node: &Node) -> TextAreaHandle {
    let textarea: web_sys::HtmlTextAreaElement = node
        .clone()
        .dyn_into()
        .expect("text_area node is not an HtmlTextAreaElement");
    TextAreaHandle::new(Rc::new(textarea), &WebTextAreaOps)
}

struct WebTextAreaOps;
impl TextAreaOps for WebTextAreaOps {
    fn focus(&self, node: &dyn Any) {
        if let Some(t) = node.downcast_ref::<web_sys::HtmlTextAreaElement>() {
            let _ = t.focus();
        }
    }
    fn blur(&self, node: &dyn Any) {
        if let Some(t) = node.downcast_ref::<web_sys::HtmlTextAreaElement>() {
            let _ = t.blur();
        }
    }
    fn select_all(&self, node: &dyn Any) {
        if let Some(t) = node.downcast_ref::<web_sys::HtmlTextAreaElement>() {
            t.select();
        }
    }
    fn insert_text(&self, node: &dyn Any, text: &str) {
        if let Some(t) = node.downcast_ref::<web_sys::HtmlTextAreaElement>() {
            let start = t.selection_start().ok().flatten().unwrap_or(0);
            let end = t.selection_end().ok().flatten().unwrap_or(start);
            let _ = t.set_range_text_with_start_and_end(text, start, end);
            if let Ok(event) = web_sys::Event::new("input") {
                let _ = t.dispatch_event(&event);
            }
        }
    }
}
