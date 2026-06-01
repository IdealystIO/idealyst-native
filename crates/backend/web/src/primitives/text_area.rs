//! `Element::TextArea` — a `<textarea>` with a controlled value
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
    wrap: bool,
    on_change: Rc<dyn Fn(String)>,
    on_key_down: Option<KeyDownHandler>,
) -> Node {
    // Hydration adoption — see `text_input::create` for the rationale.
    let textarea: web_sys::HtmlTextAreaElement = if let Some(el) = b.hydrate_next("textarea") {
        el.unchecked_into()
    } else {
        let fresh: web_sys::HtmlTextAreaElement = b
            .doc
            .create_element("textarea")
            .expect("create_element textarea failed")
            .unchecked_into();
        let node: Node = fresh.clone().unchecked_into();
        b.hydrate_note_fresh(&node);
        fresh
    };
    textarea.set_value(initial_value);
    if let Some(p) = placeholder {
        textarea.set_placeholder(p);
    }
    // Lock down only the alignment-critical, mode-independent bits inline
    // so the framework `class` (which owns font / padding / color / size
    // AND border / focus ring) still drives appearance.
    //
    //   - `margin: 0` strips the browser-default margin.
    //   - `resize: none` — the corner grab handle is noise; the layout
    //     (intrinsic height / a sized parent) owns the box.
    //   - `box-sizing: border-box` — padding lives inside the declared
    //     height (so the `scrollHeight` autosize math and the fiddle
    //     overlay's padding both line up).
    //
    // NOTE: we deliberately do *not* strip `border`/`outline` here in
    // prose mode — that would override the field stylesheet's border and
    // `:focus` ring (the bug that made the idea-ui Textarea look bare
    // next to a bordered Field). `text_input` doesn't strip them either.
    // The code-editor shape *does* strip them (below), matching the
    // fiddle's borderless overlay editor.
    let mut style = String::from(
        "margin: 0; resize: none; box-sizing: border-box; tab-size: 4;",
    );
    if wrap {
        // Standard prose textarea: soft-wrap long lines, break long
        // unbroken tokens so they can't force horizontal scroll.
        // `overflow-y: hidden` is the *resting* state — `autosize` keeps
        // the box exactly as tall as its content, so there's nothing to
        // scroll. It flips overflow-y back to `auto` only once a
        // `max-height` cap clips the content. Crucially, measuring with
        // the scrollbar hidden stops a transient scrollbar from
        // narrowing the content width mid-measure and wrapping the text
        // an extra line early.
        style.push_str(
            " white-space: pre-wrap; overflow-wrap: break-word; word-break: break-word; \
             overflow-x: hidden; overflow-y: hidden;",
        );
    } else {
        // Code-editor shape: keep lines unwrapped and scroll both ways.
        // Pairs with `wrap="off"` below and the `text_area`-over-
        // `code_block` overlay pattern in the fiddle. A code editor is
        // fixed-height (sized by its parent), so it does not autosize.
        // Strip the browser-default border/outline so the editor sits
        // flush over its syntax-highlight overlay (the fiddle supplies
        // its own chrome via the attached class).
        style.push_str(" border: 0; outline: none; white-space: pre; overflow: auto;");
    }
    let _ = textarea.set_attribute("style", &style);
    if wrap {
        // Soft wrap is the `<textarea>` default; be explicit so a
        // re-adopted (hydrated) node that previously had `wrap="off"`
        // is reset. The `wrap` attribute also *is* the autosize-eligible
        // signal read back by `autosize` / `update_value` — no separate
        // marker needed.
        let _ = textarea.set_attribute("wrap", "soft");
        // Baseline the `height: auto` measurement at a single line so
        // `scrollHeight` reflects only the real content. Without this a
        // `<textarea>` snaps to its default `rows` (2) when height is
        // `auto`, flooring every measurement at two lines. The resting
        // height still comes from the CSS `min-height` (the component's
        // `rows`), which clamps the inline height `autosize` writes.
        textarea.set_rows(1);
    } else {
        // The `wrap` attribute is the historical, still-honored way to
        // keep `<textarea>` lines unwrapped. Pairs with
        // `white-space: pre`. Code editing also disables the browser
        // text features that mangle source: `spellcheck` squiggles
        // under every ident; `autocapitalize` / `autocorrect` flip
        // keywords on iOS.
        let _ = textarea.set_attribute("wrap", "off");
        let _ = textarea.set_attribute("spellcheck", "false");
        let _ = textarea.set_attribute("autocapitalize", "off");
        let _ = textarea.set_attribute("autocorrect", "off");
        let _ = textarea.set_attribute("autocomplete", "off");
    }

    let textarea_clone = textarea.clone();
    let closure = Closure::<dyn FnMut(web_sys::Event)>::new(move |_e: web_sys::Event| {
        autosize(&textarea_clone);
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
    // Fit to the initial value. The element isn't in the document yet,
    // so `scrollHeight` may read 0 here; the controlled-value Effect
    // that fires right after mount calls `update_value`, which re-runs
    // `autosize` once the node is laid out.
    autosize(&textarea);
    textarea.unchecked_into::<Node>()
}

/// Emulate the intrinsic content-height that native toolkits get from a
/// measure function: fit a wrapping textarea's height to its content so
/// the box grows and shrinks with the text, bounded by the element's
/// CSS `min-height` / `max-height`.
///
/// `<textarea>` doesn't report content height to CSS layout, so we drive
/// it. The order matters — it's tuned to avoid the box gaining a line
/// *before* the text actually wraps:
///
/// 1. Pin `overflow-y: hidden` first. A visible vertical scrollbar steals
///    content width; if one flickers in during the measurement the text
///    re-wraps at the narrower width and `scrollHeight` over-reports by a
///    line. Hiding it keeps the measurement honest.
/// 2. Collapse `height` to `auto` so the box can *shrink* when text is
///    deleted (with `rows = 1` set at create time, `auto` is one line,
///    not the textarea's default two).
/// 3. Read `scrollHeight` (content + padding, since `box-sizing:
///    border-box`) and pin `height` to it. The CSS `min-height` /
///    `max-height` then clamp the rendered height.
/// 4. If a `max-height` cap left content taller than the box (scrollHeight
///    still exceeds clientHeight), restore `overflow-y: auto` so the
///    overflow scrolls instead of clipping.
///
/// Skipped when the author owns the box geometry, mirroring native
/// "style pins the height → fixed, don't grow":
///   - `wrap == off` — the code-editor shape scrolls, it doesn't grow.
///   - `position: absolute | fixed` — the box is placed/sized by the
///     author (e.g. the fiddle's `inset: 0` editor); an inline height
///     would fight that.
fn autosize(textarea: &web_sys::HtmlTextAreaElement) {
    // `wrap == "off"` is the code-editor shape — never autosize it.
    if textarea.wrap() == "off" {
        return;
    }
    if let Some(win) = web_sys::window() {
        if let Ok(Some(cs)) = win.get_computed_style(textarea) {
            let pos = cs.get_property_value("position").unwrap_or_default();
            if pos == "absolute" || pos == "fixed" {
                return;
            }
        }
    }
    let style = textarea.style();
    let _ = style.set_property("overflow-y", "hidden");
    let _ = style.set_property("height", "auto");
    let h = textarea.scroll_height();
    let _ = style.set_property("height", &format!("{h}px"));
    // Did a `max-height` cap clip the content? If so, let it scroll.
    if textarea.scroll_height() > textarea.client_height() {
        let _ = style.set_property("overflow-y", "auto");
    }
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
        // Re-fit after a controlled-signal write (programmatic set, or
        // the post-mount Effect that runs once the node is laid out).
        // The `input` listener only fires for user edits, so without
        // this a content-sized box wouldn't track external value
        // changes. `autosize` itself no-ops for code-mode / pinned boxes.
        autosize(&textarea);
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
