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
    min_rows: Option<u32>,
    max_rows: Option<u32>,
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

    // Stash the row bounds as data-attributes so `autosize` (which runs later,
    // once the node is laid out and has a computed line-height) can derive the
    // pixel floor/cap from `rows × real line-height` — the same rows→px contract
    // the native backends honor via `resolve_text_area_height`, instead of the
    // CSS min/max-height idea-ui used to synthesize from an estimated line size.
    if let Some(r) = min_rows {
        let _ = textarea.set_attribute("data-min-rows", &r.to_string());
    }
    if let Some(r) = max_rows {
        let _ = textarea.set_attribute("data-max-rows", &r.to_string());
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
    // Fit to the initial value. The element isn't in the document yet, so a
    // synchronous measure reads `scrollHeight == 0` AND `getComputedStyle`
    // returns nothing — so `line-height` resolves to 0 and the `min_rows`→px
    // floor collapses, pinning the box to ~0px ("very short on init"). The
    // controlled-value Effect that `build_text_area` installs does NOT save us:
    // it fires synchronously during the walker build, while the node is still
    // detached, so it measures 0 too and only re-runs on a later `value`
    // change. So we measure once now (best-effort; covers an already-attached
    // hydrated node) and again after one animation frame — by which point the
    // mount has attached the node and the browser has laid it out, so both
    // `scrollHeight` and the computed `line-height` (the rows→px floor input)
    // are real. Mirrors `graphics::create`'s deferred first `on_ready`.
    autosize(&textarea);
    let textarea_for_raf = textarea.clone();
    let raf = Closure::<dyn FnMut()>::new(move || autosize(&textarea_for_raf));
    if let Some(win) = web_sys::window() {
        let _ = win.request_animation_frame(raf.as_ref().unchecked_ref());
    }
    raf.forget();
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
///    border-box`), add the border back ([`autosize_height`]), and pin
///    `height`. The CSS `min-height` / `max-height` then clamp the box.
/// 4. Decide the scrollbar from the `max-height` cap, NOT from a re-measure
///    ([`should_scroll`]). A scrollbar belongs here only when content
///    genuinely exceeds the cap; the box otherwise fits its content exactly
///    so `overflow-y: hidden` stays. (An uncapped textarea — the default
///    `max_rows = 0` — therefore never shows a scrollbar; it just grows.)
///
/// Why not the obvious `if scrollHeight > clientHeight { scroll }`? Those
/// are integers rounded from fractional layout: with the field's real
/// `line-height: normal` (~16.8px for 14px text), content height is
/// fractional, `scrollHeight` rounds *up*, `clientHeight` rounds *down*,
/// and they differ by 1px even on a perfectly-fitted box — so the box
/// permanently shows a spurious scrollbar. Comparing the (unclamped)
/// desired height against the cap sidesteps the rounding entirely.
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
    // Read border + the floor/cap inputs (and bail on author-owned geometry)
    // up front. All are constant w.r.t. the height we're about to pin, so
    // reading them before the `height: auto` write is correct and avoids a
    // second reflow.
    let mut vertical_border = 0.0_f64;
    let mut vertical_padding = 0.0_f64;
    let mut line_height = 0.0_f64;
    let mut css_max_height: Option<f64> = None;
    let mut css_min_height: Option<f64> = None;
    if let Some(win) = web_sys::window() {
        if let Ok(Some(cs)) = win.get_computed_style(textarea) {
            let pos = cs.get_property_value("position").unwrap_or_default();
            if pos == "absolute" || pos == "fixed" {
                return;
            }
            let top = parse_px(&cs.get_property_value("border-top-width").unwrap_or_default());
            let bottom =
                parse_px(&cs.get_property_value("border-bottom-width").unwrap_or_default());
            vertical_border = top + bottom;
            vertical_padding = parse_px(&cs.get_property_value("padding-top").unwrap_or_default())
                + parse_px(&cs.get_property_value("padding-bottom").unwrap_or_default());
            line_height = resolve_line_height(&cs);
            // `none` (uncapped) / `auto` parse to `None`.
            css_max_height = parse_px_opt(&cs.get_property_value("max-height").unwrap_or_default());
            css_min_height = parse_px_opt(&cs.get_property_value("min-height").unwrap_or_default());
        }
    }
    // Border-box chrome added to every rows→px conversion (the box-sizing is
    // border-box, so a row count's pixel height includes padding + border).
    let row_chrome = vertical_padding + vertical_border;
    // Row floor/cap from the primitive's `min_rows`/`max_rows` × the REAL
    // computed line-height — the web side of the cross-backend rows→px contract.
    let row_floor = read_rows(textarea, "data-min-rows")
        .map(|r| r as f64 * line_height + row_chrome);
    let row_cap = read_rows(textarea, "data-max-rows")
        .map(|r| r as f64 * line_height + row_chrome);
    // The effective bounds combine rows with any explicit CSS override: the
    // floor is the taller of the two, the cap the shorter (an explicit px
    // min/max-height wins when it's the tighter constraint).
    let floor = max_opt(row_floor, css_min_height);
    let cap = min_opt(row_cap, css_max_height);

    let style = textarea.style();
    let _ = style.set_property("overflow-y", "hidden");
    let _ = style.set_property("height", "auto");
    // The content's natural border-box height (unclamped). Captured once: it's
    // both the clamp base and the scroll-decision input, and re-reading
    // `scrollHeight` after pinning `height` would cost a second reflow.
    let natural = autosize_height(textarea.scroll_height(), vertical_border);
    let mut desired = natural;
    if let Some(f) = floor {
        desired = desired.max(f);
    }
    if let Some(c) = cap {
        desired = desired.min(c);
    }
    let _ = style.set_property("height", &format!("{desired}px"));
    // Scroll only when the natural content genuinely outgrows the cap (the
    // pinned `height` is clamped to it), so the overflow scrolls, not clips.
    if should_scroll(natural, cap) {
        let _ = style.set_property("overflow-y", "auto");
    }
}

/// Read a `data-*` row-count attribute (`data-min-rows` / `data-max-rows`) set
/// at create time, returning `None` when absent or unparseable.
fn read_rows(textarea: &web_sys::HtmlTextAreaElement, attr: &str) -> Option<u32> {
    textarea.get_attribute(attr).and_then(|v| v.trim().parse().ok())
}

/// Resolve the computed `line-height` to pixels. `getComputedStyle` usually
/// returns a px value, but `normal` (and some engines) yield a keyword — fall
/// back to `font-size × 1.2`, the conventional `normal` ratio, so the rows→px
/// conversion still lands close to the real line box.
fn resolve_line_height(cs: &web_sys::CssStyleDeclaration) -> f64 {
    if let Some(px) = parse_px_opt(&cs.get_property_value("line-height").unwrap_or_default()) {
        return px;
    }
    let font_size = parse_px(&cs.get_property_value("font-size").unwrap_or_default());
    if font_size > 0.0 {
        font_size * 1.2
    } else {
        0.0
    }
}

/// `max(a, b)` over optionals — `None` is the absence of a bound, so it never
/// tightens the result. Used to combine the rows-derived floor with a CSS
/// `min-height` override (the taller wins).
fn max_opt(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.max(y)),
        (x, None) => x,
        (None, y) => y,
    }
}

/// `min(a, b)` over optionals — `None` means "no cap", so it never tightens the
/// result. Combines the rows-derived cap with a CSS `max-height` override (the
/// shorter wins).
fn min_opt(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.min(y)),
        (x, None) => x,
        (None, y) => y,
    }
}

/// Border-box autosize height. `scrollHeight` reports content + padding
/// only — it never includes the border (per spec, measured like
/// `clientHeight`). But with `box-sizing: border-box` the CSS `height` we
/// pin *is* the border box (content + padding + border). Pinning
/// `height = scrollHeight` therefore lands `vertical_border` px short, so
/// the content overflows by exactly the border and a scrollbar pops in.
/// Adding the border back makes the border box exactly tall enough.
fn autosize_height(scroll_height: i32, vertical_border: f64) -> f64 {
    scroll_height as f64 + vertical_border
}

/// Sub-pixel slack on the `max-height` cap comparison. A box sitting
/// *exactly* at the cap (desired ≈ max, off only by fractional rounding)
/// must NOT flip a scrollbar; a genuine overflow is a whole extra line
/// (>16px past the cap), comfortably clear of this tolerance.
const AUTOSIZE_CAP_TOLERANCE_PX: f64 = 1.0;

/// Whether the autosized box needs a vertical scrollbar: only when a real
/// `max-height` cap exists *and* the content's desired height exceeds it
/// (beyond [`AUTOSIZE_CAP_TOLERANCE_PX`]). An uncapped box (`None`) fits its
/// content exactly and never scrolls — this is the fix for the spurious
/// scrollbar that a `scrollHeight > clientHeight` re-measure produced on a
/// perfectly-fitted box (those integers round in opposite directions).
fn should_scroll(desired_height: f64, max_height: Option<f64>) -> bool {
    match max_height {
        Some(max) => desired_height > max + AUTOSIZE_CAP_TOLERANCE_PX,
        None => false,
    }
}

/// Parse a computed `<len>px` string into pixels, defaulting unparseable
/// input (the empty string for an unset property) to `0`. See
/// [`parse_px_opt`] for the cap case where "unset/none" must be
/// distinguished from a real `0px`.
fn parse_px(value: &str) -> f64 {
    parse_px_opt(value).unwrap_or(0.0)
}

/// Parse a computed `<len>px` string into pixels, returning `None` for any
/// value without a `px` suffix — notably `"none"` (an uncapped
/// `max-height`) and `""`. `getComputedStyle` always resolves lengths to
/// `px`, so a trailing-`px` strip + parse covers every real length.
fn parse_px_opt(value: &str) -> Option<f64> {
    value.trim().strip_suffix("px")?.trim().parse().ok()
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

#[cfg(test)]
mod tests {
    use super::*;
    use wasm_bindgen::JsCast;
    use wasm_bindgen_test::*;

    #[wasm_bindgen_test]
    fn autosize_height_adds_vertical_border() {
        // Border-box: the pinned height must add back the border that
        // `scrollHeight` omits, or the box lands short and overflows.
        assert_eq!(autosize_height(40, 2.0), 42.0);
        assert_eq!(autosize_height(40, 0.0), 40.0);
    }

    #[wasm_bindgen_test]
    fn parse_px_handles_values_and_garbage() {
        assert_eq!(parse_px("1px"), 1.0);
        assert_eq!(parse_px(" 0.5px "), 0.5);
        assert_eq!(parse_px(""), 0.0);
        assert_eq!(parse_px("auto"), 0.0);
    }

    #[wasm_bindgen_test]
    fn parse_px_opt_distinguishes_none_from_zero() {
        assert_eq!(parse_px_opt("204px"), Some(204.0));
        assert_eq!(parse_px_opt("0px"), Some(0.0));
        // Uncapped / unset must be `None`, NOT `Some(0.0)` — a `0` cap would
        // make every box "scroll" while `none` means "never scroll".
        assert_eq!(parse_px_opt("none"), None);
        assert_eq!(parse_px_opt(""), None);
    }

    /// The core of the spurious-scrollbar fix: scrolling is decided from the
    /// `max-height` cap, never from a re-measure. An uncapped box never
    /// scrolls no matter how tall it grew; a capped box scrolls only once
    /// content clears the cap by more than the sub-pixel tolerance.
    #[wasm_bindgen_test]
    fn should_scroll_only_when_content_clears_the_cap() {
        // Uncapped (the default `max_rows = 0`) → never scroll, even tall.
        assert!(!should_scroll(10_000.0, None));
        // Comfortably under the cap → fits, no scrollbar.
        assert!(!should_scroll(100.0, Some(200.0)));
        // Sitting essentially at the cap (off by sub-pixel rounding) → the
        // tolerance suppresses the flicker.
        assert!(!should_scroll(200.4, Some(200.0)));
        // A genuine extra line past the cap → scroll.
        assert!(should_scroll(217.0, Some(200.0)));
    }

    /// Regression for the reported bug: an uncapped, bordered, `border-box`
    /// wrapping textarea with a realistic fractional `line-height` showed a
    /// permanent vertical scrollbar even though its content fit — because
    /// the old `scrollHeight > clientHeight` re-measure compares two
    /// integers that round in opposite directions, differing by 1px on a
    /// perfectly-fitted box. After the fix an uncapped box decides "no
    /// scroll" from the absent cap, so `overflow-y` stays `hidden`.
    ///
    /// Lives here (not a Rust unit test) because the bug only manifests in
    /// real layout — it needs a browser to round `scrollHeight`/
    /// `clientHeight` against a fractional content height, which only
    /// `wasm-bindgen-test` provides.
    #[wasm_bindgen_test]
    fn regression_uncapped_textarea_never_shows_scrollbar() {
        let doc = web_sys::window().unwrap().document().unwrap();
        let ta: web_sys::HtmlTextAreaElement =
            doc.create_element("textarea").unwrap().unchecked_into();
        // Mirror the idea-ui Field input geometry: 1px border, `border-box`,
        // soft wrap, a fixed narrow width so content wraps to several lines,
        // and crucially `line-height: normal` (fractional for 14px text) —
        // the exact condition that made the re-measure round to a 1px
        // overflow. No `max-height` → uncapped (the `max_rows = 0` default).
        ta.set_attribute(
            "style",
            "box-sizing: border-box; border: 1px solid #000; padding: 8px; \
             width: 120px; margin: 0; resize: none; white-space: pre-wrap; \
             overflow-wrap: break-word; line-height: normal; font-size: 14px; \
             font-family: sans-serif;",
        )
        .unwrap();
        ta.set_attribute("wrap", "soft").unwrap();
        ta.set_rows(1);
        ta.set_value("123 123 123 123 123 123 o123 o12y oiu2y3 41y23o 4y 1o234ui");
        doc.body().unwrap().append_child(&ta).unwrap();

        autosize(&ta);

        // The reported symptom: a scrollbar on a box that fits. An uncapped
        // box must never enable scrolling.
        assert_eq!(
            ta.style().get_property_value("overflow-y").unwrap(),
            "hidden",
            "uncapped autosized textarea must not show a scrollbar",
        );
        // And it grew to fit rather than collapsing.
        assert!(
            ta.client_height() >= 20,
            "autosized box should be at least one line tall, got {}",
            ta.client_height(),
        );

        doc.body().unwrap().remove_child(&ta).unwrap();
    }

    /// Await a single animation frame so a deferred (rAF) `autosize` has run.
    async fn next_animation_frame() {
        let promise = js_sys::Promise::new(&mut |resolve, _reject| {
            let win = web_sys::window().unwrap();
            // `once_into_js` leaks the closure into JS — fine for a one-shot
            // test rAF; the frame fires once and the closure is collected.
            let cb = wasm_bindgen::closure::Closure::once_into_js(move || {
                let _ = resolve.call0(&wasm_bindgen::JsValue::NULL);
            });
            let _ = win.request_animation_frame(cb.unchecked_ref());
        });
        let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
    }

    /// REGRESSION for "textarea very short on init" (the reported bug).
    ///
    /// `create` measures the box in JS (`scrollHeight` + the `min_rows`→px
    /// floor). Both the create-time measure and the controlled-value Effect
    /// run while the node is still DETACHED, where `getComputedStyle` is empty
    /// → `line-height` resolves to 0 → the rows floor collapses → the box pins
    /// to ~0px. Nothing re-measured after attach, so the field shipped a
    /// single squashed line. The fix defers one `autosize` to the next
    /// animation frame, by which point the node is attached + laid out.
    ///
    /// This drives the real `create` path, attaches the node, then awaits a
    /// frame. PRE-FIX the height stays at the detached ~0px collapse; POST-FIX
    /// the deferred measure floors it to `rows` (3) lines. It genuinely fails
    /// before the fix — the only thing that sizes the box is the rAF this test
    /// adds coverage for. Must be a browser `wasm-bindgen-test`: the bug only
    /// exists against real layout (a detached node's empty computed style).
    #[wasm_bindgen_test]
    async fn regression_textarea_not_collapsed_on_init() {
        install_mount_body();
        let mut backend = crate::WebBackend::new("#app");

        // A wrapping (prose) textarea with a 3-row floor and empty initial
        // value — exactly the idea-ui `Textarea(rows = 3)` default shape.
        let node = create(
            &mut backend,
            "",
            Some("placeholder"),
            /* wrap */ true,
            /* min_rows */ Some(3),
            /* max_rows */ None,
            Rc::new(|_| {}),
            None,
        );
        // Give it a known, non-collapsing line box so the 3-row floor is a
        // concrete pixel target regardless of UA form-control defaults.
        let ta: web_sys::HtmlTextAreaElement = node.clone().unchecked_into();
        let _ = ta.style().set_property("line-height", "20px");
        let _ = ta.style().set_property("font-size", "14px");
        let _ = ta.style().set_property("width", "200px");

        // Attach into the live document, then let one frame pass so the
        // deferred autosize fires against real layout.
        let doc = web_sys::window().unwrap().document().unwrap();
        doc.get_element_by_id("app").unwrap().append_child(&node).unwrap();
        next_animation_frame().await;

        // 3 rows × 20px = 60px floor (plus any padding/border). Pre-fix the box
        // was the detached ~0px collapse (well under one line). Assert it
        // cleared two lines — the floor is honored once laid out.
        let pinned = parse_px(&ta.style().get_property_value("height").unwrap());
        assert!(
            pinned >= 56.0,
            "textarea must autosize to its 3-row floor (~60px) after attach, got {pinned}px"
        );
    }

    /// `#app` mount that survives an async test (the shared `tests::install_mount`
    /// isn't reachable from this module; inline the minimal equivalent).
    fn install_mount_body() {
        let doc = web_sys::window().unwrap().document().unwrap();
        if let Some(existing) = doc.get_element_by_id("app") {
            existing.remove();
        }
        let div = doc.create_element("div").unwrap();
        div.set_id("app");
        doc.body().unwrap().append_child(&div).unwrap();
    }

    /// Complement: a *capped* textarea (`max_rows`-equivalent `max-height`)
    /// whose content overflows the cap DOES scroll — the fix must not
    /// suppress legitimate scrollbars.
    #[wasm_bindgen_test]
    fn capped_textarea_scrolls_when_content_exceeds_cap() {
        let doc = web_sys::window().unwrap().document().unwrap();
        let ta: web_sys::HtmlTextAreaElement =
            doc.create_element("textarea").unwrap().unchecked_into();
        ta.set_attribute(
            "style",
            "box-sizing: border-box; border: 1px solid #000; padding: 8px; \
             width: 120px; max-height: 60px; margin: 0; resize: none; \
             white-space: pre-wrap; overflow-wrap: break-word; \
             line-height: 20px; font-size: 14px; font-family: sans-serif;",
        )
        .unwrap();
        ta.set_attribute("wrap", "soft").unwrap();
        ta.set_rows(1);
        // Many lines — far past the 60px (~2 line) cap.
        ta.set_value("one two three four five six seven eight nine ten eleven twelve");
        doc.body().unwrap().append_child(&ta).unwrap();

        autosize(&ta);

        assert_eq!(
            ta.style().get_property_value("overflow-y").unwrap(),
            "auto",
            "capped textarea with overflowing content must scroll",
        );

        doc.body().unwrap().remove_child(&ta).unwrap();
    }
}
